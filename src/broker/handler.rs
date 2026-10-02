//! The request handler mounted on the broker's listener.
//!
//! One `match` over [`Message`], written once for every transport. Anything
//! the broker does not expect is a [`Message::Nack`]; nothing here panics on
//! peer input, and the registry lock is never held across an `.await`.
//!
//! Relayed requests ([`Message::Deliver`], [`Message::StoreRelay`]) check the
//! sender's token and act on the registry inside one critical section, so
//! nothing can remove the sender or re-pair it in between. A
//! [`Message::StoreByKey`] carries no token: the broker resolves the
//! rendezvous key to one party in that same critical section and answers as
//! if that party had relayed the operation. Store keys and values are never
//! logged, only the operation's name, and neither is a rendezvous key.
//!
//! The handler is where requests, registrations, refusals and store
//! operations are counted ([`Broker::metrics`]): every request once by kind
//! and outcome in [`Handler::handle`], the rest where the decision is made.

use std::future::Future;
use std::sync::Arc;

use tokio::time::{Instant, sleep};
use tracing::{debug, trace};

use super::metrics::{Outcome, RefusalReason, RequestKind, StoreOpKind, StoreOutcome};
use super::monitor::Broker;
use crate::net::Addr;
use crate::protocol::{Message, RegToken, Role};
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
    /// when the policy demands it, and the per-host registration cap. A
    /// refusal is counted here with its reason.
    fn admission(&self, bind_addr: &Addr, peer: &PeerInfo) -> Option<Message> {
        let refusal = self.admission_check(bind_addr, peer);
        if let Some((_, reason)) = &refusal {
            self.broker.metrics().refused(*reason);
        }
        refusal.map(|(nack, _)| nack)
    }

    fn admission_check(
        &self,
        bind_addr: &Addr,
        peer: &PeerInfo,
    ) -> Option<(Message, RefusalReason)> {
        if bind_addr.port == 0 {
            return Some((
                Message::nack("bind_addr must carry the actual port"),
                RefusalReason::BadPort,
            ));
        }
        let policy = self.broker.policy();
        let source = peer.remote.ip().to_string();
        if bind_addr.host != source {
            if policy.require_matching_host {
                return Some((
                    Message::nack(format!(
                        "advertised host {} does not match the connection source {source}",
                        bind_addr.host
                    )),
                    RefusalReason::HostMismatch,
                ));
            }
            debug!(remote = %peer.remote, advertised = %bind_addr.host, "party advertises an address other than the one it connected from");
        }
        let count = self
            .broker
            .with_registry(|r| r.count_for_host(&bind_addr.host));
        if count >= policy.max_registrations_per_host {
            return Some((
                Message::nack(format!(
                    "too many registrations from {} ({count})",
                    bind_addr.host
                )),
                RefusalReason::PerHost,
            ));
        }
        None
    }

    /// Count a refusal of a registration by the registry: the only reason
    /// it gives is a full registry.
    fn refused_by_registry(&self, reason: String) -> Message {
        self.broker.metrics().refused(RefusalReason::Full);
        Message::nack(reason)
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
                let token = RegToken::generate()?;
                let now = Instant::now();
                match self
                    .broker
                    .with_registry(|r| r.publish(key, service_addr, bind_addr, ping, token, now))
                {
                    Ok(id) => {
                        self.broker.metrics().registered(Role::Service);
                        self.broker.watch(id);
                        Ok(Message::Registered { id, token })
                    }
                    Err(Error::Rejected(reason)) => Ok(self.refused_by_registry(reason)),
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
                let token = RegToken::generate()?;
                loop {
                    let now = Instant::now();
                    match self
                        .broker
                        .with_registry(|r| r.claim(key, bind_addr.clone(), ping, token, now))
                    {
                        Ok((id, service)) => {
                            self.broker.metrics().registered(Role::Client);
                            self.broker.watch(id);
                            return Ok(Message::Paired { id, token, service });
                        }
                        Err(Error::NoService(_)) if Instant::now() < deadline => {
                            // A client may start before its service has
                            // published; wait a little without holding the lock.
                            sleep(pause).await;
                        }
                        Err(Error::NoService(k)) => {
                            self.broker.metrics().refused(RefusalReason::NoService);
                            return Ok(Message::nack(format!("no service available for key {k}")));
                        }
                        Err(Error::Rejected(reason)) => {
                            return Ok(self.refused_by_registry(reason));
                        }
                        Err(e) => return Err(e),
                    }
                }
            }

            Message::Ping { id, token } => {
                let now = Instant::now();
                // One refusal text for "unknown id" and "wrong token", so a
                // caller cannot enumerate ids; only ping-mode parties may ping.
                let reply = self.broker.with_registry(|r| {
                    if !r.verify(id, &token) {
                        return Err("unknown party or wrong token");
                    }
                    if r.is_ping(id) != Some(true) {
                        return Err("party is not in ping mode");
                    }
                    r.mark_alive(id, now);
                    Ok(r.heartbeat_for(id))
                });
                Ok(match reply {
                    Ok(Some(heartbeat)) => heartbeat,
                    Ok(None) => Message::nack("unknown party or wrong token"),
                    Err(reason) => Message::nack(reason),
                })
            }

            Message::Deliver { from, token, text } => {
                // The registry knows the sender's peer; the sender names none.
                let outcome = self.broker.with_registry(|r| {
                    if !r.verify(from, &token) {
                        return Ok(Err("unknown party or wrong token"));
                    }
                    r.deliver(from, text).map(Ok)
                });
                match outcome {
                    Ok(Ok(())) => Ok(Message::Delivered),
                    Ok(Err(reason)) => Ok(Message::nack(reason)),
                    Err(Error::Rejected(reason)) => Ok(Message::nack(reason)),
                    Err(e) => Err(e),
                }
            }

            Message::StoreRelay { from, token, op } => {
                debug!(%from, op = op.kind(), "store request");
                let kind = StoreOpKind::from(&op);
                // The registry finds the sender's store; the sender names none.
                let outcome = self.broker.with_registry(|r| {
                    if !r.verify(from, &token) {
                        return Ok(Err("unknown party or wrong token"));
                    }
                    r.store(from, op).map(Ok)
                });
                let metrics = self.broker.metrics();
                match outcome {
                    Ok(Ok(stored)) => {
                        metrics.store_op(kind, StoreOutcome::of(&stored));
                        Ok(Message::Stored(stored))
                    }
                    Ok(Err(reason)) => {
                        metrics.store_op(kind, StoreOutcome::Refused);
                        Ok(Message::nack(reason))
                    }
                    Err(Error::Rejected(reason)) => {
                        metrics.store_op(kind, StoreOutcome::Refused);
                        Ok(Message::nack(reason))
                    }
                    Err(e) => Err(e),
                }
            }

            Message::StoreByKey {
                rendezvous,
                party_id,
                op,
            } => {
                debug!(
                    op = op.kind(),
                    named = party_id.is_some(),
                    "store request by key"
                );
                let kind = StoreOpKind::from(&op);
                // The registry resolves the key to one party; no token, the
                // key is the capability (discovery plan, decision L6).
                let outcome = self
                    .broker
                    .with_registry(|r| r.store_by_key(rendezvous, party_id, op));
                let metrics = self.broker.metrics();
                match outcome {
                    Ok(stored) => {
                        metrics.store_op(kind, StoreOutcome::of(&stored));
                        Ok(Message::Stored(stored))
                    }
                    Err(Error::Rejected(reason)) => {
                        metrics.store_op(kind, StoreOutcome::Refused);
                        Ok(Message::nack(reason))
                    }
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
        let kind = RequestKind::from(&msg);
        async move {
            let reply = self.dispatch(msg, peer).await;
            let outcome = match &reply {
                Ok(Message::Nack { .. }) => Outcome::Nack,
                Ok(_) => Outcome::Ok,
                Err(_) => Outcome::Error,
            };
            self.broker.metrics().request(kind, outcome);
            reply
        }
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

    fn registered(reply: Message) -> (PartyId, RegToken) {
        match reply {
            Message::Registered { id, token } => (id, token),
            other => panic!("expected Registered, got {other:?}"),
        }
    }

    fn paired(reply: Message) -> (PartyId, RegToken, PartyId) {
        match reply {
            Message::Paired { id, token, service } => (id, token, service.id),
            other => panic!("expected Paired, got {other:?}"),
        }
    }

    fn wrong() -> RegToken {
        RegToken::from_bytes([0xee; 16])
    }

    #[tokio::test]
    async fn every_request_and_decision_is_counted() {
        crate::tls::install_default_provider();
        let h = handler(BrokerPolicy {
            max_registrations_per_host: 2,
            ..BrokerPolicy::default()
        });
        let m = h.broker.metrics();

        // A granted service, then a refused claim (no service under key 2).
        let (sid, stoken) = registered(h.handle(publish("127.0.0.1", 7000), peer()).await.unwrap());
        let refused = h
            .handle(
                Message::Claim {
                    key: 2,
                    bind_addr: Addr::tcp("127.0.0.1", 7001),
                    ping: true,
                },
                peer(),
            )
            .await
            .unwrap();
        assert!(matches!(refused, Message::Nack { .. }), "{refused:?}");
        // A granted client under key 1.
        let (cid, ctoken, _) = paired(
            h.handle(
                Message::Claim {
                    key: 1,
                    bind_addr: Addr::tcp("127.0.0.1", 7002),
                    ping: true,
                },
                peer(),
            )
            .await
            .unwrap(),
        );
        assert_ne!(sid, cid);
        // The per-host cap (2) refuses the third, port 0 the fourth.
        let capped = h.handle(publish("127.0.0.1", 7003), peer()).await.unwrap();
        assert!(matches!(capped, Message::Nack { .. }), "{capped:?}");
        let bad_port = h.handle(publish("10.0.0.5", 0), peer()).await.unwrap();
        assert!(matches!(bad_port, Message::Nack { .. }), "{bad_port:?}");
        // Store operations: applied, not applied, refused (wrong token).
        let key: crate::protocol::StoreKey = "k".parse().unwrap();
        let put = |if_version| Message::StoreRelay {
            from: cid,
            token: ctoken,
            op: crate::protocol::StoreOp::Put {
                key: key.clone(),
                value: "v".into(),
                if_version,
            },
        };
        assert!(matches!(
            h.handle(put(None), peer()).await.unwrap(),
            Message::Stored(_)
        ));
        match h.handle(put(Some(0)), peer()).await.unwrap() {
            Message::Stored(s) => assert!(!s.applied),
            other => panic!("{other:?}"),
        }
        let forged = Message::StoreRelay {
            from: sid,
            token: wrong(),
            op: crate::protocol::StoreOp::List,
        };
        assert!(matches!(
            h.handle(forged, peer()).await.unwrap(),
            Message::Nack { .. }
        ));
        // By key: one answered (the key's one claim), one refused (no such
        // key), each counted as a request of its own kind and as a store
        // operation.
        let by_key = |rendezvous, op| Message::StoreByKey {
            rendezvous,
            party_id: None,
            op,
        };
        match h
            .handle(by_key(1, crate::protocol::StoreOp::List), peer())
            .await
            .unwrap()
        {
            Message::Stored(s) => assert_eq!(s.client, Some(cid)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            h.handle(by_key(2, crate::protocol::StoreOp::List), peer())
                .await
                .unwrap(),
            Message::Nack { .. }
        ));
        // A refused deliver (the service has a client, so use a wrong token)
        // and an unexpected message.
        let deliver = Message::Deliver {
            from: sid,
            token: wrong(),
            text: "x".into(),
        };
        assert!(matches!(
            h.handle(deliver, peer()).await.unwrap(),
            Message::Nack { .. }
        ));
        let _ = stoken;
        assert!(matches!(
            h.handle(Message::Delivered, peer()).await.unwrap(),
            Message::Nack { .. }
        ));

        assert_eq!(m.requests(RequestKind::Publish, Outcome::Ok), 1);
        assert_eq!(m.requests(RequestKind::Publish, Outcome::Nack), 2);
        assert_eq!(m.requests(RequestKind::Claim, Outcome::Ok), 1);
        assert_eq!(m.requests(RequestKind::Claim, Outcome::Nack), 1);
        assert_eq!(m.requests(RequestKind::StoreRelay, Outcome::Ok), 2);
        assert_eq!(m.requests(RequestKind::StoreRelay, Outcome::Nack), 1);
        assert_eq!(m.requests(RequestKind::StoreByKey, Outcome::Ok), 1);
        assert_eq!(m.requests(RequestKind::StoreByKey, Outcome::Nack), 1);
        assert_eq!(m.requests(RequestKind::Deliver, Outcome::Nack), 1);
        assert_eq!(m.requests(RequestKind::Other, Outcome::Nack), 1);
        assert_eq!(m.requests(RequestKind::Ping, Outcome::Ok), 0);
        assert_eq!(m.registrations(Role::Service), 1);
        assert_eq!(m.registrations(Role::Client), 1);
        assert_eq!(m.refusals(RefusalReason::NoService), 1);
        assert_eq!(m.refusals(RefusalReason::PerHost), 1);
        assert_eq!(m.refusals(RefusalReason::BadPort), 1);
        assert_eq!(m.refusals(RefusalReason::HostMismatch), 0);
        assert_eq!(m.refusals(RefusalReason::Full), 0);
        assert_eq!(m.store_ops(StoreOpKind::Put, StoreOutcome::Applied), 1);
        assert_eq!(m.store_ops(StoreOpKind::Put, StoreOutcome::NotApplied), 1);
        assert_eq!(m.store_ops(StoreOpKind::List, StoreOutcome::Refused), 2);
        assert_eq!(m.store_ops(StoreOpKind::List, StoreOutcome::Applied), 1);
        assert_eq!(m.store_ops(StoreOpKind::Get, StoreOutcome::Applied), 0);

        // The gauges follow the registry, and the exposition renders them.
        let g = h.broker.gauges();
        assert_eq!(g.parties_of(Role::Service), 1);
        assert_eq!(g.parties_of(Role::Client), 1);
        assert_eq!(g.services_unclaimed, 0);
        assert_eq!(g.store_entries, 1);
        let text = h.broker.render_metrics();
        assert!(text.contains("nsm_requests_total{kind=\"publish\",outcome=\"nack\"} 2\n"));
        assert!(text.contains("nsm_requests_total{kind=\"store_by_key\",outcome=\"ok\"} 1\n"));
        assert!(text.contains("nsm_registrations_refused_total{reason=\"per_host\"} 1\n"));
        let status = h.broker.status(None);
        assert_eq!(status.counts.clients, 1);
        assert_eq!(status.totals.requests["store_relay"]["ok"], 2);
        assert_eq!(status.parties.len(), 2);
        assert_eq!(status.bound, None);
    }

    #[tokio::test]
    async fn a_full_registry_is_counted_as_such() {
        crate::tls::install_default_provider();
        let client = Arc::new(Client::new(
            TlsPaths::default(),
            Timing::fast(),
            Limits::default(),
        ));
        let h = BrokerHandler::new(Broker::new(
            client,
            Timing::fast(),
            Limits {
                max_registrations: 1,
                ..Limits::default()
            },
            BrokerPolicy::default(),
            CancellationToken::new(),
        ));
        registered(h.handle(publish("127.0.0.1", 7000), peer()).await.unwrap());
        let full = h.handle(publish("127.0.0.1", 7001), peer()).await.unwrap();
        assert!(matches!(full, Message::Nack { .. }), "{full:?}");
        let claim = h
            .handle(
                Message::Claim {
                    key: 1,
                    bind_addr: Addr::tcp("127.0.0.1", 7002),
                    ping: true,
                },
                peer(),
            )
            .await
            .unwrap();
        assert!(matches!(claim, Message::Nack { .. }), "{claim:?}");
        assert_eq!(h.broker.metrics().refusals(RefusalReason::Full), 2);
        assert_eq!(h.broker.metrics().refusals(RefusalReason::NoService), 0);
    }

    #[tokio::test]
    async fn ping_needs_the_right_token_and_ping_mode() {
        crate::tls::install_default_provider();
        let h = handler(BrokerPolicy::default());
        let (pinger, token) = registered(h.handle(publish("127.0.0.1", 1), peer()).await.unwrap());
        let two_sided = Message::Publish {
            key: 1,
            service_port: 9000,
            bind_addr: Addr::tcp("127.0.0.1", 2),
            ping: false,
        };
        let (quiet, quiet_token) = registered(h.handle(two_sided, peer()).await.unwrap());
        assert_ne!(token, quiet_token);

        // Wrong token: refused, with the same text as an unknown id.
        let bad = h
            .handle(
                Message::Ping {
                    id: pinger,
                    token: wrong(),
                },
                peer(),
            )
            .await
            .unwrap();
        let unknown = h
            .handle(
                Message::Ping {
                    id: PartyId(99),
                    token: wrong(),
                },
                peer(),
            )
            .await
            .unwrap();
        assert!(matches!(bad, Message::Nack { .. }), "{bad:?}");
        assert_eq!(bad, unknown);
        // Right token, but a two-sided party: refused.
        let reply = h
            .handle(
                Message::Ping {
                    id: quiet,
                    token: quiet_token,
                },
                peer(),
            )
            .await
            .unwrap();
        assert!(matches!(reply, Message::Nack { .. }), "{reply:?}");
        // Right token, ping-mode party: a heartbeat carrying that token.
        let reply = h
            .handle(Message::Ping { id: pinger, token }, peer())
            .await
            .unwrap();
        assert_eq!(
            reply,
            Message::Heartbeat {
                token,
                inbox: None,
                service: None
            }
        );
    }

    #[tokio::test]
    async fn deliver_needs_a_token_and_a_peer_and_flows_both_ways() {
        crate::tls::install_default_provider();
        let h = handler(BrokerPolicy::default());
        let (service, service_token) =
            registered(h.handle(publish("127.0.0.1", 1), peer()).await.unwrap());
        let deliver = |from, token, text: &str| Message::Deliver {
            from,
            token,
            text: text.into(),
        };
        let ping = |id, token| Message::Ping { id, token };
        let heartbeat = |token, inbox: Option<&str>| Message::Heartbeat {
            token,
            inbox: inbox.map(str::to_owned),
            service: None,
        };

        // Wrong token: refused like an unknown id.
        assert!(matches!(
            h.handle(deliver(service, wrong(), "x"), peer())
                .await
                .unwrap(),
            Message::Nack { .. }
        ));
        // Right token, but nobody holds the service yet.
        match h
            .handle(deliver(service, service_token, "x"), peer())
            .await
            .unwrap()
        {
            Message::Nack { reason } => assert!(reason.contains("not claimed"), "{reason}"),
            other => panic!("{other:?}"),
        }

        let claim = Message::Claim {
            key: 1,
            bind_addr: Addr::tcp("127.0.0.1", 2),
            ping: true,
        };
        let (client, client_token, paired_with) = paired(h.handle(claim, peer()).await.unwrap());
        assert_eq!(paired_with, service);

        // Client to service: the text rides on the service's next ping reply.
        assert_eq!(
            h.handle(deliver(client, client_token, "job 17"), peer())
                .await
                .unwrap(),
            Message::Delivered
        );
        assert_eq!(
            h.handle(ping(service, service_token), peer())
                .await
                .unwrap(),
            heartbeat(service_token, Some("job 17"))
        );
        // Service to client: the same picture mirrored.
        assert_eq!(
            h.handle(deliver(service, service_token, "ready"), peer())
                .await
                .unwrap(),
            Message::Delivered
        );
        assert_eq!(
            h.handle(ping(client, client_token), peer()).await.unwrap(),
            heartbeat(client_token, Some("ready"))
        );
        // Delivered once.
        assert_eq!(
            h.handle(ping(client, client_token), peer()).await.unwrap(),
            heartbeat(client_token, None)
        );
    }

    #[tokio::test]
    async fn store_relay_needs_a_token_and_follows_the_claim() {
        use crate::protocol::message::store_key;
        use crate::protocol::{StoreEntry, StoreOp, Stored};

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            crate::tls::install_default_provider();
            let h = handler(BrokerPolicy::default());
            // A ping-mode service and a two-sided client: the relay does not
            // depend on the liveness mode. (Nothing here yields to the runtime,
            // so the client's heartbeat task never runs and cannot remove it.)
            let (service, service_token) =
                registered(h.handle(publish("127.0.0.1", 1), peer()).await.unwrap());
            let relay = |from, token, op| Message::StoreRelay { from, token, op };
            let put = |value: &str| StoreOp::Put {
                key: store_key("step"),
                value: value.into(),
                if_version: None,
            };
            let nack = |reason: &str| Message::nack(reason);

            // Wrong token and unknown id: one text for both, before anything
            // else is looked at.
            let wrong_token = h
                .handle(relay(service, wrong(), StoreOp::List), peer())
                .await
                .unwrap();
            let unknown = h
                .handle(relay(PartyId(99), wrong(), put("5")), peer())
                .await
                .unwrap();
            assert_eq!(wrong_token, nack("unknown party or wrong token"));
            assert_eq!(unknown, wrong_token);

            // Right token, but nobody holds the service: it reads an empty
            // store (a list shows the broker's own entries alone) and may
            // not write.
            match h
                .handle(relay(service, service_token, StoreOp::List), peer())
                .await
                .unwrap()
            {
                Message::Stored(empty) => {
                    assert_eq!(
                        (empty.client, empty.revision, empty.applied),
                        (None, 0, true)
                    );
                    assert!(
                        empty.entries.iter().all(|e| e.key.is_reserved()),
                        "{empty:?}"
                    );
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(
                h.handle(relay(service, service_token, put("5")), peer())
                    .await
                    .unwrap(),
                nack(&format!("service {service} is not claimed"))
            );

            let claim = Message::Claim {
                key: 1,
                bind_addr: Addr::tcp("127.0.0.1", 2),
                ping: false,
            };
            let (client, client_token, paired_with) =
                paired(h.handle(claim, peer()).await.unwrap());
            assert_eq!(paired_with, service);
            let written = StoreEntry {
                key: store_key("step"),
                value: "5".into(),
                version: 1,
            };
            assert_eq!(
                h.handle(relay(client, client_token, put("5")), peer())
                    .await
                    .unwrap(),
                Message::Stored(Stored {
                    client: Some(client),
                    revision: 1,
                    applied: true,
                    entries: vec![written.clone()],
                })
            );
            assert_eq!(
                h.handle(
                    relay(
                        service,
                        service_token,
                        StoreOp::Get {
                            key: store_key("step")
                        }
                    ),
                    peer()
                )
                .await
                .unwrap(),
                Message::Stored(Stored {
                    client: Some(client),
                    revision: 1,
                    applied: true,
                    entries: vec![written],
                }),
                "the service reads the client's write"
            );
            // A party's token is its own: the client's does not work for the
            // service.
            assert_eq!(
                h.handle(relay(service, client_token, StoreOp::List), peer())
                    .await
                    .unwrap(),
                nack("unknown party or wrong token")
            );
            // A write that does not fit is refused with the budget's numbers.
            match h
                .handle(
                    relay(client, client_token, put(&"x".repeat(20_000))),
                    peer(),
                )
                .await
                .unwrap()
            {
                Message::Nack { reason } => assert!(reason.starts_with("store full"), "{reason}"),
                other => panic!("{other:?}"),
            }
        })
        .await
        .expect("the store relay test finished in time");
    }

    #[tokio::test]
    async fn store_by_key_resolves_the_claim_and_needs_no_token() {
        use crate::protocol::StoreOp;
        use crate::protocol::message::store_key;

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            crate::tls::install_default_provider();
            let h = handler(BrokerPolicy::default());
            let by_key = |rendezvous, party_id, op| Message::StoreByKey {
                rendezvous,
                party_id,
                op,
            };
            let put = |value: &str| StoreOp::Put {
                key: store_key("step"),
                value: value.into(),
                if_version: None,
            };
            // Nothing under the key yet.
            assert_eq!(
                h.handle(by_key(1, None, StoreOp::List), peer())
                    .await
                    .unwrap(),
                Message::nack("no party under key 1")
            );
            // A service nobody holds: an empty store, no writes.
            let (service, _) = registered(h.handle(publish("127.0.0.1", 1), peer()).await.unwrap());
            match h
                .handle(by_key(1, None, StoreOp::List), peer())
                .await
                .unwrap()
            {
                Message::Stored(s) => assert_eq!((s.client, s.revision), (None, 0)),
                other => panic!("{other:?}"),
            }
            assert_eq!(
                h.handle(by_key(1, None, put("5")), peer()).await.unwrap(),
                Message::nack(format!("service {service} is not claimed"))
            );
            // One claim: its store, through the key alone or either party.
            let claim = Message::Claim {
                key: 1,
                bind_addr: Addr::tcp("127.0.0.1", 2),
                ping: true,
            };
            let (client, client_token, _) = paired(h.handle(claim, peer()).await.unwrap());
            let written = match h.handle(by_key(1, None, put("5")), peer()).await.unwrap() {
                Message::Stored(s) => {
                    assert_eq!(s.client, Some(client));
                    s
                }
                other => panic!("{other:?}"),
            };
            let get = StoreOp::Get {
                key: store_key("step"),
            };
            for party_id in [None, Some(service), Some(client)] {
                assert_eq!(
                    h.handle(by_key(1, party_id, get.clone()), peer())
                        .await
                        .unwrap(),
                    Message::Stored(written.clone()),
                    "{party_id:?}"
                );
            }
            // The relay with the client's token sees the same store.
            assert_eq!(
                h.handle(
                    Message::StoreRelay {
                        from: client,
                        token: client_token,
                        op: get.clone(),
                    },
                    peer()
                )
                .await
                .unwrap(),
                Message::Stored(written)
            );
            // A party under another key is not under this one.
            assert_eq!(
                h.handle(by_key(2, Some(client), get), peer())
                    .await
                    .unwrap(),
                Message::nack(format!("no party {client} under key 2"))
            );
        })
        .await
        .expect("the test finished in time");
    }

    #[tokio::test]
    async fn a_store_relay_is_not_a_sign_of_life() {
        use crate::protocol::StoreOp;

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            crate::tls::install_default_provider();
            let h = handler(BrokerPolicy::default());
            let _service = registered(h.handle(publish("127.0.0.1", 1), peer()).await.unwrap());
            let claim = Message::Claim {
                key: 1,
                bind_addr: Addr::tcp("127.0.0.1", 2),
                ping: true,
            };
            let (client, token, _) = paired(h.handle(claim, peer()).await.unwrap());
            let liveness = || {
                h.broker
                    .with_registry(|r| r.get(client).map(|p| (p.failures(), p.last_seen())))
            };
            let _ = h.broker.with_registry(|r| r.record_failure(client));
            let before = liveness();
            assert_eq!(before.map(|(failures, _)| failures), Some(1));

            // Only heartbeats and pings count; a relay, like a deliver,
            // leaves the failure count and the last contact alone.
            let reply = h
                .handle(
                    Message::StoreRelay {
                        from: client,
                        token,
                        op: StoreOp::List,
                    },
                    peer(),
                )
                .await
                .unwrap();
            assert!(matches!(reply, Message::Stored(_)), "{reply:?}");
            assert_eq!(liveness(), before);
        })
        .await
        .unwrap();
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
            h.handle(
                Message::Ping {
                    id: PartyId(99),
                    token: wrong()
                },
                peer()
            )
            .await
            .unwrap(),
            Message::Nack { .. }
        ));
        for msg in [
            Message::Collect,
            Message::Heartbeat {
                token: wrong(),
                inbox: None,
                service: None,
            },
            Message::Send { text: "x".into() },
            Message::Delivered,
            // `store` carries no credentials: only a party's relay is
            // answered.
            Message::Store {
                op: crate::protocol::StoreOp::List,
            },
            Message::Stored(crate::protocol::Stored {
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
