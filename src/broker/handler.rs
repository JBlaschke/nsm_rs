//! The request handler mounted on the broker's listener.
//!
//! One `match` over [`Message`], written once for every transport. Anything
//! the broker does not expect is a [`Message::Nack`]; nothing here panics on
//! peer input, and the registry lock is never held across an `.await`.

use std::future::Future;
use std::sync::Arc;

use tokio::time::{sleep, Instant};
use tracing::{debug, trace};

use super::monitor::Broker;
use crate::net::Addr;
use crate::protocol::Message;
use crate::transport::{Handler, PeerInfo};
use crate::{Error, Result};

/// [`Handler`] for the broker.
#[derive(Debug, Clone)]
pub struct BrokerHandler {
    broker: Arc<Broker>,
}

impl BrokerHandler {
    /// A handler over the given broker state.
    pub fn new(broker: Arc<Broker>) -> Self {
        BrokerHandler { broker }
    }

    /// Checks every registration must pass before it touches the registry:
    /// a real port, an advertised host that matches the connection source
    /// when the policy demands it, and the per-host registration cap.
    fn admission(&self, bind_addr: &Addr, peer: &PeerInfo) -> Option<Message> {
        if bind_addr.port == 0 {
            return Some(Message::nack("bind_addr must carry the actual port"));
        }
        let policy = self.broker.policy();
        let source = peer.remote.ip().to_string();
        if bind_addr.host != source {
            if policy.require_matching_host {
                return Some(Message::nack(format!(
                    "advertised host {} does not match the connection source {source}",
                    bind_addr.host
                )));
            }
            debug!(remote = %peer.remote, advertised = %bind_addr.host, "party advertises an address other than the one it connected from");
        }
        let count = self
            .broker
            .with_registry(|r| r.count_for_host(&bind_addr.host));
        if count >= policy.max_registrations_per_host {
            return Some(Message::nack(format!(
                "too many registrations from {} ({count})",
                bind_addr.host
            )));
        }
        None
    }

    async fn dispatch(&self, msg: Message, peer: PeerInfo) -> Result<Message> {
        trace!(kind = msg.kind(), remote = %peer.remote, "broker request");
        match msg {
            Message::Publish {
                key,
                service_port,
                bind_addr,
                ping,
            } => {
                if let Some(refusal) = self.admission(&bind_addr, &peer) {
                    return Ok(refusal);
                }
                let service_addr = Addr::tcp(bind_addr.host.clone(), service_port);
                let now = Instant::now();
                match self
                    .broker
                    .with_registry(|r| r.publish(key, service_addr, bind_addr, ping, now))
                {
                    Ok(id) => {
                        self.broker.watch(id);
                        Ok(Message::Registered { id })
                    }
                    Err(Error::Rejected(reason)) => Ok(Message::nack(reason)),
                    Err(e) => Err(e),
                }
            }

            Message::Claim {
                key,
                bind_addr,
                ping,
            } => {
                if let Some(refusal) = self.admission(&bind_addr, &peer) {
                    return Ok(refusal);
                }
                let t = self.broker.timing();
                let deadline = Instant::now() + t.claim_wait;
                let pause = (t.claim_wait / 5).max(std::time::Duration::from_millis(1));
                loop {
                    let now = Instant::now();
                    match self
                        .broker
                        .with_registry(|r| r.claim(key, bind_addr.clone(), ping, now))
                    {
                        Ok((id, service)) => {
                            self.broker.watch(id);
                            return Ok(Message::Paired { id, service });
                        }
                        Err(Error::NoService(_)) if Instant::now() < deadline => {
                            // A client may start before its service has
                            // published; wait a little without holding the lock.
                            sleep(pause).await;
                        }
                        Err(Error::NoService(k)) => {
                            return Ok(Message::nack(format!("no service available for key {k}")));
                        }
                        Err(Error::Rejected(reason)) => return Ok(Message::nack(reason)),
                        Err(e) => return Err(e),
                    }
                }
            }

            Message::Ping { id } => {
                let now = Instant::now();
                let reply = self.broker.with_registry(|r| {
                    if r.mark_alive(id, now) {
                        r.heartbeat_for(id)
                    } else {
                        None
                    }
                });
                Ok(reply.unwrap_or_else(|| Message::nack(format!("unknown party {id}"))))
            }

            Message::Deliver { to, text } => {
                match self.broker.with_registry(|r| r.deliver(to, text)) {
                    Ok(()) => Ok(Message::Delivered),
                    Err(Error::Rejected(reason)) => Ok(Message::nack(reason)),
                    Err(e) => Err(e),
                }
            }

            other => Ok(Message::nack(format!(
                "unexpected {} at the broker",
                other.kind()
            ))),
        }
    }
}

