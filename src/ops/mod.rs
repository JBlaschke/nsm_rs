//! The operations, written once for the CLI and the REST control plane.
//!
//! Each function takes a typed request, does no printing and returns typed
//! results or [`Error`]s; the caller decides how to present them. Long-running
//! operations (`listen`, `publish`, `claim`) return a handle whose `run`
//! future the caller awaits, so the same code serves a foreground CLI process
//! and a background job in the control plane.

use std::net::IpAddr;

use tokio_util::sync::CancellationToken;

use crate::broker::listen::{listen as start_broker, BrokerHandle, ListenOpts};
use crate::config::{BrokerPolicy, Limits, Timing, TlsPaths};
use crate::net::{interfaces, Addr, IpVersion, LocalAddr, Selector, Transport};
use crate::party::{ClaimOpts, PartyOpts, PublishOpts, Session};
use crate::protocol::{Key, Message, ServiceHandle};
use crate::transport::Client;
use crate::{Error, Result};

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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Collected {
    /// A service's last delivered text.
    pub text: Option<String>,
    /// A client's paired service.
    pub service: Option<ServiceHandle>,
}

/// Ask a party (by its heartbeat address) what it holds.
pub async fn collect(party: &Addr, net: &NetOpts) -> Result<Collected> {
    match net.client().call(party, Message::Collect).await? {
        Message::Collected { text, service } => Ok(Collected { text, service }),
        Message::Nack { reason } => Err(Error::Rejected(reason)),
        other => Err(Error::protocol(format!(
            "collect answered with {}",
            other.kind()
        ))),
    }
}

/// Hand `text` to a client (by its heartbeat address) for its paired service.
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

#[cfg(test)]
mod tests {
    use super::*;

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
