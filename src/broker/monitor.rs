//! Liveness monitoring and removal.
//!
//! [`Broker`] owns the [`Registry`] behind a `std::sync::Mutex` whose guard
//! is never held across an `.await`, plus one heartbeat task per two-sided
//! party and one sweeper task for one-sided (ping) parties. Removal is a
//! single code path, [`Broker::drop_party`], which also re-pairs or removes
//! the clients orphaned by a vanished service.
//!
//! Compared with the previous implementation this replaces the shared
//! `VecDeque` that was popped once per 200 ms tick (so the heartbeat period
//! grew with the number of parties), the shared result tuple that lost
//! failures, and the re-claim that mutated a clone of the state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::task::AbortHandle;
use tokio::time::{sleep, timeout, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::registry::{Party, Registry, Removed};
use crate::config::{BrokerPolicy, Limits, Timing};
use crate::net::Addr;
use crate::protocol::{Key, Message, PartyId};
use crate::transport::Client;

/// The broker's shared state and background tasks.
#[derive(Debug)]
pub struct Broker {
    registry: Mutex<Registry>,
    client: Arc<Client>,
    timing: Timing,
    policy: BrokerPolicy,
    tasks: Mutex<HashMap<PartyId, AbortHandle>>,
    shutdown: CancellationToken,
}

/// One row of [`Broker::snapshot`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PartySummary {
    /// Broker-assigned id.
    pub id: PartyId,
    /// `"service"` or `"client"`.
    pub kind: &'static str,
    /// Rendezvous key.
    pub key: Key,
    /// Heartbeat endpoint.
    pub bind_addr: Addr,
    /// One-sided liveness.
    pub ping: bool,
    /// Consecutive failed heartbeats so far.
    pub failures: u32,
    /// For a service: the client holding it; for a client: its service.
    pub paired_with: Option<PartyId>,
}

