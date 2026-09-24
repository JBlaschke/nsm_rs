//! The broker's pure state: which services are published, which clients hold
//! them, and what is waiting to be delivered to whom.
//!
//! [`Registry`] is synchronous and does no I/O. Every method returns
//! immediately, and the clock is only ever read by the caller, who passes
//! `now: Instant` in; the registry never calls `Instant::now()` itself, so
//! tests can drive time explicitly and the monitor's paused-time tests see a
//! consistent clock. The broker owns one `Registry` behind a
//! `std::sync::Mutex`, takes the guard for the duration of a single method
//! call and never holds it across an `await`.
//!
//! # Model
//!
//! A *party* is a service or a client. Both draw their [`PartyId`] from one
//! counter that starts at 1 and never repeats a value, so an id names the
//! same party for the broker's whole life, even after removal.
//!
//! - A service [`publish`](Registry::publish)es under a [`Key`] and is
//!   *unclaimed* until a client takes it.
//! - A [`claim`](Registry::claim) registers a client and pairs it with the
//!   lowest-id unclaimed service of its key, exclusively: the service stays
//!   claimed until that client is [`remove`](Registry::remove)d (decision D7
//!   of the plan; there is no time-based lease).
//! - Removing a service *orphans* its client. The client stays registered,
//!   still naming the dead service, and the owner calls
//!   [`reclaim`](Registry::reclaim) for it: that pairs the orphan with
//!   another unclaimed service of the same key and stores the new
//!   [`ServiceHandle`] as *pending*, to be carried by the client's next
//!   heartbeat. If no service is available the owner removes the client.
//! - Removing a client frees its service for the next claim.
//! - [`deliver`](Registry::deliver) parks text as a service's pending
//!   *inbox* (a later delivery replaces an earlier one), and
//!   [`heartbeat_for`](Registry::heartbeat_for) builds the
//!   [`Message::Heartbeat`] for a party, taking the pending inbox text or
//!   service handle with it so each is delivered exactly once.
//!
//! Liveness bookkeeping ([`mark_alive`](Registry::mark_alive),
//! [`record_failure`](Registry::record_failure), [`stale`](Registry::stale))
//! only records what the monitor observed. Deciding that a party is dead is
//! the monitor's job, made from the counts and timestamps kept here and the
//! thresholds in [`Timing`](crate::config::Timing).
//!
//! [`Limits::max_registrations`] bounds services and clients together.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::config::Limits;
use crate::net::Addr;
use crate::protocol::{ClientRecord, Key, Message, PartyId, ServiceHandle, ServiceRecord};
use crate::{Error, Result};

/// A published service as the broker tracks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    /// Id, key, data-plane and heartbeat endpoints, liveness mode.
    pub record: ServiceRecord,
    /// The client holding this service, or `None` while it is unclaimed.
    pub claimed_by: Option<PartyId>,
    /// Text delivered to this service and not yet carried by a heartbeat. A
    /// later [`Registry::deliver`] replaces an earlier one.
    pub inbox: Option<String>,
    /// Consecutive failed heartbeats since the broker last heard from the
    /// service.
    pub failures: u32,
    /// When the broker last heard from the service: at registration, on a
    /// successful heartbeat or on a ping.
    pub last_seen: Instant,
}

/// A claiming client as the broker tracks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEntry {
    /// Id, key, heartbeat endpoint, paired service, liveness mode. After the
    /// service died, `record.service` keeps naming it until
    /// [`Registry::reclaim`] re-pairs the client.
    pub record: ClientRecord,
    /// A new pairing from [`Registry::reclaim`] that the client has not been
    /// told about yet; carried by its next heartbeat.
    pub pending_service: Option<ServiceHandle>,
    /// Consecutive failed heartbeats since the broker last heard from the
    /// client.
    pub failures: u32,
    /// When the broker last heard from the client.
    pub last_seen: Instant,
}

/// What [`Registry::remove`] found and undid.
#[must_use = "a removed service leaves orphaned clients that must be re-paired or removed"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removed {
    /// A service was removed. The listed clients were paired with it and now
    /// name a dead id; the owner should [`reclaim`](Registry::reclaim) each
    /// of them and remove those it cannot re-pair.
    Service {
        /// Clients that lost their service, in ascending id order.
        orphaned_clients: Vec<PartyId>,
    },
    /// A client was removed. `freed_service` names the service it held, now
    /// unclaimed again, or is `None` when the client was an orphan whose
    /// service had already gone.
    Client {
        /// The service this removal released, if any.
        freed_service: Option<PartyId>,
    },
    /// No party has this id: it was never issued, or already removed.
    Unknown,
}

