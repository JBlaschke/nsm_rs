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
//! - Each claim owns one [`Store`], kept in the client's entry: created
//!   empty by `claim`, kept by `reclaim` across re-pairings (so the
//!   replacement service reads every earlier write) and dropped when the
//!   client is removed (decision D17).
//!   [`store`](Registry::store) finds the store the way `deliver` finds the
//!   peer: a client uses its own, including between losing its service and
//!   being re-paired; a service uses the store of the client holding it, and
//!   a service nobody holds reads an empty store and may not write (D17).
//!   Every write takes the next number from one version counter for the
//!   broker's whole life, starting at 1 (decision D18).
//! - Store keys starting with `nsm_` are the broker's (decision D26) and
//!   never reach a [`Store`]: they are the entries of
//!   [`mesh_data`](Registry::mesh_data), where the parties of the asking
//!   party's claim listen, projected from the records at version 0
//!   (`nsm_mesh_data` as one JSON value, and one entry per field that is
//!   set). `store` answers a get of one from there, adds them all to a
//!   list, and refuses a put or a delete of any of them.
//! - [`store_by_key`](Registry::store_by_key) is `store` for the party
//!   [`resolve_key`](Registry::resolve_key) finds under a rendezvous key
//!   (decision D27): the one client under the key, or the
//!   one service when no client is, or the named `party_id`; an ambiguous
//!   key is refused with the candidates listed.
//! - [`deliver`](Registry::deliver) parks text as the sender's peer's
//!   pending *inbox* (a later delivery replaces an earlier one): a client's
//!   text goes to its service, a service's text to the client holding it.
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
//! [`Limits::max_registrations`] bounds services and clients together, and
//! [`Limits::max_store_bytes`] bounds each store.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::time::Instant;

use super::store::Store;
use crate::config::Limits;
use crate::net::Addr;
use crate::protocol::{
    ClientRecord, Key, MeshData, Message, PartyId, RESERVED_STORE_KEY_PREFIX, RegToken,
    ServiceHandle, ServiceRecord, StoreEntry, StoreKey, StoreOp, Stored,
};
use crate::{Error, Result};

