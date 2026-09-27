//! Payload records carried by [`Message`](super::Message).
//!
//! Three views of a registered party, and its [`Role`]:
//!
//! - [`ServiceRecord`] and [`ClientRecord`] are what the broker keeps for a
//!   published service and for a claiming client;
//! - [`ServiceHandle`] is the part of a service record a claimer receives and
//!   prints: enough to connect to the service's data-plane endpoint;
//! - [`Role`] says which of the two kinds a party is, so that a reply from a
//!   party can state which of its fields apply.
//!
//! And the shared store a client and its service keep at the broker:
//!
//! - [`StoreKey`] names one entry; it is validated wherever it is built, so
//!   an invalid store key cannot exist in any typed layer;
//! - [`StoreOp`] is one operation on the store (get, put, delete, list);
//! - [`StoreEntry`] is one key with its value and version, and [`Stored`] is
//!   the answer to every operation: whose store it is, its revision and the
//!   entries the operation returns.
//!
//! All of them serialise as plain JSON and travel inside
//! [`Message`](super::Message) variants; none of them is ever sent bare.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::net::Addr;

/// Broker-assigned identity of a party (a service or a client).
///
/// The broker allocates ids from a monotonically increasing counter starting
/// at 1 and never reuses one within its lifetime. A party learns its own id
/// from the registration reply ([`Message::Registered`](super::Message::Registered)
/// or [`Message::Paired`](super::Message::Paired)) and quotes it in every
/// later exchange. On the wire a `PartyId` is a bare JSON integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PartyId(pub u64);

impl fmt::Display for PartyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<u64> for PartyId {
    fn from(id: u64) -> Self {
        PartyId(id)
    }
}

impl From<PartyId> for u64 {
    fn from(id: PartyId) -> Self {
        id.0
    }
}

/// Secret the broker issues to a party when it registers.
///
/// Every later message that acts on the party's behalf ([`Message::Ping`],
/// [`Message::Deliver`]) must carry it, and the broker's heartbeats to the
/// party carry it too, so a third party that merely knows a party's id (ids
/// are small sequential integers) can neither impersonate it nor spoof its
/// broker. 128 random bits, sent as 32 hex characters, compared in constant
/// time and never printed (`Debug` redacts it). Like the rendezvous key it is
/// confidential only as far as the transport is: use TLS on untrusted links.
///
/// [`Message::Ping`]: super::Message::Ping
/// [`Message::Deliver`]: super::Message::Deliver
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegToken([u8; 16]);

impl RegToken {
    /// A fresh random token from the installed crypto provider.
    pub fn generate() -> crate::Result<Self> {
        let mut bytes = [0u8; 16];
        crate::tls::fill_random(&mut bytes)?;
        Ok(RegToken(bytes))
    }

    /// A token from fixed bytes (tests and deterministic fixtures).
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        RegToken(bytes)
    }

    /// Constant-time equality.
    pub fn ct_eq(&self, other: &RegToken) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }

    /// The 32-character lowercase hex form used on the wire.
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(32);
        for b in self.0 {
            out.push(DIGITS[usize::from(b >> 4)] as char);
            out.push(DIGITS[usize::from(b & 0x0f)] as char);
        }
        out
    }

    /// Parse the hex form; `None` unless exactly 32 hex digits.
    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.len() != 32 || !text.is_ascii() {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            bytes[i] = ((hi << 4) | lo) as u8;
        }
        Some(RegToken(bytes))
    }
}

impl fmt::Debug for RegToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RegToken(redacted)")
    }
}

impl Serialize for RegToken {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RegToken {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        RegToken::from_hex(&text)
            .ok_or_else(|| serde::de::Error::custom("expected a 32-character hex token"))
    }
}

/// Rendezvous key shared by a service and the clients allowed to claim it.
///
/// Services publish under a key; a claim for the same key is paired with one
/// of them. The key carries no other meaning to the broker.
pub type Key = u64;

/// Which side of a pairing a party is.
///
/// A service ([`Message::Publish`](super::Message::Publish)) receives text;
/// a client ([`Message::Claim`](super::Message::Claim)) holds a pairing and
/// relays `send`. A party states its role in
/// [`Message::Collected`](super::Message::Collected) so that whoever asks
/// knows which of the reply's fields applies. On the wire a role is the
/// string `"service"` or `"client"`; `Display` uses the same spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A published service.
    Service,
    /// A client that claimed a service.
    Client,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Service => "service",
            Role::Client => "client",
        })
    }
}

