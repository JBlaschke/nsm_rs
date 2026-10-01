//! The broker's admin listener: `GET /metrics`, `GET /v1/status` and
//! `GET /healthz` over plain HTTP, started by `nsm listen --admin-bind`.
//!
//! It is a second socket next to the protocol listener (monitoring plan,
//! decision M6): Prometheus scrapes HTTP, and the protocol listener may be
//! raw TCP or TLS, so a separate listener is the one way that works for
//! every transport. Parties never see it, and nothing here changes the
//! broker's state. It is off unless asked for, because a fixed default port
//! would collide when two brokers share a host.
//!
//! Exposure follows the control plane (decision D9): a non-loopback bind
//! requires a bearer token, checked on every route in constant time, and the
//! check happens before anything is bound. [`bind`] and
//! [`AdminListener::serve`] are two steps so that `listen` can bind the
//! admin address before it starts anything else and serve it once the
//! protocol listener's address, which `/v1/status` reports, is known.
//!
//! | Method and path | Result |
//! |---|---|
//! | `GET /healthz` | `{"ok":true}` |
//! | `GET /metrics` | the Prometheus text exposition, `text/plain; version=0.0.4; charset=utf-8` |
//! | `GET /v1/status` | [`Status`] as JSON |
//!
//! Anything else is 404; a missing or wrong token is 401
//! `{"error":"missing or invalid bearer token"}`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::metrics::Status;
use super::monitor::Broker;
use crate::net::Addr;
use crate::{Error, Result};

/// The content type of the text exposition.
pub const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Settings for the admin listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminOpts {
    /// Address to bind.
    pub bind: SocketAddr,
    /// Bearer token; mandatory unless `bind` is a loopback address. An
    /// empty token counts as none.
    pub token: Option<String>,
}

impl AdminOpts {
    /// Refuse a non-loopback bind without a token, before anything binds.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] naming the flags.
    pub fn check(&self) -> Result<()> {
        if !self.bind.ip().is_loopback() && self.token.as_deref().is_none_or(str::is_empty) {
            return Err(Error::config(format!(
                "binding the admin listener to {} requires --admin-token (or NSM_ADMIN_TOKEN)",
                self.bind
            )));
        }
        Ok(())
    }
}

/// A running admin listener.
#[derive(Debug)]
pub struct AdminServer {
    local_addr: SocketAddr,
    task: JoinHandle<()>,
}

impl AdminServer {
    /// The address actually bound.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Wait for the listener to stop (it stops with the broker's token).
    pub async fn wait(self) {
        let _ = self.task.await;
    }
}

/// An admin listener that is bound but not serving yet: the result of
/// [`bind`], started by [`AdminListener::serve`]. Dropping it closes the
/// socket.
#[derive(Debug)]
pub struct AdminListener {
    listener: TcpListener,
    local_addr: SocketAddr,
    token: Option<String>,
}

impl AdminListener {
    /// The address actually bound.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Start serving over the given broker. `bound` is the protocol
    /// listener's address, reported by `/v1/status`. The listener stops
    /// when `shutdown` is cancelled.
    pub fn serve(
        self,
        broker: Arc<Broker>,
        bound: Addr,
        shutdown: CancellationToken,
    ) -> AdminServer {
        let AdminListener {
            listener,
            local_addr,
            token,
        } = self;
        let app = router(AdminState::new(broker, bound, token));
        let task = tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await;
            if let Err(e) = result {
                warn!(error = %e, "admin listener stopped with an error");
            }
        });
        info!(%local_addr, "admin listener listening");
        AdminServer { local_addr, task }
    }
}

/// What the routes read: the broker and the protocol listener's address.
#[derive(Debug)]
pub struct AdminState {
    broker: Arc<Broker>,
    bound: Addr,
    token: Option<String>,
}

impl AdminState {
    /// State for tests and embedding.
    pub fn new(broker: Arc<Broker>, bound: Addr, token: Option<String>) -> Arc<Self> {
        Arc::new(AdminState {
            broker,
            bound,
            token: token.filter(|t| !t.is_empty()),
        })
    }
}

/// Check the loopback-or-token rule and bind the address, without serving
/// yet.
///
/// # Errors
///
/// [`AdminOpts::check`]'s error, and [`Error::Bind`] when the address
/// cannot be bound.
pub async fn bind(opts: AdminOpts) -> Result<AdminListener> {
    opts.check()?;
    let listener = TcpListener::bind(opts.bind)
        .await
        .map_err(|source| Error::Bind {
            addr: opts.bind,
            source,
        })?;
    let local_addr = listener.local_addr()?;
    Ok(AdminListener {
        listener,
        local_addr,
        token: opts.token,
    })
}

/// [`bind`] and [`AdminListener::serve`] in one step, for a broker that is
/// already running. `bound` is the protocol listener's address, reported by
/// `/v1/status`.
///
/// # Errors
///
/// Those of [`bind`].
pub async fn serve(
    opts: AdminOpts,
    broker: Arc<Broker>,
    bound: Addr,
    shutdown: CancellationToken,
) -> Result<AdminServer> {
    Ok(bind(opts).await?.serve(broker, bound, shutdown))
}

/// Build the router over shared state (exposed for tests).
pub fn router(state: Arc<AdminState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/v1/status", get(status))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_token,
        ))
        .with_state(state)
}

