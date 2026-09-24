//! The request handler a party mounts on its bind address.
//!
//! It answers three kinds of caller: the broker (two-sided heartbeats), the
//! `collect` operation, and, for clients only, the `send` operation whose
//! text is relayed to the broker for the paired service. Anything else is a
//! [`Message::Nack`]; nothing here panics on the request contents.

use std::future::Future;
use std::sync::Arc;

use tracing::{debug, trace};

use super::{PartyState, Role};
use crate::protocol::{Message, PartyId};
use crate::transport::{Handler, PeerInfo};
use crate::Result;

/// [`Handler`] for a party's own listener.
#[derive(Debug, Clone)]
pub struct PartyHandler {
    state: Arc<PartyState>,
}

impl PartyHandler {
    /// A handler over the given shared state.
    pub fn new(state: Arc<PartyState>) -> Self {
        PartyHandler { state }
    }

    /// The shared state.
    pub fn state(&self) -> &Arc<PartyState> {
        &self.state
    }

    async fn dispatch(&self, msg: Message, peer: PeerInfo) -> Result<Message> {
        trace!(kind = msg.kind(), remote = %peer.remote, "party request");
        match msg {
            Message::Heartbeat { inbox, service } => {
                self.state.apply_heartbeat(inbox, service);
                // Before the registration reply has been processed the id is
                // unknown; the broker accepts any id in the acknowledgement
                // and only uses it for logging.
                Ok(Message::HeartbeatAck {
                    id: self.state.id().unwrap_or(PartyId(0)),
                })
            }
            Message::Collect => Ok(Message::Collected {
                text: self.state.inbox(),
                service: self.state.service(),
            }),
            Message::Send { text } => match self.state.role() {
                Role::Publisher => Ok(Message::nack(
                    "services do not accept send; address the client",
                )),
                Role::Claimer => {
                    let Some(service) = self.state.service() else {
                        return Ok(Message::nack("client is not paired with a service"));
                    };
                    debug!(to = %service.id, "relaying message to the broker");
                    let reply = self
                        .state
                        .client()
                        .call(
                            self.state.broker(),
                            Message::Deliver {
                                to: service.id,
                                text,
                            },
                        )
                        .await?;
                    Ok(match reply {
                        Message::Delivered => Message::Delivered,
                        Message::Nack { reason } => Message::Nack { reason },
                        other => Message::nack(format!(
                            "broker answered a deliver with {}",
                            other.kind()
                        )),
                    })
                }
            },
            other => Ok(Message::nack(format!(
                "unexpected {} at a {}",
                other.kind(),
                self.state.role()
            ))),
        }
    }
}

impl Handler for PartyHandler {
    fn handle(&self, msg: Message, peer: PeerInfo) -> impl Future<Output = Result<Message>> + Send {
        self.dispatch(msg, peer)
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;
    use crate::config::{Limits, Timing, TlsPaths};
    use crate::net::{Addr, Transport};
    use crate::protocol::ServiceHandle;
    use crate::transport::Client;

    fn handler(role: Role) -> PartyHandler {
        let client = Arc::new(Client::new(
            TlsPaths::default(),
            Timing::fast(),
            Limits::default(),
        ));
        PartyHandler::new(PartyState::new(role, Addr::tcp("127.0.0.1", 1), 42, client))
    }

    fn peer() -> PeerInfo {
        PeerInfo {
            remote: "127.0.0.1:5".parse::<SocketAddr>().unwrap(),
            transport: Transport::Tcp,
        }
    }

    fn handle() -> ServiceHandle {
        ServiceHandle {
            id: PartyId(9),
            key: 42,
            host: "10.0.0.9".into(),
            service_port: 9000,
        }
    }

    #[tokio::test]
    async fn heartbeat_is_acked_and_applied() {
        let h = handler(Role::Publisher);
        let reply = h
            .handle(
                Message::Heartbeat {
                    inbox: Some("job 7".into()),
                    service: None,
                },
                peer(),
            )
            .await
            .unwrap();
        assert_eq!(reply, Message::HeartbeatAck { id: PartyId(0) });
        h.state().set_id(PartyId(3));
        let reply = h
            .handle(
                Message::Heartbeat {
                    inbox: None,
                    service: None,
                },
                peer(),
            )
            .await
            .unwrap();
        assert_eq!(reply, Message::HeartbeatAck { id: PartyId(3) });
        assert_eq!(h.state().inbox().as_deref(), Some("job 7"));
    }

    #[tokio::test]
    async fn collect_returns_inbox_for_services_and_service_for_clients() {
        let s = handler(Role::Publisher);
        assert_eq!(
            s.handle(Message::Collect, peer()).await.unwrap(),
            Message::Collected {
                text: None,
                service: None
            }
        );
        s.state().apply_heartbeat(Some("x".into()), None);
        assert_eq!(
            s.handle(Message::Collect, peer()).await.unwrap(),
            Message::Collected {
                text: Some("x".into()),
                service: None
            }
        );

        let c = handler(Role::Claimer);
        c.state().set_service(handle());
        assert_eq!(
            c.handle(Message::Collect, peer()).await.unwrap(),
            Message::Collected {
                text: None,
                service: Some(handle())
            }
        );
    }

    #[tokio::test]
    async fn send_is_refused_by_services_and_unpaired_clients() {
        let s = handler(Role::Publisher);
        assert!(matches!(
            s.handle(Message::Send { text: "hi".into() }, peer())
                .await
                .unwrap(),
            Message::Nack { .. }
        ));
        let c = handler(Role::Claimer);
        assert!(matches!(
            c.handle(Message::Send { text: "hi".into() }, peer())
                .await
                .unwrap(),
            Message::Nack { .. }
        ));
    }

    #[tokio::test]
    async fn unexpected_messages_are_nacked_not_panicked() {
        let h = handler(Role::Claimer);
        for msg in [
            Message::Publish {
                key: 1,
                service_port: 1,
                bind_addr: Addr::tcp("h", 1),
                ping: false,
            },
            Message::Claim {
                key: 1,
                bind_addr: Addr::tcp("h", 1),
                ping: false,
            },
            Message::Ping { id: PartyId(1) },
            Message::Deliver {
                to: PartyId(1),
                text: "x".into(),
            },
            Message::Registered { id: PartyId(1) },
            Message::Delivered,
            Message::nack("x"),
        ] {
            let kind = msg.kind();
            match h.handle(msg, peer()).await.unwrap() {
                Message::Nack { reason } => assert!(reason.contains(kind), "{reason}"),
                other => panic!("{kind}: {other:?}"),
            }
        }
    }
}