/// What a claimer receives: enough to reach one service.
///
/// This is the value printed by `nsm claim` (once per pairing) and returned
/// by `nsm peer` when asked of a client. It deliberately omits the service's
/// heartbeat endpoint, which is the broker's business only, and the
/// rendezvous key, which the claimer already holds and which must not leak
/// to whoever asks a client what it is paired with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceHandle {
    /// The service's broker-assigned id.
    pub id: PartyId,
    /// Host of the service's data-plane endpoint: a DNS name or an IP
    /// literal (IPv6 without brackets).
    pub host: String,
    /// Port the service itself listens on (not its heartbeat port).
    pub service_port: u16,
}

impl ServiceHandle {
    /// `host:port` of the service's data-plane endpoint, with an IPv6
    /// literal in brackets, ready to be handed to a connecting client.
    pub fn socket_string(&self) -> String {
        Addr::tcp(self.host.as_str(), self.service_port).authority()
    }
}

impl fmt::Display for ServiceHandle {
    /// Same as [`ServiceHandle::socket_string`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.socket_string())
    }
}

/// Everything the broker keeps about a published service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceRecord {
    /// Broker-assigned id.
    pub id: PartyId,
    /// Key the service published under.
    pub key: Key,
    /// Data-plane endpoint handed to claimers: the service's host and its
    /// `service_port`. Always [`Transport::Tcp`](crate::net::Transport::Tcp);
    /// what the service speaks on that port is not the broker's concern.
    pub service_addr: Addr,
    /// Heartbeat endpoint the broker dials in two-sided mode, including the
    /// transport the party listens with.
    pub bind_addr: Addr,
    /// One-sided liveness: the service pings the broker instead of being
    /// dialled at `bind_addr`.
    pub ping: bool,
}

impl ServiceRecord {
    /// The claimer-facing view of this record.
    pub fn handle(&self) -> ServiceHandle {
        ServiceHandle {
            id: self.id,
            host: self.service_addr.host.clone(),
            service_port: self.service_addr.port,
        }
    }
}

/// Everything the broker keeps about a claiming client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRecord {
    /// Broker-assigned id.
    pub id: PartyId,
    /// Key the client claimed.
    pub key: Key,
    /// Heartbeat endpoint the broker dials in two-sided mode, including the
    /// transport the party listens with.
    pub bind_addr: Addr,
    /// Id of the service this client is currently paired with. Changes when
    /// the broker re-pairs the client after its service disappeared.
    pub service: PartyId,
    /// One-sided liveness: the client pings the broker instead of being
    /// dialled at `bind_addr`.
    pub ping: bool,
}

/// Longest [`StoreKey`], in bytes.
pub const MAX_STORE_KEY_BYTES: usize = 128;

/// The name of one entry in a shared store.
///
/// A store key is 1 to [`MAX_STORE_KEY_BYTES`] characters from `A-Z`,
/// `a-z`, `0-9` and `. _ - : /`, and does not start with `-`: one shell word
/// that never needs quoting, never globs and never parses as a flag, so a
/// list of keys prints one per line. Every way of building one checks this
/// ([`FromStr`], [`TryFrom<String>`] and deserialisation), so an invalid key
/// is a usage error at the command line and a decode error on the wire.
///
/// A store key is unrelated to the rendezvous [`Key`] a service publishes
/// under; the documentation says "store key" wherever both appear. On the
/// wire it is a JSON string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StoreKey(String);

/// Why a text is not a [`StoreKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a store key is 1 to 128 characters from A-Z a-z 0-9 . _ - : / and does not start with -")]
pub struct StoreKeyError;

impl StoreKey {
    /// The key as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when `text` is a valid store key.
    fn is_valid(text: &str) -> bool {
        let bytes = text.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= MAX_STORE_KEY_BYTES
            && bytes[0] != b'-'
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':' | b'/'))
    }
}

impl TryFrom<String> for StoreKey {
    type Error = StoreKeyError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        if StoreKey::is_valid(&text) {
            Ok(StoreKey(text))
        } else {
            Err(StoreKeyError)
        }
    }
}

impl FromStr for StoreKey {
    type Err = StoreKeyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        StoreKey::try_from(text.to_owned())
    }
}

