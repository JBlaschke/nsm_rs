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

use tokio::sync::watch;
use tokio::time::Instant;

use crate::net::Addr;
use crate::protocol::{Key, PartyId, RegToken, ServiceHandle};
use crate::transport::Client;

pub use crate::protocol::Role;
pub use handler::PartyHandler;
pub use session::{ClaimOpts, PartyOpts, PublishOpts, Session};

/// State shared between a party's server handler and its liveness loop.
///
/// Every field is behind a `std::sync` primitive (the pairing behind a
/// `tokio::sync::watch` channel, which is one too), and every accessor takes
/// the lock only for the duration of a copy, so nothing here is ever held
/// across an `.await`. The pairing is a channel rather than a slot so that
/// whoever needs the current service can also be woken when it changes.
#[derive(Debug)]
pub struct PartyState {
    role: Role,
    broker: Addr,
    key: Key,
    id: OnceLock<PartyId>,
    token: OnceLock<RegToken>,
    inbox: Mutex<Option<String>>,
    service: watch::Sender<Option<ServiceHandle>>,
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
            token: OnceLock::new(),
            inbox: Mutex::new(None),
            service: watch::Sender::new(None),
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

    /// The registration token, once registered.
    pub fn token(&self) -> Option<RegToken> {
        self.token.get().copied()
    }

    /// Record the token from the registration reply (the first one wins).
    pub fn set_token(&self, token: RegToken) -> bool {
        self.token.set(token).is_ok()
    }

    /// True when `presented` is this party's registration token. Before
    /// registration nothing is accepted.
    pub fn accepts_token(&self, presented: &RegToken) -> bool {
        self.token.get().is_some_and(|own| own.ct_eq(presented))
    }

    /// Last text delivered to this party by its peer, if any.
    pub fn inbox(&self) -> Option<String> {
        lock(&self.inbox).clone()
    }

    /// The service this client is paired with, if any.
    pub fn service(&self) -> Option<ServiceHandle> {
        self.service.borrow().clone()
    }

    /// Record a pairing (from the claim reply or a later re-pairing) and
    /// wake the [`pairings`](Self::pairings) receivers. Returns `false`, and
    /// wakes nobody, when `handle` is the pairing already held.
    pub fn set_service(&self, handle: ServiceHandle) -> bool {
        self.service.send_if_modified(|current| {
            if current.as_ref() == Some(&handle) {
                false
            } else {
                *current = Some(handle);
                true
            }
        })
    }

    /// A receiver of this party's pairings: `borrow` is the current service,
    /// `changed` resolves whenever the broker re-pairs the party. For a
    /// service it never changes.
    pub fn pairings(&self) -> watch::Receiver<Option<ServiceHandle>> {
        self.service.subscribe()
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
            self.set_service(handle);
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
        let s = state(Role::Service);
        assert_eq!(s.id(), None);
        assert!(s.set_id(PartyId(5)));
        assert!(!s.set_id(PartyId(6)));
        assert_eq!(s.id(), Some(PartyId(5)));
    }

    #[test]
    fn heartbeat_contents_are_stored_and_contact_refreshed() {
        let s = state(Role::Client);
        let before = s.last_contact();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let handle = ServiceHandle {
            id: PartyId(1),
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

    #[tokio::test]
    async fn pairings_report_each_new_service_once() {
        let s = state(Role::Client);
        let handle = |id: u64| ServiceHandle {
            id: PartyId(id),
            host: "10.0.0.1".into(),
            service_port: 9000,
        };
        let mut rx = s.pairings();
        assert_eq!(*rx.borrow_and_update(), None);

        // The claim reply: a change.
        assert!(s.set_service(handle(1)));
        assert!(rx.has_changed().unwrap());
        assert_eq!(rx.borrow_and_update().clone(), Some(handle(1)));
        // The same pairing again: not a change, nobody is woken.
        assert!(!s.set_service(handle(1)));
        assert!(!rx.has_changed().unwrap());
        // A heartbeat that re-pairs: a change, seen through `changed`.
        s.apply_heartbeat(None, Some(handle(2)));
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow_and_update().clone(), Some(handle(2)));
        assert_eq!(s.service(), Some(handle(2)));
        // A receiver taken later starts from the current pairing.
        assert_eq!(*s.pairings().borrow(), Some(handle(2)));
    }
}
