//! The operations, written once for the CLI and the REST control plane.
//!
//! Each function takes a typed request, does no printing and returns typed
//! results or [`Error`]s; the caller decides how to present them. Long-running
//! operations (`listen`, `publish`, `claim`) return a handle whose `run`
//! future the caller awaits, so the same code serves a foreground CLI process
//! and a background job in the control plane.

use std::net::IpAddr;

use tokio_util::sync::CancellationToken;

use crate::broker::admin::AdminOpts;
use crate::broker::listen::{BrokerHandle, ListenOpts, listen as start_broker};
use crate::config::{BrokerPolicy, Limits, Timing, TlsPaths};
use crate::net::{Addr, IpVersion, LocalAddr, Selector, Transport, interfaces};
use crate::party::{ClaimOpts, PartyOpts, PublishOpts, Session};
use crate::protocol::{Key, Message, Role, ServiceHandle};
use crate::transport::Client;
use crate::{Error, Result};

pub use crate::protocol::{StoreEntry, StoreKey, StoreOp, Stored};

/// Names of the interfaces carrying an address of the given family (any
/// family when `None`).
pub fn list_interfaces(version: Option<IpVersion>) -> Result<Vec<String>> {
    let addrs = interfaces::local_addrs()?;
    Ok(interfaces::interface_names(&addrs, version))
}

/// Local addresses passing the selector.
pub fn list_ips(selector: &Selector) -> Result<Vec<LocalAddr>> {
    let addrs = interfaces::local_addrs()?;
    Ok(selector.filter(&addrs))
}

/// The one local address a party advertises; an error listing the
/// candidates when the selector is ambiguous.
pub fn select_local_ip(selector: &Selector) -> Result<IpAddr> {
    let addrs = interfaces::local_addrs()?;
    Ok(selector.select_one(&addrs)?.ip)
}

/// Settings shared by every operation that talks to the network.
#[derive(Debug, Clone, Default)]
pub struct NetOpts {
    /// Certificate material: identity for TLS listeners, roots for dialling.
    pub tls: TlsPaths,
    /// Intervals and timeouts.
    pub timing: Timing,
    /// Size and count limits.
    pub limits: Limits,
}

impl NetOpts {
    fn client(&self) -> Client {
        Client::new(self.tls.clone(), self.timing.clone(), self.limits.clone())
    }
}

/// Run a broker.
#[derive(Debug, Clone)]
pub struct ListenRequest {
    /// Wire to serve; `tls`/`https` need a certificate and key.
    pub transport: Transport,
    /// Port to listen on (0 picks a free port).
    pub bind_port: u16,
    /// Which local address to bind.
    pub selector: Selector,
    /// Network settings.
    pub net: NetOpts,
    /// Admission policy.
    pub policy: BrokerPolicy,
    /// The admin listener (`/metrics`, `/v1/status`), when wanted.
    pub admin: Option<AdminOpts>,
}

/// Bind the broker's listener and start its monitor; the returned handle
/// runs until `shutdown` is cancelled.
pub async fn listen(req: ListenRequest, shutdown: CancellationToken) -> Result<BrokerHandle> {
    let ip = select_local_ip(&req.selector)?;
    start_broker(
        ListenOpts {
            bind: Addr::new(req.transport, ip.to_string(), req.bind_port),
            tls: req.net.tls,
            timing: req.net.timing,
            limits: req.net.limits,
            policy: req.policy,
            admin: req.admin,
        },
        shutdown,
    )
    .await
}

/// Register a service with a broker and keep it registered.
#[derive(Debug, Clone)]
pub struct PublishRequest {
    /// Broker address; its transport family is also this party's.
    pub broker: Addr,
    /// Rendezvous key.
    pub key: Key,
    /// Heartbeat port to bind (0 picks a free port).
    pub bind_port: u16,
    /// Port the service itself listens on.
    pub service_port: u16,
    /// Which local address to advertise.
    pub selector: Selector,
    /// Serve TLS on the heartbeat listener.
    pub serve_tls: bool,
    /// One-sided liveness.
    pub ping: bool,
    /// Network settings.
    pub net: NetOpts,
}

