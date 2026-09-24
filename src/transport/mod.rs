//! Transport abstraction: one request, one reply, over TCP, TCP+TLS, HTTP or
//! HTTPS.
//!
//! Everything above this module (broker, parties, operations) speaks
//! [`Message`]s and never sees sockets. A server side implements [`Handler`]
//! and is mounted with [`serve`] on an [`Addr`] whose transport decides the
//! wire; the client side is [`Client::call`], which dials the transport the
//! target [`Addr`] names. Both sides share [`protocol::codec`](crate::protocol::codec)
//! for the JSON body; the TCP flavours add the length-prefixed frame, the HTTP
//! flavours use `POST /v1/message`.
//!
//! Every connection carries exactly one request and one reply and is then
//! closed (HTTP clients may keep the connection pooled; that is invisible
//! here). Malformed input is dropped or answered with an HTTP error by the
//! transport itself and never reaches a handler; a handler that returns an
//! error produces a [`Message::Nack`] on HTTP and closes the connection on
//! TCP. Nothing in this module panics on peer input or holds a lock across an
//! `.await`.

pub mod http;
pub mod tcp;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{Limits, Timing, TlsPaths};
use crate::net::{Addr, Transport};
use crate::protocol::Message;
use crate::{Error, Result};

/// Who sent a request, as far as the transport can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerInfo {
    /// Remote socket address of the connection.
    pub remote: SocketAddr,
    /// Transport the request arrived on.
    pub transport: Transport,
}

/// Application logic behind a server: turns one request into one reply.
///
/// Implementations must be cheap to call concurrently (they are shared across
/// connections behind an [`Arc`]) and must not panic on the contents of
/// `msg`: unexpected variants are answered with [`Message::nack`].
pub trait Handler: Send + Sync + 'static {
    /// Handle one request.
    fn handle(&self, msg: Message, peer: PeerInfo) -> impl Future<Output = Result<Message>> + Send;
}

impl<H: Handler> Handler for Arc<H> {
    fn handle(&self, msg: Message, peer: PeerInfo) -> impl Future<Output = Result<Message>> + Send {
        (**self).handle(msg, peer)
    }
}

/// A running server. Dropping it does not stop the server; call
/// [`Server::shutdown`] (or cancel the token from [`Server::shutdown_token`]).
#[derive(Debug)]
pub struct Server {
    local_addr: SocketAddr,
    transport: Transport,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl Server {
    /// Assemble a server handle; used by the transport implementations.
    pub(crate) fn new(
        local_addr: SocketAddr,
        transport: Transport,
        shutdown: CancellationToken,
        task: JoinHandle<()>,
    ) -> Self {
        Server {
            local_addr,
            transport,
            shutdown,
            task,
        }
    }

    /// The socket address actually bound (useful after binding port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The bound address as an [`Addr`] peers can dial, including the
    /// transport.
    pub fn bound(&self) -> Addr {
        Addr::new(
            self.transport,
            self.local_addr.ip().to_string(),
            self.local_addr.port(),
        )
    }

    /// A token that stops the server when cancelled.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Stop accepting connections and wait for the accept loop to end.
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        let _ = self.task.await;
    }

    /// Wait until the server stops (because its token was cancelled).
    pub async fn wait(self) {
        let _ = self.task.await;
    }
}

/// Serve `handler` on `bind`, choosing the wire from `bind.transport`.
///
/// `bind` may use port 0; the actual address is in [`Server::local_addr`].
/// [`Transport::Tls`] and [`Transport::Https`] need [`TlsPaths`] with a
/// server identity, otherwise this is [`Error::Config`]. The server runs
/// until `shutdown` is cancelled.
pub async fn serve<H: Handler>(
    bind: &Addr,
    handler: Arc<H>,
    tls: &TlsPaths,
    limits: &Limits,
    timing: &Timing,
    shutdown: CancellationToken,
) -> Result<Server> {
    if bind.transport.is_tls() && !tls.has_server_identity() {
        return Err(Error::config(format!(
            "{bind} needs a certificate and key (--tls-cert/--tls-key or CERT_PATH/KEY_PATH) to serve TLS"
        )));
    }
    match bind.transport {
        Transport::Tcp | Transport::Tls => {
            tcp::serve(bind, handler, tls, limits, timing, shutdown).await
        }
        Transport::Http | Transport::Https => {
            http::serve(bind, handler, tls, limits, timing, shutdown).await
        }
    }
}

/// Client side of every transport: dials the address, sends one request,
/// returns the reply.
///
/// TLS material is loaded lazily, the first time an address that needs it is
/// dialled, so a client used only for plain TCP or HTTP never touches the
/// trust store.
#[derive(Debug)]
pub struct Client {
    timing: Timing,
    limits: Limits,
    tls: TlsPaths,
    tls_config: OnceLock<Arc<rustls::ClientConfig>>,
    http_plain: OnceLock<reqwest::Client>,
    http_tls: OnceLock<reqwest::Client>,
}

impl Client {
    /// A client with the given timeouts, limits and TLS material.
    pub fn new(tls: TlsPaths, timing: Timing, limits: Limits) -> Self {
        Client {
            timing,
            limits,
            tls,
            tls_config: OnceLock::new(),
            http_plain: OnceLock::new(),
            http_tls: OnceLock::new(),
        }
    }