impl Broker {
    /// A broker with an empty registry. `shutdown` stops every task it spawns.
    pub fn new(
        client: Arc<Client>,
        timing: Timing,
        limits: Limits,
        policy: BrokerPolicy,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Broker {
            registry: Mutex::new(Registry::new(limits)),
            client,
            timing,
            policy,
            tasks: Mutex::new(HashMap::new()),
            shutdown,
        })
    }

    /// Intervals and thresholds in force.
    pub fn timing(&self) -> &Timing {
        &self.timing
    }

    /// Admission policy in force.
    pub fn policy(&self) -> &BrokerPolicy {
        &self.policy
    }

    /// The token that stops the broker's tasks.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Run `f` with the registry locked. The closure must not block or
    /// await; the guard is released when it returns.
    pub fn with_registry<R>(&self, f: impl FnOnce(&mut Registry) -> R) -> R {
        let mut guard = lock(&self.registry);
        f(&mut guard)
    }

    /// Start heartbeating a two-sided party. A no-op for ping-mode parties
    /// (the sweeper covers them) and for unknown ids.
    pub fn watch(self: &Arc<Self>, id: PartyId) {
        match self.with_registry(|r| r.is_ping(id)) {
            Some(false) => {}
            Some(true) => return,
            None => {
                debug!(%id, "watch requested for an unknown party");
                return;
            }
        }
        let handle = tokio::spawn(heartbeat_loop(Arc::clone(self), id)).abort_handle();
        if let Some(old) = lock(&self.tasks).insert(id, handle) {
            old.abort();
        }
    }

    /// Start the task that removes ping-mode parties that fell silent for
    /// longer than [`Timing::ping_staleness`]. Call once.
    pub fn start_sweeper(self: &Arc<Self>) {
        tokio::spawn(sweeper_loop(Arc::clone(self)));
    }

    /// Remove a party and deal with the consequences: a vanished service's
    /// clients are re-paired with another service of the same key when one
    /// is free, otherwise removed; a vanished client frees its service.
    pub fn drop_party(&self, id: PartyId, reason: &str) -> Removed {
        let removed = self.with_registry(|r| r.remove(id));
        self.abort_task(id);
        match &removed {
            Removed::Service { orphaned_clients } => {
                info!(%id, reason, orphans = orphaned_clients.len(), "service removed");
                for client in orphaned_clients {
                    match self.with_registry(|r| r.reclaim(*client)) {
                        Some(handle) => {
                            info!(client = %client, service = %handle.id, "client re-paired")
                        }
                        None => {
                            warn!(client = %client, "no replacement service; client removed");
                            let _ = self.with_registry(|r| r.remove(*client));
                            self.abort_task(*client);
                        }
                    }
                }
            }
            Removed::Client { freed_service } => {
                info!(%id, reason, freed = ?freed_service, "client removed");
            }
            Removed::Unknown => debug!(%id, reason, "removal of an unknown party"),
        }
        removed
    }

    /// Everything the broker currently knows, for status output and tests.
    pub fn snapshot(&self) -> Vec<PartySummary> {
        self.with_registry(|r| {
            let mut ids = r.service_ids();
            ids.extend(r.client_ids());
            ids.sort_unstable();
            ids.into_iter()
                .filter_map(|id| match r.get(id)? {
                    Party::Service(s) => Some(PartySummary {
                        id,
                        kind: "service",
                        key: s.record.key,
                        bind_addr: s.record.bind_addr.clone(),
                        ping: s.record.ping,
                        failures: s.failures,
                        paired_with: s.claimed_by,
                    }),
                    Party::Client(c) => Some(PartySummary {
                        id,
                        kind: "client",
                        key: c.record.key,
                        bind_addr: c.record.bind_addr.clone(),
                        ping: c.record.ping,
                        failures: c.failures,
                        paired_with: Some(c.record.service),
                    }),
                })
                .collect()
        })
    }

    fn abort_task(&self, id: PartyId) {
        if let Some(handle) = lock(&self.tasks).remove(&id) {
            handle.abort();
        }
    }
}

/// Lock a `std::sync::Mutex`, recovering from poisoning.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Dial one two-sided party every heartbeat interval until it fails
/// `fail_threshold` times in a row, is removed, or the broker shuts down.
async fn heartbeat_loop(broker: Arc<Broker>, id: PartyId) {
    let t = broker.timing.clone();
    loop {
        tokio::select! {
            _ = broker.shutdown.cancelled() => return,
            _ = sleep(t.heartbeat_interval) => {}
        }
        let Some((addr, hb)) = broker.with_registry(|r| {
            let addr = r.bind_addr(id)?.clone();
            let hb = r.heartbeat_for(id)?;
            Some((addr, hb))
        }) else {
            return;
        };
        let pending = match &hb {
            Message::Heartbeat { inbox, service, .. } => (inbox.clone(), service.clone()),
            _ => (None, None),
        };
        match timeout(t.heartbeat_timeout, broker.client.call(&addr, hb)).await {
            Ok(Ok(Message::HeartbeatAck { id: acked })) => {
                if acked != id && acked != PartyId(0) {
                    debug!(%id, %acked, "heartbeat acknowledged with another id");
                }
                broker.with_registry(|r| r.mark_alive(id, Instant::now()));
            }
            outcome => {
                let failures = broker.with_registry(|r| {
                    r.restore(id, pending.0, pending.1);
                    r.record_failure(id)
                });
                let Some(failures) = failures else { return };
                debug!(%id, %addr, failures, ?outcome, "heartbeat failed");
                if failures >= t.fail_threshold {
                    let _ = broker.drop_party(id, "heartbeats failed");
                    return;
                }
            }
        }
    }
}

