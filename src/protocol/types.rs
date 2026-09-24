//! Payload records carried by [`Message`](super::Message).
//!
//! Three views of a registered party:
//!
//! - [`ServiceRecord`] and [`ClientRecord`] are what the broker keeps for a
//!   published service and for a claiming client;
//! - [`ServiceHandle`] is the part of a service record a claimer receives and
//!   prints: enough to connect to the service's data-plane endpoint.
//!
//! All of them serialise as plain JSON objects and travel inside
//! [`Message`](super::Message) variants; none of them is ever sent bare.

use std::fmt;

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

/// What a claimer receives: enough to reach one service.
///
/// This is the value printed by `nsm claim` and returned by `nsm collect`
/// when asked of a client. It deliberately omits the service's heartbeat
/// endpoint, which is the broker's business only, and the rendezvous key,
/// which the claimer already holds and which must not leak to whoever asks a
/// client what it is paired with.
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
}
