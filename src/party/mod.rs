//! Parties: the service side (`publish`) and the client side (`claim`).
//!
//! A party runs a small server on its bind address (the broker dials it for
//! two-sided heartbeats; `send` and `collect` talk to it too), registers with
//! the broker, and then keeps itself alive: either by answering the broker's
//! heartbeats and watching for their absence, or, with `--ping`, by pinging
//! the broker itself. All of that is [`session::Session`]; the request logic
//! mounted on the server is [`handler::PartyHandler`]; the state they share
//! is [`PartyState`].

pub mod handler;
pub mod session;

use std::sync::{Arc, Mutex, OnceLock};

use tokio::time::Instant;

use crate::net::Addr;
use crate::protocol::{Key, PartyId, ServiceHandle};
use crate::transport::Client;

pub use handler::PartyHandler;
pub use session::{ClaimOpts, PartyOpts, PublishOpts, Session};

/// Which side of a pairing a party is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A published service.
    Publisher,
    /// A client that claimed a service.
    Claimer,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Role::Publisher => "service",
            Role::Claimer => "client",
        })
    }
}

/// State shared between a party's server handler and its liveness loop.
///
/// All fields are behind `std::sync` primitives and every accessor takes the
/// lock only for the duration of a copy, so nothing here is ever held across
/// an `.await`.
#[derive(Debug)]
pub struct PartyState {
    role: Role,
    broker: Addr,
    key: Key,
    id: OnceLock<PartyId>,
    inbox: Mutex<Option<String>>,
    service: Mutex<Option<ServiceHandle>>,
    last_contact: Mutex<Instant>,
    client: Arc<Client>,
}

impl PartyState {
    /// Fresh state for a party that has not registered yet.
    pub fn new(role: Role, broker: Addr, key: Key, client: Arc<Client>) -> Arc<Self> {
        Arc::new(PartyState {
            role,
            broker,
            key,
            id: OnceLock::new(),
            inbox: Mutex::new(None),
            service: Mutex::new(None),
            last_contact: Mutex::new(Instant::now()),
            client,
        })
    }

    /// Service or client.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The broker this party registered with.
    pub fn broker(&self) -> &Addr {
        &self.broker
    }

    /// The rendezvous key.
    pub fn key(&self) -> Key {
        self.key
    }

    /// The transport client used to reach the broker.
    pub fn client(&self) -> &Arc<Client> {
        &self.client
    }

    /// The broker-assigned id, once registered.
    pub fn id(&self) -> Option<PartyId> {
        self.id.get().copied()
    }

    /// Record the id from the registration reply. Returns `false` if an id
    /// was already set (the first one wins).
    pub fn set_id(&self, id: PartyId) -> bool {
        self.id.set(id).is_ok()
    }

    /// Last text delivered to this service, if any.
    pub fn inbox(&self) -> Option<String> {
        lock(&self.inbox).clone()
    }

    /// The service this client is paired with, if any.
    pub fn service(&self) -> Option<ServiceHandle> {
        lock(&self.service).clone()
    }

    /// Record a new pairing (from the claim reply or a later re-pairing).
    pub fn set_service(&self, handle: ServiceHandle) {
        *lock(&self.service) = Some(handle);
    }

    /// When the broker was last heard from.
    pub fn last_contact(&self) -> Instant {
        *lock(&self.last_contact)
    }

    /// Note that the broker was just heard from.
    pub fn touch(&self) {
        *lock(&self.last_contact) = Instant::now();
    }

    /// Apply the contents of a heartbeat from the broker: store delivered
    /// text and a new pairing, and refresh the contact time.
    pub fn apply_heartbeat(&self, inbox: Option<String>, service: Option<ServiceHandle>) {
        if let Some(text) = inbox {
            *lock(&self.inbox) = Some(text);
        }
        if let Some(handle) = service {
            *lock(&self.service) = Some(handle);
        }
        self.touch();
    }
}

/// Lock a `std::sync::Mutex`, recovering from poisoning: the guarded values
/// are plain data that cannot be left half-updated.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Limits, Timing, TlsPaths};

    fn state(role: Role) -> Arc<PartyState> {
        let client = Arc::new(Client::new(
            TlsPaths::default(),
            Timing::fast(),
            Limits::default(),
        ));
        PartyState::new(role, Addr::tcp("127.0.0.1", 1), 42, client)
    }

    #[test]
    fn id_is_set_once() {
        let s = state(Role::Publisher);
        assert_eq!(s.id(), None);
        assert!(s.set_id(PartyId(5)));
        assert!(!s.set_id(PartyId(6)));
        assert_eq!(s.id(), Some(PartyId(5)));
    }

    #[test]
    fn heartbeat_contents_are_stored_and_contact_refreshed() {
        let s = state(Role::Claimer);
        let before = s.last_contact();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let handle = ServiceHandle {
            id: PartyId(1),
            key: 42,
            host: "10.0.0.1".into(),
            service_port: 9000,
        };
        s.apply_heartbeat(Some("hello".into()), Some(handle.clone()));
        assert_eq!(s.inbox().as_deref(), Some("hello"));
        assert_eq!(s.service(), Some(handle.clone()));
        assert!(s.last_contact() > before);
        // An empty heartbeat keeps what was stored.
        s.apply_heartbeat(None, None);
        assert_eq!(s.inbox().as_deref(), Some("hello"));
        assert_eq!(s.service(), Some(handle));
    }

    #[test]
    fn role_displays_as_service_or_client() {
        assert_eq!(Role::Publisher.to_string(), "service");
        assert_eq!(Role::Claimer.to_string(), "client");
        assert_eq!(
            serde_json::to_string(&Role::Claimer).unwrap(),
            "\"claimer\""
        );
    }
}
