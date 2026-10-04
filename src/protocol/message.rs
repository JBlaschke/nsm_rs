//! The tagged wire message enum.
//!
//! Every exchange in NSM is one request followed by exactly one reply, on
//! every transport: a length-prefixed frame each way on a TCP or TLS
//! connection (see [`codec`](super::codec)), or an HTTP request body and its
//! response body. The same [`Message`] JSON is used in both cases.
//!
//! A message is a JSON object whose `"type"` field is the snake_case variant
//! name, followed by the variant's fields, e.g.
//! `{"type":"publish","key":"42","service_port":9000,"bind_addr":"https://10.0.0.5:9001","ping":false}`.
//! Variants without fields are just the tag: `{"type":"collect"}`. The store
//! messages carry a [`StoreOp`] whose own fields sit next to `type`, and
//! [`Stored`](Message::Stored) carries the fields of a [`Stored`] record the
//! same way: `{"type":"store","op":"get","key":"step"}`. An unknown
//! tag, a missing field, a wrong type or trailing bytes are decode errors;
//! unknown fields are ignored so that a newer peer may add some.
//!
//! | Request | Direction | Reply |
//! |---|---|---|
//! | [`Publish`](Message::Publish) | service → broker | [`Registered`](Message::Registered) or [`Nack`](Message::Nack) |
//! | [`Claim`](Message::Claim) | client → broker | [`Paired`](Message::Paired) or [`Nack`](Message::Nack) |
//! | [`Ping`](Message::Ping) | ping-mode party → broker, with its token | [`Heartbeat`](Message::Heartbeat) or [`Nack`](Message::Nack) |
//! | [`Send`](Message::Send) | `send` → party | [`Delivered`](Message::Delivered) or [`Nack`](Message::Nack) |
//! | [`Deliver`](Message::Deliver) | party → broker (relay of a `Send`), with its token | [`Delivered`](Message::Delivered) or [`Nack`](Message::Nack) |
//! | [`Heartbeat`](Message::Heartbeat) | broker → party (two-sided liveness) | [`HeartbeatAck`](Message::HeartbeatAck) |
//! | [`Collect`](Message::Collect) | `collect` → party | [`Collected`](Message::Collected) |
//! | [`Store`](Message::Store) | `store` → party | [`Stored`](Message::Stored) or [`Nack`](Message::Nack) |
//! | [`StoreRelay`](Message::StoreRelay) | party → broker (relay of a `Store`), with its token | [`Stored`](Message::Stored) or [`Nack`](Message::Nack) |
//! | [`StoreByKey`](Message::StoreByKey) | `store --key` → broker, by rendezvous key | [`Stored`](Message::Stored) or [`Nack`](Message::Nack) |
//!
//! "Party" means a service or a client; each runs a small server on its
//! `bind_addr` that the broker (and the `send`, `collect` and `store`
//! operations) talk to. The broker is the only party with a fixed, well-known address.
//!
//! Registration returns a [`RegToken`]: a random secret that the party
//! quotes in every [`Ping`](Message::Ping), [`Deliver`](Message::Deliver) and
//! [`StoreRelay`](Message::StoreRelay),
//! and that the broker quotes in every [`Heartbeat`](Message::Heartbeat) it
//! sends. Party ids are small sequential integers and are not secrets; the
//! token is what stops a third party from acting on another party's behalf
//! or from spoofing its broker. [`StoreByKey`](Message::StoreByKey) carries
//! no token: the rendezvous key is the capability there, as it is for
//! [`Claim`](Message::Claim) and [`Publish`](Message::Publish).

use serde::{Deserialize, Serialize};

use super::types::{Key, PartyId, RegToken, Role, ServiceHandle, StoreOp, Stored};
use crate::net::Addr;