/// Bind the heartbeat listener, register, and return the session to run.
pub async fn publish(req: PublishRequest) -> Result<Session> {
    let local_ip = select_local_ip(&req.selector)?;
    Session::publish(PublishOpts {
        party: PartyOpts {
            broker: req.broker,
            key: req.key,
            local_ip,
            bind_port: req.bind_port,
            serve_tls: req.serve_tls,
            ping: req.ping,
            tls: req.net.tls,
            timing: req.net.timing,
            limits: req.net.limits,
        },
        service_port: req.service_port,
    })
    .await
}

/// Pair with a service and stay paired.
#[derive(Debug, Clone)]
pub struct ClaimRequest {
    /// Broker address; its transport family is also this party's.
    pub broker: Addr,
    /// Rendezvous key of the wanted service.
    pub key: Key,
    /// Heartbeat port to bind (0 picks a free port).
    pub bind_port: u16,
    /// Which local address to advertise.
    pub selector: Selector,
    /// Serve TLS on the heartbeat listener.
    pub serve_tls: bool,
    /// One-sided liveness.
    pub ping: bool,
    /// Network settings.
    pub net: NetOpts,
}

/// Bind the heartbeat listener, claim, and return the session to run; the
/// pairing is [`Session::service`].
pub async fn claim(req: ClaimRequest) -> Result<Session> {
    let local_ip = select_local_ip(&req.selector)?;
    Session::claim(ClaimOpts {
        party: PartyOpts {
            broker: req.broker,
            key: req.key,
            local_ip,
            bind_port: req.bind_port,
            serve_tls: req.serve_tls,
            ping: req.ping,
            tls: req.net.tls,
            timing: req.net.timing,
            limits: req.net.limits,
        },
    })
    .await
}

/// What a party holds, as returned by [`collect`].
///
/// The variant is the party's role, and each variant carries only what that
/// role holds, so a caller never has to guess which field applies. For the
/// control plane it serialises with the role as a `role` tag next to the
/// variant's fields: `{"role":"service","text":"job 17"}` or
/// `{"role":"client","service":{...},"text":"ready"}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Collected {
    /// The party is a service.
    Service {
        /// The last text delivered to it; `None` before the first.
        text: Option<String>,
    },
    /// The party is a client.
    Client {
        /// The service it is paired with; `None` only in the moment between
        /// binding its listener and registering.
        service: Option<ServiceHandle>,
        /// The last text its service sent it; `None` before the first.
        text: Option<String>,
    },
}

impl Collected {
    /// The last text the party received from its peer: `None` before the
    /// first delivery. Both roles receive text, so this never refuses.
    pub fn text(self) -> Option<String> {
        match self {
            Collected::Service { text } | Collected::Client { text, .. } => text,
        }
    }

    /// The service a client is paired with: `None` only before it has
    /// registered. An [`Error::WrongRole`] when the party is a service,
    /// which has no peer.
    pub fn service(self) -> Result<Option<ServiceHandle>> {
        match self {
            Collected::Client { service, .. } => Ok(service),
            Collected::Service { .. } => Err(Error::WrongRole {
                role: Role::Service,
                hint: "only a client has a peer",
            }),
        }
    }
}

/// Ask a party (by its heartbeat address) what it holds.
pub async fn collect(party: &Addr, net: &NetOpts) -> Result<Collected> {
    match net.client().call(party, Message::Collect).await? {
        Message::Collected {
            role: Role::Service,
            text,
            ..
        } => Ok(Collected::Service { text }),
        Message::Collected {
            role: Role::Client,
            text,
            service,
        } => Ok(Collected::Client { service, text }),
        Message::Nack { reason } => Err(Error::Rejected(reason)),
        other => Err(Error::protocol(format!(
            "collect answered with {}",
            other.kind()
        ))),
    }
}

