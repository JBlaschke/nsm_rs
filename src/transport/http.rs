//! HTTP and HTTPS: `POST /v1/message` with the JSON [`Message`] as body.
//!
//! Server side is an axum router served by hyper, one task per connection,
//! bounded by [`Limits::max_connections`]; for HTTPS the TLS handshake is done
//! with the acceptor from [`crate::tls`] before hyper sees the stream. Client
//! side is one memoised `reqwest` client per flavour: the plain one never
//! touches the trust store, the TLS one is built from the same rustls
//! configuration as the framed TCP transport and is `https_only`, so a
//! misconfigured scheme cannot silently downgrade to plaintext (audit S20).
//!
//! A malformed body is answered with `400` and a [`Message::Nack`] body before
//! any handler runs; a handler error is `400` (protocol-level rejection) or
//! `500` with a `Nack` body. `GET /healthz` answers `{"ok":true}`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hyper::service::Service as _;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use super::{Client, Handler, PeerInfo, Server};
use crate::config::{Limits, Timing, TlsPaths};
use crate::net::{Addr, Transport};
use crate::protocol::{decode, encode, Message};
use crate::{Error, Result};

struct AppState<H> {
    handler: Arc<H>,
    transport: Transport,
}

fn router<H: Handler>(handler: Arc<H>, transport: Transport, limits: &Limits) -> Router {
    Router::new()
        .route("/v1/message", post(message::<H>))
        .route("/healthz", get(healthz))
        .layer(DefaultBodyLimit::max(limits.max_frame_bytes))
        .with_state(Arc::new(AppState { handler, transport }))
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

async fn message<H: Handler>(
    State(app): State<Arc<AppState<H>>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    body: Bytes,
) -> Response {
    let msg = match decode(&body) {
        Ok(msg) => msg,
        Err(e) => return nack(StatusCode::BAD_REQUEST, format!("invalid message: {e}")),
    };
    trace!(%remote, kind = msg.kind(), "request");
    let peer = PeerInfo {
        remote,
        transport: app.transport,
    };
    match app.handler.handle(msg, peer).await {
        Ok(reply) => (StatusCode::OK, Json(reply)).into_response(),
        Err(e) => {
            let status = match e {
                Error::Protocol(_) | Error::Rejected(_) | Error::NoService(_) => {
                    StatusCode::BAD_REQUEST
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            nack(status, e.to_string())
        }
    }
}

fn nack(status: StatusCode, reason: String) -> Response {
    (status, Json(Message::nack(reason))).into_response()
}

/// Serve `handler` on an HTTP (or HTTPS) listener bound to `bind`.
pub(super) async fn serve<H: Handler>(
    bind: &Addr,
    handler: Arc<H>,
    tls: &TlsPaths,
    limits: &Limits,
    timing: &Timing,
    shutdown: CancellationToken,
) -> Result<Server> {
    let sock = bind.resolve().await?;
    let listener = TcpListener::bind(sock)
        .await
        .map_err(|source| Error::Bind { addr: sock, source })?;
    let local_addr = listener.local_addr()?;
    let transport = bind.transport;
    let acceptor = if transport.is_tls() {
        Some(crate::tls::acceptor(tls, &["http/1.1"])?)
    } else {
        None
    };
    let router = router(handler, transport, limits);
    let limits = limits.clone();
    let timing = timing.clone();
    let token = shutdown.clone();

    let task = tokio::spawn(async move {
        let permits = Arc::new(Semaphore::new(limits.max_connections.max(1)));
        let mut conns: JoinSet<()> = JoinSet::new();
        loop {
            // Reap connection tasks that have already finished.
            while conns.try_join_next().is_some() {}
            let permit = tokio::select! {
                _ = token.cancelled() => break,
                p = Arc::clone(&permits).acquire_owned() => match p {
                    Ok(p) => p,
                    Err(_) => break,
                },
            };
            let (stream, remote) = tokio::select! {
                _ = token.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        warn!(error = %e, "accept failed; retrying");
                        drop(permit);
                        sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                },
            };
            let router = router.clone();
            let acceptor = acceptor.clone();
            let timing = timing.clone();
            let conn_token = token.clone();
            conns.spawn(async move {
                let _permit = permit;
                tokio::select! {
                    _ = conn_token.cancelled() => {}
                    result = handle_connection(stream, remote, acceptor, router, &timing) => {
                        if let Err(e) = result {
                            debug!(%remote, error = %e, "connection dropped");
                        }
                    }
                }
            });
        }
        // Connections observe the same token; wait for them to wind down.
        while conns.join_next().await.is_some() {}
        debug!(%local_addr, "http listener stopped");
    });

    Ok(Server::new(local_addr, transport, shutdown, task))
}

async fn handle_connection(
    stream: TcpStream,
    remote: SocketAddr,
    acceptor: Option<TlsAcceptor>,
    router: Router,
    timing: &Timing,
) -> Result<()> {
    let _ = stream.set_nodelay(true);
    let hyper_service = TowerToHyperService::new(router);
    let service =
        hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
            req.extensions_mut().insert(ConnectInfo(remote));
            hyper_service.call(req)
        });
    // HTTP/1.1 only: both sides offer just `http/1.1`, and skipping the auto
    // builder's version sniffing matters, because that phase is not covered by
    // hyper's header timeout: a client that connects and stays silent would
    // otherwise hold a connection permit until it went away. With
    // `http1_only` the header timeout runs from the first poll of every
    // request, idle keep-alive waits included.
    let mut builder = Builder::new(TokioExecutor::new()).http1_only();
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(timing.request_timeout);
    match acceptor {
        Some(acceptor) => {
            let tls_stream = timeout(timing.connect_timeout, acceptor.accept(stream))
                .await
                .map_err(|_| Error::Timeout(timing.connect_timeout))??;
            builder
                .serve_connection(TokioIo::new(tls_stream), service)
                .await
                .map_err(|e| Error::protocol(format!("http connection: {e}")))
        }
        None => builder
            .serve_connection(TokioIo::new(stream), service)
            .await
            .map_err(|e| Error::protocol(format!("http connection: {e}"))),
    }
}

