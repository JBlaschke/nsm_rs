//! Framed TCP and TCP+TLS: one request, one reply per connection.
//!
//! The server accepts a connection, optionally completes a TLS handshake,
//! reads exactly one length-prefixed [`Message`], runs the handler, writes
//! the reply and closes. Every step is bounded by a timeout from
//! [`Timing`], the number of concurrently served connections by
//! [`Limits::max_connections`], and a bad connection is logged and dropped
//! without touching the accept loop (the old code awaited each handler inline
//! and one malformed frame ended accepting for good, audit S1/S2/P4).
//!
//! A handler error closes the connection without a reply; on this transport
//! that is the only way to say "no" below the protocol level, and the client
//! sees [`Error::Closed`].

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use super::{Client, Handler, PeerInfo, Server};
use crate::config::{Limits, Timing, TlsPaths};
use crate::net::{Addr, Transport};
use crate::protocol::{framed, Message};
use crate::{Error, Result};

/// Serve `handler` on a TCP (or TLS) listener bound to `bind`.
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
        Some(crate::tls::acceptor(tls, &[])?)
    } else {
        None
    };
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
            let handler = Arc::clone(&handler);
            let acceptor = acceptor.clone();
            let limits = limits.clone();
            let timing = timing.clone();
            let conn_token = token.clone();
            conns.spawn(async move {
                let _permit = permit;
                tokio::select! {
                    _ = conn_token.cancelled() => {}
                    result = handle_connection(
                        stream, remote, transport, acceptor, handler, &limits, &timing,
                    ) => {
                        if let Err(e) = result {
                            debug!(%remote, error = %e, "connection dropped");
                        }
                    }
                }
            });
        }
        // Connections observe the same token; wait for them to wind down.
        while conns.join_next().await.is_some() {}
        debug!(%local_addr, "tcp listener stopped");
    });

    Ok(Server::new(local_addr, transport, shutdown, task))
}

async fn handle_connection<H: Handler>(
    stream: TcpStream,
    remote: SocketAddr,
    transport: Transport,
    acceptor: Option<TlsAcceptor>,
    handler: Arc<H>,
    limits: &Limits,
    timing: &Timing,
) -> Result<()> {
    let _ = stream.set_nodelay(true);
    let peer = PeerInfo { remote, transport };
    match acceptor {
        Some(acceptor) => {
            let tls_stream = timeout(timing.connect_timeout, acceptor.accept(stream))
                .await
                .map_err(|_| Error::Timeout(timing.connect_timeout))??;
            exchange(tls_stream, peer, handler, limits, timing).await
        }
        None => exchange(stream, peer, handler, limits, timing).await,
    }
}

