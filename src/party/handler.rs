//! The request handler a party mounts on its bind address.
//!
//! It answers three kinds of caller: the broker (two-sided heartbeats), the
//! `collect` operation, and the `send` operation, whose text is relayed to
//! the broker for the party's peer. Anything else is a [`Message::Nack`];
//! nothing here panics on the request contents.

use std::future::Future;
use std::sync::Arc;

use tracing::{debug, trace};

use super::{PartyState, Role};
use crate::Result;
use crate::protocol::{Message, PartyId};
use crate::transport::{Handler, PeerInfo};

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
            Message::Heartbeat {
                token,
                inbox,
                service,
            } => {
                // Only the broker knows this party's token; anything else is
                // a stranger trying to deliver text, re-pair us or keep our
                // watchdog quiet.
                if !self.state.accepts_token(&token) {
                    debug!(remote = %peer.remote, "heartbeat without our token ignored");
                    return Ok(Message::nack("heartbeat does not carry this party's token"));
                }
                // A service has no pairing to update.
                let service = match self.state.role() {
                    Role::Service => None,
                    Role::Client => service,
                };
                self.state.apply_heartbeat(inbox, service);
                Ok(Message::HeartbeatAck {
                    id: self.state.id().unwrap_or(PartyId(0)),
                })
            }
            Message::Collect => Ok(Message::Collected {
                role: self.state.role(),
                text: self.state.inbox(),
                service: self.state.service(),
            }),
            Message::Send { text } => {
                // Either role relays to its peer; the broker knows who that is.
                let (Some(from), Some(token)) = (self.state.id(), self.state.token()) else {
                    return Ok(Message::nack(format!(
                        "{} is not registered yet",
                        self.state.role()
                    )));
                };
                debug!(%from, "relaying text to the broker for the peer");
                let reply = self
                    .state
                    .client()
                    .call(self.state.broker(), Message::Deliver { from, token, text })
                    .await?;
                Ok(match reply {
                    Message::Delivered => Message::Delivered,
                    Message::Nack { reason } => Message::Nack { reason },
                    other => {
                        Message::nack(format!("broker answered a deliver with {}", other.kind()))
                    }
                })
            }
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
    use crate::protocol::{RegToken, ServiceHandle};
    use crate::transport::Client;

    fn tok() -> RegToken {
        RegToken::from_bytes([7; 16])
    }

    fn registered(h: &PartyHandler, id: u64) {
        h.state().set_id(PartyId(id));
        h.state().set_token(tok());
    }

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
            host: "10.0.0.9".into(),
            service_port: 9000,
        }
    }

    #[tokio::test]
    async fn heartbeat_is_acked_and_applied_only_with_our_token() {
        let h = handler(Role::Service);
        let forged = Message::Heartbeat {
            token: RegToken::from_bytes([0xee; 16]),
            inbox: Some("planted".into()),
            service: None,
        };
        // Not registered yet: nothing is accepted.
        assert!(matches!(
            h.handle(forged.clone(), peer()).await.unwrap(),
            Message::Nack { .. }
        ));
        registered(&h, 3);
        let before = h.state().last_contact();
        // Wrong token: refused, nothing stored, watchdog not refreshed.
        assert!(matches!(
            h.handle(forged, peer()).await.unwrap(),
            Message::Nack { .. }
        ));
        assert_eq!(h.state().inbox(), None);
        assert_eq!(h.state().last_contact(), before);
        // Our token: applied and acknowledged with our id.
        let reply = h
            .handle(
                Message::Heartbeat {
                    token: tok(),
                    inbox: Some("job 7".into()),
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
    async fn services_ignore_pairings_and_clients_take_them() {
        let s = handler(Role::Service);
        registered(&s, 1);
        s.handle(
            Message::Heartbeat {
                token: tok(),
                inbox: None,
                service: Some(handle()),
            },
            peer(),
        )
        .await
        .unwrap();
        assert_eq!(s.state().service(), None);

        let c = handler(Role::Client);
        registered(&c, 2);
        c.handle(
            Message::Heartbeat {
                token: tok(),
                inbox: None,
                service: Some(handle()),
            },
            peer(),
        )
        .await
        .unwrap();
        assert_eq!(c.state().service(), Some(handle()));
    }

    #[tokio::test]
    async fn collect_returns_inbox_for_services_and_service_for_clients() {
        let s = handler(Role::Service);
        assert_eq!(
            s.handle(Message::Collect, peer()).await.unwrap(),
            Message::Collected {
                role: Role::Service,
                text: None,
                service: None
            }
        );
        s.state().apply_heartbeat(Some("x".into()), None);
        assert_eq!(
            s.handle(Message::Collect, peer()).await.unwrap(),
            Message::Collected {
                role: Role::Service,
                text: Some("x".into()),
                service: None
            }
        );

        // A client answers with its role even before it is paired.
        let c = handler(Role::Client);
        assert_eq!(
            c.handle(Message::Collect, peer()).await.unwrap(),
            Message::Collected {
                role: Role::Client,
                text: None,
                service: None
            }
        );
        c.state().set_service(handle());
        assert_eq!(
            c.handle(Message::Collect, peer()).await.unwrap(),
            Message::Collected {
                role: Role::Client,
                text: None,
                service: Some(handle())
            }
        );
    }

    #[tokio::test]
    async fn send_is_relayed_for_either_role_once_registered() {
        for role in [Role::Service, Role::Client] {
            // Before registration there is nothing to relay with.
            let h = handler(role);
            match h
                .handle(Message::Send { text: "hi".into() }, peer())
                .await
                .unwrap()
            {
                Message::Nack { reason } => {
                    assert!(reason.contains("not registered"), "{role}: {reason}")
                }
                other => panic!("{role}: {other:?}"),
            }
            // Registered, the relay is attempted: with no broker at the
            // configured address it fails as a transport error.
            registered(&h, 3);
            assert!(
                h.handle(Message::Send { text: "hi".into() }, peer())
                    .await
                    .is_err(),
                "{role}"
            );
        }
    }

    #[tokio::test]
    async fn unexpected_messages_are_nacked_not_panicked() {
        let h = handler(Role::Client);
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
            Message::Ping {
                id: PartyId(1),
                token: tok(),
            },
            Message::Deliver {
                from: PartyId(2),
                token: tok(),
                text: "x".into(),
            },
            Message::Registered {
                id: PartyId(1),
                token: tok(),
            },
            Message::Delivered,
            Message::nack("x"),
            // The broker answers store relays; parties do not relay `store`
            // yet, so it is as unexpected here as the relay and its reply.
            Message::Store {
                op: crate::protocol::StoreOp::List,
            },
            Message::StoreRelay {
                from: PartyId(2),
                token: tok(),
                op: crate::protocol::StoreOp::List,
            },
            Message::Stored(crate::protocol::Stored {
                client: None,
                revision: 0,
                entries: vec![],
            }),
        ] {
            let kind = msg.kind();
            match h.handle(msg, peer()).await.unwrap() {
                Message::Nack { reason } => assert!(reason.contains(kind), "{reason}"),
                other => panic!("{kind}: {other:?}"),
            }
        }
    }
}
