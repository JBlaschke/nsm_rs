//! The request handler a party mounts on its bind address.
//!
//! It answers four kinds of caller: the broker (two-sided heartbeats), the
//! `collect` operation, the `send` operation, whose text is relayed to the
//! broker for the party's peer, and the `store` operation, which is relayed
//! to the broker that keeps the store the party shares with its peer. Both
//! relays go through one helper, which adds the party's id and token and
//! refuses before registration, so the two cannot drift apart. Anything
//! else, a `store_by_key` meant for the broker included, is a
//! [`Message::Nack`]; nothing here panics on the request contents, and
//! store keys and values are never logged.

use std::future::Future;
use std::sync::Arc;

use tracing::{debug, trace};

use super::{PartyState, Role};
use crate::Result;
use crate::protocol::{Message, PartyId, RegToken};
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
                let reply = self
                    .relay(|from, token| {
                        debug!(%from, "relaying text to the broker for the peer");
                        Message::Deliver { from, token, text }
                    })
                    .await?;
                Ok(match reply {
                    Message::Delivered => Message::Delivered,
                    Message::Nack { reason } => Message::Nack { reason },
                    other => {
                        Message::nack(format!("broker answered a deliver with {}", other.kind()))
                    }
                })
            }
            Message::Store { op } => {
                // Either role relays; the broker knows whose store it is.
                let reply = self
                    .relay(|from, token| {
                        debug!(%from, op = op.kind(), "relaying a store request to the broker");
                        Message::StoreRelay { from, token, op }
                    })
                    .await?;
                Ok(match reply {
                    stored @ Message::Stored(_) => stored,
                    Message::Nack { reason } => Message::Nack { reason },
                    other => Message::nack(format!(
                        "broker answered a store relay with {}",
                        other.kind()
                    )),
                })
            }
            other => Ok(Message::nack(format!(
                "unexpected {} at a {}",
                other.kind(),
                self.state.role()
            ))),
        }
    }

    /// Relay a request to the broker on this party's behalf and return the
    /// broker's reply. `request` builds the message from the party's id and
    /// token. Before registration there is nothing to relay with: the broker
    /// is not dialled, and the reply is the party's own
    /// `<role> is not registered yet` nack, which the callers pass through
    /// like a nack from the broker. A failed broker call is an error, which
    /// the transport turns into a closed connection or a 500.
    async fn relay(&self, request: impl FnOnce(PartyId, RegToken) -> Message) -> Result<Message> {
        let (Some(from), Some(token)) = (self.state.id(), self.state.token()) else {
            return Ok(Message::nack(format!(
                "{} is not registered yet",
                self.state.role()
            )));
        };
        self.state
            .client()
            .call(self.state.broker(), request(from, token))
            .await
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
    use crate::protocol::message::store_key;
    use crate::protocol::{RegToken, ServiceHandle, StoreEntry, StoreOp, Stored};
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

    /// A broker stand-in that records every request and answers from a
    /// script: `deliver` is delivered; a store `get` or `list` gets
    /// [`scripted_stored`], a `put` the nack "store full", and a `delete` a
    /// reply of the wrong kind.
    #[derive(Default)]
    struct ScriptedBroker {
        seen: Arc<std::sync::Mutex<Vec<Message>>>,
    }

    fn scripted_stored() -> Message {
        Message::Stored(Stored {
            client: Some(PartyId(4)),
            revision: 9,
            applied: true,
            entries: vec![StoreEntry {
                key: store_key("step"),
                value: "5 \"quoted\"\n".into(),
                version: 9,
            }],
        })
    }

    impl Handler for ScriptedBroker {
        async fn handle(&self, msg: Message, _peer: PeerInfo) -> Result<Message> {
            self.seen
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(msg.clone());
            Ok(match msg {
                Message::Deliver { .. } => Message::Delivered,
                Message::StoreRelay { op, .. } => match op {
                    StoreOp::Get { .. } | StoreOp::List => scripted_stored(),
                    StoreOp::Put { .. } => Message::nack("store full: scripted"),
                    StoreOp::Delete { .. } => Message::Delivered,
                },
                other => Message::nack(format!("unexpected {} at the script", other.kind())),
            })
        }
    }

    #[tokio::test]
    async fn store_before_registration_is_refused_without_dialling() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for role in [Role::Service, Role::Client] {
                // Nothing listens at the configured broker (port 1): a dial
                // would fail as an error, not answer with a nack.
                let h = handler(role);
                for op in [
                    StoreOp::List,
                    StoreOp::Put {
                        key: store_key("step"),
                        value: "5".into(),
                        if_version: Some(0),
                    },
                ] {
                    assert_eq!(
                        h.handle(Message::Store { op }, peer()).await.unwrap(),
                        Message::nack(format!("{role} is not registered yet")),
                        "{role}"
                    );
                }
            }
        })
        .await
        .expect("the test finished in time");
    }

    #[tokio::test]
    async fn relays_carry_the_partys_own_id_and_token_and_pass_replies_back() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let script = ScriptedBroker::default();
            let seen = Arc::clone(&script.seen);
            let (server, client, _certs) =
                crate::transport::testing::start(Transport::Tcp, script).await;
            let client = Arc::new(client);
            for (i, role) in [Role::Service, Role::Client].into_iter().enumerate() {
                let id = PartyId(3 + i as u64);
                let h = PartyHandler::new(PartyState::new(
                    role,
                    server.bound(),
                    42,
                    Arc::clone(&client),
                ));
                h.state().set_id(id);
                h.state().set_token(tok());
                let store = |op| Message::Store { op };
                let get = StoreOp::Get {
                    key: store_key("step"),
                };
                // Conditions travel to the broker unchanged, like the rest.
                let put = StoreOp::Put {
                    key: store_key("step"),
                    value: "6".into(),
                    if_version: Some(9),
                };
                let delete = StoreOp::Delete {
                    key: store_key("step"),
                    if_version: Some(0),
                };

                // `stored` comes back exactly as the broker sent it.
                assert_eq!(
                    h.handle(store(get.clone()), peer()).await.unwrap(),
                    scripted_stored(),
                    "{role}"
                );
                assert_eq!(
                    h.handle(store(StoreOp::List), peer()).await.unwrap(),
                    scripted_stored(),
                    "{role}"
                );
                // A refusal passes through with the broker's reason.
                assert_eq!(
                    h.handle(store(put.clone()), peer()).await.unwrap(),
                    Message::nack("store full: scripted"),
                    "{role}"
                );
                // A reply of the wrong kind is named, not passed on.
                assert_eq!(
                    h.handle(store(delete.clone()), peer()).await.unwrap(),
                    Message::nack("broker answered a store relay with delivered"),
                    "{role}"
                );
                // `send` goes through the same relay.
                assert_eq!(
                    h.handle(Message::Send { text: "hi".into() }, peer())
                        .await
                        .unwrap(),
                    Message::Delivered,
                    "{role}"
                );

                // Every relay named this party and carried its token and the
                // operation unchanged.
                let relayed = std::mem::take(&mut *seen.lock().unwrap());
                let expected: Vec<Message> = [get, StoreOp::List, put, delete]
                    .into_iter()
                    .map(|op| Message::StoreRelay {
                        from: id,
                        token: tok(),
                        op,
                    })
                    .chain([Message::Deliver {
                        from: id,
                        token: tok(),
                        text: "hi".into(),
                    }])
                    .collect();
                assert_eq!(relayed, expected, "{role}");
            }
            server.shutdown().await;
        })
        .await
        .expect("the test finished in time");
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
            // A party relays `store` but not `store_relay`, which only the
            // broker answers, and a `stored` reply is never a request.
            Message::StoreRelay {
                from: PartyId(2),
                token: tok(),
                op: StoreOp::List,
            },
            // A store by rendezvous key is the broker's to answer.
            Message::StoreByKey {
                rendezvous: 1,
                party_id: None,
                op: StoreOp::List,
            },
            Message::Stored(Stored {
                client: None,
                revision: 0,
                applied: true,
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