/// A borrowed view of a party of either kind, from [`Registry::get`] and
/// [`Registry::parties`].
///
/// The accessors answer the questions the monitor asks of any party without
/// caring which kind it is; match on the variant for the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Party<'a> {
    /// The id belongs to a service.
    Service(&'a ServiceEntry),
    /// The id belongs to a client.
    Client(&'a ClientEntry),
}

impl<'a> Party<'a> {
    /// The party's id.
    pub fn id(self) -> PartyId {
        match self {
            Party::Service(s) => s.record.id,
            Party::Client(c) => c.record.id,
        }
    }

    /// The key the party published or claimed.
    pub fn key(self) -> Key {
        match self {
            Party::Service(s) => s.record.key,
            Party::Client(c) => c.record.key,
        }
    }

    /// Heartbeat endpoint the broker dials in two-sided mode.
    pub fn bind_addr(self) -> &'a Addr {
        match self {
            Party::Service(s) => &s.record.bind_addr,
            Party::Client(c) => &c.record.bind_addr,
        }
    }

    /// True when the party pings the broker instead of being dialled.
    pub fn is_ping(self) -> bool {
        match self {
            Party::Service(s) => s.record.ping,
            Party::Client(c) => c.record.ping,
        }
    }

    /// Consecutive failed heartbeats since the broker last heard from the
    /// party.
    pub fn failures(self) -> u32 {
        match self {
            Party::Service(s) => s.failures,
            Party::Client(c) => c.failures,
        }
    }

    /// When the broker last heard from the party.
    pub fn last_seen(self) -> Instant {
        match self {
            Party::Service(s) => s.last_seen,
            Party::Client(c) => c.last_seen,
        }
    }
}

/// Source of party ids: starts at 1, strictly increasing, never reused.
#[derive(Debug)]
struct IdCounter(u64);

impl IdCounter {
    fn allocate(&mut self) -> PartyId {
        let id = PartyId(self.0);
        self.0 += 1;
        id
    }
}

/// The broker's registry of services and clients. See the [module
/// docs](self) for the model it implements.
///
/// Lookups by id are logarithmic; finding a service to claim is linear in
/// the number of services, which is fine for the sizes
/// [`Limits::max_registrations`] allows and keeps the state a plain pair of
/// maps.
#[derive(Debug)]
pub struct Registry {
    limits: Limits,
    ids: IdCounter,
    services: BTreeMap<PartyId, ServiceEntry>,
    clients: BTreeMap<PartyId, ClientEntry>,
}

impl Default for Registry {
    /// An empty registry with [`Limits::default`].
    fn default() -> Self {
        Registry::new(Limits::default())
    }
}

impl Registry {
    /// An empty registry that admits at most `limits.max_registrations`
    /// parties, services and clients together.
    pub fn new(limits: Limits) -> Self {
        Registry {
            limits,
            ids: IdCounter(1),
            services: BTreeMap::new(),
            clients: BTreeMap::new(),
        }
    }

    /// The limits this registry enforces.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    // ----- registration -----------------------------------------------------

    /// Register a service under `key` and return its new id.
    ///
    /// `service_addr` is the data-plane endpoint claimers are told about,
    /// `bind_addr` the heartbeat endpoint the broker dials (with `ping` the
    /// service pings instead), and `now` initialises `last_seen`.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] (`"registry full"`) when services plus clients
    /// already reach [`Limits::max_registrations`]; no id is allocated.
    pub fn publish(
        &mut self,
        key: Key,
        service_addr: Addr,
        bind_addr: Addr,
        ping: bool,
        now: Instant,
    ) -> Result<PartyId> {
        self.check_capacity()?;
        let id = self.ids.allocate();
        self.services.insert(
            id,
            ServiceEntry {
                record: ServiceRecord {
                    id,
                    key,
                    service_addr,
                    bind_addr,
                    ping,
                },
                claimed_by: None,
                inbox: None,
                failures: 0,
                last_seen: now,
            },
        );
        Ok(id)
    }

    /// Register a client and pair it with the lowest-id unclaimed service
    /// published under `key`.
    ///
    /// Returns the client's new id and the handle of its service, which is
    /// now claimed by that client until the client is removed.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] (`"registry full"`) at the registration limit,
    /// checked before any search so that a full broker answers the same way
    /// whether or not a service is available; [`Error::NoService`] when no
    /// unclaimed service is published under `key`. Neither allocates an id
    /// or changes any service.
    pub fn claim(
        &mut self,
        key: Key,
        bind_addr: Addr,
        ping: bool,
        now: Instant,
    ) -> Result<(PartyId, ServiceHandle)> {
        self.check_capacity()?;
        let service =
            Self::lowest_unclaimed(&mut self.services, key).ok_or(Error::NoService(key))?;
        let id = self.ids.allocate();
        service.claimed_by = Some(id);
        let handle = service.record.handle();
        self.clients.insert(
            id,
            ClientEntry {
                record: ClientRecord {
                    id,
                    key,
                    bind_addr,
                    service: handle.id,
                    ping,
                },
                pending_service: None,
                failures: 0,
                last_seen: now,
            },
        );
        Ok((id, handle))
    }

