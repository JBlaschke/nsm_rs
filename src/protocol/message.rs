//! The tagged wire message enum.
//!
//! Every exchange in NSM is one request followed by exactly one reply, on
//! every transport: a length-prefixed frame each way on a TCP or TLS
//! connection (see [`codec`](super::codec)), or an HTTP request body and its
//! response body. The same [`Message`] JSON is used in both cases.
//!
//! A message is a JSON object whose `"type"` field is the snake_case variant
//! name, followed by the variant's fields, e.g.
//! `{"type":"publish","key":42,"service_port":9000,"bind_addr":"https://10.0.0.5:9001","ping":false}`.
//! Variants without fields are just the tag: `{"type":"collect"}`. An unknown
//! tag, a missing field, a wrong type or trailing bytes are decode errors;
//! unknown fields are ignored so that a newer peer may add some.
//!
//! | Request | Direction | Reply |
//! |---|---|---|
//! | [`Publish`](Message::Publish) | service → broker | [`Registered`](Message::Registered) or [`Nack`](Message::Nack) |
//! | [`Claim`](Message::Claim) | client → broker | [`Paired`](Message::Paired) or [`Nack`](Message::Nack) |
//! | [`Ping`](Message::Ping) | ping-mode party → broker, with its token | [`Heartbeat`](Message::Heartbeat) or [`Nack`](Message::Nack) |
//! | [`Send`](Message::Send) | `send` → client | [`Delivered`](Message::Delivered) or [`Nack`](Message::Nack) |
//! | [`Deliver`](Message::Deliver) | client → broker (relay of a `Send`), with its token | [`Delivered`](Message::Delivered) or [`Nack`](Message::Nack) |
//! | [`Heartbeat`](Message::Heartbeat) | broker → party (two-sided liveness) | [`HeartbeatAck`](Message::HeartbeatAck) |
//! | [`Collect`](Message::Collect) | `collect` → party | [`Collected`](Message::Collected) |
//!
//! "Party" means a service or a client; each runs a small server on its
//! `bind_addr` that the broker (and the `send` / `collect` operations) talk
//! to. The broker is the only party with a fixed, well-known address.
//!
//! Registration returns a [`RegToken`]: a random secret that the party
//! quotes in every [`Ping`](Message::Ping) and [`Deliver`](Message::Deliver),
//! and that the broker quotes in every [`Heartbeat`](Message::Heartbeat) it
//! sends. Party ids are small sequential integers and are not secrets; the
//! token is what stops a third party from acting on another party's behalf
//! or from spoofing its broker.

use serde::{Deserialize, Serialize};

use super::types::{Key, PartyId, RegToken, ServiceHandle};
use crate::net::Addr;

/// Version of the wire protocol described in this module.
///
/// Bumped on any change to [`Message`] or to the framing that an older peer
/// could not decode. Adding an optional field or a new variant does not
/// require a bump.
pub const PROTOCOL_VERSION: u16 = 1;