/// Version of the wire protocol described in this module.
///
/// Bumped on any change to [`Message`] or to the framing that an older peer
/// could not decode. Adding an optional field or a new variant does not
/// require a bump.
///
/// | Version | Change |
/// |---|---|
/// | 1 | the first versioned format |
/// | 2 | [`Collected`](Message::Collected) carries the answering party's `role` |
/// | 3 | [`Deliver`](Message::Deliver) names no target: the broker delivers to the sender's peer, so text flows both ways |
/// | 4 | the rendezvous [`Key`] is a string (`key` of [`Publish`](Message::Publish) and [`Claim`](Message::Claim), `rendezvous` of [`StoreByKey`](Message::StoreByKey), `nsm_key` of a [`MeshData`](super::MeshData)); an unsigned integer, what version 3 sent, still decodes as its decimal text |
///
/// Version 3 also carries [`Store`](Message::Store),
/// [`StoreRelay`](Message::StoreRelay) and [`Stored`](Message::Stored), added
/// later as new variants, the optional `if_version` of a put or a delete
/// with `applied` on `stored`, added later still as fields that decode when
/// absent, and [`StoreByKey`](Message::StoreByKey), one more variant; none
/// needs a bump.
pub const PROTOCOL_VERSION: u16 = 4;

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

    /// Hand `text` to a party for its peer.
    ///
    /// Sent by the `send` operation to either party's bind address: a
    /// client's, for its service, or a service's, for the client holding
    /// it. The party relays the text to the broker as [`Message::Deliver`]
    /// and passes the broker's answer back.
    ///
    /// Reply: [`Message::Delivered`], or [`Message::Nack`] when the party has
    /// not registered yet or, from the broker, when it has no peer (a
    /// service no client holds).
    Send {
        /// The text itself, carried verbatim (no extra JSON encoding).
        text: String,
    },

    /// Carry `text` to the sender's peer, to be delivered in the peer's next
    /// heartbeat.
    ///
    /// Sent by a party to the broker when relaying a [`Message::Send`]. The
    /// party identifies itself with `from` and its `token`; the broker knows
    /// who its peer is (a client's current service, the client holding a
    /// service) and no target is named on the wire, so a text that races a
    /// re-pairing reaches the new service instead of being refused. The
    /// broker stores the text as the peer's pending `inbox` and hands it
    /// over in the peer's next [`Message::Heartbeat`] (or in the reply to
    /// its next [`Message::Ping`]); `collect` on the peer then returns it. A
    /// later `Deliver` before the hand-over replaces the text.
    ///
    /// Reply: [`Message::Delivered`], or [`Message::Nack`] when `from`/`token`
    /// do not name a registered party, or the party has no peer (a service
    /// no client holds; a client whose service died and that has not been
    /// re-paired yet).
    Deliver {
        /// Id of the relaying party.
        from: PartyId,
        /// The relaying party's registration token.
        token: RegToken,
        /// The text itself, carried verbatim (no extra JSON encoding).
        text: String,
    },

    /// Apply `op` to the store the relaying party shares with its peer.
    ///
    /// Sent by a party to the broker when relaying a [`Message::Store`]. The
    /// party identifies itself with `from` and its `token`, and names no
    /// store: the broker keeps one store per claim and finds it from `from`,
    /// as it finds the peer for [`Message::Deliver`]. A client uses its own
    /// claim's store, also between losing its service and being re-paired;
    /// a service uses the store of the client holding it. A service nobody
    /// holds reads an empty store (`client: null`) and may not write.
    ///
    /// Reply: [`Message::Stored`], with `applied: false` when a put or a
    /// delete stated an `if_version` that did not match (see
    /// [`StoreOp`]), or [`Message::Nack`] when `from`/`token` do not name a
    /// registered party, a service nobody holds tries to write, or a put
    /// does not fit the store's budget.
    StoreRelay {
        /// Id of the relaying party.
        from: PartyId,
        /// The relaying party's registration token.
        token: RegToken,
        /// The operation, its fields next to `type` on the wire.
        #[serde(flatten)]
        op: StoreOp,
    },

    /// Apply `op` to the store of the claim under a rendezvous key.
    ///
    /// Sent by the `store` operation to the broker when the operator knows
    /// the key and the broker's address but not where the parties listen
    /// (decision D27). The broker resolves the key
    /// to one party and answers as if that party had relayed the operation:
    /// the one client under the key, so its claim's store; when no client
    /// is under it, the one service, which reads an empty store and may not
    /// write; with `party_id`, that party, which must be under the key. A
    /// key with two or more clients, or with no client and two or more
    /// services, is refused with the candidates listed. No token travels:
    /// the rendezvous key is the capability, as it is for
    /// [`Message::Claim`] and [`Message::Publish`].
    ///
    /// Reply: [`Message::Stored`] or [`Message::Nack`], as for
    /// [`Message::StoreRelay`], plus the refusals of the resolution (`no
    /// party under key 1234`, `key 1234 has 2 clients (4, 7); name one with
    /// party_id`, `no party 7 under key 1234`).
    StoreByKey {
        /// The rendezvous key of the claim.
        rendezvous: Key,
        /// One party of the key, a client or a service, when the key has
        /// more than one claim.
        #[serde(default)]
        party_id: Option<PartyId>,
        /// The operation, its fields next to `type` on the wire.
        #[serde(flatten)]
        op: StoreOp,
    },

    // ----- broker → party ---------------------------------------------------
    /// Two-sided heartbeat: the broker checks that a party is alive and
    /// delivers whatever is pending for it.
    ///
    /// Sent by the broker to a party's bind address at the heartbeat
    /// interval, and also returned by the broker as the reply to a
    /// [`Message::Ping`]. `inbox` is text the party has been sent (see
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
        /// Text pending for the party, delivered once.
        inbox: Option<String>,
        /// A client's new service after a re-pairing.
        service: Option<ServiceHandle>,
    },

    // ----- requests to a party ----------------------------------------------
    /// Ask a party what it holds.
    ///
    /// Sent by the `collect` operation to a party's bind address.
    ///
    /// Reply: [`Message::Collected`], which names the party's role. Either
    /// party answers with the last text it received in a heartbeat (`text`,
    /// `None` if nothing was ever delivered); a client also answers with the
    /// handle of the service it is paired with (`service`).
    Collect,

    /// Apply `op` to the store a party shares with its peer.
    ///
    /// Meant for either party's bind address. The party keeps no copy: it
    /// relays the operation to the broker as [`Message::StoreRelay`] and
    /// passes the broker's answer back.
    ///
    /// Reply: [`Message::Stored`], passed back unchanged, or
    /// [`Message::Nack`] when the party has not registered yet or the broker
    /// refused the operation.
    Store {
        /// The operation, its fields next to `type` on the wire.
        #[serde(flatten)]
        op: StoreOp,
    },

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

    /// Reply to [`Message::Collect`]: what the party holds, and which kind
    /// of party it is. Both kinds fill `text` with the last text delivered
    /// to them (`None` before the first). A client (`role: client`) also
    /// fills `service` with its current pairing (`None` only in the moment
    /// between binding its listener and registering); a service
    /// (`role: service`) leaves it empty. The role tells the asker which
    /// fields apply, so it never has to guess from what is set.
    Collected {
        /// Which kind of party answered.
        role: Role,
        /// Last text delivered to the party.
        text: Option<String>,
        /// The service a client is paired with.
        service: Option<ServiceHandle>,
    },

    /// Reply to [`Message::StoreRelay`] and [`Message::Store`]: whose store
    /// it is, its revision, whether a write was applied and the entries the
    /// operation returns. The fields of the [`Stored`] record sit next to
    /// `type` on the wire:
    /// `{"type":"stored","client":2,"revision":3,"applied":true,"entries":[...]}`.
    Stored(Stored),

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
                | Message::Stored(_)
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
            Message::StoreRelay { .. } => "store_relay",
            Message::StoreByKey { .. } => "store_by_key",
            Message::Collect => "collect",
            Message::Store { .. } => "store",
            Message::Registered { .. } => "registered",
            Message::Paired { .. } => "paired",
            Message::HeartbeatAck { .. } => "heartbeat_ack",
            Message::Delivered => "delivered",
            Message::Collected { .. } => "collected",
            Message::Stored(_) => "stored",
            Message::Nack { .. } => "nack",
        }
    }
}