    /// Remove a party of either kind.
    ///
    /// Removing a service orphans the clients paired with it: they stay
    /// registered, keep naming the dead service in `record.service`, lose any
    /// pending handle for it, and are listed in the result so the owner can
    /// [`reclaim`](Registry::reclaim) them. Removing a client frees the
    /// service it held. Pending inbox text of a removed service is dropped.
    /// Ids are never reissued, so a removed id stays unknown from now on.
    pub fn remove(&mut self, id: PartyId) -> Removed {
        if self.services.remove(&id).is_some() {
            let mut orphaned_clients = Vec::new();
            for client in self.clients.values_mut().filter(|c| c.record.service == id) {
                // Never tell a client about a service that is already gone.
                if client.pending_service.as_ref().is_some_and(|h| h.id == id) {
                    client.pending_service = None;
                }
                orphaned_clients.push(client.record.id);
            }
            return Removed::Service { orphaned_clients };
        }
        match self.clients.remove(&id) {
            Some(client) => {
                let freed_service = match self.services.get_mut(&client.record.service) {
                    Some(service) if service.claimed_by == Some(id) => {
                        service.claimed_by = None;
                        Some(service.record.id)
                    }
                    _ => None,
                };
                Removed::Client { freed_service }
            }
            None => Removed::Unknown,
        }
    }

    /// Re-pair an orphaned client with the lowest-id unclaimed service of
    /// its key.
    ///
    /// On success the client's `record.service` names the new service, the
    /// service is claimed by the client, and the service's handle is stored
    /// as the client's `pending_service` so that its next
    /// [`heartbeat_for`](Registry::heartbeat_for) carries it. The same
    /// handle is returned for the caller's logs.
    ///
    /// Returns `None`, changing nothing, when `client` is not a client id or
    /// no unclaimed service of its key exists; the owner then removes the
    /// client. A client whose current service is still alive and held by it
    /// is not touched either: its current handle is returned and nothing
    /// becomes pending.
    pub fn reclaim(&mut self, client: PartyId) -> Option<ServiceHandle> {
        let entry = self.clients.get_mut(&client)?;
        if let Some(current) = self.services.get(&entry.record.service) {
            if current.claimed_by == Some(client) {
                return Some(current.record.handle());
            }
        }
        let service = Self::lowest_unclaimed(&mut self.services, entry.record.key)?;
        service.claimed_by = Some(client);
        let handle = service.record.handle();
        entry.record.service = handle.id;
        entry.pending_service = Some(handle.clone());
        Some(handle)
    }

    // ----- delivery ---------------------------------------------------------

    /// Park `text` as the pending inbox of service `to`, replacing whatever
    /// was pending; the next [`heartbeat_for`](Registry::heartbeat_for) that
    /// service carries it.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] (`"unknown service <id>"`) when `to` is not a
    /// registered service; a client id is rejected too, because text is
    /// only ever delivered to services.
    pub fn deliver(&mut self, to: PartyId, text: String) -> Result<()> {
        match self.services.get_mut(&to) {
            Some(service) => {
                service.inbox = Some(text);
                Ok(())
            }
            None => Err(Error::Rejected(format!("unknown service {to}"))),
        }
    }

    /// Build the [`Message::Heartbeat`] for a party, taking whatever is
    /// pending for it: a service's inbox text, a client's new service
    /// handle. The pending value is cleared, so a second call returns an
    /// empty heartbeat until something new is pending.
    ///
    /// Built for every party regardless of its liveness mode: the owner
    /// sends it to a two-sided party at the heartbeat interval and returns
    /// it as the reply to a one-sided party's [`Message::Ping`], so both
    /// modes deliver the same things. `None` for an unknown id.
    pub fn heartbeat_for(&mut self, id: PartyId) -> Option<Message> {
        if let Some(service) = self.services.get_mut(&id) {
            return Some(Message::Heartbeat {
                inbox: service.inbox.take(),
                service: None,
            });
        }
        let client = self.clients.get_mut(&id)?;
        Some(Message::Heartbeat {
            inbox: None,
            service: client.pending_service.take(),
        })
    }

    /// Put back pending items taken by [`Registry::heartbeat_for`] when the
    /// heartbeat that carried them failed, unless something newer arrived in
    /// the meantime (a later `deliver` or re-pairing wins). A no-op for
    /// unknown ids.
    pub fn restore(&mut self, id: PartyId, inbox: Option<String>, service: Option<ServiceHandle>) {
        if let Some(entry) = self.services.get_mut(&id) {
            if entry.inbox.is_none() {
                entry.inbox = inbox;
            }
        } else if let Some(entry) = self.clients.get_mut(&id) {
            if entry.pending_service.is_none() {
                entry.pending_service = service;
            }
        }
    }

    // ----- liveness ---------------------------------------------------------

    /// Record that the party answered (a heartbeat succeeded or a ping
    /// arrived) at `now`: its failure count returns to zero and `last_seen`
    /// is updated. Returns false for an unknown id.
    pub fn mark_alive(&mut self, id: PartyId, now: Instant) -> bool {
        match self.liveness_mut(id) {
            Some((failures, last_seen)) => {
                *failures = 0;
                *last_seen = now;
                true
            }
            None => false,
        }
    }