/// One wire message: a request to the broker or to a party, or a reply.
///
/// See the [module docs](self) for the request/reply table and the JSON
/// shape. Construct replies that reject a request with [`Message::nack`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    // ----- requests to the broker -------------------------------------------
    /// A service registers itself under `key`.
    ///
    /// Sent by a service to the broker. Its data-plane endpoint, which
    /// claimers are told about, is `bind_addr.host` together with
    /// `service_port`; `bind_addr` itself is where the broker heartbeats it
    /// (two-sided mode) and includes the transport the service listens with.
    /// With `ping: true` the service will send [`Message::Ping`] instead of
    /// being dialled.
    ///
    /// Reply: [`Message::Registered`] with the assigned id, or
    /// [`Message::Nack`] (for example when the broker is full).
    Publish {
        /// Key clients will claim.
        key: Key,
        /// Port the service itself listens on, at `bind_addr.host`.
        service_port: u16,
        /// Heartbeat endpoint of the service, including its transport.
        bind_addr: Addr,
        /// One-sided liveness (the service pings) instead of two-sided
        /// heartbeats (the broker dials `bind_addr`).
        ping: bool,
    },

    /// A client asks to be paired with a service published under `key`.
    ///
    /// Sent by a client to the broker. `bind_addr` and `ping` mean the same
    /// as in [`Message::Publish`].
    ///
    /// Reply: [`Message::Paired`] with the client's id and the service's
    /// handle, or [`Message::Nack`] when no service is available under `key`
    /// (or the broker is full).
    Claim {
        /// Key of the wanted service.
        key: Key,
        /// Heartbeat endpoint of the client, including its transport.
        bind_addr: Addr,
        /// One-sided liveness (the client pings) instead of two-sided
        /// heartbeats (the broker dials `bind_addr`).
        ping: bool,
    },

    /// One-sided heartbeat: a party registered with `ping: true` tells the
    /// broker it is still alive.
    ///
    /// Sent by a service or a client to the broker, at the heartbeat
    /// interval. The broker refreshes the party's liveness timestamp and
    /// removes parties that stay silent for the configured staleness.
    ///
    /// Only parties registered with `ping: true` may ping, and only with the
    /// right `token`; anything else is refused without revealing whether the
    /// id exists.
    ///
    /// Reply: a [`Message::Heartbeat`] carrying whatever is pending for the
    /// party (delivered text for a service, a new pairing for a client), or
    /// [`Message::Nack`] when the id or token is wrong (the party should treat
    /// that as lost registration). The reply is the same message the broker
    /// would have sent in two-sided mode, so both modes deliver the same
    /// things.
    Ping {
        /// The pinging party's own id.
        id: PartyId,
        /// The token the broker issued to that party at registration.
        token: RegToken,
    },

    /// Hand `text` to a client for its paired service.
    ///
    /// Sent by the `send` operation to a *client's* bind address; `send`
    /// knows only that address, not the service's id. The client relays the
    /// text to the broker as [`Message::Deliver`] with its paired service as
    /// the target and passes the broker's answer back.
    ///
    /// Reply: [`Message::Delivered`], or [`Message::Nack`] when the party is
    /// a service, or a client that is not paired.
    Send {
        /// The text itself, carried verbatim (no extra JSON encoding).
        text: String,
    },

    /// Carry `text` to the service `to`, to be delivered in its next
    /// heartbeat.
    ///
    /// Sent by a client to the broker when relaying a [`Message::Send`]. The
    /// client identifies itself with `from` and its `token`, and the broker
    /// accepts the text only when that client is currently paired with `to`;
    /// it then stores the text as the service's pending `inbox` and hands it
    /// over in the next [`Message::Heartbeat`] (or in the reply to the
    /// service's next [`Message::Ping`]); `collect` on the service then
    /// returns it. A later `Deliver` before the hand-over replaces the text.
    ///
    /// Reply: [`Message::Delivered`], or [`Message::Nack`] when `from`/`token`
    /// do not name a registered client, or that client is not paired with
    /// `to`.
    Deliver {
        /// Id of the relaying client.
        from: PartyId,
        /// The relaying client's registration token.
        token: RegToken,
        /// Id of the service that should receive the text.
        to: PartyId,
        /// The text itself, carried verbatim (no extra JSON encoding).
        text: String,
    },

    // ----- broker → party ---------------------------------------------------
    /// Two-sided heartbeat: the broker checks that a party is alive and
    /// delivers whatever is pending for it.
    ///
    /// Sent by the broker to a party's bind address at the heartbeat
    /// interval, and also returned by the broker as the reply to a
    /// [`Message::Ping`]. `inbox` is text a service has been sent (see
    /// [`Message::Deliver`]); `service` is a client's new pairing after the
    /// broker re-claimed on its behalf because its previous service went
    /// away. Both are `None` on an ordinary beat, and each pending item is
    /// delivered once. A party stores what it receives so that `collect` can
    /// return it.
    ///
    /// `token` is the party's own registration token; a party ignores (and
    /// Nacks) a heartbeat that does not carry it, so nobody but the broker
    /// can deliver text, re-pair a client or refresh its watchdog.
    ///
    /// Reply (when sent by the broker): [`Message::HeartbeatAck`] carrying the
    /// party's own id; the broker logs a mismatch but treats any
    /// acknowledgement as proof of life.
    Heartbeat {
        /// The receiving party's registration token.
        token: RegToken,
        /// Text pending for a service, delivered once.
        inbox: Option<String>,
        /// A client's new service after a re-pairing.
        service: Option<ServiceHandle>,
    },

    // ----- requests to a party ----------------------------------------------
    /// Ask a party what it holds.
    ///
    /// Sent by the `collect` operation to a party's bind address.
    ///
    /// Reply: [`Message::Collected`]. A service answers with the last text it
    /// received in a heartbeat (`text`, `None` if nothing was ever
    /// delivered); a client answers with the handle of the service it is
    /// paired with (`service`).
    Collect,

    // ----- replies ----------------------------------------------------------
    /// Reply to [`Message::Publish`]: the service is registered.
    Registered {
        /// Id assigned to the service; it quotes this in pings.
        id: PartyId,
        /// Secret the service quotes in pings and expects in heartbeats.
        token: RegToken,
    },

    /// Reply to [`Message::Claim`]: the client is registered and paired.
    Paired {
        /// Id assigned to the client; it quotes this in pings and relays.
        id: PartyId,
        /// Secret the client quotes in pings and relays and expects in
        /// heartbeats.
        token: RegToken,
        /// The service the client was paired with.
        service: ServiceHandle,
    },

    /// Reply to [`Message::Heartbeat`], from the party, with its own id.
    HeartbeatAck {
        /// The party the heartbeat concerns.
        id: PartyId,
    },

    /// Reply to [`Message::Deliver`]: the text was accepted for delivery.
    Delivered,

    /// Reply to [`Message::Collect`]. Exactly one field is `Some` in
    /// practice: `text` from a service that has received something,
    /// `service` from a client. A service that has never received anything
    /// answers with both `None`.
    Collected {
        /// Last text delivered to a service.
        text: Option<String>,
        /// The service a client is paired with.
        service: Option<ServiceHandle>,
    },

    /// Negative reply to any request: the request was understood but cannot
    /// be honoured (no service under the key, unknown id, broker full, ...).
    ///
    /// Malformed input is not answered with a `Nack`; the transport drops
    /// the connection or returns an HTTP error instead.
    Nack {
        /// Human-readable reason, for logs and error messages.
        reason: String,
    },
}