/// Remove ping-mode parties that have been silent too long.
async fn sweeper_loop(broker: Arc<Broker>) {
    let t = broker.timing.clone();
    loop {
        tokio::select! {
            _ = broker.shutdown.cancelled() => return,
            _ = sleep(t.heartbeat_interval) => {}
        }
        let stale = broker.with_registry(|r| r.stale(Instant::now(), t.ping_staleness));
        for id in stale {
            let _ = broker.drop_party(id, "no ping received");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::config::TlsPaths;
    use crate::net::Transport;
    use crate::protocol::{RegToken, ServiceHandle};
    use crate::transport::{serve, Handler, PeerInfo, Server};
    use crate::Result;

    /// How a scripted party answers the broker's heartbeats.
    #[derive(Debug, Clone, Copy)]
    enum Respond {
        /// The normal acknowledgement.
        Ack,
        /// Acknowledges with somebody else's id (logged, still an ack).
        ForeignId,
        /// Two refusals, then acknowledgements: failures must reset.
        FailTwiceThenAck,
        /// Refuses every heartbeat.
        Nack,
        /// Answers with an unrelated message.
        Wrong,
        /// Never answers within the heartbeat timeout.
        Hang,
    }

    struct Scripted {
        mode: Respond,
        calls: AtomicUsize,
    }

    impl Handler for Scripted {
        async fn handle(&self, msg: Message, _peer: PeerInfo) -> Result<Message> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(matches!(msg, Message::Heartbeat { .. }), "{msg:?}");
            Ok(match self.mode {
                Respond::Ack => Message::HeartbeatAck { id: PartyId(0) },
                Respond::ForeignId => Message::HeartbeatAck { id: PartyId(4242) },
                Respond::FailTwiceThenAck if n < 2 => Message::nack("warming up"),
                Respond::FailTwiceThenAck => Message::HeartbeatAck { id: PartyId(0) },
                Respond::Nack => Message::nack("not today"),
                Respond::Wrong => Message::Delivered,
                Respond::Hang => {
                    sleep(Duration::from_secs(10)).await;
                    Message::Delivered
                }
            })
        }
    }

    async fn party(mode: Respond) -> (Server, Arc<Scripted>) {
        let handler = Arc::new(Scripted {
            mode,
            calls: AtomicUsize::new(0),
        });
        let server = serve(
            &Addr::new(Transport::Tcp, "127.0.0.1", 0),
            Arc::clone(&handler),
            &TlsPaths::default(),
            &Limits::default(),
            &Timing::fast(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        (server, handler)
    }

    fn broker() -> Arc<Broker> {
        crate::tls::install_default_provider();
        let client = Arc::new(Client::new(
            TlsPaths::default(),
            Timing::fast(),
            Limits::default(),
        ));
        Broker::new(
            client,
            Timing::fast(),
            Limits::default(),
            BrokerPolicy::default(),
            CancellationToken::new(),
        )
    }

    fn token(n: u8) -> RegToken {
        RegToken::from_bytes([n; 16])
    }

    fn publish(b: &Broker, key: Key, bind: &Addr, ping: bool) -> PartyId {
        b.with_registry(|r| {
            r.publish(
                key,
                Addr::tcp(bind.host.clone(), 9000),
                bind.clone(),
                ping,
                token(1),
                Instant::now(),
            )
        })
        .unwrap()
    }

    fn claim(b: &Broker, key: Key, bind: &Addr) -> (PartyId, ServiceHandle) {
        b.with_registry(|r| r.claim(key, bind.clone(), true, token(2), Instant::now()))
            .unwrap()
    }

    fn present(b: &Broker, id: PartyId) -> bool {
        b.with_registry(|r| r.get(id).is_some())
    }

    fn row(b: &Broker, id: PartyId) -> Option<PartySummary> {
        b.snapshot().into_iter().find(|p| p.id == id)
    }

    async fn wait_for(mut pred: impl FnMut() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pred() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            sleep(Duration::from_millis(10)).await;
        }
    }

    fn unused_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn heartbeat_replies_decide_who_stays() {
        let table = [
            (Respond::Ack, true),
            (Respond::ForeignId, true),
            (Respond::FailTwiceThenAck, true),
            (Respond::Nack, false),
            (Respond::Wrong, false),
            (Respond::Hang, false),
        ];
        let t = Timing::fast();
        let window = (t.heartbeat_interval + t.heartbeat_timeout) * (t.fail_threshold + 2);
        for (mode, stays) in table {
            let (server, handler) = party(mode).await;
            let b = broker();
            let id = publish(&b, 1, &server.bound(), false);
            b.watch(id);
            if stays {
                sleep(window).await;
                assert!(present(&b, id), "{mode:?}: party was removed");
                let calls = handler.calls.load(Ordering::SeqCst);
                assert!(calls >= 3, "{mode:?}: only {calls} heartbeats");
                assert_eq!(row(&b, id).map(|p| p.failures), Some(0), "{mode:?}");
            } else {
                wait_for(|| !present(&b, id), &format!("{mode:?} removal")).await;
                let calls = handler.calls.load(Ordering::SeqCst);
                let threshold = t.fail_threshold as usize;
                assert!(
                    (threshold..=threshold + 1).contains(&calls),
                    "{mode:?}: {calls} heartbeats before removal"
                );
                assert!(lock(&b.tasks).is_empty(), "{mode:?}: task not cleaned up");
            }
            b.shutdown_token().cancel();
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn unreachable_party_is_removed_after_the_threshold() {
        let b = broker();
        let id = publish(&b, 1, &Addr::tcp("127.0.0.1", unused_port()), false);
        b.watch(id);
        wait_for(|| !present(&b, id), "removal of an unreachable party").await;
        assert!(lock(&b.tasks).is_empty());
        b.shutdown_token().cancel();
    }

    #[tokio::test]
    async fn watch_ignores_ping_parties_and_unknown_ids() {
        let b = broker();
        let pinger = publish(&b, 1, &Addr::tcp("10.0.0.1", 1), true);
        b.watch(pinger);
        b.watch(PartyId(77));
        assert!(lock(&b.tasks).is_empty());
        let two_sided = publish(&b, 2, &Addr::tcp("127.0.0.1", unused_port()), false);
        b.watch(two_sided);
        assert_eq!(lock(&b.tasks).len(), 1);
        // Re-watching replaces the task instead of doubling the heartbeats.
        b.watch(two_sided);
        assert_eq!(lock(&b.tasks).len(), 1);
        b.shutdown_token().cancel();
    }

    #[tokio::test]
    async fn drop_party_repairs_orphans_when_a_service_is_free() {
        let b = broker();
        // Ping-mode parties: no heartbeat tasks, so the registry alone decides.
        let first = publish(&b, 1, &Addr::tcp("10.0.0.1", 1), true);
        let second = publish(&b, 1, &Addr::tcp("10.0.0.2", 1), true);
        let (client, handle) = claim(&b, 1, &Addr::tcp("10.0.0.3", 1));
        let (taken, spare) = if handle.id == first {
            (first, second)
        } else {
            (second, first)
        };

        // The claimed service vanishes: its client moves to the spare one.
        match b.drop_party(taken, "test") {
            Removed::Service { orphaned_clients } => assert_eq!(orphaned_clients, vec![client]),
            other => panic!("{other:?}"),
        }
        assert!(!present(&b, taken));
        assert_eq!(row(&b, client).unwrap().paired_with, Some(spare));
        assert_eq!(row(&b, spare).unwrap().paired_with, Some(client));

        // The spare vanishes too: nothing is left for the client, so it goes.
        match b.drop_party(spare, "test") {
            Removed::Service { orphaned_clients } => assert_eq!(orphaned_clients, vec![client]),
            other => panic!("{other:?}"),
        }
        assert!(b.with_registry(|r| r.is_empty()));
        assert!(matches!(b.drop_party(client, "test"), Removed::Unknown));
    }

    #[tokio::test]
    async fn drop_party_removes_orphans_when_every_other_service_is_taken() {
        let b = broker();
        let s1 = publish(&b, 1, &Addr::tcp("10.0.0.1", 1), true);
        let s2 = publish(&b, 1, &Addr::tcp("10.0.0.2", 1), true);
        let (c1, h1) = claim(&b, 1, &Addr::tcp("10.0.0.3", 1));
        let (c2, h2) = claim(&b, 1, &Addr::tcp("10.0.0.4", 1));
        assert_ne!(h1.id, h2.id);
        let orphan = if h1.id == s1 { c1 } else { c2 };
        match b.drop_party(s1, "test") {
            Removed::Service { orphaned_clients } => assert_eq!(orphaned_clients, vec![orphan]),
            other => panic!("{other:?}"),
        }
        assert!(
            !present(&b, orphan),
            "no free service, so the orphan is removed"
        );
        assert!(present(&b, s2));
        assert_eq!(b.snapshot().len(), 2);
    }

    #[tokio::test]
    async fn dropping_a_client_frees_its_service() {
        let b = broker();
        let s = publish(&b, 1, &Addr::tcp("10.0.0.1", 1), true);
        let (c, handle) = claim(&b, 1, &Addr::tcp("10.0.0.3", 1));
        assert_eq!(handle.id, s);
        assert_eq!(row(&b, s).unwrap().paired_with, Some(c));
        match b.drop_party(c, "test") {
            Removed::Client { freed_service } => assert_eq!(freed_service, Some(s)),
            other => panic!("{other:?}"),
        }
        assert_eq!(row(&b, s).unwrap().paired_with, None);
        // The freed service can be claimed again.
        let (again, handle) = claim(&b, 1, &Addr::tcp("10.0.0.5", 1));
        assert_eq!(handle.id, s);
        assert_eq!(row(&b, s).unwrap().paired_with, Some(again));
        assert!(matches!(
            b.drop_party(PartyId(999), "test"),
            Removed::Unknown
        ));
    }

    #[tokio::test]
    async fn sweeper_removes_silent_ping_parties_only() {
        let b = broker();
        b.start_sweeper();
        let silent = publish(&b, 1, &Addr::tcp("10.0.0.1", 1), true);
        let chatty = publish(&b, 2, &Addr::tcp("10.0.0.2", 1), true);
        let unwatched_two_sided = publish(&b, 3, &Addr::tcp("10.0.0.3", 1), false);
        let keep = Arc::clone(&b);
        let pinger = tokio::spawn(async move {
            loop {
                sleep(Duration::from_millis(20)).await;
                let _ = keep.with_registry(|r| r.mark_alive(chatty, Instant::now()));
            }
        });
        wait_for(|| !present(&b, silent), "the silent party to be swept").await;
        assert!(present(&b, chatty));
        assert!(present(&b, unwatched_two_sided));
        pinger.abort();
        b.shutdown_token().cancel();
    }

    #[test]
    fn snapshot_rows_describe_both_kinds() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();
        let b = broker();
        let s = publish(
            &b,
            5,
            &Addr::new(Transport::Https, "svc.example", 4433),
            true,
        );
        let (c, _) = claim(&b, 5, &Addr::tcp("10.0.0.9", 7000));
        let snap = b.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap[0],
            PartySummary {
                id: s,
                kind: "service",
                key: 5,
                bind_addr: Addr::new(Transport::Https, "svc.example", 4433),
                ping: true,
                failures: 0,
                paired_with: Some(c),
            }
        );
        assert_eq!(snap[1].kind, "client");
        assert_eq!(snap[1].paired_with, Some(s));
        assert_eq!(
            serde_json::to_value(&snap[1]).unwrap()["bind_addr"],
            serde_json::json!("10.0.0.9:7000")
        );
    }
}