impl From<Stored> for Message {
    /// The [`Message::Stored`] reply carrying `stored`.
    fn from(stored: Stored) -> Self {
        Message::Stored(stored)
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
    use super::types::StoreEntry;
    use crate::net::Transport;

    let handle = ServiceHandle {
        id: PartyId(3),
        host: "fe80::1".into(),
        service_port: 9000,
    };
    let token = test_token();
    vec![
        Message::Publish {
            key: Key::from(42),
            service_port: 9000,
            bind_addr: Addr::new(Transport::Https, "10.0.0.5", 9001),
            ping: false,
        },
        Message::Claim {
            key: Key::from(42),
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
            role: Role::Client,
            text: Some("last".into()),
            service: Some(handle),
        },
        Message::StoreRelay {
            from: PartyId(4),
            token,
            op: StoreOp::Put {
                key: store_key("step"),
                value: "5 \"quoted\"\n".into(),
                if_version: Some(8),
            },
        },
        Message::Store {
            op: StoreOp::Delete {
                key: store_key("input/path"),
                if_version: Some(0),
            },
        },
        Message::StoreByKey {
            rendezvous: Key::from(42),
            party_id: Some(PartyId(4)),
            op: StoreOp::Get {
                key: store_key("nsm_mesh_data"),
            },
        },
        Message::Stored(Stored {
            client: Some(PartyId(4)),
            revision: 9,
            applied: false,
            entries: vec![StoreEntry {
                key: store_key("step"),
                value: String::new(),
                version: 9,
            }],
        }),
        Message::nack("no service available for key 42"),
    ]
}

/// A store key from a literal known to be valid, for tests.
#[cfg(test)]
pub(crate) fn store_key(text: &str) -> super::types::StoreKey {
    text.parse()
        .unwrap_or_else(|e| panic!("{text:?} is not a store key: {e}"))
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
        assert_eq!(kinds.len(), 17, "a variant was added; extend all_variants");
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
            key: Key::from(42),
            service_port: 9000,
            bind_addr: Addr::new(Transport::Https, "10.0.0.5", 9001),
            ping: false,
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            concat!(
                r#"{"type":"publish","key":"42","service_port":9000,"#,
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
            decode(r#"{"type":"collected","role":"service","text":null}"#).unwrap(),
            Message::Collected {
                role: Role::Service,
                text: None,
                service: None,
            }
        );
        // The role is not optional: a version-1 reply does not decode.
        assert!(decode(r#"{"type":"collected","text":null,"service":null}"#).is_err());
    }

    #[test]
    fn collected_json_shape_names_the_role() {
        let from_service = Message::Collected {
            role: Role::Service,
            text: Some("job 17".into()),
            service: None,
        };
        assert_eq!(
            serde_json::to_string(&from_service).unwrap(),
            r#"{"type":"collected","role":"service","text":"job 17","service":null}"#
        );
        let from_client = decode(
            r#"{"type":"collected","role":"client","service":{"id":7,"host":"h","service_port":2}}"#,
        )
        .unwrap();
        assert_eq!(
            from_client,
            Message::Collected {
                role: Role::Client,
                text: None,
                service: Some(ServiceHandle {
                    id: PartyId(7),
                    host: "h".into(),
                    service_port: 2,
                }),
            }
        );
        assert!(decode(r#"{"type":"collected","role":"claimer","text":null}"#).is_err());
        assert!(decode(r#"{"type":"collected","role":1,"text":null}"#).is_err());
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
        let hex = "07".repeat(16);
        assert!(
            decode(&format!(
                r#"{{"type":"deliver","from":1,"token":"{hex}","text":1}}"#
            ))
            .is_err()
        );
        assert!(decode(r#"{"type":"claim","key":1,"bind_addr":1,"ping":false}"#).is_err());
    }

    #[test]
    fn deliver_names_no_target() {
        let hex = "07".repeat(16);
        let msg = decode(&format!(
            r#"{{"type":"deliver","from":2,"token":"{hex}","text":"job 17"}}"#
        ))
        .unwrap();
        assert_eq!(
            msg,
            Message::Deliver {
                from: PartyId(2),
                token: test_token(),
                text: "job 17".into(),
            }
        );
        assert!(!serde_json::to_string(&msg).unwrap().contains("\"to\""));
        // A version-2 deliver still decodes: its target is an unknown field.
        assert_eq!(
            decode(&format!(
                r#"{{"type":"deliver","from":2,"token":"{hex}","to":1,"text":"job 17"}}"#
            ))
            .unwrap(),
            msg
        );
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
                "registered"
                    | "paired"
                    | "heartbeat_ack"
                    | "delivered"
                    | "collected"
                    | "stored"
                    | "nack"
            );
            assert_eq!(msg.is_reply(), expected, "{}", msg.kind());
        }
    }

    #[test]
    fn store_json_shape_puts_the_operation_next_to_the_type() {
        let cases = [
            (
                StoreOp::Put {
                    key: store_key("step"),
                    value: "5".into(),
                    if_version: None,
                },
                r#"{"type":"store","op":"put","key":"step","value":"5","if_version":null}"#,
            ),
            (
                StoreOp::Put {
                    key: store_key("step"),
                    value: "5".into(),
                    if_version: Some(0),
                },
                r#"{"type":"store","op":"put","key":"step","value":"5","if_version":0}"#,
            ),
            (
                StoreOp::Get {
                    key: store_key("step"),
                },
                r#"{"type":"store","op":"get","key":"step"}"#,
            ),
            (
                StoreOp::Delete {
                    key: store_key("step"),
                    if_version: None,
                },
                r#"{"type":"store","op":"delete","key":"step","if_version":null}"#,
            ),
            (
                StoreOp::Delete {
                    key: store_key("step"),
                    if_version: Some(7),
                },
                r#"{"type":"store","op":"delete","key":"step","if_version":7}"#,
            ),
            (StoreOp::List, r#"{"type":"store","op":"list"}"#),
        ];
        for (op, json) in cases {
            let msg = Message::Store { op };
            assert_eq!(serde_json::to_string(&msg).unwrap(), json);
            assert_eq!(decode(json).unwrap(), msg, "{json}");
        }
        // Field order does not matter, and the tag may come last.
        assert_eq!(
            decode(r#"{"value":"","if_version":3,"key":"k","op":"put","type":"store"}"#).unwrap(),
            Message::Store {
                op: StoreOp::Put {
                    key: store_key("k"),
                    value: String::new(),
                    if_version: Some(3),
                },
            }
        );
        // Like every `Option`, a missing condition decodes as `None`: a write
        // from a peer that predates conditional writes is unconditional.
        for (json, op) in [
            (
                r#"{"type":"store","op":"put","key":"k","value":"v"}"#,
                StoreOp::Put {
                    key: store_key("k"),
                    value: "v".into(),
                    if_version: None,
                },
            ),
            (
                r#"{"type":"store","op":"delete","key":"k"}"#,
                StoreOp::Delete {
                    key: store_key("k"),
                    if_version: None,
                },
            ),
        ] {
            assert_eq!(decode(json).unwrap(), Message::Store { op }, "{json}");
        }
    }

    #[test]
    fn store_relay_json_shape() {
        let hex = "07".repeat(16);
        let msg = Message::StoreRelay {
            from: PartyId(2),
            token: test_token(),
            op: StoreOp::Put {
                key: store_key("step"),
                value: "5".into(),
                if_version: Some(4),
            },
        };
        let json = format!(
            r#"{{"type":"store_relay","from":2,"token":"{hex}","op":"put","key":"step","value":"5","if_version":4}}"#
        );
        assert_eq!(serde_json::to_string(&msg).unwrap(), json);
        assert_eq!(decode(&json).unwrap(), msg);
        let unconditional = Message::StoreRelay {
            from: PartyId(2),
            token: test_token(),
            op: StoreOp::Delete {
                key: store_key("step"),
                if_version: None,
            },
        };
        let json = format!(
            r#"{{"type":"store_relay","from":2,"token":"{hex}","op":"delete","key":"step","if_version":null}}"#
        );
        assert_eq!(serde_json::to_string(&unconditional).unwrap(), json);
        assert_eq!(decode(&json).unwrap(), unconditional);
        assert_eq!(
            decode(&format!(
                r#"{{"type":"store_relay","from":2,"token":"{hex}","op":"delete","key":"step"}}"#
            ))
            .unwrap(),
            unconditional
        );
        let list = Message::StoreRelay {
            from: PartyId(2),
            token: test_token(),
            op: StoreOp::List,
        };
        assert_eq!(
            serde_json::to_string(&list).unwrap(),
            format!(r#"{{"type":"store_relay","from":2,"token":"{hex}","op":"list"}}"#)
        );
    }

    #[test]
    fn store_by_key_json_shape_keeps_the_two_keys_apart() {
        // The rendezvous key is `rendezvous`; the store key stays `key`,
        // next to `type` with the rest of the operation.
        let msg = Message::StoreByKey {
            rendezvous: Key::from(1234),
            party_id: Some(PartyId(7)),
            op: StoreOp::Get {
                key: store_key("step"),
            },
        };
        let json =
            r#"{"type":"store_by_key","rendezvous":"1234","party_id":7,"op":"get","key":"step"}"#;
        assert_eq!(serde_json::to_string(&msg).unwrap(), json);
        assert_eq!(decode(json).unwrap(), msg);
        let any = Message::StoreByKey {
            rendezvous: Key::from(1234),
            party_id: None,
            op: StoreOp::Put {
                key: store_key("step"),
                value: "5".into(),
                if_version: Some(0),
            },
        };
        let json = r#"{"type":"store_by_key","rendezvous":"1234","party_id":null,"op":"put","key":"step","value":"5","if_version":0}"#;
        assert_eq!(serde_json::to_string(&any).unwrap(), json);
        assert_eq!(decode(json).unwrap(), any);
        // Like every Option, a missing party_id is none; the rendezvous
        // key and the operation are required, and it carries no token.
        assert_eq!(
            decode(r#"{"type":"store_by_key","rendezvous":"1234","op":"list"}"#).unwrap(),
            Message::StoreByKey {
                rendezvous: Key::from(1234),
                party_id: None,
                op: StoreOp::List,
            }
        );
        for bad in [
            r#"{"type":"store_by_key","op":"list"}"#,
            r#"{"type":"store_by_key","rendezvous":"a b","op":"list"}"#,
            r#"{"type":"store_by_key","rendezvous":"","op":"list"}"#,
            r#"{"type":"store_by_key","rendezvous":-1,"op":"list"}"#,
            r#"{"type":"store_by_key","rendezvous":"1234"}"#,
            r#"{"type":"store_by_key","rendezvous":"1234","party_id":-1,"op":"list"}"#,
            r#"{"type":"store_by_key","rendezvous":"1234","op":"get"}"#,
        ] {
            assert!(decode(bad).is_err(), "{bad} should not decode");
        }
        assert!(!serde_json::to_string(&any).unwrap().contains("token"));
    }

    #[test]
    fn rendezvous_keys_are_strings_on_the_wire_and_integers_still_decode() {
        // Protocol version 4: the key travels as a string. An unsigned
        // integer, what earlier parties sent, decodes as its decimal text
        // and so names the same key (decision D30).
        let publish = Message::Publish {
            key: Key::from(1234),
            service_port: 9000,
            bind_addr: Addr::tcp("10.0.0.5", 12010),
            ping: false,
        };
        let json = serde_json::to_string(&publish).unwrap();
        assert!(json.contains(r#""key":"1234""#), "{json}");
        assert_eq!(decode(&json).unwrap(), publish);
        let legacy = json.replace(r#""key":"1234""#, r#""key":1234"#);
        assert_ne!(legacy, json);
        assert_eq!(decode(&legacy).unwrap(), publish);
        let text = Message::Claim {
            key: "job-17/step.2:a".parse().unwrap(),
            bind_addr: Addr::tcp("10.0.0.6", 12020),
            ping: true,
        };
        let json = serde_json::to_string(&text).unwrap();
        assert!(json.contains(r#""key":"job-17/step.2:a""#), "{json}");
        assert_eq!(decode(&json).unwrap(), text);
        for bad in [
            r#"{"type":"claim","key":"","bind_addr":"10.0.0.6:1","ping":false}"#,
            r#"{"type":"claim","key":"a b","bind_addr":"10.0.0.6:1","ping":false}"#,
            r#"{"type":"claim","key":-1,"bind_addr":"10.0.0.6:1","ping":false}"#,
            r#"{"type":"claim","key":1.5,"bind_addr":"10.0.0.6:1","ping":false}"#,
            r#"{"type":"claim","key":null,"bind_addr":"10.0.0.6:1","ping":false}"#,
            r#"{"type":"publish","key":true,"service_port":1,"bind_addr":"10.0.0.6:1","ping":false}"#,
        ] {
            let err = decode(bad).unwrap_err();
            assert!(err.to_string().contains("rendezvous key"), "{bad}: {err}");
        }
    }

    #[test]
    fn stored_json_shape_carries_the_record_inline() {
        use super::super::types::StoreEntry;
        let msg = Message::from(Stored {
            client: Some(PartyId(2)),
            revision: 3,
            applied: true,
            entries: vec![StoreEntry {
                key: store_key("step"),
                value: "5".into(),
                version: 3,
            }],
        });
        let json = r#"{"type":"stored","client":2,"revision":3,"applied":true,"entries":[{"key":"step","value":"5","version":3}]}"#;
        assert_eq!(serde_json::to_string(&msg).unwrap(), json);
        assert_eq!(decode(json).unwrap(), msg);
        // `applied` is always sent, and a reply without it decodes as
        // applied: the field was added compatibly.
        assert_eq!(
            decode(r#"{"type":"stored","client":2,"revision":3,"entries":[{"key":"step","value":"5","version":3}]}"#)
                .unwrap(),
            msg
        );
        let refused = Message::from(Stored {
            client: Some(PartyId(2)),
            revision: 5,
            applied: false,
            entries: vec![],
        });
        let json = r#"{"type":"stored","client":2,"revision":5,"applied":false,"entries":[]}"#;
        assert_eq!(serde_json::to_string(&refused).unwrap(), json);
        assert_eq!(decode(json).unwrap(), refused);

        let empty = r#"{"type":"stored","client":null,"revision":0,"applied":true,"entries":[]}"#;
        let unclaimed = Message::Stored(Stored {
            client: None,
            revision: 0,
            applied: true,
            entries: vec![],
        });
        assert_eq!(serde_json::to_string(&unclaimed).unwrap(), empty);
        assert_eq!(decode(empty).unwrap(), unclaimed);
        // Like every `Option`, a missing client decodes as `None`.
        assert_eq!(
            decode(r#"{"type":"stored","revision":0,"entries":[]}"#).unwrap(),
            unclaimed
        );
        // The largest ids and versions survive the round trip.
        let max = format!(
            r#"{{"type":"stored","client":{m},"revision":{m},"entries":[{{"key":"k","value":"v","version":{m}}}]}}"#,
            m = u64::MAX
        );
        match decode(&max).unwrap() {
            Message::Stored(stored) => {
                assert_eq!(stored.client, Some(PartyId(u64::MAX)));
                assert_eq!(stored.revision, u64::MAX);
                assert_eq!(stored.entries[0].version, u64::MAX);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn malformed_store_messages_do_not_decode() {
        let hex = "07".repeat(16);
        let too_long = "k".repeat(129);
        let mut cases: Vec<String> = [
            "",
            "-step",
            "a b",
            "a\\n",
            "\\u00e9",
            "a*",
            too_long.as_str(),
        ]
        .iter()
        .map(|key| format!(r#"{{"type":"store","op":"get","key":"{key}"}}"#))
        .collect();
        cases.extend(
            [
                r#"{"type":"store"}"#,
                r#"{"type":"store","op":"increment","key":"k"}"#,
                r#"{"type":"store","op":"Get","key":"k"}"#,
                r#"{"type":"store","op":"put","key":"k"}"#,
                r#"{"type":"store","op":"put","value":"v"}"#,
                r#"{"type":"store","op":"put","key":"k","value":5}"#,
                r#"{"type":"store","op":"get","key":7}"#,
                r#"{"type":"store","op":"get"}"#,
                r#"{"type":"store","op":null}"#,
                r#"{"type":"store","op":{"op":"list"}}"#,
                r#"{"type":"stored","client":1,"entries":[]}"#,
                r#"{"type":"stored","client":1,"revision":0}"#,
                r#"{"type":"stored","client":1,"revision":-1,"entries":[]}"#,
                r#"{"type":"stored","client":1,"revision":0,"entries":[{"key":"k","value":"v"}]}"#,
                r#"{"type":"stored","client":1,"revision":0,"entries":[{"key":"-k","value":"v","version":1}]}"#,
                r#"{"type":"stored","client":1,"revision":0,"applied":"yes","entries":[]}"#,
                r#"{"type":"store","op":"put","key":"k","value":"v","if_version":-1}"#,
                r#"{"type":"store","op":"delete","key":"k","if_version":"1"}"#,
            ]
            .map(str::to_owned),
        );
        // A relay without credentials, or with a malformed token.
        cases.extend([
            format!(r#"{{"type":"store_relay","token":"{hex}","op":"list"}}"#),
            r#"{"type":"store_relay","from":2,"op":"list"}"#.to_owned(),
            r#"{"type":"store_relay","from":2,"token":"short","op":"list"}"#.to_owned(),
            format!(r#"{{"type":"store_relay","from":"2","token":"{hex}","op":"list"}}"#),
            format!(r#"{{"type":"store_relay","from":2,"token":"{hex}"}}"#),
        ]);
        for json in &cases {
            assert!(decode(json).is_err(), "{json} should not decode");
        }
    }

    #[test]
    fn store_messages_ignore_unknown_fields() {
        let hex = "07".repeat(16);
        assert_eq!(
            decode(r#"{"type":"store","op":"list","key":"ignored","extra":[1,2]}"#).unwrap(),
            Message::Store { op: StoreOp::List }
        );
        assert_eq!(
            decode(&format!(
                r#"{{"type":"store_relay","from":2,"token":"{hex}","op":"get","key":"k","if_version":3}}"#
            ))
            .unwrap(),
            Message::StoreRelay {
                from: PartyId(2),
                token: test_token(),
                op: StoreOp::Get {
                    key: store_key("k"),
                },
            }
        );
        match decode(r#"{"type":"stored","client":1,"revision":0,"entries":[],"writer":"client"}"#)
            .unwrap()
        {
            Message::Stored(stored) => assert!(stored.entries.is_empty() && stored.applied),
            other => panic!("{other:?}"),
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