impl Message {
    /// Build a [`Message::Nack`] from anything that converts into a string.
    pub fn nack(reason: impl Into<String>) -> Message {
        Message::Nack {
            reason: reason.into(),
        }
    }

    /// True for the variants that answer a request; false for requests
    /// (including the broker-initiated [`Message::Heartbeat`]).
    pub fn is_reply(&self) -> bool {
        matches!(
            self,
            Message::Registered { .. }
                | Message::Paired { .. }
                | Message::HeartbeatAck { .. }
                | Message::Delivered
                | Message::Collected { .. }
                | Message::Nack { .. }
        )
    }

    /// The variant's wire tag (the `"type"` field), for logs and errors.
    pub fn kind(&self) -> &'static str {
        match self {
            Message::Publish { .. } => "publish",
            Message::Claim { .. } => "claim",
            Message::Ping { .. } => "ping",
            Message::Send { .. } => "send",
            Message::Deliver { .. } => "deliver",
            Message::Heartbeat { .. } => "heartbeat",
            Message::Collect => "collect",
            Message::Registered { .. } => "registered",
            Message::Paired { .. } => "paired",
            Message::HeartbeatAck { .. } => "heartbeat_ack",
            Message::Delivered => "delivered",
            Message::Collected { .. } => "collected",
            Message::Nack { .. } => "nack",
        }
    }
}

/// A fixed token for tests across the crate.
#[cfg(test)]
pub(crate) fn test_token() -> RegToken {
    RegToken::from_bytes([7; 16])
}

