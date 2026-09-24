//! The REST control plane behind `nsm serve`.
//!
//! An axum router that exposes the [`crate::ops`] operations over HTTP so that an
//! orchestrator (a Kubernetes sidecar, a workflow engine) can drive NSM
//! without a shell. Long-running operations become *jobs*: `POST /v1/publish`
//! and `POST /v1/claim` register the party synchronously, then keep its
//! session running in the background and answer `202 Accepted` with a job id;
//! `GET /v1/jobs/{id}` reports its state and `DELETE` stops it.
//!
//! Exposure rules (audit S5, decision D9): the server binds loopback by
//! default; binding anything else requires a bearer token, checked on every
//! request; request bodies never carry file paths (certificate material comes
//! from the `serve` process's own configuration); operations that fail report
//! their real outcome instead of "success" before anything happened.
//!
//! | Method and path | Body | Result |
//! |---|---|---|
//! | `GET /healthz` | | `{"ok":true}` |
//! | `GET /v1/interfaces?ip_version=4` | | `{"interfaces":[..]}` |
//! | `GET /v1/ips?interface=&ip_start=&ip_version=` | | `{"addresses":[{"interface","ip"}]}` |
//! | `POST /v1/publish` | [`PublishBody`] | `202` [`JobView`] |
//! | `POST /v1/claim` | [`ClaimBody`] | `202` [`JobView`] (with `service`) |
//! | `GET /v1/jobs` | | `[`[`JobView`]`]` |
//! | `GET /v1/jobs/{id}` | | [`JobView`] or `404` |
//! | `DELETE /v1/jobs/{id}` | | [`JobView`] (state `cancelled`) or `404` |
//! | `POST /v1/collect` | [`PartyBody`] | [`Collected`] |
//! | `POST /v1/send` | [`SendBody`] | `{"delivered":true}` |
//!
//! Errors are `{"error": "<message>"}` with `400` for bad input, `401` for a
//! missing or wrong token, `404` for unknown jobs, `502` when a peer or the
//! broker could not be reached, `500` otherwise.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::cli::IfaceOpts;
use crate::net::{Addr, IpVersion};
use crate::ops::{self, Collected, NetOpts};
use crate::party::Session;
use crate::protocol::{Key, PartyId, ServiceHandle};
use crate::{Error, Result};

/// Settings for the control plane.
#[derive(Debug, Clone)]
pub struct ServeOpts {
    /// Address to bind.
    pub bind: SocketAddr,
    /// Bearer token; mandatory unless `bind` is a loopback address.
    pub token: Option<String>,
    /// Network settings handed to every operation the server runs.
    pub net: NetOpts,
}

/// A running control plane.
#[derive(Debug)]
pub struct ControlPlane {
    local_addr: SocketAddr,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl ControlPlane {
    /// The address actually bound.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Run until the shutdown token is cancelled.
    pub async fn run(self) -> Result<()> {
        let _ = self.task.await;
        Ok(())
    }

    /// Stop the server and every job it started.
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        let _ = self.task.await;
    }
}

/// Bind and start serving. Fails with [`Error::Config`] when a non-loopback
/// bind address is requested without a token.
pub async fn serve(opts: ServeOpts, shutdown: CancellationToken) -> Result<ControlPlane> {
    if !opts.bind.ip().is_loopback() && opts.token.as_deref().is_none_or(str::is_empty) {
        return Err(Error::config(format!(
            "binding the control plane to {} requires --token (or NSM_TOKEN)",
            opts.bind
        )));
    }
    let listener = TcpListener::bind(opts.bind)
        .await
        .map_err(|source| Error::Bind {
            addr: opts.bind,
            source,
        })?;
    let local_addr = listener.local_addr()?;
    let app = Arc::new(AppState {
        token: opts.token.filter(|t| !t.is_empty()),
        net: opts.net,
        jobs: Mutex::new(BTreeMap::new()),
        next_job: AtomicU64::new(1),
        shutdown: shutdown.clone(),
    });
    let router = router(Arc::clone(&app));
    let token = shutdown.clone();
    let task = tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move { token.cancelled().await })
            .await;
        if let Err(e) = result {
            warn!(error = %e, "control plane stopped with an error");
        }
        app.cancel_all_jobs();
    });
    info!(%local_addr, "control plane listening");
    Ok(ControlPlane {
        local_addr,
        shutdown,
        task,
    })
}