/// Build the `reqwest` client used for plain HTTP (`tls == false`) or HTTPS.
pub(super) fn build_client(client: &Client, tls: bool) -> Result<reqwest::Client> {
    let t = client.timing();
    let mut builder = reqwest::Client::builder()
        .connect_timeout(t.connect_timeout)
        .timeout(t.request_timeout)
        .pool_idle_timeout(Some(Duration::from_secs(30)))
        .no_proxy();
    let config = if tls {
        let mut config = (*client.tls_config()?).clone();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        builder = builder.https_only(true);
        config
    } else {
        // Never used for TLS (plain URLs only), but reqwest builds its TLS
        // connector eagerly; an empty root store keeps it away from the
        // platform trust store.
        crate::tls::install_default_provider();
        rustls::ClientConfig::builder_with_provider(
            rustls::crypto::CryptoProvider::get_default()
                .cloned()
                .ok_or_else(|| {
                    Error::config(
                        "no rustls crypto provider is compiled in; build with the `aws-lc-rs` or `ring` feature",
                    )
                })?,
        )
        .with_safe_default_protocol_versions()?
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth()
    };
    builder
        .use_preconfigured_tls(config)
        .build()
        .map_err(|e| Error::config(format!("cannot build HTTP client: {e}")))
}

/// One `POST /v1/message` exchange with `to`.
pub(super) async fn call(client: &Client, to: &Addr, msg: Message) -> Result<Message> {
    let url = to
        .url("v1/message")
        .ok_or_else(|| Error::protocol(format!("{to} is not an HTTP address")))?;
    let http = client.http_client(to.transport.is_tls())?;
    let max = client.limits().max_frame_bytes;
    let body = encode(&msg)?;
    if body.len() > max {
        return Err(Error::FrameTooLarge {
            size: body.len(),
            limit: max,
        });
    }
    let response = http
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| map_reqwest(e, &url, client.timing()))?;
    let status = response.status();
    if let Some(len) = response.content_length() {
        if len > max as u64 {
            return Err(Error::FrameTooLarge {
                size: len as usize,
                limit: max,
            });
        }
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| map_reqwest(e, &url, client.timing()))?
    {
        if bytes.len() + chunk.len() > max {
            return Err(Error::FrameTooLarge {
                size: bytes.len() + chunk.len(),
                limit: max,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    match decode(&bytes) {
        Ok(reply) => Ok(reply),
        // A non-2xx whose body is a `Message` (a `Nack`, typically) is a valid
        // reply and was returned above; anything else is not our protocol.
        Err(e) if status.is_success() => Err(Error::protocol(format!(
            "HTTP {status} from {url}: reply is not a message: {e}"
        ))),
        Err(_) => Err(Error::protocol(format!("HTTP {status} from {url}"))),
    }
}

/// Map a `reqwest` failure onto the crate error: deadlines become
/// [`Error::Timeout`] (the connect deadline when the connection never came up,
/// the request deadline otherwise), connection failures become [`Error::Io`]
/// with the underlying kind when there is one, and everything else is a
/// protocol error. The URL is always in the text; `reqwest` prints it too, so
/// its own copy is stripped first.
fn map_reqwest(e: reqwest::Error, url: &str, timing: &Timing) -> Error {
    let connect = e.is_connect();
    if e.is_timeout() {
        return Error::Timeout(if connect {
            timing.connect_timeout
        } else {
            timing.request_timeout
        });
    }
    let e = e.without_url();
    if connect {
        let kind = io_kind(&e).unwrap_or(std::io::ErrorKind::ConnectionRefused);
        return Error::Io(std::io::Error::new(
            kind,
            format!("connecting to {url}: {e}"),
        ));
    }
    Error::protocol(format!("{url}: {e}"))
}

/// The kind of the first `io::Error` in `e`'s source chain, if any.
fn io_kind(e: &(dyn std::error::Error + 'static)) -> Option<std::io::ErrorKind> {
    let mut source = e.source();
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return Some(io.kind());
        }
        source = err.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::super::testing::{certs, start, Echo, PeerReporter};
    use super::*;
    use crate::protocol::PartyId;

    const DEADLINE: Duration = Duration::from_secs(10);

    #[tokio::test]
    async fn round_trip_over_http_and_https() {
        for transport in [Transport::Http, Transport::Https] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) = start(transport, Echo).await;
                let reply = client
                    .call(&server.bound(), Message::Ping { id: PartyId(7) })
                    .await
                    .unwrap();
                assert_eq!(reply, Message::Ping { id: PartyId(7) }, "{transport:?}");
                let reply = client
                    .call(&server.bound(), Message::nack("no"))
                    .await
                    .unwrap();
                assert_eq!(reply, Message::nack("no"));
                server.shutdown().await;
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn handler_error_becomes_a_nack() {
        for transport in [Transport::Http, Transport::Https] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) = start(transport, Echo).await;
                let reply = client
                    .call(&server.bound(), Message::Collect)
                    .await
                    .unwrap();
                match reply {
                    Message::Nack { reason } => {
                        assert!(reason.contains("boom"), "{transport:?}: {reason}");
                    }
                    other => panic!("{transport:?}: {other:?}"),
                }
                server.shutdown().await;
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn malformed_bodies_get_400_and_the_server_keeps_going() {
        tokio::time::timeout(DEADLINE, async {
            let (server, client, _certs) = start(Transport::Http, Echo).await;
            let url = server.bound().url("v1/message").unwrap();
            let raw = client.http_client(false).unwrap();
            for body in ["not json", r#"{"type":"nope"}"#, r#"{"type":"ping"}"#] {
                let resp = raw.post(&url).body(body).send().await.unwrap();
                assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
                let text = resp.text().await.unwrap();
                let nack: Message = serde_json::from_str(&text).unwrap();
                assert!(matches!(nack, Message::Nack { .. }), "{text}");
            }
            let health = raw
                .get(server.bound().url("healthz").unwrap())
                .send()
                .await
                .unwrap();
            assert_eq!(health.status(), StatusCode::OK);
            let reply = client
                .call(&server.bound(), Message::Delivered)
                .await
                .unwrap();
            assert_eq!(reply, Message::Delivered);
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn oversized_bodies_are_refused() {
        tokio::time::timeout(DEADLINE, async {
            let (server, _client, _certs) = start(Transport::Http, Echo).await;
            let tiny = Client::new(
                TlsPaths::default(),
                Timing::fast(),
                Limits {
                    max_frame_bytes: 8,
                    ..Limits::default()
                },
            );
            let err = tiny
                .call(&server.bound(), Message::Delivered)
                .await
                .unwrap_err();
            assert!(matches!(err, Error::FrameTooLarge { .. }), "{err:?}");
            let huge = Client::new(
                TlsPaths::default(),
                Timing::fast(),
                Limits {
                    max_frame_bytes: 10 * 1024 * 1024,
                    ..Limits::default()
                },
            );
            let big = Message::nack("x".repeat(Limits::default().max_frame_bytes + 1));
            let err = huge.call(&server.bound(), big).await.unwrap_err();
            assert!(
                matches!(err, Error::Protocol(ref m) if m.contains("413")),
                "{err:?}"
            );
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn https_client_with_a_foreign_root_is_rejected() {
        tokio::time::timeout(DEADLINE, async {
            let (server, _client, _certs) = start(Transport::Https, Echo).await;
            let (_dir, other) = certs();
            let stranger = Client::new(other, Timing::fast(), Limits::default());
            let err = stranger
                .call(&server.bound(), Message::Delivered)
                .await
                .unwrap_err();
            assert!(!matches!(err, Error::Config(_)), "{err:?}");
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn plain_client_refuses_https_urls_only_via_scheme() {
        // The TLS client is https_only; a plain-http address dialled through
        // it would be a programming error, so the dispatcher chooses by scheme.
        tokio::time::timeout(DEADLINE, async {
            let (server, client, _certs) = start(Transport::Https, Echo).await;
            let plain = server.bound().with_transport(Transport::Http);
            let err = client.call(&plain, Message::Delivered).await.unwrap_err();
            assert!(!matches!(err, Error::Config(_)), "{err:?}");
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn peer_info_reports_loopback_and_transport() {
        for (transport, tag) in [(Transport::Http, "http"), (Transport::Https, "https")] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) = start(transport, PeerReporter).await;
                let reply = client
                    .call(&server.bound(), Message::Collect)
                    .await
                    .unwrap();
                match reply {
                    Message::Nack { reason } => {
                        assert!(reason.starts_with("127.0.0.1:"), "{reason}");
                        assert!(reason.ends_with(tag), "{transport:?}: {reason}");
                    }
                    other => panic!("{other:?}"),
                }
                server.shutdown().await;
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn shutdown_stops_accepting() {
        for transport in [Transport::Http, Transport::Https] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) = start(transport, Echo).await;
                let addr = server.bound();
                // The first call leaves a pooled connection behind; shutdown
                // must end that too, not only the listener.
                client.call(&addr, Message::Delivered).await.unwrap();
                server.shutdown().await;
                let err = client.call(&addr, Message::Delivered).await.unwrap_err();
                assert!(
                    matches!(err, Error::Io(_) | Error::Timeout(_) | Error::Protocol(_)),
                    "{transport:?}: {err:?}"
                );
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn idle_connection_does_not_hold_the_listener_hostage() {
        tokio::time::timeout(DEADLINE, async {
            let server = super::super::serve(
                &Addr::new(Transport::Http, "127.0.0.1", 0),
                Arc::new(Echo),
                &TlsPaths::default(),
                &Limits {
                    max_connections: 1,
                    ..Limits::default()
                },
                &Timing::fast(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
            // Connects and never sends a byte: it holds the only permit until
            // hyper's header timeout (the request timeout) drops it.
            let _idle = TcpStream::connect(server.local_addr()).await.unwrap();
            let patient = Client::new(
                TlsPaths::default(),
                Timing {
                    request_timeout: Duration::from_secs(5),
                    ..Timing::fast()
                },
                Limits::default(),
            );
            let reply = patient
                .call(&server.bound(), Message::Delivered)
                .await
                .unwrap();
            assert_eq!(reply, Message::Delivered);
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn foreign_replies_timeouts_and_refusals_are_mapped() {
        tokio::time::timeout(DEADLINE, async {
            let client = Client::new(TlsPaths::default(), Timing::fast(), Limits::default());
            let ping = Message::Ping { id: PartyId(7) };

            // Not our protocol at all: a plain-text error page.
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let target = Addr::new(Transport::Http, "127.0.0.1", port);
            let page = tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                s.write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 3\r\nconnection: close\r\n\r\nbad",
                )
                .await
                .unwrap();
                // Drain until the client closes so no unread bytes turn the
                // close into a reset.
                let _ = s.read_to_end(&mut Vec::new()).await;
            });
            match client.call(&target, ping.clone()).await {
                Err(Error::Protocol(m)) => {
                    assert!(m.contains("502") && m.contains(&target.to_string()), "{m}");
                }
                other => panic!("expected a protocol error, got {other:?}"),
            }
            page.await.unwrap();

            // Accepts, then never answers.
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let target = Addr::new(Transport::Http, "127.0.0.1", port);
            let mute = tokio::spawn(async move {
                let (_s, _) = listener.accept().await.unwrap();
                sleep(Duration::from_secs(5)).await;
            });
            match client.call(&target, ping.clone()).await {
                Err(Error::Timeout(d)) => assert_eq!(d, Timing::fast().request_timeout),
                other => panic!("expected a timeout, got {other:?}"),
            }
            mute.abort();

            // Nobody listens.
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            let target = Addr::new(Transport::Http, "127.0.0.1", port);
            match client.call(&target, ping).await {
                Err(Error::Io(e)) => assert!(e.to_string().contains(&target.to_string()), "{e}"),
                other => panic!("expected an I/O error, got {other:?}"),
            }
        })
        .await
        .unwrap();
    }
}