impl From<StoreKey> for String {
    fn from(key: StoreKey) -> Self {
        key.0
    }
}

impl AsRef<str> for StoreKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StoreKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One operation on the store a client shares with its service.
///
/// On the wire the operation's name is the `op` field and its own fields sit
/// next to it, inside the message that carries it:
/// `{"op":"put","key":"step","value":"5","if_version":null}`,
/// `{"op":"get","key":"step"}`,
/// `{"op":"delete","key":"step","if_version":null}` and `{"op":"list"}`.
///
/// # Conditional writes
///
/// A put or a delete may state a condition in `if_version`, checked against
/// the key's current state in the same step that applies the write:
/// `Some(0)` means the key must not be set, `Some(n)` means the key's
/// current version must be `n`, and `None` (the default, also when the field
/// is left out) means no condition. Versions start at 1, so 0 never names an
/// entry. A write whose condition does not hold is not an error: the reply is
/// a [`Stored`] with `applied: false` carrying the key's current entry (or
/// none when it is not set), and nothing changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StoreOp {
    /// Read one entry. The reply carries it, or no entry when the key is
    /// not set.
    Get {
        /// The entry to read.
        key: StoreKey,
    },
    /// Set `key` to `value`, replacing what was there. The reply carries the
    /// entry as written, with its new version.
    Put {
        /// The entry to write.
        key: StoreKey,
        /// The new value: any UTF-8 text, including empty text and newlines.
        value: String,
        /// Write only if the key is at this version (0: only if it is not
        /// set); `None` writes unconditionally. See [conditional
        /// writes](StoreOp#conditional-writes).
        #[serde(default)]
        if_version: Option<u64>,
    },
    /// Remove one entry. Removing a key that is not set succeeds and changes
    /// nothing. The reply carries the removed entry, or no entry.
    Delete {
        /// The entry to remove.
        key: StoreKey,
        /// Remove only if the key is at this version (0: only if it is not
        /// set, which removes nothing); `None` removes unconditionally. See
        /// [conditional writes](StoreOp#conditional-writes).
        #[serde(default)]
        if_version: Option<u64>,
    },
    /// Read every entry at once: one consistent snapshot, in ascending byte
    /// order of key.
    List,
}

impl StoreOp {
    /// The operation's wire name (the `op` field), for logs. Never includes
    /// the key or the value.
    pub fn kind(&self) -> &'static str {
        match self {
            StoreOp::Get { .. } => "get",
            StoreOp::Put { .. } => "put",
            StoreOp::Delete { .. } => "delete",
            StoreOp::List => "list",
        }
    }

    /// The key the operation names; `None` for [`StoreOp::List`].
    pub fn key(&self) -> Option<&StoreKey> {
        match self {
            StoreOp::Get { key } | StoreOp::Put { key, .. } | StoreOp::Delete { key, .. } => {
                Some(key)
            }
            StoreOp::List => None,
        }
    }

    /// The condition a put or a delete states (see [conditional
    /// writes](StoreOp#conditional-writes)); `None` for an unconditional
    /// write and for the reads.
    pub fn if_version(&self) -> Option<u64> {
        match self {
            StoreOp::Put { if_version, .. } | StoreOp::Delete { if_version, .. } => *if_version,
            StoreOp::Get { .. } | StoreOp::List => None,
        }
    }

    /// True for the operations that may change the store (put and delete).
    pub fn is_write(&self) -> bool {
        matches!(self, StoreOp::Put { .. } | StoreOp::Delete { .. })
    }
}

/// One entry of a shared store, as a reply carries it:
/// `{"key":"step","value":"5","version":3}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreEntry {
    /// The entry's name.
    pub key: StoreKey,
    /// The entry's value, verbatim. An empty value is set, not absent.
    pub value: String,
    /// The number of the entry's last write. Versions come from one counter
    /// for the broker's whole life, so a version names one state of one
    /// entry and is never issued twice.
    pub version: u64,
}