/// One instance of every variant, with every `Option` populated, for
/// round-trip tests here and in the codec.
#[cfg(test)]
pub(crate) fn all_variants() -> Vec<Message> {
    use crate::net::Transport;

    let handle = ServiceHandle {
        id: PartyId(3),
        host: "fe80::1".into(),
        service_port: 9000,
    };
    let token = test_token();
    vec![
        Message::Publish {
            key: 42,
            service_port: 9000,
            bind_addr: Addr::new(Transport::Https, "10.0.0.5", 9001),
            ping: false,
        },
        Message::Claim {
            key: 42,
            bind_addr: Addr::tcp("10.0.0.6", 7000),
            ping: true,
        },
        Message::Ping {
            id: PartyId(3),
            token,
        },
        Message::Send {
            text: "for the service".into(),
        },
        Message::Deliver {
            from: PartyId(4),
            token,
            to: PartyId(3),
            text: "hello \"world\" \u{1F600} \\ / \n".into(),
        },
        Message::Heartbeat {
            token,
            inbox: Some("pending".into()),
            service: Some(handle.clone()),
        },
        Message::Collect,
        Message::Registered {
            id: PartyId(3),
            token,
        },
        Message::Paired {
            id: PartyId(4),
            token,
            service: handle.clone(),
        },
        Message::HeartbeatAck { id: PartyId(4) },
        Message::Delivered,
        Message::Collected {
            text: Some("last".into()),
            service: Some(handle),
        },
        Message::nack("no service available for key 42"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Transport;

    fn decode(json: &str) -> serde_json::Result<Message> {
        serde_json::from_str(json)
    }

    #[test]
    fn all_variants_covers_every_variant_once() {
        let kinds: Vec<&str> = all_variants().iter().map(Message::kind).collect();
        let mut unique = kinds.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            kinds.len(),
            "duplicate variant in all_variants"
        );
        assert_eq!(kinds.len(), 13, "a variant was added; extend all_variants");
    }

    #[test]
    fn every_variant_round_trips_through_json() {
        for msg in all_variants() {
            let json = serde_json::to_string(&msg).unwrap();
            let back = decode(&json).unwrap_or_else(|e| panic!("{json}: {e}"));
            assert_eq!(back, msg, "{json}");
        }
    }

    #[test]
    fn kind_matches_the_serialized_tag() {
        for msg in all_variants() {
            let value = serde_json::to_value(&msg).unwrap();
            assert_eq!(value["type"], msg.kind(), "{value}");
        }
    }

    #[test]
    fn publish_json_shape() {
        let msg = Message::Publish {
            key: 42,
            service_port: 9000,
            bind_addr: Addr::new(Transport::Https, "10.0.0.5", 9001),
            ping: false,
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            concat!(
                r#"{"type":"publish","key":42,"service_port":9000,"#,
                r#""bind_addr":"https://10.0.0.5:9001","#,
                r#""ping":false}"#
            )
        );
    }

    #[test]
    fn nack_json_shape() {
        assert_eq!(
            serde_json::to_string(&Message::nack("full")).unwrap(),
            r#"{"type":"nack","reason":"full"}"#
        );
    }

    #[test]
    fn ids_and_handles_json_shape() {
        let hex = "07".repeat(16);
        assert_eq!(
            serde_json::to_string(&Message::Registered {
                id: PartyId(7),
                token: test_token(),
            })
            .unwrap(),
            format!(r#"{{"type":"registered","id":7,"token":"{hex}"}}"#)
        );
        let paired = decode(&format!(
            r#"{{"type":"paired","id":8,"token":"{hex}","service":{{"id":7,"host":"h","service_port":2}}}}"#,
        ))
        .unwrap();
        assert_eq!(
            paired,
            Message::Paired {
                id: PartyId(8),
                token: test_token(),
                service: ServiceHandle {
                    id: PartyId(7),
                    host: "h".into(),
                    service_port: 2,
                },
            }
        );
        // A handle on the wire never carries the rendezvous key.
        assert!(!serde_json::to_string(&paired).unwrap().contains("key"));
    }

    #[test]
    fn unit_variants_are_bare_tags() {
        assert_eq!(
            serde_json::to_string(&Message::Collect).unwrap(),
            r#"{"type":"collect"}"#
        );
        assert_eq!(decode(r#"{"type":"collect"}"#).unwrap(), Message::Collect);
        assert_eq!(
            decode(r#"{"type":"delivered"}"#).unwrap(),
            Message::Delivered
        );
    }

    #[test]
    fn missing_options_decode_as_none() {
        let hex = "07".repeat(16);
        assert_eq!(
            decode(&format!(r#"{{"type":"heartbeat","token":"{hex}"}}"#)).unwrap(),
            Message::Heartbeat {
                token: test_token(),
                inbox: None,
                service: None,
            }
        );
        // The token is not optional.
        assert!(decode(r#"{"type":"heartbeat"}"#).is_err());
        assert_eq!(
            decode(r#"{"type":"collected","text":null}"#).unwrap(),
            Message::Collected {
                text: None,
                service: None,
            }
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = format!(
            r#"{{"type":"ping","id":1,"token":"{}","extra":true}}"#,
            "07".repeat(16)
        );
        assert_eq!(
            decode(&json).unwrap(),
            Message::Ping {
                id: PartyId(1),
                token: test_token()
            }
        );
    }

    #[test]
    fn unknown_tag_is_an_error() {
        assert!(decode(r#"{"type":"explode"}"#).is_err());
        assert!(
            decode(r#"{"type":"Publish"}"#).is_err(),
            "tags are snake_case"
        );
        assert!(
            decode(r#"{"kind":"ping","id":1}"#).is_err(),
            "tag field is `type`"
        );
    }

    #[test]
    fn missing_field_is_an_error() {
        assert!(decode(r#"{"type":"ping"}"#).is_err());
        assert!(decode(r#"{"type":"publish","key":1,"service_port":1,"ping":false}"#).is_err());
        assert!(decode(r#"{"type":"nack"}"#).is_err());
    }

    #[test]
    fn wrong_field_type_is_an_error() {
        assert!(decode(r#"{"type":"ping","id":"1"}"#).is_err());
        assert!(decode(r#"{"type":"ping","id":-1}"#).is_err());
        assert!(decode(r#"{"type":"deliver","to":1,"text":null}"#).is_err());
        assert!(decode(r#"{"type":"claim","key":1,"bind_addr":1,"ping":false}"#).is_err());
    }

    #[test]
    fn trailing_data_is_an_error() {
        assert!(decode(r#"{"type":"collect"}{"type":"collect"}"#).is_err());
        assert!(decode(r#"{"type":"collect"} x"#).is_err());
        assert!(
            decode(r#"{"type":"collect"}  "#).is_ok(),
            "whitespace is fine"
        );
    }

    #[test]
    fn garbage_never_panics() {
        let deep = "[".repeat(100_000);
        for junk in [
            "",
            " ",
            "{",
            "}",
            "[]",
            "null",
            "42",
            "\"collect\"",
            "{\"type\":null}",
            "{\"type\":42}",
            "\u{0}",
            "{\"type\":\"collect\"",
            deep.as_str(),
        ] {
            assert!(decode(junk).is_err(), "{junk:?} should not decode");
        }
        assert!(serde_json::from_slice::<Message>(&[0xff, 0xfe, b'{']).is_err());
    }

    #[test]
    fn is_reply_partitions_the_variants() {
        for msg in all_variants() {
            let expected = matches!(
                msg.kind(),
                "registered" | "paired" | "heartbeat_ack" | "delivered" | "collected" | "nack"
            );
            assert_eq!(msg.is_reply(), expected, "{}", msg.kind());
        }
    }

    #[test]
    fn nack_helper_accepts_str_and_string() {
        let expected = Message::Nack {
            reason: "why".into(),
        };
        assert_eq!(Message::nack("why"), expected);
        assert_eq!(Message::nack(String::from("why")), expected);
    }
}