/// Hand `text` to a party (by its heartbeat address) for its peer: a
/// client's service, or the client holding a service.
pub async fn send(party: &Addr, text: String, net: &NetOpts) -> Result<()> {
    match net.client().call(party, Message::Send { text }).await? {
        Message::Delivered => Ok(()),
        Message::Nack { reason } => Err(Error::Rejected(reason)),
        other => Err(Error::protocol(format!(
            "send answered with {}",
            other.kind()
        ))),
    }
}

/// Apply `op` to the store a party shares with its peer, through the party
/// at `party` (its heartbeat address, of either role), which relays it to the
/// broker with its token.
///
/// The answer names the claim's client and carries what the operation
/// returns; a key that is not set is an answer with no entry, not an error.
/// So is a put or a delete whose `if_version` did not match: the answer has
/// `applied: false` and the key's current entry, or none when it is not set.
/// A refusal (the party is not registered yet, a service nobody holds tried
/// to write, the store is full, the party's registration is gone) is an
/// [`Error::Rejected`] with the reason.
///
/// Only a write that stated a condition can miss, so an answer with
/// `applied: false` to any other operation is an [`Error::Protocol`]: the
/// front-ends can then read `applied: false` as "the condition did not
/// hold" without checking what was asked.
pub async fn store(party: &Addr, op: StoreOp, net: &NetOpts) -> Result<Stored> {
    let kind = op.kind();
    let conditional = op.if_version().is_some();
    match net.client().call(party, Message::Store { op }).await? {
        Message::Stored(stored) if !stored.applied && !conditional => Err(Error::protocol(
            format!("the {kind} was answered as not applied although it stated no condition"),
        )),
        Message::Stored(stored) => Ok(stored),
        Message::Nack { reason } => Err(Error::Rejected(reason)),
        other => Err(Error::protocol(format!(
            "store answered with {}",
            other.kind()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PartyId;

    #[test]
    fn collected_serialises_with_the_role_as_the_tag() {
        let from_service = Collected::Service {
            text: Some("job 17".into()),
        };
        assert_eq!(
            serde_json::to_string(&from_service).unwrap(),
            r#"{"role":"service","text":"job 17"}"#
        );
        let from_client = Collected::Client {
            service: None,
            text: None,
        };
        assert_eq!(
            serde_json::to_string(&from_client).unwrap(),
            r#"{"role":"client","service":null,"text":null}"#
        );
        let handle = ServiceHandle {
            id: PartyId(1),
            host: "h".into(),
            service_port: 2,
        };
        assert_eq!(
            serde_json::from_str::<Collected>(
                r#"{"role":"client","service":{"id":1,"host":"h","service_port":2},"text":"ready"}"#
            )
            .unwrap(),
            Collected::Client {
                service: Some(handle),
                text: Some("ready".into()),
            }
        );
        // Without the tag there is no way to tell what applies.
        assert!(serde_json::from_str::<Collected>(r#"{"text":"x","service":null}"#).is_err());
    }

    #[test]
    fn accessors_answer_by_role() {
        let handle = ServiceHandle {
            id: PartyId(1),
            host: "h".into(),
            service_port: 2,
        };
        let service = || Collected::Service {
            text: Some("job 17".into()),
        };
        let client = || Collected::Client {
            service: Some(handle.clone()),
            text: Some("ready".into()),
        };
        // Text is held by either role.
        assert_eq!(service().text().as_deref(), Some("job 17"));
        assert_eq!(client().text().as_deref(), Some("ready"));
        assert_eq!(Collected::Service { text: None }.text(), None);
        // A peer only by a client; "nothing yet" is `None`, not an error.
        assert_eq!(client().service().unwrap(), Some(handle.clone()));
        assert_eq!(
            Collected::Client {
                service: None,
                text: None
            }
            .service()
            .unwrap(),
            None
        );
        match service().service() {
            Err(Error::WrongRole { role, .. }) => assert_eq!(role, Role::Service),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn store_maps_a_nack_to_rejected_and_other_replies_to_protocol_errors() {
        use crate::transport::testing::{Echo, PeerReporter, start};
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let net = NetOpts {
                timing: Timing::fast(),
                ..NetOpts::default()
            };
            // A listener that answers every request with a nack.
            let (server, _, _) = start(Transport::Tcp, PeerReporter).await;
            let err = store(&server.bound(), StoreOp::List, &net)
                .await
                .unwrap_err();
            assert!(matches!(err, Error::Rejected(_)), "{err}");
            server.shutdown().await;
            // A listener that echoes the request: `store` is not an answer.
            let (server, _, _) = start(Transport::Tcp, Echo).await;
            let err = store(&server.bound(), StoreOp::List, &net)
                .await
                .unwrap_err();
            assert!(
                matches!(&err, Error::Protocol(text) if text == "store answered with store"),
                "{err}"
            );
            server.shutdown().await;
        })
        .await
        .expect("the test finished in time");
    }

    /// A party stand-in that answers every store request with `applied:
    /// false`, whatever it asked.
    struct NeverApplies;

    impl crate::transport::Handler for NeverApplies {
        async fn handle(&self, msg: Message, _peer: crate::transport::PeerInfo) -> Result<Message> {
            Ok(match msg {
                Message::Store { .. } => Message::Stored(Stored {
                    client: Some(PartyId(2)),
                    revision: 9,
                    applied: false,
                    entries: vec![],
                }),
                other => Message::nack(format!("unexpected {} at the script", other.kind())),
            })
        }
    }

    #[tokio::test]
    async fn only_a_conditional_write_may_be_answered_as_not_applied() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (server, _client, _certs) =
                crate::transport::testing::start(Transport::Tcp, NeverApplies).await;
            let net = NetOpts {
                timing: Timing::fast(),
                ..NetOpts::default()
            };
            let key = crate::protocol::message::store_key;
            let put = |if_version| StoreOp::Put {
                key: key("step"),
                value: "5".into(),
                if_version,
            };
            let delete = |if_version| StoreOp::Delete {
                key: key("step"),
                if_version,
            };

            // A miss is how a conditional write is answered.
            for op in [put(Some(0)), put(Some(7)), delete(Some(0)), delete(Some(7))] {
                let what = format!("{op:?}");
                let stored = store(&server.bound(), op, &net).await.expect(&what);
                assert!(!stored.applied, "{what}");
            }
            // Any other operation cannot miss, so the answer is a violation.
            for op in [
                StoreOp::Get { key: key("step") },
                StoreOp::List,
                put(None),
                delete(None),
            ] {
                let kind = op.kind();
                match store(&server.bound(), op, &net).await {
                    Err(Error::Protocol(reason)) => assert_eq!(
                        reason,
                        format!(
                            "the {kind} was answered as not applied although it stated no condition"
                        )
                    ),
                    other => panic!("{kind}: {other:?}"),
                }
            }
            server.shutdown().await;
        })
        .await
        .expect("the test finished in time");
    }

    #[test]
    fn interface_listing_matches_enumeration() {
        let names = list_interfaces(None).unwrap();
        assert!(!names.is_empty());
        let v4 = list_interfaces(Some(IpVersion::V4)).unwrap();
        assert!(v4.iter().all(|n| names.contains(n)));
    }

    #[test]
    fn loopback_can_be_selected_unambiguously() {
        let all = list_ips(&Selector::default()).unwrap();
        let lo = all
            .iter()
            .find(|a| a.ip.is_loopback() && a.ip.is_ipv4())
            .expect("an IPv4 loopback address");
        let selector = Selector {
            interface: Some(lo.interface.clone()),
            prefix: Some("127.".into()),
            version: Some(IpVersion::V4),
        };
        assert_eq!(select_local_ip(&selector).unwrap(), lo.ip);
        let ambiguous = Selector::default();
        if all.len() > 1 {
            assert!(matches!(
                select_local_ip(&ambiguous),
                Err(Error::AmbiguousAddress { .. })
            ));
        }
    }
}