/// The answer to every store operation: whose store it is, its revision and
/// the entries the operation returns.
///
/// The broker builds it, the party passes it back unchanged, and the
/// operations layer returns it as is. What `entries` holds depends on the
/// operation: for get, the entry or nothing; for put, the entry as written;
/// for delete, the removed entry or nothing; for list, every entry in
/// ascending byte order of key; and for a conditional write whose condition
/// did not hold (`applied: false`), the key's current entry or nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    /// Id of the client whose claim owns the store, so that a service whose
    /// client was replaced by a new claimer can tell the two stores apart.
    /// `None` when a service nobody holds reads its (then empty) store.
    pub client: Option<PartyId>,
    /// The number of the last write to this store, 0 for a store never
    /// written. A delete that removed something counts as a write.
    pub revision: u64,
    /// False when a put or a delete stated an `if_version` that did not
    /// match, so nothing changed; true for every other answer. Always sent;
    /// a reply without it decodes as true. Such a reply comes from a broker,
    /// or passed through a relaying party, that predates conditional
    /// writes; that broker or party also dropped `if_version`, so the write
    /// was applied unconditionally. Conditions need both to have them.
    #[serde(default = "applied_by_default")]
    pub applied: bool,
    /// The entries the operation returns.
    pub entries: Vec<StoreEntry>,
}

/// What a [`Stored`] reply without `applied` means: the operation was
/// applied, as every operation was before conditional writes existed.
fn applied_by_default() -> bool {
    true
}

impl Stored {
    /// The entry for `key` among the returned entries: the value read by a
    /// get, the entry written by a put, the entry removed by a delete.
    pub fn get(&self, key: &StoreKey) -> Option<&StoreEntry> {
        self.entries.iter().find(|e| &e.key == key)
    }

    /// The keys of the returned entries, in the order the broker sent them
    /// (ascending for a list).
    pub fn keys(&self) -> impl Iterator<Item = &StoreKey> {
        self.entries.iter().map(|e| &e.key)
    }