/// Read one request, answer it, close.
async fn exchange<S, H>(
    io: S,
    peer: PeerInfo,
    handler: Arc<H>,
    limits: &Limits,
    timing: &Timing,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Handler,
{
    let mut framed = framed(io, limits.max_frame_bytes);
    let request = match timeout(timing.request_timeout, framed.next()).await {
        Err(_) => return Err(Error::Timeout(timing.request_timeout)),
        Ok(None) => return Err(Error::Closed),
        Ok(Some(Err(e))) => return Err(e),
        Ok(Some(Ok(msg))) => msg,
    };
    trace!(remote = %peer.remote, kind = request.kind(), "request");
    // The handler gets the same deadline as each I/O step, so a stuck handler
    // cannot hold a connection permit forever.
    let reply = match timeout(timing.request_timeout, handler.handle(request, peer)).await {
        Err(_) => return Err(Error::Timeout(timing.request_timeout)),
        Ok(Err(e)) => {
            // No reply on this transport. Close cleanly (FIN, and close_notify
            // over TLS) so the peer reads EOF rather than a reset.
            let _ = framed.get_mut().shutdown().await;
            return Err(e);
        }
        Ok(Ok(reply)) => reply,
    };
    timeout(timing.request_timeout, framed.send(reply))
        .await
        .map_err(|_| Error::Timeout(timing.request_timeout))??;
    let mut io = framed.into_inner();
    let _ = io.shutdown().await;
    Ok(())
}

/// One framed request/reply exchange with `to`.
pub(super) async fn call(client: &Client, to: &Addr, msg: Message) -> Result<Message> {
    let t = client.timing();
    let sock = to.resolve().await?;
    let stream = timeout(t.connect_timeout, TcpStream::connect(sock))
        .await
        .map_err(|_| Error::Timeout(t.connect_timeout))??;
    let _ = stream.set_nodelay(true);
    let max = client.limits().max_frame_bytes;
    if to.transport.is_tls() {
        let connector = TlsConnector::from(client.tls_config()?);
        let name = crate::tls::server_name(&to.host)?;
        let tls_stream = timeout(t.connect_timeout, connector.connect(name, stream))
            .await
            .map_err(|_| Error::Timeout(t.connect_timeout))??;
        roundtrip(tls_stream, msg, max, t.request_timeout).await
    } else {
        roundtrip(stream, msg, max, t.request_timeout).await
    }
}

async fn roundtrip<S: AsyncRead + AsyncWrite + Unpin>(
    io: S,
    msg: Message,
    max_frame: usize,
    deadline: Duration,
) -> Result<Message> {
    let mut framed = framed(io, max_frame);
    timeout(deadline, framed.send(msg))
        .await
        .map_err(|_| Error::Timeout(deadline))??;
    match timeout(deadline, framed.next()).await {
        Err(_) => Err(Error::Timeout(deadline)),
        Ok(None) => Err(Error::Closed),
        Ok(Some(reply)) => reply,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::super::testing::{certs, start, Echo};
    use super::*;
    use crate::protocol::PartyId;

    const DEADLINE: Duration = Duration::from_secs(10);

    #[tokio::test]
    async fn round_trip_over_tcp_and_tls() {
        for transport in [Transport::Tcp, Transport::Tls] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) = start(transport, Echo).await;
                let reply = client
                    .call(&server.bound(), Message::Ping { id: PartyId(7) })
                    .await
                    .unwrap();
                assert_eq!(reply, Message::Ping { id: PartyId(7) }, "{transport:?}");
                // A Nack is a reply like any other.
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
    async fn handler_error_closes_the_connection() {
        for transport in [Transport::Tcp, Transport::Tls] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) = start(transport, Echo).await;
                let err = client
                    .call(&server.bound(), Message::Collect)
                    .await
                    .unwrap_err();
                // A clean close on both flavours: over TLS the server sends
                // close_notify, so this is EOF and not an I/O error.
                assert!(matches!(err, Error::Closed), "{transport:?}: {err:?}");
                server.shutdown().await;
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn garbage_and_probes_do_not_stop_the_server() {
        tokio::time::timeout(DEADLINE, async {
            let (server, client, _certs) = start(Transport::Tcp, Echo).await;
            let addr = server.local_addr();
            // Oversized prefix, then junk, then connect-and-close.
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(&[0xff, 0xff, 0xff, 0xff, b'x']).await.unwrap();
            drop(s);
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(&[0, 0, 0, 3, b'a', b'b', b'c']).await.unwrap();
            drop(s);
            drop(TcpStream::connect(addr).await.unwrap());
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
    async fn oversized_messages_are_refused_on_both_sides() {
        tokio::time::timeout(DEADLINE, async {
            let (server, _client, _certs) = start(Transport::Tcp, Echo).await;
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
            // A client with a big limit sending a frame above the server's limit
            // sees the connection closed without a reply.
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
            assert!(err.is_disconnect(), "{err:?}");
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn tls_client_with_a_foreign_root_is_rejected() {
        tokio::time::timeout(DEADLINE, async {
            let (server, _client, _certs) = start(Transport::Tls, Echo).await;
            let (_other_dir, other) = certs();
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
    async fn shutdown_stops_accepting() {
        for transport in [Transport::Tcp, Transport::Tls] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) = start(transport, Echo).await;
                let addr = server.bound();
                client.call(&addr, Message::Delivered).await.unwrap();
                server.shutdown().await;
                let err = client.call(&addr, Message::Delivered).await.unwrap_err();
                assert!(
                    matches!(err, Error::Io(_) | Error::Timeout(_)),
                    "{transport:?}: {err:?}"
                );
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn peer_info_reports_loopback_and_transport() {
        for (transport, tag) in [(Transport::Tcp, "tcp"), (Transport::Tls, "tls")] {
            tokio::time::timeout(DEADLINE, async {
                let (server, client, _certs) =
                    start(transport, super::super::testing::PeerReporter).await;
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
    async fn tls_listener_without_identity_is_a_config_error() {
        let err = super::super::serve(
            &Addr::new(Transport::Tls, "127.0.0.1", 0),
            Arc::new(Echo),
            &TlsPaths::default(),
            &Limits::default(),
            &Timing::fast(),
            CancellationToken::new(),
        )
        .await
        .expect_err("must fail");
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }

    #[tokio::test]
    async fn oversized_prefix_closes_the_connection_before_any_body() {
        tokio::time::timeout(DEADLINE, async {
            let (server, _client, _certs) = start(Transport::Tcp, Echo).await;
            let mut s = TcpStream::connect(server.local_addr()).await.unwrap();
            let too_big = u32::try_from(Limits::default().max_frame_bytes + 1).unwrap();
            s.write_all(&too_big.to_be_bytes()).await.unwrap();
            // No body follows; the server must close rather than wait for one.
            let mut sink = Vec::new();
            let n = s.read_to_end(&mut sink).await.unwrap_or(0);
            assert_eq!(n, 0, "nothing may be sent back: {sink:?}");
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn silent_connection_is_dropped_at_the_request_timeout() {
        tokio::time::timeout(DEADLINE, async {
            let (server, _client, _certs) = start(Transport::Tcp, Echo).await;
            let mut s = TcpStream::connect(server.local_addr()).await.unwrap();
            let started = std::time::Instant::now();
            let mut sink = Vec::new();
            s.read_to_end(&mut sink).await.unwrap_or(0);
            let elapsed = started.elapsed();
            assert!(
                elapsed >= Timing::fast().request_timeout / 2,
                "closed too early: {elapsed:?}"
            );
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn slow_handler_is_cut_off_at_the_request_timeout() {
        struct Sleepy;
        impl Handler for Sleepy {
            async fn handle(&self, _msg: Message, _peer: PeerInfo) -> Result<Message> {
                sleep(Duration::from_secs(5)).await;
                Ok(Message::Delivered)
            }
        }
        tokio::time::timeout(DEADLINE, async {
            let (server, _client, _certs) = start(Transport::Tcp, Sleepy).await;
            // More patience than the server has, so the server's deadline is
            // the one that fires.
            let patient = Client::new(
                TlsPaths::default(),
                Timing {
                    request_timeout: Duration::from_secs(5),
                    ..Timing::fast()
                },
                Limits::default(),
            );
            let started = std::time::Instant::now();
            let err = patient
                .call(&server.bound(), Message::Delivered)
                .await
                .unwrap_err();
            assert!(err.is_disconnect(), "{err:?}");
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "{:?}",
                started.elapsed()
            );
            server.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn connection_limit_queues_without_starving() {
        tokio::time::timeout(DEADLINE, async {
            let server = super::super::serve(
                &Addr::new(Transport::Tcp, "127.0.0.1", 0),
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
            // Holds the only permit until the server times it out.
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
}