/// Build the router over shared state (exposed for tests).
pub fn router(app: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/interfaces", get(interfaces))
        .route("/v1/ips", get(ips))
        .route("/v1/publish", post(publish))
        .route("/v1/claim", post(claim))
        .route("/v1/jobs", get(jobs))
        .route("/v1/jobs/{id}", get(job).delete(cancel_job))
        .route("/v1/collect", post(collect))
        .route("/v1/send", post(send))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&app),
            require_token,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .with_state(app)
}

/// Shared state of the control plane.
#[derive(Debug)]
pub struct AppState {
    token: Option<String>,
    net: NetOpts,
    jobs: Mutex<BTreeMap<u64, Job>>,
    next_job: AtomicU64,
    shutdown: CancellationToken,
}

impl AppState {
    /// State for tests and embedding: no token, default network settings.
    pub fn new(token: Option<String>, net: NetOpts, shutdown: CancellationToken) -> Arc<Self> {
        Arc::new(AppState {
            token,
            net,
            jobs: Mutex::new(BTreeMap::new()),
            next_job: AtomicU64::new(1),
            shutdown,
        })
    }

    fn cancel_all_jobs(&self) {
        for job in lock(&self.jobs).values() {
            job.cancel.cancel();
        }
    }

    fn start_job(
        self: &Arc<Self>,
        kind: &'static str,
        session: Session,
        broker: Addr,
        key: Key,
    ) -> JobView {
        let id = self.next_job.fetch_add(1, Ordering::Relaxed);
        let cancel = session.shutdown_token();
        let view = JobView {
            id,
            kind,
            state: JobState::Running,
            error: None,
            party_id: Some(session.id()),
            bind_addr: Some(session.bound()),
            service: session.service(),
            broker,
            key,
        };
        lock(&self.jobs).insert(
            id,
            Job {
                view: view.clone(),
                cancel: cancel.clone(),
                state: Arc::clone(session.state()),
            },
        );
        let app = Arc::clone(self);
        let parent = self.shutdown.clone();
        tokio::spawn(async move {
            let outcome = tokio::select! {
                r = session.run() => r,
                _ = parent.cancelled() => Ok(()),
            };
            if let Some(job) = lock(&app.jobs).get_mut(&id) {
                job.view.state = match &outcome {
                    Ok(()) if job.cancel.is_cancelled() => JobState::Cancelled,
                    Ok(()) => JobState::Finished,
                    Err(_) => JobState::Failed,
                };
                job.view.error = outcome.err().map(|e| e.to_string());
            }
        });
        view
    }

    fn job_view(&self, id: u64) -> Option<JobView> {
        lock(&self.jobs).get(&id).map(|j| {
            let mut v = j.view.clone();
            v.service = j.state.service().or(v.service);
            v
        })
    }
}

#[derive(Debug)]
struct Job {
    view: JobView,
    cancel: CancellationToken,
    state: Arc<crate::party::PartyState>,
}

/// Lifecycle of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// The session is alive.
    Running,
    /// The session ended because the broker was lost or rejected the party.
    Failed,
    /// The session ended after a `DELETE`.
    Cancelled,
    /// The session ended for another reason (shutdown of the server).
    Finished,
}