impl Handler for BrokerHandler {
    fn handle(&self, msg: Message, peer: PeerInfo) -> impl Future<Output = Result<Message>> + Send {
        self.dispatch(msg, peer)
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::config::{BrokerPolicy, Limits, Timing, TlsPaths};
    use crate::net::Transport;
    use crate::protocol::PartyId;
    use crate::transport::Client;

    fn handler(policy: BrokerPolicy) -> BrokerHandler {
        let client = Arc::new(Client::new(
            TlsPaths::default(),
            Timing::fast(),
            Limits::default(),
        ));
        BrokerHandler::new(Broker::new(
            client,
            Timing::fast(),
            Limits::default(),
            policy,
            CancellationToken::new(),
        ))
    }

    fn peer() -> PeerInfo {
        PeerInfo {
            remote: "127.0.0.1:40000".parse::<SocketAddr>().unwrap(),
            transport: Transport::Tcp,
        }
    }

    fn publish(host: &str, port: u16) -> Message {
        Message::Publish {
            key: 1,
            service_port: 9000,
            bind_addr: Addr::tcp(host, port),
            ping: true, // no heartbeat task is spawned for ping parties
        }
    }

    #[tokio::test]
    async fn zero_port_is_refused() {
        let h = handler(BrokerPolicy::default());
        let reply = h.handle(publish("127.0.0.1", 0), peer()).await.unwrap();
        assert!(matches!(reply, Message::Nack { .. }), "{reply:?}");
    }

    #[tokio::test]
    async fn matching_host_is_enforced_only_when_asked() {
        let lenient = handler(BrokerPolicy::default());
        let reply = lenient
            .handle(publish("10.0.0.9", 1), peer())
            .await
            .unwrap();
        assert!(matches!(reply, Message::Registered { .. }), "{reply:?}");

        let strict = handler(BrokerPolicy {
            require_matching_host: true,
            ..BrokerPolicy::default()
        });
        let reply = strict.handle(publish("10.0.0.9", 1), peer()).await.unwrap();
        match reply {
            Message::Nack { reason } => assert!(reason.contains("10.0.0.9"), "{reason}"),
            other => panic!("{other:?}"),
        }
        let reply = strict
            .handle(publish("127.0.0.1", 1), peer())
            .await
            .unwrap();
        assert!(matches!(reply, Message::Registered { .. }), "{reply:?}");
    }

    #[tokio::test]
    async fn per_host_cap_counts_services_and_clients() {
        let h = handler(BrokerPolicy {
            max_registrations_per_host: 2,
            ..BrokerPolicy::default()
        });
        assert!(matches!(
            h.handle(publish("127.0.0.1", 1), peer()).await.unwrap(),
            Message::Registered { .. }
        ));
        let claim = Message::Claim {
            key: 1,
            bind_addr: Addr::tcp("127.0.0.1", 2),
            ping: true,
        };
        assert!(matches!(
            h.handle(claim, peer()).await.unwrap(),
            Message::Paired { .. }
        ));
        match h.handle(publish("127.0.0.1", 3), peer()).await.unwrap() {
            Message::Nack { reason } => assert!(reason.contains("too many"), "{reason}"),
            other => panic!("{other:?}"),
        }
        // Another host is unaffected.
        assert!(matches!(
            h.handle(publish("10.0.0.2", 3), peer()).await.unwrap(),
            Message::Registered { .. }
        ));
    }

    #[tokio::test]
    async fn unknown_ping_and_unexpected_messages_are_nacked() {
        let h = handler(BrokerPolicy::default());
        assert!(matches!(
            h.handle(Message::Ping { id: PartyId(99) }, peer())
                .await
                .unwrap(),
            Message::Nack { .. }
        ));
        for msg in [
            Message::Collect,
            Message::Heartbeat {
                inbox: None,
                service: None,
            },
            Message::Send { text: "x".into() },
            Message::Delivered,
        ] {
            let kind = msg.kind();
            match h.handle(msg, peer()).await.unwrap() {
                Message::Nack { reason } => assert!(reason.contains(kind), "{reason}"),
                other => panic!("{kind}: {other:?}"),
            }
        }
    }
}