/// A published service as the broker tracks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    /// Id, key, data-plane and heartbeat endpoints, liveness mode.
    pub record: ServiceRecord,
    /// Secret issued at registration; the service proves itself with it.
    pub token: RegToken,
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
    /// Secret issued at registration; the client proves itself with it.
    pub token: RegToken,
    /// A new pairing from [`Registry::reclaim`] that the client has not been
    /// told about yet; carried by its next heartbeat.
    pub pending_service: Option<ServiceHandle>,
    /// Text delivered to this client by its service and not yet carried by
    /// a heartbeat. A later [`Registry::deliver`] replaces an earlier one.
    pub inbox: Option<String>,
    /// Consecutive failed heartbeats since the broker last heard from the
    /// client.
    pub failures: u32,
    /// When the broker last heard from the client.
    pub last_seen: Instant,
    /// The store this claim shares with its service: created empty by
    /// [`Registry::claim`], kept by [`Registry::reclaim`], dropped with the
    /// entry.
    pub store: Store,
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
    pub fn key(self) -> &'a Key {
        match self {
            Party::Service(s) => &s.record.key,
            Party::Client(c) => &c.record.key,
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

/// Source of store versions: starts at 1, strictly increasing, shared by
/// every store for the broker's whole life. Stops before `u64::MAX` rather
/// than wrapping.
#[derive(Debug)]
struct VersionCounter(u64);

impl VersionCounter {
    fn next(&mut self) -> Option<u64> {
        let version = self.0;
        self.0 = version.checked_add(1)?;
        Some(version)
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
    versions: VersionCounter,
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
            versions: VersionCounter(1),
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
        token: RegToken,
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
                token,
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
        token: RegToken,
        now: Instant,
    ) -> Result<(PartyId, ServiceHandle)> {
        self.check_capacity()?;
        let service = Self::lowest_unclaimed(&mut self.services, &key)
            .ok_or_else(|| Error::NoService(key.clone()))?;
        let id = self.ids.allocate();
        service.claimed_by = Some(id);
        let handle = service.record.handle();
        self.clients.insert(
            id,
            ClientEntry {
                token,
                record: ClientRecord {
                    id,
                    key,
                    bind_addr,
                    service: handle.id,
                    ping,
                },
                pending_service: None,
                inbox: None,
                failures: 0,
                last_seen: now,
                store: Store::default(),
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
    /// service it held. Pending inbox text of a removed party is dropped.
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
        if let Some(current) = self.services.get(&entry.record.service)
            && current.claimed_by == Some(client)
        {
            return Some(current.record.handle());
        }
        let service = Self::lowest_unclaimed(&mut self.services, &entry.record.key)?;
        service.claimed_by = Some(client);
        let handle = service.record.handle();
        entry.record.service = handle.id;
        entry.pending_service = Some(handle.clone());
        Some(handle)
    }

    // ----- delivery ---------------------------------------------------------

    /// Park `text` as the pending inbox of the peer of party `from`,
    /// replacing whatever was pending; the peer's next
    /// [`heartbeat_for`](Registry::heartbeat_for) carries it. A client's peer
    /// is its current service, a service's peer the client holding it.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] when `from` is not a registered party
    /// (`"unknown party <id>"`), when it is a service no client holds
    /// (`"service <id> is not claimed"`), or when it is a client whose
    /// service died and that has not been re-paired yet
    /// (`"client <id> has no service"`).
    pub fn deliver(&mut self, from: PartyId, text: String) -> Result<()> {
        if let Some(service) = self.services.get(&from) {
            let client = service
                .claimed_by
                .and_then(|id| self.clients.get_mut(&id))
                .ok_or_else(|| Error::Rejected(format!("service {from} is not claimed")))?;
            client.inbox = Some(text);
            return Ok(());
        }
        let Some(client) = self.clients.get(&from) else {
            return Err(Error::Rejected(format!("unknown party {from}")));
        };
        let service = self
            .services
            .get_mut(&client.record.service)
            .ok_or_else(|| Error::Rejected(format!("client {from} has no service")))?;
        service.inbox = Some(text);
        Ok(())
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
                token: service.token,
                inbox: service.inbox.take(),
                service: None,
            });
        }
        let client = self.clients.get_mut(&id)?;
        Some(Message::Heartbeat {
            token: client.token,
            inbox: client.inbox.take(),
            service: client.pending_service.take(),
        })
    }

    /// True when `token` is the registration token of party `id` (constant
    /// time; false for unknown ids, so callers cannot tell the two apart).
    pub fn verify(&self, id: PartyId, token: &RegToken) -> bool {
        let stored = self
            .services
            .get(&id)
            .map(|s| s.token)
            .or_else(|| self.clients.get(&id).map(|c| c.token));
        match stored {
            Some(stored) => stored.ct_eq(token),
            None => false,
        }
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
            if entry.inbox.is_none() {
                entry.inbox = inbox;
            }
            if entry.pending_service.is_none() {
                entry.pending_service = service;
            }
        }
    }

    /// Registrations (services plus clients) whose advertised bind host is
    /// `host`; used for the per-host admission cap.
    pub fn count_for_host(&self, host: &str) -> usize {
        self.services
            .values()
            .filter(|s| s.record.bind_addr.host == host)
            .count()
            + self
                .clients
                .values()
                .filter(|c| c.record.bind_addr.host == host)
                .count()
    }

    // ----- the shared store -------------------------------------------------

    /// Apply `op` to the store party `from` shares with its peer and say
    /// whose store it is.
    ///
    /// A client uses its own claim's store, with no check on its service, so
    /// it keeps access between losing its service and being re-paired. A
    /// service uses the store of the client holding it. A service nobody
    /// holds has no store: a get or a list answers an empty one with
    /// `client: None` and revision 0, and nothing is created. Writes take
    /// their numbers from the broker-wide version counter. A put or a delete
    /// whose `if_version` does not hold is answered with `applied: false`
    /// and the key's current entry, changing nothing and taking no number;
    /// that is an answer, not an error. Tokens are not checked here; the
    /// handler verifies them first.
    ///
    /// Store keys starting with `nsm_` are the broker's (decision D26) and
    /// never reach the [`Store`]: they are the entries of
    /// [`mesh_data`](Registry::mesh_data), at version 0. A get of one
    /// answers it (no entry when that side of the claim is not there, or
    /// when the key is one the broker does not know), a list carries them
    /// all beside the stored entries in one key order, and a put or a
    /// delete of one is refused. `client` and `revision` are the store's as
    /// for any read, so `client` is `None` at a service nobody holds, whose
    /// list is the broker's entries alone.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] when `from` is not a registered party
    /// (`"unknown party <id>"`), when a put or a delete names a reserved
    /// key (`"<key> is reserved: store keys starting with nsm_ are the
    /// broker's"`, whatever the party and the condition), when a service
    /// nobody holds tries to write (`"service <id> is not claimed"`,
    /// whatever the write's condition), when a put does not fit
    /// [`Limits::max_store_bytes`] (`"store full: ..."`), and when the
    /// version counter is exhausted (`"store versions exhausted"`). A
    /// refused operation changes nothing.
    pub fn store(&mut self, from: PartyId, op: StoreOp) -> Result<Stored> {
        // Whose store: a client's own, the holder's for a service, none for
        // a service nobody holds.
        let owner = if let Some(service) = self.services.get(&from) {
            service
                .claimed_by
                .filter(|client| self.clients.contains_key(client))
        } else if self.clients.contains_key(&from) {
            Some(from)
        } else {
            return Err(Error::Rejected(format!("unknown party {from}")));
        };
        if let Some(key) = op.key().filter(|key| key.is_reserved()) {
            return self.reserved(from, owner, key, op.is_write());
        }
        let is_list = matches!(op, StoreOp::List);
        let Some(client) = owner else {
            if op.is_write() {
                return Err(Error::Rejected(format!("service {from} is not claimed")));
            }
            // No store to read, but a list still shows the broker's entries.
            let entries = if is_list {
                self.reserved_entries(from)?
            } else {
                Vec::new()
            };
            return Ok(Stored {
                client: None,
                revision: 0,
                applied: true,
                entries,
            });
        };
        let Some(entry) = self.clients.get_mut(&client) else {
            return Err(Error::Rejected(format!("unknown party {from}")));
        };
        let versions = &mut self.versions;
        let outcome = entry
            .store
            .apply(op, self.limits.max_store_bytes, || versions.next())?;
        let mut entries = outcome.entries;
        if is_list {
            // The broker's entries beside the stored ones, in one order.
            entries.extend(self.reserved_entries(from)?);
            entries.sort_by(|a, b| a.key.cmp(&b.key));
        }
        Ok(Stored {
            client: Some(client),
            revision: outcome.revision,
            applied: outcome.applied,
            entries,
        })
    }

    /// The broker's entries for party `from`'s claim:
    /// [`MeshData::entries`] of [`mesh_data`](Registry::mesh_data), none for
    /// an unknown id.
    fn reserved_entries(&self, from: PartyId) -> Result<Vec<StoreEntry>> {
        self.mesh_data(from)
            .map(|data| data.entries())
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Answer an operation on a reserved key (one starting with `nsm_`) for
    /// party `from`, whose store is `owner`'s (none at a service nobody
    /// holds): a write is refused; a get answers the broker's entry of that
    /// name ([`MeshData::entries`]), or no entry when that side of the
    /// claim is not there or the key is unknown. `client` and `revision`
    /// are the store's, as for every read.
    fn reserved(
        &self,
        from: PartyId,
        owner: Option<PartyId>,
        key: &StoreKey,
        write: bool,
    ) -> Result<Stored> {
        if write {
            return Err(Error::Rejected(format!(
                "{key} is reserved: store keys starting with {RESERVED_STORE_KEY_PREFIX} are the broker's"
            )));
        }
        let entries = self
            .reserved_entries(from)?
            .into_iter()
            .filter(|entry| &entry.key == key)
            .collect();
        Ok(Stored {
            client: owner,
            revision: owner
                .and_then(|client| self.clients.get(&client))
                .map_or(0, |client| client.store.revision()),
            applied: true,
            entries,
        })
    }

    /// The party a store operation addressed by rendezvous key means
    /// (decision D27): with `party_id`, that party, which
    /// must be under `key`; otherwise the one client under `key` (so its
    /// claim's store), or, when no client is under it, the one service
    /// (which reads an empty store and may not write). Ids are not secrets,
    /// so the refusals name them.
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] when nothing is under the key (`"no party under
    /// key 1234"`), when the named party is not under it or does not exist
    /// (`"no party 7 under key 1234"`, one text for both), and when the
    /// key is ambiguous (`"key 1234 has 2 clients (4, 7); name one with
    /// party_id"`, or `"key 1234 has 2 unclaimed services (1, 3); name one
    /// with party_id"`).
    pub fn resolve_key(&self, key: &Key, party_id: Option<PartyId>) -> Result<PartyId> {
        if let Some(id) = party_id {
            return match self.get(id) {
                Some(party) if party.key() == key => Ok(id),
                _ => Err(Error::Rejected(format!("no party {id} under key {key}"))),
            };
        }
        let clients: Vec<PartyId> = self
            .clients
            .values()
            .filter(|c| c.record.key == *key)
            .map(|c| c.record.id)
            .collect();
        match clients.as_slice() {
            [one] => return Ok(*one),
            [] => {}
            many => return Err(ambiguous_key(key, "clients", many)),
        }
        let services: Vec<PartyId> = self
            .services
            .values()
            .filter(|s| s.record.key == *key)
            .map(|s| s.record.id)
            .collect();
        match services.as_slice() {
            [one] => Ok(*one),
            [] => Err(Error::Rejected(format!("no party under key {key}"))),
            many => Err(ambiguous_key(key, "unclaimed services", many)),
        }
    }

    /// [`store`](Registry::store) for the party
    /// [`resolve_key`](Registry::resolve_key) finds under `key`: what a
    /// `store_by_key` from an operator asks for.
    ///
    /// # Errors
    ///
    /// Those of [`resolve_key`](Registry::resolve_key), then those of
    /// [`store`](Registry::store).
    pub fn store_by_key(
        &mut self,
        key: &Key,
        party_id: Option<PartyId>,
        op: StoreOp,
    ) -> Result<Stored> {
        let from = self.resolve_key(key, party_id)?;
        self.store(from, op)
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

    /// Where the parties of `from`'s claim listen, for the reserved store
    /// entry `nsm_mesh_data` (decisions D25 and D26): for a
    /// client, itself and the service it holds (none between losing its
    /// service and being re-paired); for a service, itself and the client
    /// holding it (none while it is unclaimed). Built from the records on
    /// every call, so it is always current. `None` for an unknown id.
    pub fn mesh_data(&self, from: PartyId) -> Option<MeshData> {
        if let Some(service) = self.services.get(&from) {
            let client = service.claimed_by.and_then(|id| self.clients.get(&id));
            return Some(MeshData::new(
                service.record.key.clone(),
                Some(&service.record),
                client.map(|c| &c.record),
            ));
        }
        let client = self.clients.get(&from)?;
        let service = self
            .services
            .get(&client.record.service)
            .filter(|s| s.claimed_by == Some(from));
        Some(MeshData::new(
            client.record.key.clone(),
            service.map(|s| &s.record),
            Some(&client.record),
        ))
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
    fn lowest_unclaimed<'s>(
        services: &'s mut BTreeMap<PartyId, ServiceEntry>,
        key: &Key,
    ) -> Option<&'s mut ServiceEntry> {
        services
            .values_mut()
            .find(|s| s.record.key == *key && s.claimed_by.is_none())
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

/// The refusal of a key with more than one candidate: `key 1234 has 2
/// clients (4, 7); name one with party_id`.
fn ambiguous_key(key: &Key, what: &str, ids: &[PartyId]) -> Error {
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    Error::Rejected(format!(
        "key {key} has {} {what} ({}); name one with party_id",
        ids.len(),
        ids.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::store::entry_cost;
    use crate::net::Transport;
    use crate::protocol::MESH_DATA_KEY;
    use crate::protocol::message::store_key as skey;
    use crate::testing::Rng;

    const KEY: u64 = 42;

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

    fn tok() -> RegToken {
        RegToken::from_bytes([7; 16])
    }

    fn publish(r: &mut Registry, key: impl Into<Key>, ping: bool, t: Instant) -> PartyId {
        r.publish(key.into(), addr(9000), addr(9001), ping, tok(), t)
            .unwrap()
    }

    fn claim(
        r: &mut Registry,
        key: impl Into<Key>,
        ping: bool,
        t: Instant,
    ) -> (PartyId, ServiceHandle) {
        r.claim(key.into(), addr(7000), ping, tok(), t).unwrap()
    }

    fn handle_of(r: &Registry, service: PartyId) -> ServiceHandle {
        r.service(service).unwrap().record.handle()
    }

    fn claimed_by(r: &Registry, service: PartyId) -> Option<PartyId> {
        r.service(service).unwrap().claimed_by
    }

    fn heartbeat(inbox: Option<&str>, service: Option<ServiceHandle>) -> Message {
        Message::Heartbeat {
            token: tok(),
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
        let id = r
            .publish(Key::from(7), addr(9000), bind.clone(), true, tok(), t)
            .unwrap();
        assert_eq!(
            r.service(id).unwrap(),
            &ServiceEntry {
                token: tok(),
                record: ServiceRecord {
                    id,
                    key: Key::from(7),
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
                token: tok(),
                record: ClientRecord {
                    id: c1,
                    key: Key::from(KEY),
                    bind_addr: addr(7000),
                    service: s1,
                    ping: true,
                },
                pending_service: None,
                inbox: None,
                failures: 0,
                last_seen: t,
                store: Store::default(),
            }
        );

        // The second claim gets the other service of the key; the third
        // finds none, even though a service of another key is free.
        let (c2, h2) = claim(&mut r, KEY, false, t);
        assert_eq!((c2, h2), (PartyId(5), handle_of(&r, s2)));
        assert_eq!(claimed_by(&r, s2), Some(c2));
        assert!(matches!(
            r.claim(Key::from(KEY), addr(7000), false, tok(), t),
            Err(Error::NoService(k)) if k == Key::from(KEY)
        ));
        assert_eq!(r.len(), 5, "a failed claim registers nothing");
    }

    #[test]
    fn claim_for_an_unknown_key_is_no_service_and_allocates_no_id() {
        let mut r = registry(8);
        let t = now();
        assert!(matches!(
            r.claim(Key::from(99), addr(7000), false, tok(), t),
            Err(Error::NoService(k)) if k == Key::from(99)
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
    fn deliver_then_heartbeat_carries_the_inbox_once_in_either_direction() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        assert_eq!(
            r.heartbeat_for(s),
            Some(heartbeat(None, None)),
            "nothing pending"
        );

        // Client to service.
        r.deliver(c, "hello".into()).unwrap();
        assert_eq!(r.service(s).unwrap().inbox.as_deref(), Some("hello"));
        assert_eq!(r.client(c).unwrap().inbox, None, "parked on the peer only");
        assert_eq!(r.heartbeat_for(s), Some(heartbeat(Some("hello"), None)));
        assert_eq!(
            r.heartbeat_for(s),
            Some(heartbeat(None, None)),
            "delivered once"
        );
        assert_eq!(r.service(s).unwrap().inbox, None);

        // Service to client.
        r.deliver(s, "ready".into()).unwrap();
        assert_eq!(r.client(c).unwrap().inbox.as_deref(), Some("ready"));
        assert_eq!(r.service(s).unwrap().inbox, None, "parked on the peer only");
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(Some("ready"), None)));
        assert_eq!(
            r.heartbeat_for(c),
            Some(heartbeat(None, None)),
            "delivered once"
        );
    }

    #[test]
    fn inbox_last_write_wins_for_either_party() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        r.deliver(c, "first".into()).unwrap();
        r.deliver(c, "second".into()).unwrap();
        assert_eq!(r.heartbeat_for(s), Some(heartbeat(Some("second"), None)));
        assert_eq!(r.heartbeat_for(s), Some(heartbeat(None, None)));
        r.deliver(s, "one".into()).unwrap();
        r.deliver(s, "two".into()).unwrap();
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(Some("two"), None)));
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(None, None)));
    }

    #[test]
    fn deliver_needs_a_registered_sender_with_a_peer() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let rejected =
            |r: &mut Registry, from: PartyId, expected: &str| match r.deliver(from, "x".into()) {
                Err(Error::Rejected(reason)) => assert_eq!(reason, expected),
                other => panic!("deliver from {from}: {other:?}"),
            };
        // Unknown sender; a service nobody holds.
        rejected(&mut r, PartyId(99), "unknown party 99");
        rejected(&mut r, s, &format!("service {s} is not claimed"));
        assert_eq!(r.service(s).unwrap().inbox, None);

        // A client whose service died, before it is re-paired.
        let (c, _) = claim(&mut r, KEY, false, t);
        let _ = r.remove(s);
        rejected(&mut r, c, &format!("client {c} has no service"));
        assert_eq!(
            r.heartbeat_for(c),
            Some(heartbeat(None, None)),
            "nothing was stored anywhere"
        );
    }

    #[test]
    fn a_clients_pending_text_survives_its_re_pairing() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = publish(&mut r, KEY, false, t);
        let (c, h1) = claim(&mut r, KEY, false, t);
        assert_eq!(h1.id, s1);
        r.deliver(s1, "last words".into()).unwrap();
        let _ = r.remove(s1);
        let h2 = r.reclaim(c).unwrap();
        assert_eq!(h2.id, s2);
        // The text is the client's; the next heartbeat carries it together
        // with the new pairing.
        assert_eq!(
            r.heartbeat_for(c),
            Some(heartbeat(Some("last words"), Some(h2)))
        );
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(None, None)));
    }

    #[test]
    fn restore_puts_back_a_clients_text_unless_something_newer_arrived() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        r.deliver(s, "ready".into()).unwrap();
        let taken = r.heartbeat_for(c);
        assert_eq!(taken, Some(heartbeat(Some("ready"), None)));
        // The heartbeat failed: put the text back, it rides the next one.
        r.restore(c, Some("ready".into()), None);
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(Some("ready"), None)));
        // Newer text arrived before the restore: the newer one wins.
        r.deliver(s, "newer".into()).unwrap();
        r.restore(c, Some("ready".into()), None);
        assert_eq!(r.heartbeat_for(c), Some(heartbeat(Some("newer"), None)));
        // Removing the client drops what is pending for it.
        r.deliver(s, "gone".into()).unwrap();
        let _ = r.remove(c);
        assert_eq!(r.heartbeat_for(c), None);
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

        assert!(is_full(r.publish(
            Key::from(KEY),
            addr(9000),
            addr(9001),
            false,
            tok(),
            t
        )));
        assert!(
            is_full(r.claim(Key::from(KEY), addr(7000), false, tok(), t)),
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
        assert!(is_full(r.claim(
            Key::from(KEY),
            addr(7000),
            false,
            tok(),
            t
        )));
        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![]
            }
        );
        assert_eq!(claim(&mut r, KEY, false, t).0, PartyId(5));
        assert!(is_full(r.publish(
            Key::from(KEY),
            addr(9000),
            addr(9001),
            false,
            tok(),
            t
        )));
    }

    #[test]
    fn a_full_registry_rejects_a_claim_before_looking_for_a_service() {
        let mut r = registry(2);
        let t = now();
        let _s = publish(&mut r, KEY, false, t);
        let _c = claim(&mut r, KEY, false, t);
        // No unclaimed service either way; the answer is still "full".
        assert!(is_full(r.claim(
            Key::from(KEY),
            addr(7000),
            false,
            tok(),
            t
        )));
        assert!(is_full(r.claim(
            Key::from(KEY + 1),
            addr(7000),
            false,
            tok(),
            t
        )));
    }

    #[test]
    fn a_zero_limit_admits_nobody() {
        let mut r = registry(0);
        let t = now();
        assert!(is_full(r.publish(
            Key::from(KEY),
            addr(9000),
            addr(9001),
            false,
            tok(),
            t
        )));
        assert!(is_full(r.claim(
            Key::from(KEY),
            addr(7000),
            false,
            tok(),
            t
        )));
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
            .publish(Key::from(KEY), addr(9000), s_bind.clone(), false, tok(), t)
            .unwrap();
        let (c, _) = r
            .claim(Key::from(KEY), c_bind.clone(), true, tok(), t)
            .unwrap();

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
            (s, &Key::from(KEY), &s_bind, false)
        );
        assert_eq!((p.failures(), p.last_seen()), (0, t));
        let p = r.get(c).unwrap();
        assert_eq!(
            (p.id(), p.key(), p.bind_addr(), p.is_ping()),
            (c, &Key::from(KEY), &c_bind, true)
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

    // ----- the shared store -------------------------------------------------

    fn get(k: &str) -> StoreOp {
        StoreOp::Get { key: skey(k) }
    }

    fn put(k: &str, value: &str) -> StoreOp {
        StoreOp::Put {
            key: skey(k),
            value: value.into(),
            if_version: None,
        }
    }

    fn delete(k: &str) -> StoreOp {
        StoreOp::Delete {
            key: skey(k),
            if_version: None,
        }
    }

    fn entry(k: &str, value: &str, version: u64) -> StoreEntry {
        StoreEntry {
            key: skey(k),
            value: value.into(),
            version,
        }
    }

    fn stored(client: PartyId, revision: u64, entries: Vec<StoreEntry>) -> Stored {
        Stored {
            client: Some(client),
            revision,
            applied: true,
            entries,
        }
    }

    fn empty_unclaimed() -> Stored {
        Stored {
            client: None,
            revision: 0,
            applied: true,
            entries: vec![],
        }
    }

    fn store_refusal(r: &mut Registry, from: PartyId, op: StoreOp) -> String {
        match r.store(from, op) {
            Err(Error::Rejected(reason)) => reason,
            other => panic!("store from {from}: expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn claim_creates_an_empty_store() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        let store = &r.client(c).unwrap().store;
        assert!(store.is_empty());
        assert_eq!((store.revision(), store.bytes()), (0, 0));
        assert_eq!(
            stored_only(r.store(c, StoreOp::List).unwrap()),
            stored(c, 0, vec![])
        );
        assert_eq!(
            stored_only(r.store(s, StoreOp::List).unwrap()),
            stored(c, 0, vec![])
        );
    }

    #[test]
    fn a_client_and_its_service_share_one_store_named_after_the_client() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);

        assert_eq!(
            r.store(c, put("step", "5")).unwrap(),
            stored(c, 1, vec![entry("step", "5", 1)])
        );
        assert_eq!(
            r.store(s, get("step")).unwrap(),
            stored(c, 1, vec![entry("step", "5", 1)]),
            "the service reads the client's write, and the reply names the client"
        );
        assert_eq!(
            r.store(s, put("ready", "yes")).unwrap(),
            stored(c, 2, vec![entry("ready", "yes", 2)])
        );
        let both = stored(c, 2, vec![entry("ready", "yes", 2), entry("step", "5", 1)]);
        assert_eq!(stored_only(r.store(c, StoreOp::List).unwrap()), both);
        assert_eq!(stored_only(r.store(s, StoreOp::List).unwrap()), both);

        assert_eq!(
            r.store(s, delete("step")).unwrap(),
            stored(c, 3, vec![entry("step", "5", 1)])
        );
        assert_eq!(r.store(c, get("step")).unwrap(), stored(c, 3, vec![]));
        assert_eq!(
            r.store(c, delete("step")).unwrap(),
            stored(c, 3, vec![]),
            "deleting an absent key succeeds and takes no number"
        );
        // Neither the service's entry nor anything else holds a copy.
        assert_eq!(r.client(c).unwrap().store.len(), 1);
    }

    #[test]
    fn an_unclaimed_service_reads_an_empty_store_and_cannot_write() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        assert_eq!(
            stored_only(r.store(s, StoreOp::List).unwrap()),
            empty_unclaimed()
        );
        assert_eq!(r.store(s, get("step")).unwrap(), empty_unclaimed());
        let not_claimed = format!("service {s} is not claimed");
        assert_eq!(store_refusal(&mut r, s, put("step", "5")), not_claimed);
        assert_eq!(store_refusal(&mut r, s, delete("step")), not_claimed);

        // Nothing was seeded for the future claim, and no number was taken.
        let (c, _) = claim(&mut r, KEY, false, t);
        assert_eq!(
            stored_only(r.store(s, StoreOp::List).unwrap()),
            stored(c, 0, vec![])
        );
        assert_eq!(
            r.store(c, put("step", "5")).unwrap(),
            stored(c, 1, vec![entry("step", "5", 1)])
        );
    }

    #[test]
    fn an_unclaimed_service_cannot_write_whatever_the_condition() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let not_claimed = format!("service {s} is not claimed");
        for if_version in [None, Some(0), Some(1), Some(u64::MAX)] {
            let writes = [
                StoreOp::Put {
                    key: skey("step"),
                    value: "5".into(),
                    if_version,
                },
                StoreOp::Delete {
                    key: skey("step"),
                    if_version,
                },
            ];
            for op in writes {
                assert_eq!(
                    store_refusal(&mut r, s, op),
                    not_claimed,
                    "{if_version:?}: a refusal, not a missed condition"
                );
            }
        }
        // Nothing was written and no number was taken.
        let (c, _) = claim(&mut r, KEY, false, t);
        assert_eq!(
            r.store(c, put("step", "5")).unwrap(),
            stored(c, 1, vec![entry("step", "5", 1)])
        );
    }

    #[test]
    fn a_missed_condition_answers_with_the_current_entry_and_takes_no_number() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        let cas = |value: &str, if_version| StoreOp::Put {
            key: skey("counter"),
            value: value.into(),
            if_version: Some(if_version),
        };
        // Create-only: the first wins, the second learns the winner's entry.
        assert_eq!(
            r.store(c, cas("1", 0)).unwrap(),
            stored(c, 1, vec![entry("counter", "1", 1)])
        );
        let lost = r.store(s, cas("1", 0)).unwrap();
        assert_eq!(
            lost,
            Stored {
                client: Some(c),
                revision: 1,
                applied: false,
                entries: vec![entry("counter", "1", 1)],
            }
        );
        // The loser retries against the version it was told about.
        assert_eq!(
            r.store(s, cas("2", 1)).unwrap(),
            stored(c, 2, vec![entry("counter", "2", 2)]),
            "the missed write took no number"
        );
        // A stale delete misses; a current one removes the entry.
        let del = |if_version| StoreOp::Delete {
            key: skey("counter"),
            if_version: Some(if_version),
        };
        assert!(!r.store(c, del(1)).unwrap().applied);
        assert_eq!(
            r.store(c, del(2)).unwrap(),
            stored(c, 3, vec![entry("counter", "2", 2)])
        );
        assert_eq!(
            r.store(c, del(3)).unwrap(),
            Stored {
                client: Some(c),
                revision: 3,
                applied: false,
                entries: vec![],
            },
            "a delete of an absent key at a version misses"
        );
        assert_eq!(r.store(c, del(0)).unwrap(), stored(c, 3, vec![]));
    }

    #[test]
    fn unknown_and_removed_ids_are_refused() {
        let mut r = registry(8);
        let t = now();
        for op in [get("k"), put("k", "v"), delete("k"), StoreOp::List] {
            assert_eq!(store_refusal(&mut r, PartyId(99), op), "unknown party 99");
        }
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        r.store(c, put("k", "v")).unwrap();
        let _ = r.remove(c);
        let _ = r.remove(s);
        for id in [s, c] {
            for op in [get("k"), put("k", "v"), delete("k"), StoreOp::List] {
                assert_eq!(store_refusal(&mut r, id, op), format!("unknown party {id}"));
            }
        }
    }

    #[test]
    fn the_orphan_keeps_its_store_until_it_is_re_paired() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        r.store(s1, put("from-a", "1")).unwrap();
        assert_eq!(
            r.remove(s1),
            Removed::Service {
                orphaned_clients: vec![c]
            }
        );
        // No service at all: unlike deliver, the client keeps full access.
        assert_eq!(
            r.store(c, put("orphan", "2")).unwrap(),
            stored(c, 2, vec![entry("orphan", "2", 2)])
        );
        assert_eq!(
            stored_only(r.store(c, StoreOp::List).unwrap()),
            stored(c, 2, vec![entry("from-a", "1", 1), entry("orphan", "2", 2)])
        );
        assert_eq!(
            store_refusal(&mut r, s1, get("from-a")),
            format!("unknown party {s1}")
        );
    }

    #[test]
    fn the_store_survives_a_re_pairing_with_versions_continuing() {
        let mut r = registry(8);
        let t = now();
        let a = publish(&mut r, KEY, false, t);
        let b = publish(&mut r, KEY, false, t);
        let other_service = publish(&mut r, KEY + 1, false, t);
        let (c, h) = claim(&mut r, KEY, false, t);
        assert_eq!(h.id, a);
        let (other, _) = claim(&mut r, KEY + 1, false, t);

        r.store(c, put("from-client", "c")).unwrap(); // 1
        r.store(a, put("from-a", "a")).unwrap(); // 2
        // Another claim's writes take numbers from the same counter.
        assert_eq!(
            r.store(other_service, put("elsewhere", "x")).unwrap(),
            stored(other, 3, vec![entry("elsewhere", "x", 3)])
        );
        // Until the reclaim, b is an unclaimed service like any other.
        let _ = r.remove(a);
        assert_eq!(
            stored_only(r.store(b, StoreOp::List).unwrap()),
            empty_unclaimed()
        );
        assert_eq!(
            store_refusal(&mut r, b, put("early", "e")),
            format!("service {b} is not claimed")
        );

        assert_eq!(r.reclaim(c).map(|h| h.id), Some(b));
        assert_eq!(
            stored_only(r.store(b, StoreOp::List).unwrap()),
            stored(
                c,
                2,
                vec![entry("from-a", "a", 2), entry("from-client", "c", 1)]
            ),
            "the replacement reads the client's and the dead service's writes"
        );
        assert_eq!(
            r.store(b, put("from-b", "b")).unwrap(),
            stored(c, 4, vec![entry("from-b", "b", 4)]),
            "versions continue from the broker-wide counter"
        );
        assert_eq!(r.store(c, get("from-b")).unwrap().revision, 4);
        assert_eq!(
            stored_only(r.store(other, StoreOp::List).unwrap()),
            stored(other, 3, vec![entry("elsewhere", "x", 3)]),
            "the other claim's store is untouched"
        );
    }

    #[test]
    fn removing_the_client_drops_the_store_and_the_next_claim_starts_empty() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c1, _) = claim(&mut r, KEY, false, t);
        r.store(c1, put("secret", "for c1")).unwrap();
        r.store(s, put("step", "5")).unwrap();
        assert_eq!(
            r.remove(c1),
            Removed::Client {
                freed_service: Some(s)
            }
        );
        assert_eq!(
            store_refusal(&mut r, c1, StoreOp::List),
            format!("unknown party {c1}")
        );
        assert_eq!(
            stored_only(r.store(s, StoreOp::List).unwrap()),
            empty_unclaimed()
        );
        assert_eq!(
            store_refusal(&mut r, s, put("step", "6")),
            format!("service {s} is not claimed")
        );

        let (c2, h) = claim(&mut r, KEY, false, t);
        assert_eq!(h.id, s);
        assert_ne!(c2, c1);
        assert_eq!(
            stored_only(r.store(s, StoreOp::List).unwrap()),
            stored(c2, 0, vec![])
        );
        assert_eq!(
            r.store(c2, put("step", "1")).unwrap(),
            stored(c2, 3, vec![entry("step", "1", 3)]),
            "a new store, but numbers never repeat"
        );
    }

    #[test]
    fn a_store_is_bounded_by_the_registry_limit() {
        let mut r = Registry::new(Limits {
            max_store_bytes: 256,
            ..Limits::default()
        });
        let t = now();
        let _s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        let fits = "x".repeat(256 - entry_cost(&skey("k"), ""));
        r.store(c, put("k", &fits)).unwrap();
        assert_eq!(r.client(c).unwrap().store.bytes(), 256);
        let reason = store_refusal(&mut r, c, put("j", ""));
        assert!(reason.starts_with("store full:"), "{reason}");
        assert!(!reason.contains('x') && !reason.contains('j'), "{reason}");
        assert_eq!(stored_only(r.store(c, StoreOp::List).unwrap()).revision, 1);
    }

    #[test]
    fn an_exhausted_version_counter_refuses_writes_and_changes_nothing() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let (c, _) = claim(&mut r, KEY, false, t);
        r.versions = VersionCounter(u64::MAX - 1);
        assert_eq!(
            r.store(c, put("k", "v")).unwrap(),
            stored(c, u64::MAX - 1, vec![entry("k", "v", u64::MAX - 1)])
        );
        for (from, op) in [(c, put("k", "w")), (s, put("j", "w")), (c, delete("k"))] {
            assert_eq!(store_refusal(&mut r, from, op), "store versions exhausted");
        }
        assert_eq!(
            stored_only(r.store(s, StoreOp::List).unwrap()),
            stored(c, u64::MAX - 1, vec![entry("k", "v", u64::MAX - 1)])
        );
    }

    #[test]
    fn mesh_data_names_both_sides_of_a_claim_from_either_party() {
        let mut r = registry(8);
        let t = now();
        let s_bind = Addr::new(Transport::Https, "10.0.0.1", 9001);
        let s = r
            .publish(Key::from(KEY), addr(9000), s_bind.clone(), false, tok(), t)
            .unwrap();
        // Unclaimed: the service alone, no client.
        let alone = r.mesh_data(s).unwrap();
        assert_eq!(
            alone,
            MeshData::new(Key::from(KEY), Some(&r.service(s).unwrap().record), None)
        );
        assert_eq!(alone.nsm_mesh_service, Some(s_bind));
        assert_eq!(alone.nsm_service_port, Some(9000));
        assert_eq!(alone.nsm_service_address.as_deref(), Some("10.0.0.1"));
        assert_eq!((alone.nsm_service_id, alone.nsm_client_id), (Some(s), None));
        assert_eq!(alone.nsm_mesh_client, None);

        let c_bind = Addr::new(Transport::Https, "10.0.0.2", 7000);
        let (c, _) = r
            .claim(Key::from(KEY), c_bind.clone(), true, tok(), t)
            .unwrap();
        let from_client = r.mesh_data(c).unwrap();
        let from_service = r.mesh_data(s).unwrap();
        assert_eq!(from_client, from_service, "one claim, one answer");
        assert_eq!(
            from_client,
            MeshData::new(
                Key::from(KEY),
                Some(&r.service(s).unwrap().record),
                Some(&r.client(c).unwrap().record)
            )
        );
        assert_eq!(from_client.nsm_mesh_client, Some(c_bind));
        assert_eq!(from_client.nsm_mesh_client_port, Some(7000));
        assert_eq!(
            (from_client.nsm_service_id, from_client.nsm_client_id),
            (Some(s), Some(c))
        );
        assert_eq!(r.mesh_data(PartyId(99)), None);
    }

    #[test]
    fn mesh_data_follows_the_claim_through_a_re_pairing() {
        let mut r = registry(8);
        let t = now();
        let s1 = publish(&mut r, KEY, false, t);
        let s2 = r
            .publish(Key::from(KEY), addr(9002), addr(9003), false, tok(), t)
            .unwrap();
        let (c, _) = claim(&mut r, KEY, false, t);
        assert_eq!(r.mesh_data(c).unwrap().nsm_service_id, Some(s1));
        let _ = r.remove(s1);
        // Orphaned: the client alone, no service.
        let orphan = r.mesh_data(c).unwrap();
        assert_eq!(
            orphan,
            MeshData::new(Key::from(KEY), None, Some(&r.client(c).unwrap().record))
        );
        assert_eq!(orphan.nsm_mesh_client, Some(addr(7000)));
        // The spare is still unclaimed and says so.
        assert_eq!(r.mesh_data(s2).unwrap().nsm_client_id, None);
        assert_eq!(r.reclaim(c).map(|h| h.id), Some(s2));
        let repaired = r.mesh_data(c).unwrap();
        assert_eq!(
            (repaired.nsm_service_id, repaired.nsm_service_port),
            (Some(s2), Some(9002))
        );
        assert_eq!(repaired.nsm_mesh_service, Some(addr(9003)));
        assert_eq!(repaired, r.mesh_data(s2).unwrap());
    }

    #[test]
    fn reserved_keys_are_answered_by_the_broker_and_never_stored() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        let reserved =
            |k: &str| format!("{k} is reserved: store keys starting with nsm_ are the broker's");
        // At a service nobody holds: the service's side, no client,
        // revision 0; a write is told the key is reserved, not that the
        // service is unclaimed.
        let reply = r.store(s, get(MESH_DATA_KEY)).unwrap();
        assert_eq!(
            (reply.client, reply.revision, reply.applied),
            (None, 0, true)
        );
        assert_eq!(reply.entries.len(), 1);
        assert_eq!(reply.entries[0].version, 0);
        assert_eq!(reply.mesh_data().unwrap(), r.mesh_data(s));
        assert_eq!(
            store_refusal(&mut r, s, put(MESH_DATA_KEY, "x")),
            reserved(MESH_DATA_KEY)
        );

        let (c, _) = claim(&mut r, KEY, false, t);
        r.store(c, put("step", "5")).unwrap();
        // Through either party: the claim's client and revision, one answer.
        for from in [c, s] {
            let reply = r.store(from, get(MESH_DATA_KEY)).unwrap();
            assert_eq!((reply.client, reply.revision), (Some(c), 1), "{from}");
            assert_eq!(reply.entries.len(), 1, "{from}");
            assert_eq!(reply.mesh_data().unwrap(), r.mesh_data(c), "{from}");
        }
        // Each field is an entry of its own, with the field's text; a
        // field that is null, or a reserved key the broker does not know,
        // is not set.
        assert_eq!(
            r.store(s, get("nsm_service_port")).unwrap(),
            stored(
                c,
                1,
                vec![StoreEntry {
                    key: skey("nsm_service_port"),
                    value: "9000".into(),
                    version: 0,
                }]
            )
        );
        assert_eq!(
            r.store(c, get("nsm_mesh_client")).unwrap().entries[0].value,
            addr(7000).to_string()
        );
        assert_eq!(r.store(c, get("nsm_other")).unwrap(), stored(c, 1, vec![]));
        let lonely = publish(&mut r, KEY + 1, false, t);
        assert_eq!(
            r.store(lonely, get("nsm_mesh_client")).unwrap(),
            empty_unclaimed(),
            "no client yet: not set"
        );
        assert_eq!(
            r.store(lonely, get("nsm_service_id")).unwrap().entries[0].value,
            lonely.to_string()
        );
        // Writes of a reserved key are refused for everyone, with or
        // without a condition, and take no number.
        for from in [c, s] {
            for if_version in [None, Some(0), Some(1)] {
                for op in [
                    StoreOp::Put {
                        key: skey(MESH_DATA_KEY),
                        value: "x".into(),
                        if_version,
                    },
                    StoreOp::Delete {
                        key: skey("nsm_other"),
                        if_version,
                    },
                ] {
                    let k = op.key().unwrap().to_string();
                    assert_eq!(
                        store_refusal(&mut r, from, op),
                        reserved(&k),
                        "{from} {if_version:?}"
                    );
                }
            }
        }
        // Nothing reserved is in the store, and nothing took a number.
        assert_eq!(
            stored_only(r.store(c, StoreOp::List).unwrap()),
            stored(c, 1, vec![entry("step", "5", 1)])
        );
        assert_eq!(
            r.store(c, put("next", "6")).unwrap(),
            stored(c, 2, vec![entry("next", "6", 2)])
        );
        // An unknown party is unknown first, reserved key or not.
        assert_eq!(
            store_refusal(&mut r, PartyId(99), get(MESH_DATA_KEY)),
            "unknown party 99"
        );
    }

    #[test]
    fn list_carries_the_brokers_entries_beside_the_stored_ones() {
        let mut r = registry(8);
        let t = now();
        let s = publish(&mut r, KEY, false, t);
        // Nobody holds the service: no store, but the broker's entries.
        let listed = r.store(s, StoreOp::List).unwrap();
        assert_eq!((listed.client, listed.revision), (None, 0));
        assert_eq!(listed.entries, r.mesh_data(s).unwrap().entries().unwrap());
        assert!(
            listed
                .entries
                .iter()
                .all(|e| e.key.is_reserved() && e.version == 0)
        );
        let (c, _) = claim(&mut r, KEY, false, t);
        r.store(c, put("step", "5")).unwrap();
        r.store(s, put("zz", "last")).unwrap();
        r.store(c, put("aa", "first")).unwrap();
        for from in [c, s] {
            let listed = r.store(from, StoreOp::List).unwrap();
            assert_eq!((listed.client, listed.revision), (Some(c), 3), "{from}");
            // One order over both kinds, stored and reserved interleaved.
            let keys: Vec<&str> = listed.entries.iter().map(|e| e.key.as_str()).collect();
            let mut sorted = keys.clone();
            sorted.sort_unstable();
            assert_eq!(keys, sorted, "{from}");
            assert_eq!(
                (keys.first(), keys.last()),
                (Some(&"aa"), Some(&"zz")),
                "{from}"
            );
            let stored_keys: Vec<&str> = keys
                .iter()
                .copied()
                .filter(|k| !k.starts_with("nsm_"))
                .collect();
            assert_eq!(stored_keys, ["aa", "step", "zz"], "{from}");
            let reserved: Vec<StoreEntry> = listed
                .entries
                .iter()
                .filter(|e| e.key.is_reserved())
                .cloned()
                .collect();
            assert_eq!(
                reserved,
                r.mesh_data(c).unwrap().entries().unwrap(),
                "{from}"
            );
        }
        // The store itself holds and counts only what was written.
        assert_eq!(r.client(c).unwrap().store.len(), 3);
    }

    /// A reply without the broker's own entries: what the stored entries
    /// alone look like.
    fn stored_only(mut reply: Stored) -> Stored {
        reply.entries.retain(|e| !e.key.is_reserved());
        reply
    }

    #[test]
    fn a_key_resolves_to_its_one_claim_or_its_one_service() {
        let mut r = registry(8);
        let t = now();
        let refused =
            |r: &Registry, key: &Key, party: Option<PartyId>| match r.resolve_key(key, party) {
                Err(Error::Rejected(reason)) => reason,
                other => panic!("resolve {key} {party:?}: {other:?}"),
            };
        assert_eq!(refused(&r, &Key::from(KEY), None), "no party under key 42");
        assert_eq!(
            refused(&r, &Key::from(KEY), Some(PartyId(1))),
            "no party 1 under key 42"
        );

        // One service, nobody holding it: the service.
        let s1 = publish(&mut r, KEY, false, t);
        assert_eq!(r.resolve_key(&Key::from(KEY), None).unwrap(), s1);
        assert_eq!(
            stored_only(
                r.store_by_key(&Key::from(KEY), None, StoreOp::List)
                    .unwrap()
            ),
            empty_unclaimed()
        );
        assert_eq!(
            store_refusal_by_key(&mut r, &Key::from(KEY), None, put("step", "5")),
            format!("service {s1} is not claimed")
        );
        // Two unclaimed services: ambiguous, unless one is named.
        let s2 = publish(&mut r, KEY, false, t);
        assert_eq!(
            refused(&r, &Key::from(KEY), None),
            "key 42 has 2 unclaimed services (1, 2); name one with party_id"
        );
        assert_eq!(r.resolve_key(&Key::from(KEY), Some(s2)).unwrap(), s2);
        // One client: its claim, whichever service is spare.
        let (c1, h1) = claim(&mut r, KEY, false, t);
        assert_eq!(h1.id, s1);
        assert_eq!(r.resolve_key(&Key::from(KEY), None).unwrap(), c1);
        assert_eq!(
            r.store_by_key(&Key::from(KEY), None, put("step", "5"))
                .unwrap(),
            stored(c1, 1, vec![entry("step", "5", 1)])
        );
        // Either side of the claim names the same store; the spare its own
        // empty view.
        assert_eq!(
            r.store_by_key(&Key::from(KEY), Some(s1), get("step"))
                .unwrap(),
            stored(c1, 1, vec![entry("step", "5", 1)])
        );
        assert_eq!(
            r.store_by_key(&Key::from(KEY), Some(s2), get("step"))
                .unwrap(),
            empty_unclaimed()
        );
        // The reserved entry resolves the same way.
        assert_eq!(
            r.store_by_key(&Key::from(KEY), None, get(MESH_DATA_KEY))
                .unwrap()
                .mesh_data()
                .unwrap(),
            r.mesh_data(c1)
        );
        // Two clients: ambiguous, unless one party of a claim is named.
        let (c2, h2) = claim(&mut r, KEY, false, t);
        assert_eq!(h2.id, s2);
        assert_eq!(
            refused(&r, &Key::from(KEY), None),
            format!("key 42 has 2 clients ({c1}, {c2}); name one with party_id")
        );
        assert_eq!(r.resolve_key(&Key::from(KEY), Some(c2)).unwrap(), c2);
        assert_eq!(
            r.store_by_key(&Key::from(KEY), Some(s2), get("step"))
                .unwrap(),
            stored(c2, 0, vec![]),
            "the second claim's own, empty store, through its service"
        );
        // A party under another key, or none at all, is not under this one.
        let other = publish(&mut r, KEY + 1, false, t);
        assert_eq!(
            refused(&r, &Key::from(KEY), Some(other)),
            format!("no party {other} under key 42")
        );
        assert_eq!(
            refused(&r, &Key::from(KEY), Some(PartyId(99))),
            "no party 99 under key 42"
        );
        assert_eq!(r.resolve_key(&Key::from(KEY + 1), None).unwrap(), other);
        // An orphan is still the key's one client.
        let _ = r.remove(s1);
        let _ = r.remove(c2);
        let _ = r.remove(s2);
        assert_eq!(r.resolve_key(&Key::from(KEY), None).unwrap(), c1);
        assert_eq!(
            r.store_by_key(&Key::from(KEY), None, get("step")).unwrap(),
            stored(c1, 1, vec![entry("step", "5", 1)])
        );
    }

    fn store_refusal_by_key(
        r: &mut Registry,
        key: &Key,
        party: Option<PartyId>,
        op: StoreOp,
    ) -> String {
        match r.store_by_key(key, party, op) {
            Err(Error::Rejected(reason)) => reason,
            other => panic!("store by key {key}: expected a refusal, got {other:?}"),
        }
    }

    /// What the churn test expects of one claim's store.
    #[derive(Debug, Default)]
    struct ModelStore {
        entries: BTreeMap<StoreKey, (String, u64)>,
        revision: u64,
    }

    impl ModelStore {
        fn bytes(&self) -> usize {
            self.entries
                .iter()
                .map(|(k, (v, _))| entry_cost(k, v))
                .sum()
        }

        fn list(&self) -> Vec<StoreEntry> {
            self.entries
                .iter()
                .map(|(k, (v, version))| StoreEntry {
                    key: k.clone(),
                    value: v.clone(),
                    version: *version,
                })
                .collect()
        }

        fn get(&self, key: &StoreKey) -> Vec<StoreEntry> {
            self.entries
                .get(key)
                .map(|(v, version)| StoreEntry {
                    key: key.clone(),
                    value: v.clone(),
                    version: *version,
                })
                .into_iter()
                .collect()
        }
    }

    #[test]
    fn random_churn_keeps_every_store_with_its_claim() {
        const BUDGET: usize = 512;
        let mut rng = Rng::new(0x5702e);
        let mut r = Registry::new(Limits {
            max_registrations: 40,
            max_store_bytes: BUDGET,
            ..Limits::default()
        });
        let t = now();
        let keys: Vec<StoreKey> = (0..6).map(|_| rng.store_key()).collect();
        let mut model: BTreeMap<PartyId, ModelStore> = BTreeMap::new();
        let mut next_version = 1u64;
        let mut issued: Vec<PartyId> = Vec::new();
        let (mut writes, mut full, mut not_claimed, mut orphan_writes) = (0, 0, 0, 0);

        for i in 0..4000 {
            match rng.below(10) {
                0 => {
                    if let Ok(id) = r.publish(
                        Key::from(rng.range(1, 2) as u64),
                        addr(1),
                        addr(2),
                        false,
                        tok(),
                        t,
                    ) {
                        issued.push(id);
                    }
                }
                1 => {
                    if let Ok((id, _)) =
                        r.claim(Key::from(rng.range(1, 2) as u64), addr(3), false, tok(), t)
                    {
                        issued.push(id);
                        assert!(model.insert(id, ModelStore::default()).is_none());
                    }
                }
                2 if !issued.is_empty() => {
                    // Remove a party the way the monitor's drop_party does:
                    // re-pair each orphan or remove it; sometimes the orphan
                    // writes in between.
                    let id = *rng.pick(&issued);
                    match r.remove(id) {
                        Removed::Service { orphaned_clients } => {
                            for orphan in orphaned_clients {
                                if rng.chance(2) {
                                    let op = StoreOp::Put {
                                        key: rng.pick(&keys).clone(),
                                        value: rng.text(20),
                                        if_version: None,
                                    };
                                    let m = model.get_mut(&orphan).unwrap();
                                    match r.store(orphan, op.clone()) {
                                        Ok(reply) => {
                                            let (StoreOp::Put { key, value, .. }, [written]) =
                                                (op, reply.entries.as_slice())
                                            else {
                                                panic!("iteration {i}: {reply:?}");
                                            };
                                            assert_eq!(
                                                written.version, next_version,
                                                "iteration {i}"
                                            );
                                            m.entries.insert(key, (value, next_version));
                                            m.revision = next_version;
                                            next_version += 1;
                                            orphan_writes += 1;
                                        }
                                        Err(Error::Rejected(reason)) => {
                                            assert!(
                                                reason.starts_with("store full"),
                                                "iteration {i}: {reason}"
                                            );
                                        }
                                        Err(e) => panic!("iteration {i}: {e}"),
                                    }
                                }
                                if r.reclaim(orphan).is_none() {
                                    let _ = r.remove(orphan);
                                    model.remove(&orphan);
                                }
                            }
                        }
                        Removed::Client { .. } => {
                            model.remove(&id);
                        }
                        Removed::Unknown => {}
                    }
                }
                _ if !issued.is_empty() => {
                    // A store operation from any id ever issued, live or not.
                    let from = *rng.pick(&issued);
                    let key = rng.pick(&keys).clone();
                    let op = match rng.below(5) {
                        0 => StoreOp::Get { key },
                        1 | 2 => StoreOp::Put {
                            key,
                            value: rng.text(60),
                            if_version: None,
                        },
                        3 => StoreOp::Delete {
                            key,
                            if_version: None,
                        },
                        _ => StoreOp::List,
                    };
                    let owner = match r.get(from) {
                        None => None,
                        Some(Party::Client(c)) => Some(Some(c.record.id)),
                        Some(Party::Service(s)) => Some(s.claimed_by),
                    };
                    let result = r.store(from, op.clone());
                    match (owner, result) {
                        (None, Err(Error::Rejected(reason))) => {
                            assert_eq!(reason, format!("unknown party {from}"), "iteration {i}");
                        }
                        (Some(None), Ok(reply)) => {
                            assert!(!op.is_write(), "iteration {i}");
                            // A list at an unclaimed service carries the
                            // broker's entries alone.
                            assert_eq!(stored_only(reply), empty_unclaimed(), "iteration {i}");
                        }
                        (Some(None), Err(Error::Rejected(reason))) => {
                            assert!(op.is_write(), "iteration {i}");
                            assert_eq!(
                                reason,
                                format!("service {from} is not claimed"),
                                "iteration {i}"
                            );
                            not_claimed += 1;
                        }
                        (Some(Some(client)), result) => {
                            let m = model.get_mut(&client).unwrap();
                            let expected = match op {
                                StoreOp::Get { key } => Ok(m.get(&key)),
                                StoreOp::List => Ok(m.list()),
                                StoreOp::Put { key, value, .. } => {
                                    let old =
                                        m.entries.get(&key).map_or(0, |(v, _)| entry_cost(&key, v));
                                    if m.bytes() - old + entry_cost(&key, &value) > BUDGET {
                                        full += 1;
                                        Err(())
                                    } else {
                                        let written = StoreEntry {
                                            key: key.clone(),
                                            value: value.clone(),
                                            version: next_version,
                                        };
                                        m.entries.insert(key, (value, next_version));
                                        m.revision = next_version;
                                        next_version += 1;
                                        writes += 1;
                                        Ok(vec![written])
                                    }
                                }
                                StoreOp::Delete { key, .. } => {
                                    let removed = m.get(&key);
                                    if m.entries.remove(&key).is_some() {
                                        m.revision = next_version;
                                        next_version += 1;
                                        writes += 1;
                                    }
                                    Ok(removed)
                                }
                            };
                            match (expected, result) {
                                (Ok(entries), Ok(reply)) => assert_eq!(
                                    stored_only(reply),
                                    stored(client, m.revision, entries),
                                    "iteration {i}"
                                ),
                                (Err(()), Err(Error::Rejected(reason))) => {
                                    assert!(
                                        reason.starts_with("store full"),
                                        "iteration {i}: {reason}"
                                    );
                                }
                                (expected, result) => {
                                    panic!("iteration {i}: expected {expected:?}, got {result:?}")
                                }
                            }
                        }
                        (owner, result) => panic!("iteration {i}: {owner:?} gave {result:?}"),
                    }
                }
                _ => {}
            }

            // After every step: every live client's store matches the model
            // and is the one its service sees; unclaimed services see none.
            assert_eq!(
                r.client_ids(),
                model.keys().copied().collect::<Vec<_>>(),
                "iteration {i}"
            );
            for (&client, m) in &model {
                let entry = r.client(client).unwrap();
                let (bytes, service) = (entry.store.bytes(), entry.record.service);
                assert_eq!(bytes, m.bytes(), "iteration {i}");
                assert!(bytes <= BUDGET, "iteration {i}");
                let expected = stored(client, m.revision, m.list());
                assert_eq!(
                    stored_only(r.store(client, StoreOp::List).unwrap()),
                    expected,
                    "iteration {i}"
                );
                if r.service(service)
                    .is_some_and(|s| s.claimed_by == Some(client))
                {
                    assert_eq!(
                        stored_only(r.store(service, StoreOp::List).unwrap()),
                        expected,
                        "iteration {i}"
                    );
                }
            }
            let unclaimed: Vec<PartyId> = r
                .services()
                .filter(|s| s.claimed_by.is_none())
                .map(|s| s.record.id)
                .collect();
            for id in unclaimed {
                assert_eq!(
                    stored_only(r.store(id, StoreOp::List).unwrap()),
                    empty_unclaimed(),
                    "iteration {i}"
                );
            }
        }
        assert!(
            writes > 100 && full > 0 && not_claimed > 0 && orphan_writes > 0,
            "the churn exercised too little: {writes} writes, {full} full, {not_claimed} unclaimed, {orphan_writes} orphan writes"
        );
    }
}