/// What the API reports about a job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JobView {
    /// Job id, unique within this server process.
    pub id: u64,
    /// `"publish"` or `"claim"`.
    pub kind: &'static str,
    /// Lifecycle state.
    pub state: JobState,
    /// Why the job failed, when it did.
    pub error: Option<String>,
    /// The party's broker-assigned id.
    pub party_id: Option<PartyId>,
    /// Where the party listens for heartbeats.
    pub bind_addr: Option<Addr>,
    /// For a claim: the paired service (updated on re-pairing).
    pub service: Option<ServiceHandle>,
    /// The broker the party registered with.
    pub broker: Addr,
    /// Rendezvous key.
    pub key: Key,
}

/// `POST /v1/publish`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PublishBody {
    /// Broker address.
    pub broker: Addr,
    /// Rendezvous key.
    pub key: Key,
    /// Heartbeat port (0 or omitted: any free port).
    #[serde(default)]
    pub bind_port: u16,
    /// Port the service listens on.
    pub service_port: u16,
    /// Local address selection (`interface`, `ip_start`, `ip_version`).
    #[serde(flatten)]
    pub iface: IfaceOpts,
    /// Serve TLS on the heartbeat listener (uses the server's certificate).
    #[serde(default)]
    pub tls: bool,
    /// One-sided liveness.
    #[serde(default)]
    pub ping: bool,
}

/// `POST /v1/claim`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClaimBody {
    /// Broker address.
    pub broker: Addr,
    /// Rendezvous key.
    pub key: Key,
    /// Heartbeat port (0 or omitted: any free port).
    #[serde(default)]
    pub bind_port: u16,
    /// Local address selection.
    #[serde(flatten)]
    pub iface: IfaceOpts,
    /// Serve TLS on the heartbeat listener.
    #[serde(default)]
    pub tls: bool,
    /// One-sided liveness.
    #[serde(default)]
    pub ping: bool,
}

/// `POST /v1/collect`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PartyBody {
    /// The party's heartbeat address.
    pub party: Addr,
}

/// `POST /v1/send`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SendBody {
    /// The client's heartbeat address.
    pub party: Addr,
    /// Text to deliver.
    pub msg: String,
}

/// Query string of `GET /v1/interfaces` and `GET /v1/ips`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct AddrQuery {
    /// Interface name filter.
    pub interface: Option<String>,
    /// Address prefix filter.
    pub ip_start: Option<String>,
    /// Address family filter.
    pub ip_version: Option<IpVersion>,
}

/// An [`Error`] as an HTTP response, with an optional explicit status.
#[derive(Debug)]
pub struct ApiError {
    status: Option<StatusCode>,
    error: Error,
}

impl ApiError {
    fn not_found(what: impl std::fmt::Display) -> Self {
        ApiError {
            status: Some(StatusCode::NOT_FOUND),
            error: Error::Rejected(format!("{what} not found")),
        }
    }
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        ApiError {
            status: None,
            error,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status.unwrap_or(match &self.error {
            Error::Json(_)
            | Error::Addr(_)
            | Error::Config(_)
            | Error::Protocol(_)
            | Error::Rejected(_)
            | Error::NoService(_)
            | Error::AmbiguousAddress { .. }
            | Error::FrameTooLarge { .. } => StatusCode::BAD_REQUEST,
            Error::Timeout(_) | Error::BrokerLost(_) | Error::PeerLost(_) | Error::Closed => {
                StatusCode::BAD_GATEWAY
            }
            Error::Resolve(_) => StatusCode::BAD_GATEWAY,
            Error::Io(e) if is_unreachable(e) => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        });
        (
            status,
            Json(serde_json::json!({ "error": self.error.to_string() })),
        )
            .into_response()
    }
}

/// Connection-level I/O failures: the peer or the broker could not be
/// reached, as opposed to a local file or socket problem.
fn is_unreachable(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        e.kind(),
        ConnectionRefused
            | ConnectionReset
            | ConnectionAborted
            | NotConnected
            | HostUnreachable
            | NetworkUnreachable
            | NetworkDown
            | TimedOut
            | UnexpectedEof
            | BrokenPipe
    )
}