    /// Record one more consecutive failed heartbeat and return the new
    /// count, which the monitor compares with
    /// [`Timing::fail_threshold`](crate::config::Timing::fail_threshold).
    /// Saturates rather than wrapping. `None` for an unknown id.
    pub fn record_failure(&mut self, id: PartyId) -> Option<u32> {
        let (failures, _) = self.liveness_mut(id)?;
        *failures = failures.saturating_add(1);
        Some(*failures)
    }

    /// Ping-mode parties from which nothing has been heard for longer than
    /// `older_than` as of `now`, in ascending id order: the candidates for
    /// removal in one-sided mode.
    ///
    /// Two-sided parties are never listed; their liveness is judged from
    /// their failure count. A `last_seen` later than `now` counts as no
    /// silence at all.
    pub fn stale(&self, now: Instant, older_than: Duration) -> Vec<PartyId> {
        let mut ids: Vec<PartyId> = self
            .parties()
            .filter(|p| p.is_ping() && now.saturating_duration_since(p.last_seen()) > older_than)
            .map(Party::id)
            .collect();
        ids.sort_unstable();
        ids
    }

    // ----- queries ----------------------------------------------------------

    /// The party with this id, of either kind.
    pub fn get(&self, id: PartyId) -> Option<Party<'_>> {
        if let Some(service) = self.services.get(&id) {
            return Some(Party::Service(service));
        }
        self.clients.get(&id).map(Party::Client)
    }

    /// The service with this id; `None` for a client id or an unknown id.
    pub fn service(&self, id: PartyId) -> Option<&ServiceEntry> {
        self.services.get(&id)
    }

    /// The client with this id; `None` for a service id or an unknown id.
    pub fn client(&self, id: PartyId) -> Option<&ClientEntry> {
        self.clients.get(&id)
    }

    /// Heartbeat endpoint of the party with this id.
    pub fn bind_addr(&self, id: PartyId) -> Option<&Addr> {
        self.get(id).map(Party::bind_addr)
    }

    /// Whether the party with this id pings the broker instead of being
    /// dialled.
    pub fn is_ping(&self, id: PartyId) -> Option<bool> {
        self.get(id).map(Party::is_ping)
    }

    /// Registered parties of both kinds, counted against
    /// [`Limits::max_registrations`].
    pub fn len(&self) -> usize {
        self.services.len() + self.clients.len()
    }

    /// True when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ids of all services, ascending.
    pub fn service_ids(&self) -> Vec<PartyId> {
        self.services.keys().copied().collect()
    }

    /// Ids of all clients, ascending.
    pub fn client_ids(&self) -> Vec<PartyId> {
        self.clients.keys().copied().collect()
    }

    /// All services, in ascending id order.
    pub fn services(&self) -> impl Iterator<Item = &ServiceEntry> {
        self.services.values()
    }

    /// All clients, in ascending id order.
    pub fn clients(&self) -> impl Iterator<Item = &ClientEntry> {
        self.clients.values()
    }

    /// All parties: every service in ascending id order, then every client
    /// in ascending id order.
    pub fn parties(&self) -> impl Iterator<Item = Party<'_>> {
        self.services
            .values()
            .map(Party::Service)
            .chain(self.clients.values().map(Party::Client))
    }

    // ----- helpers ----------------------------------------------------------

    /// Reject a registration when the limit is reached.
    fn check_capacity(&self) -> Result<()> {
        if self.len() >= self.limits.max_registrations {
            return Err(Error::Rejected("registry full".into()));
        }
        Ok(())
    }

    /// The unclaimed service with the lowest id under `key`. Takes the map
    /// rather than `&mut self` so callers can hold it alongside a borrow of
    /// another field.
    fn lowest_unclaimed(
        services: &mut BTreeMap<PartyId, ServiceEntry>,
        key: Key,
    ) -> Option<&mut ServiceEntry> {
        services
            .values_mut()
            .find(|s| s.record.key == key && s.claimed_by.is_none())
    }

    /// The failure count and `last_seen` of a party of either kind.
    fn liveness_mut(&mut self, id: PartyId) -> Option<(&mut u32, &mut Instant)> {
        if let Some(service) = self.services.get_mut(&id) {
            return Some((&mut service.failures, &mut service.last_seen));
        }
        self.clients
            .get_mut(&id)
            .map(|client| (&mut client.failures, &mut client.last_seen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Transport;

    const KEY: Key = 42;

    fn now() -> Instant {
        Instant::now()
    }

    fn sec(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// A registry with room for `n` parties.
    fn registry(n: usize) -> Registry {
        Registry::new(Limits {
            max_registrations: n,
            ..Limits::default()
        })
    }

    fn addr(port: u16) -> Addr {
        Addr::tcp("10.0.0.1", port)
    }

    fn publish(r: &mut Registry, key: Key, ping: bool, t: Instant) -> PartyId {
        r.publish(key, addr(9000), addr(9001), ping, t).unwrap()
    }

    fn claim(r: &mut Registry, key: Key, ping: bool, t: Instant) -> (PartyId, ServiceHandle) {
        r.claim(key, addr(7000), ping, t).unwrap()
    }

    fn handle_of(r: &Registry, service: PartyId) -> ServiceHandle {
        r.service(service).unwrap().record.handle()
    }

    fn claimed_by(r: &Registry, service: PartyId) -> Option<PartyId> {
        r.service(service).unwrap().claimed_by
    }

    fn heartbeat(inbox: Option<&str>, service: Option<ServiceHandle>) -> Message {
        Message::Heartbeat {
            inbox: inbox.map(str::to_owned),
            service,
        }
    }

    fn is_full<T: std::fmt::Debug>(result: Result<T>) -> bool {
        matches!(result, Err(Error::Rejected(reason)) if reason == "registry full")
    }

    // ----- publish and claim ------------------------------------------------

    #[test]
    fn publish_allocates_consecutive_ids_from_one() {
        let mut r = registry(8);
        let t = now();
        for expected in 1..=3 {
            assert_eq!(publish(&mut r, KEY, false, t), PartyId(expected));
        }
        assert_eq!(r.service_ids(), [PartyId(1), PartyId(2), PartyId(3)]);
        assert!(r.client_ids().is_empty());
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn publish_stores_the_record_unclaimed_and_alive() {
        let mut r = registry(8);
        let t = now();
        let bind = Addr::new(Transport::Https, "10.0.0.1", 9001);
        let id = r.publish(7, addr(9000), bind.clone(), true, t).unwrap();
        assert_eq!(
            r.service(id).unwrap(),
            &ServiceEntry {
                record: ServiceRecord {
                    id,
                    key: 7,
                    service_addr: addr(9000),
                    bind_addr: bind,
                    ping: true,
                },
                claimed_by: None,
                inbox: None,
                failures: 0,
                last_seen: t,
            }
        );
    }

    #[test]
    fn claim_pairs_with_the_lowest_unclaimed_service_and_marks_it_claimed() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let _other_key = publish(&mut r, KEY + 1, false, t);

        let (c1, h1) = claim(&mut r, KEY, true, t);
        assert_eq!(c1, PartyId(4));
        assert_eq!(h1, handle_of(&r, s1));
        assert_eq!(claimed_by(&r, s1), Some(c1));
        assert_eq!(
            r.client(c1).unwrap(),
            &ClientEntry {
                record: ClientRecord {
                    id: c1,
                    key: KEY,
                    bind_addr: addr(7000),
                    service: s1,
                    ping: true,
                },
                pending_service: None,
                failures: 0,
                last_seen: t,
            }
        );

        // The second claim gets the other service of the key; the third
        // finds none, even though a service of another key is free.
        let (c2, h2) = claim(&mut r, KEY, false, t);
        assert_eq!((c2, h2), (PartyId(5), handle_of(&r, s2)));
        assert_eq!(claimed_by(&r, s2), Some(c2));
        assert!(matches!(
            r.claim(KEY, addr(7000), false, t),
            Err(Error::NoService(KEY))
        ));
        assert_eq!(r.len(), 5, "a failed claim registers nothing");
    }

    #[test]
    fn claim_for_an_unknown_key_is_no_service_and_allocates_no_id() {
        let mut r = registry(8);
        let t = now();
        assert!(matches!(
            r.claim(99, addr(7000), false, t),
            Err(Error::NoService(99))
        ));
        assert!(r.is_empty());
        assert_eq!(publish(&mut r, KEY, false, t), PartyId(1));
    }

    // ----- remove and reclaim -----------------------------------------------

    #[test]
    fn removing_a_service_orphans_its_client() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);

        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![c]
            }
        );
        assert_eq!(r.get(s1), None);
        assert_eq!(r.service_ids(), [s2]);
        assert_eq!(r.len(), 2);
        // The orphan stays registered and keeps naming the dead service
        // until it is reclaimed.
        assert_eq!(r.client(c).unwrap().record.service, s1);

        // An unclaimed service orphans nobody.
        assert_eq!(
            r.remove(s2),
            Removed::Service {
                orphaned_clients: vec![]
            }
        );
        assert_eq!(r.client_ids(), [c]);
    }

    #[test]
    fn reclaim_moves_an_orphan_and_its_next_heartbeat_carries_the_new_service_once() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![c]
            }
        );

        let h2 = handle_of(&r, s2);
        assert_eq!(r.reclaim(c), Some(h2.clone()));
        let client = r.client(c).unwrap();
        assert_eq!(client.record.service, s2);
        assert_eq!(client.pending_service, Some(h2.clone()));
        assert_eq!(claimed_by(&r, s2), Some(c));

        assert_eq!(r.heartbeat_for(c), Some(heartbeat(None, Some(h2))));
        assert_eq!(
            r.heartbeat_for(c),
            Some(heartbeat(None, None)),
            "delivered once"
        );
        assert_eq!(r.client(c).unwrap().pending_service, None);
    }

    #[test]
    fn reclaim_without_a_candidate_is_none_and_changes_nothing() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let _other_key = publish(&mut r, KEY + 1, false, t);
        let (c1, _) = claim(&mut r, KEY, false, t);
        let (c2, _) = claim(&mut r, KEY, false, t);
        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![c1]
            }
        );

        // s2 is held by c2 and the third service has another key.
        assert_eq!(r.reclaim(c1), None);
        let orphan = r.client(c1).unwrap();
        assert_eq!(orphan.record.service, s1, "still names the dead service");
        assert_eq!(orphan.pending_service, None);
        assert_eq!(claimed_by(&r, s2), Some(c2));
        assert_eq!(r.heartbeat_for(c1), Some(heartbeat(None, None)));

        assert_eq!(r.reclaim(PartyId(99)), None, "unknown id");
        assert_eq!(r.reclaim(s2), None, "a service id is not a client");
    }

    #[test]
    fn a_pending_service_that_dies_before_delivery_is_never_announced() {
        // Double failover: s1 dies and c is moved to s2; s2 dies before a
        // heartbeat carried it; c is moved to s3 and only hears about s3.
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let s3 = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![c]
            }
        );
        assert_eq!(r.reclaim(c).map(|h| h.id), Some(s2));

        assert_eq!(
            r.remove(s2),
            Removed::Service {
                orphaned_clients: vec![c]
            }
        );
        assert_eq!(r.client(c).unwrap().pending_service, None);
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(None, None)));

        let h3 = handle_of(&r, s3);
        assert_eq!(r.reclaim(c), Some(h3.clone()));
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(None, Some(h3))));
    }

    #[test]
    fn reclaim_of_a_client_whose_service_is_alive_is_a_no_op() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let (c, h1) = claim(&mut r, KEY, false, t);

        assert_eq!(r.reclaim(c), Some(h1));
        assert_eq!(claimed_by(&r, s1), Some(c));
        assert_eq!(claimed_by(&r, s2), None);
        assert_eq!(r.client(c).unwrap().pending_service, None);
    }

    #[test]
    fn removing_a_client_frees_its_service_for_the_next_claim() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let (c1, _) = claim(&mut r, KEY, false, t);
        let (c2, _) = claim(&mut r, KEY, false, t);

        assert_eq!(
            r.remove(c1),
            Removed::Client {
                freed_service: Some(s1)
            }
        );
        assert_eq!(r.get(c1), None);
        assert_eq!(claimed_by(&r, s1), None);
        assert_eq!(
            claimed_by(&r, s2),
            Some(c2),
            "the other pairing is untouched"
        );

        // s1 is the lowest-id unclaimed service again, so the next claim
        // gets it back.
        let (c3, h) = claim(&mut r, KEY, false, t);
        assert_eq!((c3, h.id), (PartyId(5), s1));
        assert_eq!(claimed_by(&r, s1), Some(c3));
    }

    #[test]
    fn removing_an_orphan_frees_nothing() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        assert_eq!(
            r.remove(s),
            Removed::Service {
                orphaned_clients: vec![c]
            }
        );
        assert_eq!(
            r.remove(c),
            Removed::Client {
                freed_service: None
            }
        );
        assert!(r.is_empty());
    }

    #[test]
    fn removing_an_unknown_or_already_removed_id_is_unknown() {
        let mut r = registry(8);
        let t = now();
        assert_eq!(r.remove(PartyId(0)), Removed::Unknown);
        assert_eq!(r.remove(PartyId(1)), Removed::Unknown);
        let s = publish(&mut r, KEY, false, t);
        assert_eq!(
            r.remove(s),
            Removed::Service {
                orphaned_clients: vec![]
            }
        );
        assert_eq!(r.remove(s), Removed::Unknown);
    }

    #[test]
    fn ids_are_never_reused_after_removals() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![]
            }
        );
        let s3 = publish(&mut r, KEY, false, t);
        assert_eq!(s3, PartyId(3));
        let (c4, h) = claim(&mut r, KEY, false, t);
        assert_eq!((c4, h.id), (PartyId(4), s2));

        assert_eq!(
            r.remove(s2),
            Removed::Service {
                orphaned_clients: vec![c4]
            }
        );
        assert_eq!(
            r.remove(c4),
            Removed::Client {
                freed_service: None
            }
        );
        assert_eq!(
            r.remove(s3),
            Removed::Service {
                orphaned_clients: vec![]
            }
        );
        assert!(r.is_empty());

        assert_eq!(publish(&mut r, KEY, false, t), PartyId(5));
        assert_eq!(claim(&mut r, KEY, false, t).0, PartyId(6));
        for old in [s1, s2, s3, c4] {
            assert_eq!(r.get(old), None, "{old} stays unknown");
        }
    }

    // ----- delivery ---------------------------------------------------------

    #[test]
    fn deliver_then_heartbeat_carries_the_inbox_once() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        assert_eq!(
            r.heartbeat_for(s),
            Some(heartbeat(None, None)),
            "nothing pending"
        );

        r.deliver(s, "hello".into()).unwrap();
        assert_eq!(r.service(s).unwrap().inbox.as_deref(), Some("hello"));
        assert_eq!(r.heartbeat_for(s), Some(heartbeat(Some("hello"), None)));
        assert_eq!(
            r.heartbeat_for(s),
            Some(heartbeat(None, None)),
            "delivered once"
        );
        assert_eq!(r.service(s).unwrap().inbox, None);
    }

    #[test]
    fn inbox_last_write_wins() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        r.deliver(s, "first".into()).unwrap();
        r.deliver(s, "second".into()).unwrap();
        assert_eq!(r.heartbeat_for(s), Some(heartbeat(Some("second"), None)));
        assert_eq!(r.heartbeat_for(s), Some(heartbeat(None, None)));
    }

    #[test]
    fn deliver_to_a_client_or_unknown_id_is_rejected() {
        let mut r = registry(8);
        let t = now();
        let _s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        for to in [c, PartyId(99)] {
            match r.deliver(to, "x".into()) {
                Err(Error::Rejected(reason)) => {
                    assert_eq!(reason, format!("unknown service {to}"));
                }
                other => panic!("deliver to {to}: {other:?}"),
            }
        }
        assert_eq!(
            r.heartbeat_for(c),
            Some(heartbeat(None, None)),
            "nothing was stored on the client"
        );
    }

    #[test]
    fn heartbeat_for_an_unknown_id_is_none() {
        let mut r = registry(8);
        assert_eq!(r.heartbeat_for(PartyId(1)), None);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        assert_eq!(
            r.remove(s),
            Removed::Service {
                orphaned_clients: vec![]
            }
        );
        assert_eq!(r.heartbeat_for(s), None);
    }

    // ----- liveness ---------------------------------------------------------

    #[test]
    fn mark_alive_resets_failures_and_record_failure_counts() {
        let mut r = registry(8);
        let t0 = now();
        let s = publish(&mut r, KEY, false, t0);
        let (c, _) = claim(&mut r, KEY, false, t0);
        for id in [s, c] {
            assert_eq!(r.record_failure(id), Some(1));
            assert_eq!(r.record_failure(id), Some(2));
            assert_eq!(r.record_failure(id), Some(3));
            let p = r.get(id).unwrap();
            assert_eq!((p.failures(), p.last_seen()), (3, t0), "{id}");

            let t1 = t0 + sec(5);
            assert!(r.mark_alive(id, t1));
            let p = r.get(id).unwrap();
            assert_eq!((p.failures(), p.last_seen()), (0, t1), "{id}");
            assert_eq!(r.record_failure(id), Some(1), "counts from zero again");
        }
        assert_eq!(r.record_failure(PartyId(99)), None);
        assert!(!r.mark_alive(PartyId(99), t0));
    }

    #[test]
    fn stale_lists_only_ping_parties_silent_for_longer_than_the_threshold() {
        let mut r = registry(8);
        let t0 = now();
        let s_ping = publish(&mut r, KEY, true, t0);
        let s_dial = publish(&mut r, KEY, false, t0);
        let (c_ping, h) = claim(&mut r, KEY, true, t0);
        assert_eq!(h.id, s_ping);
        let (_c_dial, h) = claim(&mut r, KEY, false, t0);
        assert_eq!(h.id, s_dial);

        assert!(r.stale(t0, sec(0)).is_empty(), "nobody is stale at t0");
        // Everybody was seen at t0; the ping service then checks in at t0+5.
        assert!(r.mark_alive(s_ping, t0 + sec(5)));

        // (seconds after t0, threshold in seconds, expected ids)
        let cases: [(u64, u64, &[PartyId]); 6] = [
            (10, 6, &[c_ping]),
            (10, 4, &[s_ping, c_ping]),
            (10, 10, &[]),
            (6, 6, &[]),
            (7, 6, &[c_ping]),
            (5, 0, &[c_ping]),
        ];
        for (after, threshold, expected) in cases {
            assert_eq!(
                r.stale(t0 + sec(after), sec(threshold)),
                expected,
                "{after}s after t0 with a {threshold}s threshold"
            );
        }

        // A `now` before a party's `last_seen` (the ping service was marked
        // alive at t0+5) counts as no silence for that party.
        assert_eq!(r.stale(t0 + sec(1), sec(0)), [c_ping]);
    }

    // ----- limits -----------------------------------------------------------

    #[test]
    fn the_registration_limit_counts_services_and_clients_together() {
        let mut r = registry(3);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let _s2 = publish(&mut r, KEY, false, t);
        let (c1, _) = claim(&mut r, KEY, false, t);
        assert_eq!(r.len(), 3);

        assert!(is_full(r.publish(KEY, addr(9000), addr(9001), false, t)));
        assert!(
            is_full(r.claim(KEY, addr(7000), false, t)),
            "rejected although s2 is free"
        );
        assert_eq!(r.len(), 3, "rejected requests register nothing");

        // Removing a party of either kind makes room for either kind.
        assert_eq!(
            r.remove(c1),
            Removed::Client {
                freed_service: Some(s1)
            }
        );
        assert_eq!(publish(&mut r, KEY, false, t), PartyId(4));
        assert!(is_full(r.claim(KEY, addr(7000), false, t)));
        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![]
            }
        );
        assert_eq!(claim(&mut r, KEY, false, t).0, PartyId(5));
        assert!(is_full(r.publish(KEY, addr(9000), addr(9001), false, t)));
    }

    #[test]
    fn a_full_registry_rejects_a_claim_before_looking_for_a_service() {
        let mut r = registry(2);
        let t = now();
        let _s = publish(&mut r, KEY, false, t);
        let _c = claim(&mut r, KEY, false, t);
        // No unclaimed service either way; the answer is still "full".
        assert!(is_full(r.claim(KEY, addr(7000), false, t)));
        assert!(is_full(r.claim(KEY + 1, addr(7000), false, t)));
    }

    #[test]
    fn a_zero_limit_admits_nobody() {
        let mut r = registry(0);
        let t = now();
        assert!(is_full(r.publish(KEY, addr(9000), addr(9001), false, t)));
        assert!(is_full(r.claim(KEY, addr(7000), false, t)));
        assert!(r.is_empty());
    }

    // ----- queries ----------------------------------------------------------

    #[test]
    fn get_bind_addr_and_is_ping_see_both_kinds() {
        let mut r = registry(8);
        let t = now();
        let s_bind = Addr::new(Transport::Tls, "svc.example", 9001);
        let c_bind = Addr::new(Transport::Http, "fe80::1", 7000);
        let s = r
            .publish(KEY, addr(9000), s_bind.clone(), false, t)
            .unwrap();
        let (c, _) = r.claim(KEY, c_bind.clone(), true, t).unwrap();

        match r.get(s) {
            Some(Party::Service(entry)) => assert_eq!(entry, r.service(s).unwrap()),
            other => panic!("{other:?}"),
        }
        match r.get(c) {
            Some(Party::Client(entry)) => assert_eq!(entry, r.client(c).unwrap()),
            other => panic!("{other:?}"),
        }
        assert_eq!(r.get(PartyId(99)), None);
        assert_eq!(r.service(c), None);
        assert_eq!(r.client(s), None);

        let p = r.get(s).unwrap();
        assert_eq!(
            (p.id(), p.key(), p.bind_addr(), p.is_ping()),
            (s, KEY, &s_bind, false)
        );
        assert_eq!((p.failures(), p.last_seen()), (0, t));
        let p = r.get(c).unwrap();
        assert_eq!(
            (p.id(), p.key(), p.bind_addr(), p.is_ping()),
            (c, KEY, &c_bind, true)
        );

        assert_eq!(r.bind_addr(s), Some(&s_bind));
        assert_eq!(r.bind_addr(c), Some(&c_bind));
        assert_eq!(r.bind_addr(PartyId(99)), None);
        assert_eq!(r.is_ping(s), Some(false));
        assert_eq!(r.is_ping(c), Some(true));
        assert_eq!(r.is_ping(PartyId(99)), None);
    }

    #[test]
    fn id_lists_and_iterators_are_in_ascending_id_order() {
        let mut r = registry(8);
        let t = now();
        // Interleave the kinds so neither has contiguous ids.
        let s1 = publish(&mut r, KEY, false, t);
        let (c2, _) = claim(&mut r, KEY, false, t);
        let s3 = publish(&mut r, KEY, false, t);
        let (c4, _) = claim(&mut r, KEY, false, t);
        let s5 = publish(&mut r, KEY + 1, false, t);

        assert_eq!(r.service_ids(), [s1, s3, s5]);
        assert_eq!(r.client_ids(), [c2, c4]);
        let services: Vec<PartyId> = r.services().map(|e| e.record.id).collect();
        assert_eq!(services, [s1, s3, s5]);
        let clients: Vec<PartyId> = r.clients().map(|e| e.record.id).collect();
        assert_eq!(clients, [c2, c4]);
        let parties: Vec<PartyId> = r.parties().map(Party::id).collect();
        assert_eq!(parties, [s1, s3, s5, c2, c4]);
        assert_eq!(r.len(), 5);
        assert!(!r.is_empty());
    }

    #[test]
    fn default_registry_is_empty_with_default_limits() {
        let r = Registry::default();
        assert_eq!(r.limits(), &Limits::default());
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(r.service_ids().is_empty() && r.client_ids().is_empty());
        assert_eq!(r.parties().count(), 0);
    }
}