    /// The timeouts this client applies.
    pub fn timing(&self) -> &Timing {
        &self.timing
    }

    /// The size limits this client applies.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The TLS material this client verifies peers with.
    pub fn tls_paths(&self) -> &TlsPaths {
        &self.tls
    }

    /// The rustls client configuration (ALPN `http/1.1` is set by the HTTP
    /// transport itself; framed TCP uses no ALPN), built on first use.
    pub(crate) fn tls_config(&self) -> Result<Arc<rustls::ClientConfig>> {
        if let Some(c) = self.tls_config.get() {
            return Ok(Arc::clone(c));
        }
        let built = crate::tls::client_config(&self.tls, &[])?;
        let _ = self.tls_config.set(Arc::clone(&built));
        Ok(built)
    }

    /// Lazily built `reqwest` client for plain HTTP or for HTTPS; see
    /// [`http`] for how each is configured.
    pub(crate) fn http_client(&self, tls: bool) -> Result<reqwest::Client> {
        let cell = if tls {
            &self.http_tls
        } else {
            &self.http_plain
        };
        if let Some(c) = cell.get() {
            return Ok(c.clone());
        }
        let built = http::build_client(self, tls)?;
        let _ = cell.set(built.clone());
        Ok(built)
    }

    /// Send `msg` to `to` and return the peer's reply.
    ///
    /// Connect and exchange are bounded by [`Timing::connect_timeout`] and
    /// [`Timing::request_timeout`]; a reply larger than
    /// [`Limits::max_frame_bytes`] is rejected. A [`Message::Nack`] is
    /// returned as an `Ok` value: it is a valid reply, and the caller decides
    /// what it means.
    pub async fn call(&self, to: &Addr, msg: Message) -> Result<Message> {
        match to.transport {
            Transport::Tcp | Transport::Tls => tcp::call(self, to, msg).await,
            Transport::Http | Transport::Https => http::call(self, to, msg).await,
        }
    }
}

/// Shared fixtures for the transport tests: an echo handler, a handler that
/// reports the peer, throwaway certificates, and a started server + client.
#[cfg(test)]
pub(crate) mod testing {
    use std::future::Future;
    use std::sync::Arc;

    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::{serve, Client, Handler, PeerInfo, Server};
    use crate::config::{Limits, Timing, TlsPaths};
    use crate::net::{Addr, Transport};
    use crate::protocol::Message;
    use crate::{Error, Result};

    /// Returns the request unchanged; fails on `Collect` so error paths can
    /// be exercised.
    pub struct Echo;

    impl Handler for Echo {
        async fn handle(&self, msg: Message, _peer: PeerInfo) -> Result<Message> {
            match msg {
                Message::Collect => Err(Error::protocol("boom")),
                other => Ok(other),
            }
        }
    }

    /// Answers with a `Nack` whose reason is `"<remote> <transport>"`.
    pub struct PeerReporter;

    impl Handler for PeerReporter {
        fn handle(
            &self,
            _msg: Message,
            peer: PeerInfo,
        ) -> impl Future<Output = Result<Message>> + Send {
            let reason = format!(
                "{} {}",
                peer.remote,
                serde_json::to_value(peer.transport)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            );
            async move { Ok(Message::nack(reason)) }
        }
    }

    /// A CA and a leaf for `localhost`/`127.0.0.1`, written to a temp dir.
    pub fn certs() -> (TempDir, TlsPaths) {
        crate::tls::install_default_provider();
        let dir = tempfile::tempdir().expect("tempdir");
        let ca_key = KeyPair::generate().expect("CA key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "nsm transport test CA");
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA certificate");
        let leaf_key = KeyPair::generate().expect("leaf key");
        let mut leaf_params =
            CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])
                .expect("leaf params");
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, "localhost");
        let leaf_cert = leaf_params
            .signed_by(&leaf_key, &ca_cert, &ca_key)
            .expect("leaf certificate");
        let ca = dir.path().join("ca.pem");
        let cert = dir.path().join("cert.pem");
        let key = dir.path().join("key.pem");
        std::fs::write(&ca, ca_cert.pem()).expect("write ca.pem");
        std::fs::write(&cert, leaf_cert.pem()).expect("write cert.pem");
        std::fs::write(&key, leaf_key.serialize_pem()).expect("write key.pem");
        (
            dir,
            TlsPaths {
                cert: Some(cert),
                key: Some(key),
                root_ca: Some(ca),
                system_roots: false,
            },
        )
    }

    /// Start `handler` on `127.0.0.1:0` over `transport` with fast timings
    /// and return the server, a matching client and the certificate dir.
    pub async fn start<H: Handler>(
        transport: Transport,
        handler: H,
    ) -> (Server, Client, Option<TempDir>) {
        crate::tls::install_default_provider();
        let (dir, tls) = if transport.is_tls() {
            let (d, t) = certs();
            (Some(d), t)
        } else {
            (None, TlsPaths::default())
        };
        let server = serve(
            &Addr::new(transport, "127.0.0.1", 0),
            Arc::new(handler),
            &tls,
            &Limits::default(),
            &Timing::fast(),
            CancellationToken::new(),
        )
        .await
        .expect("server starts");
        let client = Client::new(tls, Timing::fast(), Limits::default());
        (server, client, dir)
    }
}
