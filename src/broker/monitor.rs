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
            Message::Heartbeat { inbox, service } => (inbox.clone(), service.clone()),
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