async fn require_token(State(app): State<Arc<AdminState>>, req: Request, next: Next) -> Response {
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

/// Compare two byte strings in time that depends on their lengths only.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

async fn metrics(State(app): State<Arc<AdminState>>) -> Response {
    let text = app.broker.render_metrics();
    ([(header::CONTENT_TYPE, METRICS_CONTENT_TYPE)], text).into_response()
}

async fn status(State(app): State<Arc<AdminState>>) -> Json<Status> {
    Json(app.broker.status(Some(app.bound.clone())))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::config::{BrokerPolicy, Limits, Timing, TlsPaths};
    use crate::transport::Client;

    fn broker() -> Arc<Broker> {
        crate::tls::install_default_provider();
        Broker::new(
            Arc::new(Client::new(
                TlsPaths::default(),
                Timing::fast(),
                Limits::default(),
            )),
            Timing::fast(),
            Limits::default(),
            BrokerPolicy::default(),
            CancellationToken::new(),
        )
    }

    /// An admin listener on an ephemeral loopback port over a fresh broker.
    async fn start(token: Option<&str>) -> (AdminServer, CancellationToken, Addr) {
        let bound = Addr::tcp("127.0.0.1", 12000);
        let shutdown = CancellationToken::new();
        let server = serve(
            AdminOpts {
                bind: "127.0.0.1:0".parse().unwrap(),
                token: token.map(str::to_owned),
            },
            broker(),
            bound.clone(),
            shutdown.clone(),
        )
        .await
        .unwrap();
        (server, shutdown, bound)
    }

    async fn call(
        server: &AdminServer,
        path: &str,
        token: Option<&str>,
    ) -> (StatusCode, String, String) {
        let mut req = reqwest::Client::new().get(format!("http://{}{path}", server.local_addr()));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = resp.text().await.unwrap();
        (status, content_type, body)
    }

    #[test]
    fn a_non_loopback_bind_needs_a_token() {
        let any = |token: Option<&str>| AdminOpts {
            bind: "0.0.0.0:0".parse().unwrap(),
            token: token.map(str::to_owned),
        };
        let err = any(None).check().unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        assert!(err.to_string().contains("--admin-token"), "{err}");
        assert!(any(Some("")).check().is_err(), "an empty token is none");
        assert!(any(Some("t")).check().is_ok());
        let local = AdminOpts {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: None,
        };
        assert!(local.check().is_ok());
        let local6 = AdminOpts {
            bind: "[::1]:0".parse().unwrap(),
            token: None,
        };
        assert!(local6.check().is_ok());
    }

    #[tokio::test]
    async fn bind_refuses_a_non_loopback_bind_without_a_token_before_binding() {
        // Port 0 on the unspecified address would bind fine; the check
        // must come first.
        let err = bind(AdminOpts {
            bind: "0.0.0.0:0".parse().unwrap(),
            token: None,
        })
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }

    #[tokio::test]
    async fn bind_reports_a_taken_port_and_holds_the_socket_until_served() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = held.local_addr().unwrap();
        let err = bind(AdminOpts {
            bind: taken,
            token: None,
        })
        .await
        .unwrap_err();
        assert!(
            matches!(err, Error::Bind { addr, .. } if addr == taken),
            "{err:?}"
        );
        drop(held);

        let bound = bind(AdminOpts {
            bind: "127.0.0.1:0".parse().unwrap(),
            token: None,
        })
        .await
        .unwrap();
        let addr = bound.local_addr();
        // Bound but not serving: a connection is accepted by the kernel's
        // backlog, but nothing answers; and the port cannot be taken again.
        assert!(std::net::TcpListener::bind(addr).is_err());
        let shutdown = CancellationToken::new();
        let server = bound.serve(broker(), Addr::tcp("127.0.0.1", 1), shutdown.clone());
        assert_eq!(server.local_addr(), addr);
        let resp = reqwest::Client::new()
            .get(format!("http://{addr}/healthz"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        shutdown.cancel();
        server.wait().await;
    }

    #[tokio::test]
    async fn routes_answer() {
        let (server, shutdown, bound) = start(None).await;
        let (status, ct, body) = call(&server, "/healthz", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(ct.starts_with("application/json"), "{ct}");
        assert_eq!(body, "{\"ok\":true}");

        let (status, ct, body) = call(&server, "/metrics", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ct, METRICS_CONTENT_TYPE);
        assert!(body.contains("# TYPE nsm_parties gauge\n"), "{body}");
        assert!(body.contains("nsm_build_info{"), "{body}");

        let (status, ct, body) = call(&server, "/v1/status", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(ct.starts_with("application/json"), "{ct}");
        let parsed: Status = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.bound, Some(bound));
        assert_eq!(parsed.counts.services, 0);
        assert_eq!(parsed.protocol_version, crate::protocol::PROTOCOL_VERSION);

        let (status, _, _) = call(&server, "/v1/nothing", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let url = format!("http://{}/healthz", server.local_addr());
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .expect("the admin listener stops when the token is cancelled");
        assert!(reqwest::Client::new().get(&url).send().await.is_err());
    }

    #[tokio::test]
    async fn the_token_guards_every_route() {
        let (empty, shutdown, _) = start(Some("")).await;
        let (status, _, _) = call(&empty, "/healthz", None).await;
        assert_eq!(status, StatusCode::OK, "an empty token is no token at all");
        shutdown.cancel();

        let (guarded, shutdown, _) = start(Some("s3cret")).await;
        for path in ["/healthz", "/metrics", "/v1/status"] {
            let (status, _, body) = call(&guarded, path, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            assert_eq!(body, "{\"error\":\"missing or invalid bearer token\"}");
            let (status, _, _) = call(&guarded, path, Some("wrong")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
            let (status, _, _) = call(&guarded, path, Some("s3cret")).await;
            assert_eq!(status, StatusCode::OK, "{path}");
        }
        shutdown.cancel();
    }

    #[test]
    fn constant_time_eq_compares_bytes() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