type ApiResult<T> = std::result::Result<T, ApiError>;

fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> ApiResult<T> {
    serde_json::from_slice(body).map_err(|e| ApiError::from(Error::Json(e)))
}

async fn require_token(State(app): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if let Some(expected) = &app.token {
        let presented = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "missing or invalid bearer token" })),
            )
                .into_response();
        }
    }
    next.run(req).await
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

async fn interfaces(Query(q): Query<AddrQuery>) -> ApiResult<Json<serde_json::Value>> {
    let names = ops::list_interfaces(q.ip_version)?;
    Ok(Json(serde_json::json!({ "interfaces": names })))
}

async fn ips(Query(q): Query<AddrQuery>) -> ApiResult<Json<serde_json::Value>> {
    let selector = crate::net::Selector {
        interface: q.interface,
        prefix: q.ip_start,
        version: q.ip_version,
    };
    let addrs = ops::list_ips(&selector)?;
    let rows: Vec<serde_json::Value> = addrs
        .iter()
        .map(|a| serde_json::json!({ "interface": a.interface, "ip": a.ip }))
        .collect();
    Ok(Json(serde_json::json!({ "addresses": rows })))
}

async fn publish(
    State(app): State<Arc<AppState>>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<JobView>)> {
    let b: PublishBody = parse_body(&body)?;
    let session = ops::publish(ops::PublishRequest {
        broker: b.broker.clone(),
        key: b.key,
        bind_port: b.bind_port,
        service_port: b.service_port,
        selector: b.iface.selector(),
        serve_tls: b.tls,
        ping: b.ping,
        net: app.net.clone(),
    })
    .await?;
    let view = app.start_job("publish", session, b.broker, b.key);
    Ok((StatusCode::ACCEPTED, Json(view)))
}

async fn claim(
    State(app): State<Arc<AppState>>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<JobView>)> {
    let b: ClaimBody = parse_body(&body)?;
    let session = ops::claim(ops::ClaimRequest {
        broker: b.broker.clone(),
        key: b.key,
        bind_port: b.bind_port,
        selector: b.iface.selector(),
        serve_tls: b.tls,
        ping: b.ping,
        net: app.net.clone(),
    })
    .await?;
    let view = app.start_job("claim", session, b.broker, b.key);
    Ok((StatusCode::ACCEPTED, Json(view)))
}

async fn jobs(State(app): State<Arc<AppState>>) -> Json<Vec<JobView>> {
    let ids: Vec<u64> = lock(&app.jobs).keys().copied().collect();
    Json(ids.into_iter().filter_map(|id| app.job_view(id)).collect())
}

async fn job(State(app): State<Arc<AppState>>, Path(id): Path<u64>) -> ApiResult<Json<JobView>> {
    app.job_view(id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("job {id}")))
}

async fn cancel_job(
    State(app): State<Arc<AppState>>,
    Path(id): Path<u64>,
) -> ApiResult<Json<JobView>> {
    let cancel = lock(&app.jobs).get(&id).map(|j| j.cancel.clone());
    let Some(token) = cancel else {
        return Err(ApiError::not_found(format!("job {id}")));
    };
    token.cancel();
    if let Some(j) = lock(&app.jobs).get_mut(&id)
        && j.view.state == JobState::Running
    {
        j.view.state = JobState::Cancelled;
    }
    app.job_view(id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("job {id}")))
}

async fn collect(State(app): State<Arc<AppState>>, body: Bytes) -> ApiResult<Json<Collected>> {
    let b: PartyBody = parse_body(&body)?;
    Ok(Json(ops::collect(&b.party, &app.net).await?))
}

async fn send(State(app): State<Arc<AppState>>, body: Bytes) -> ApiResult<Json<serde_json::Value>> {
    let b: SendBody = parse_body(&body)?;
    ops::send(&b.party, b.msg, &app.net).await?;
    Ok(Json(serde_json::json!({ "delivered": true })))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