    /// Why `op` was not applied, from what this reply to it carries:
    /// `step is at version 7` when it carries the key's entry, `step is not
    /// set` when it does not. This is how the front-ends explain a
    /// conditional write that missed. Only a put or a delete can miss, and
    /// both name a key; for an operation without one the text says only
    /// that it was not applied.
    pub fn not_applied_reason(&self, op: &StoreOp) -> String {
        match op.key() {
            Some(key) => match self.get(key) {
                Some(entry) => format!("{key} is at version {}", entry.version),
                None => format!("{key} is not set"),
            },
            None => format!("the {} was not applied", op.kind()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Transport;

    fn service() -> ServiceRecord {
        ServiceRecord {
            id: PartyId(3),
            key: 42,
            service_addr: Addr::tcp("10.0.0.5", 9000),
            bind_addr: Addr::new(Transport::Https, "10.0.0.5", 9001),
            ping: false,
        }
    }

    #[test]
    fn party_id_is_a_bare_integer_on_the_wire() {
        assert_eq!(serde_json::to_string(&PartyId(7)).unwrap(), "7");
        let id: PartyId = serde_json::from_str("7").unwrap();
        assert_eq!(id, PartyId(7));
        assert!(serde_json::from_str::<PartyId>("\"7\"").is_err());
        assert!(serde_json::from_str::<PartyId>("-1").is_err());
        assert!(serde_json::from_str::<PartyId>("{\"0\":7}").is_err());
    }

    #[test]
    fn party_id_display_and_conversions() {
        assert_eq!(PartyId(12).to_string(), "12");
        assert_eq!(format!("{:>4}", PartyId(12)), "  12");
        assert_eq!(PartyId::from(5), PartyId(5));
        assert_eq!(u64::from(PartyId(5)), 5);
        assert!(PartyId(1) < PartyId(2));
    }

    #[test]
    fn socket_string_brackets_ipv6_only() {
        let mut h = service().handle();
        assert_eq!(h.socket_string(), "10.0.0.5:9000");
        assert_eq!(h.to_string(), "10.0.0.5:9000");
        h.host = "node.example.org".into();
        assert_eq!(h.socket_string(), "node.example.org:9000");
        h.host = "fe80::1".into();
        assert_eq!(h.socket_string(), "[fe80::1]:9000");
    }

    #[test]
    fn handle_projects_the_data_plane_endpoint() {
        let s = service();
        assert_eq!(
            s.handle(),
            ServiceHandle {
                id: PartyId(3),
                host: "10.0.0.5".into(),
                service_port: 9000,
            }
        );
    }

    #[test]
    fn reg_token_hex_round_trip_constant_time_and_redacted() {
        let t = RegToken::from_bytes([0x0f, 0xa0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 0xff]);
        let hex = t.to_hex();
        assert_eq!(hex.len(), 32);
        assert!(hex.starts_with("0fa0") && hex.ends_with("ff"));
        assert_eq!(RegToken::from_hex(&hex), Some(t));
        assert_eq!(RegToken::from_hex(&hex.to_uppercase()), Some(t));
        assert_eq!(RegToken::from_hex("short"), None);
        assert_eq!(RegToken::from_hex(&"zz".repeat(16)), None);
        assert!(t.ct_eq(&t));
        assert!(!t.ct_eq(&RegToken::from_bytes([0; 16])));
        assert_eq!(format!("{t:?}"), "RegToken(redacted)");
        assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{hex}\""));
        let back: RegToken = serde_json::from_str(&format!("\"{hex}\"")).unwrap();
        assert_eq!(back, t);
        assert!(serde_json::from_str::<RegToken>("\"nope\"").is_err());
        assert!(serde_json::from_str::<RegToken>("7").is_err());
    }

    #[test]
    fn generated_tokens_are_distinct() {
        crate::tls::install_default_provider();
        let a = RegToken::generate().unwrap();
        let b = RegToken::generate().unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn role_is_spelled_service_or_client() {
        assert_eq!(
            serde_json::to_string(&Role::Service).unwrap(),
            "\"service\""
        );
        assert_eq!(serde_json::to_string(&Role::Client).unwrap(), "\"client\"");
        assert_eq!(
            serde_json::from_str::<Role>("\"client\"").unwrap(),
            Role::Client
        );
        for wrong in ["\"Service\"", "\"claimer\"", "\"publisher\"", "0", "null"] {
            assert!(serde_json::from_str::<Role>(wrong).is_err(), "{wrong}");
        }
        assert_eq!(Role::Service.to_string(), "service");
        assert_eq!(Role::Client.to_string(), "client");
    }

    #[test]
    fn service_handle_never_carries_the_key() {
        let json = serde_json::to_value(service().handle()).unwrap();
        assert!(json.get("key").is_none(), "{json}");
    }

    #[test]
    fn records_round_trip_through_json() {
        let s = service();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<ServiceRecord>(&json).unwrap(), s);

        let c = ClientRecord {
            id: PartyId(4),
            key: 42,
            bind_addr: Addr::tcp("::1", 7000),
            service: PartyId(3),
            ping: true,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<ClientRecord>(&json).unwrap(), c);

        let h = s.handle();
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, r#"{"id":3,"host":"10.0.0.5","service_port":9000}"#);
        assert_eq!(serde_json::from_str::<ServiceHandle>(&json).unwrap(), h);
    }

    fn key(text: &str) -> StoreKey {
        text.parse().unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    #[test]
    fn store_keys_are_one_shell_word() {
        let longest = "k".repeat(MAX_STORE_KEY_BYTES);
        for good in [
            "a",
            "Z",
            "0",
            ".",
            "_",
            ":",
            "/",
            "step",
            "input/path.txt",
            "job:17_a-b",
            "a-",
            longest.as_str(),
        ] {
            let parsed: StoreKey = good.parse().unwrap_or_else(|e| panic!("{good:?}: {e}"));
            assert_eq!(parsed.as_str(), good);
            assert_eq!(parsed.to_string(), good);
            assert_eq!(StoreKey::try_from(good.to_owned()), Ok(parsed.clone()));
            let json = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, format!("\"{good}\""));
            assert_eq!(serde_json::from_str::<StoreKey>(&json).unwrap(), parsed);
            assert_eq!(String::from(parsed.clone()), good);
            assert_eq!(parsed.as_ref(), good);
        }
        let too_long = "k".repeat(MAX_STORE_KEY_BYTES + 1);
        for bad in [
            "",
            too_long.as_str(),
            "-x",
            "-",
            "a b",
            " a",
            "a\n",
            "a\t",
            "a\0",
            "\u{7f}",
            "é",
            "λ",
            "a*",
            "a?",
            "[a]",
            "a~",
            "a\"",
            "a\\",
            "a=b",
            "a,b",
            "$HOME",
        ] {
            assert_eq!(bad.parse::<StoreKey>(), Err(StoreKeyError), "{bad:?}");
            assert_eq!(StoreKey::try_from(bad.to_owned()), Err(StoreKeyError));
            let json = serde_json::to_string(bad).unwrap();
            let err = serde_json::from_str::<StoreKey>(&json).unwrap_err();
            assert!(err.to_string().contains("a store key is"), "{bad:?}: {err}");
        }
        assert!(serde_json::from_str::<StoreKey>("7").is_err());
        assert!(serde_json::from_str::<StoreKey>("null").is_err());
        assert_eq!(
            StoreKeyError.to_string(),
            "a store key is 1 to 128 characters from A-Z a-z 0-9 . _ - : / and does not start with -"
        );
    }

    #[test]
    fn random_store_keys_agree_across_every_constructor() {
        let mut rng = crate::testing::Rng::new(17);
        for i in 0..1000 {
            let text = rng.store_key_text();
            let valid = !text.is_empty()
                && text.len() <= MAX_STORE_KEY_BYTES
                && !text.starts_with('-')
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_:/".contains(c));
            let parsed = text.parse::<StoreKey>();
            assert_eq!(parsed.is_ok(), valid, "iteration {i}: {text:?}");
            assert_eq!(
                StoreKey::try_from(text.clone()),
                parsed,
                "iteration {i}: {text:?}"
            );
            let json = serde_json::to_string(&text).unwrap();
            assert_eq!(
                serde_json::from_str::<StoreKey>(&json).ok(),
                parsed.ok(),
                "iteration {i}: {text:?}"
            );
            let generated = rng.store_key();
            assert_eq!(
                generated.as_str().parse::<StoreKey>(),
                Ok(generated.clone()),
                "iteration {i}"
            );
        }
    }

    #[test]
    fn store_op_names_and_keys() {
        let k = key("step");
        let ops = [
            (
                StoreOp::Get { key: k.clone() },
                "get",
                Some(&k),
                false,
                None,
            ),
            (
                StoreOp::Put {
                    key: k.clone(),
                    value: "5".into(),
                    if_version: None,
                },
                "put",
                Some(&k),
                true,
                None,
            ),
            (
                StoreOp::Put {
                    key: k.clone(),
                    value: "5".into(),
                    if_version: Some(0),
                },
                "put",
                Some(&k),
                true,
                Some(0),
            ),
            (
                StoreOp::Delete {
                    key: k.clone(),
                    if_version: Some(7),
                },
                "delete",
                Some(&k),
                true,
                Some(7),
            ),
            (StoreOp::List, "list", None, false, None),
        ];
        for (op, kind, named, write, condition) in &ops {
            assert_eq!(op.kind(), *kind);
            assert_eq!(op.key(), *named, "{kind}");
            assert_eq!(op.is_write(), *write, "{kind}");
            assert_eq!(op.if_version(), *condition, "{kind}");
            let json = serde_json::to_value(op).unwrap();
            assert_eq!(json["op"], *kind, "{json}");
        }
    }

    #[test]
    fn store_op_json_shape() {
        let put = |if_version| StoreOp::Put {
            key: key("step"),
            value: "5".into(),
            if_version,
        };
        let delete = |if_version| StoreOp::Delete {
            key: key("step"),
            if_version,
        };
        // Like every `Option`, an absent condition is sent as null.
        for (op, json) in [
            (
                put(None),
                r#"{"op":"put","key":"step","value":"5","if_version":null}"#,
            ),
            (
                put(Some(0)),
                r#"{"op":"put","key":"step","value":"5","if_version":0}"#,
            ),
            (
                delete(None),
                r#"{"op":"delete","key":"step","if_version":null}"#,
            ),
            (
                delete(Some(u64::MAX)),
                r#"{"op":"delete","key":"step","if_version":18446744073709551615}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&op).unwrap(), json);
            assert_eq!(serde_json::from_str::<StoreOp>(json).unwrap(), op, "{json}");
        }
        // A write without the field is unconditional, as before conditions
        // existed.
        assert_eq!(
            serde_json::from_str::<StoreOp>(r#"{"op":"put","key":"step","value":"5"}"#).unwrap(),
            put(None)
        );
        assert_eq!(
            serde_json::from_str::<StoreOp>(r#"{"op":"delete","key":"step"}"#).unwrap(),
            delete(None)
        );
        assert_eq!(
            serde_json::to_string(&StoreOp::List).unwrap(),
            r#"{"op":"list"}"#
        );
        assert_eq!(
            serde_json::from_str::<StoreOp>(r#"{"op":"get","key":"step","extra":1}"#).unwrap(),
            StoreOp::Get { key: key("step") }
        );
        for bad in [
            r#"{"op":"put","key":"step"}"#,
            r#"{"op":"get"}"#,
            r#"{"op":"get","key":""}"#,
            r#"{"op":"Get","key":"step"}"#,
            r#"{"op":"increment","key":"step"}"#,
            r#"{"key":"step"}"#,
            r#"{"op":"put","key":"step","value":5}"#,
            r#"{"op":"put","key":"step","value":"5","if_version":-1}"#,
            r#"{"op":"put","key":"step","value":"5","if_version":"3"}"#,
            r#"{"op":"delete","key":"step","if_version":1.5}"#,
        ] {
            assert!(serde_json::from_str::<StoreOp>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn stored_accessors_and_json_shape() {
        let stored = Stored {
            client: Some(PartyId(2)),
            revision: 7,
            applied: true,
            entries: vec![
                StoreEntry {
                    key: key("a"),
                    value: String::new(),
                    version: 3,
                },
                StoreEntry {
                    key: key("b"),
                    value: "two\nlines".into(),
                    version: 7,
                },
            ],
        };
        assert_eq!(stored.get(&key("b")).map(|e| e.version), Some(7));
        assert_eq!(stored.get(&key("a")).map(|e| e.value.as_str()), Some(""));
        assert_eq!(stored.get(&key("c")), None);
        let keys: Vec<&str> = stored.keys().map(StoreKey::as_str).collect();
        assert_eq!(keys, ["a", "b"]);
        let delete = |k| StoreOp::Delete {
            key: key(k),
            if_version: Some(1),
        };
        assert_eq!(stored.not_applied_reason(&delete("b")), "b is at version 7");
        assert_eq!(stored.not_applied_reason(&delete("c")), "c is not set");
        assert_eq!(
            stored.not_applied_reason(&StoreOp::List),
            "the list was not applied"
        );

        let json = serde_json::to_string(&stored).unwrap();
        assert_eq!(
            json,
            concat!(
                r#"{"client":2,"revision":7,"applied":true,"entries":["#,
                r#"{"key":"a","value":"","version":3},"#,
                r#"{"key":"b","value":"two\nlines","version":7}]}"#
            )
        );
        assert_eq!(serde_json::from_str::<Stored>(&json).unwrap(), stored);

        let unclaimed = Stored {
            client: None,
            revision: 0,
            applied: true,
            entries: vec![],
        };
        assert_eq!(
            serde_json::to_string(&unclaimed).unwrap(),
            r#"{"client":null,"revision":0,"applied":true,"entries":[]}"#
        );
        assert_eq!(unclaimed.keys().count(), 0);
    }

    #[test]
    fn stored_says_whether_a_write_was_applied_and_defaults_to_yes() {
        let refused = Stored {
            client: Some(PartyId(2)),
            revision: 9,
            applied: false,
            entries: vec![StoreEntry {
                key: key("step"),
                value: "5".into(),
                version: 7,
            }],
        };
        let json = r#"{"client":2,"revision":9,"applied":false,"entries":[{"key":"step","value":"5","version":7}]}"#;
        assert_eq!(serde_json::to_string(&refused).unwrap(), json);
        assert_eq!(serde_json::from_str::<Stored>(json).unwrap(), refused);
        let put = StoreOp::Put {
            key: key("step"),
            value: "6".into(),
            if_version: Some(0),
        };
        assert_eq!(refused.not_applied_reason(&put), "step is at version 7");

        // A reply from a broker that predates conditional writes has no
        // `applied`: every operation it answered was applied.
        let old: Stored =
            serde_json::from_str(r#"{"client":2,"revision":9,"entries":[]}"#).unwrap();
        assert!(old.applied);
        for bad in [
            r#"{"client":2,"revision":9,"applied":null,"entries":[]}"#,
            r#"{"client":2,"revision":9,"applied":"false","entries":[]}"#,
            r#"{"client":2,"revision":9,"applied":0,"entries":[]}"#,
        ] {
            assert!(serde_json::from_str::<Stored>(bad).is_err(), "{bad}");
        }
    }
}
