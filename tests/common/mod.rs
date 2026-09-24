//! Shared harness for the end-to-end tests: a broker plus parties on
//! ephemeral loopback ports, with fast timings and, for TLS transports,
//! freshly generated certificates.

#![allow(dead_code)]

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use nsm::broker::listen::{listen, BrokerHandle, ListenOpts};
use nsm::broker::monitor::{Broker, PartySummary};
use nsm::config::{BrokerPolicy, Limits, Timing, TlsPaths};
use nsm::net::{Addr, Transport};
use nsm::ops::NetOpts;
use nsm::party::{ClaimOpts, PartyOpts, PublishOpts, Session};
use nsm::protocol::{Key, PartyId, RegToken, ServiceHandle};
use nsm::Result;

/// Every transport the suite runs over.
pub const TRANSPORTS: &[Transport] = &[
    Transport::Tcp,
    Transport::Tls,
    Transport::Http,
    Transport::Https,
];

const LOOPBACK: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// Certificates for `localhost` / `127.0.0.1` signed by a throwaway CA.
pub fn test_certs() -> (TempDir, TlsPaths) {
    nsm::tls::install_default_provider();
    let dir = tempfile::tempdir().expect("tempdir");
    let ca_key = KeyPair::generate().expect("CA key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "nsm e2e CA");
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA certificate");

    let leaf_key = KeyPair::generate().expect("leaf key");
    let mut leaf_params =
        CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])
            .expect("leaf params");
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .expect("leaf certificate");

    let ca = dir.path().join("ca.pem");
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&ca, ca_cert.pem()).expect("write ca.pem");
    std::fs::write(&cert, leaf_cert.pem()).expect("write cert.pem");
    std::fs::write(&key, leaf_key.serialize_pem()).expect("write key.pem");
    (
        dir,
        TlsPaths {
            cert: Some(cert),
            key: Some(key),
            root_ca: Some(ca),
            system_roots: false,
        },
    )
}

/// A party started by the harness. The session is behind a mutex so a test
/// can hand it to a background task through a shared reference.
#[derive(Debug)]
pub struct Party {
    session: std::sync::Mutex<Option<Session>>,
    id: PartyId,
    token: RegToken,
    bound: Addr,
    state: Arc<nsm::party::PartyState>,
}

impl Party {
    pub fn id(&self) -> PartyId {
        self.id
    }
    pub fn token(&self) -> RegToken {
        self.token
    }
    pub fn bound(&self) -> Addr {
        self.bound.clone()
    }
    pub fn state(&self) -> &Arc<nsm::party::PartyState> {
        &self.state
    }
    pub fn service(&self) -> Option<ServiceHandle> {
        self.state.service()
    }
    pub fn take_session(&self) -> Session {
        self.session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .expect("session already taken")
    }
    pub fn into_session(self) -> Session {
        self.take_session()
    }
}

/// A broker with the settings every party in the test shares.
pub struct Cluster {
    transport: Transport,
    broker: BrokerHandle,
    net: NetOpts,
    shutdown: CancellationToken,
    _certs: Option<TempDir>,
}

impl Cluster {
    pub async fn start(transport: Transport) -> Cluster {
        Self::start_with(transport, BrokerPolicy::default()).await
    }

    pub async fn start_with(transport: Transport, policy: BrokerPolicy) -> Cluster {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("NSM_LOG_LEVEL")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_test_writer()
            .try_init();
        nsm::tls::install_default_provider();
        let (certs, tls) = if transport.is_tls() {
            let (dir, paths) = test_certs();
            (Some(dir), paths)
        } else {
            (None, TlsPaths::default())
        };
        let net = NetOpts {
            tls,
            timing: Timing::fast(),
            limits: Limits::default(),
        };
        let shutdown = CancellationToken::new();
        let broker = listen(
            ListenOpts {
                bind: Addr::new(transport, LOOPBACK.to_string(), 0),
                tls: net.tls.clone(),
                timing: net.timing.clone(),
                limits: net.limits.clone(),
                policy,
            },
            shutdown.clone(),
        )
        .await
        .expect("broker starts");
        Cluster {
            transport,
            broker,
            net,
            shutdown,
            _certs: certs,
        }
    }

    pub fn broker(&self) -> &Arc<Broker> {
        self.broker.broker()
    }
    pub fn broker_addr(&self) -> Addr {
        self.broker.bound()
    }
    pub fn net(&self) -> &NetOpts {
        &self.net
    }
    pub fn timing(&self) -> &Timing {
        &self.net.timing
    }
    /// A raw transport client with the cluster's settings, for hand-crafted
    /// protocol messages.
    pub fn raw_client(&self) -> nsm::transport::Client {
        nsm::transport::Client::new(
            self.net.tls.clone(),
            self.net.timing.clone(),
            self.net.limits.clone(),
        )
    }

    fn party_opts(&self, key: Key, ping: bool) -> PartyOpts {
        PartyOpts {
            broker: self.broker_addr(),
            key,
            local_ip: LOOPBACK,
            bind_port: 0,
            serve_tls: self.transport.is_tls(),
            ping,
            tls: self.net.tls.clone(),
            timing: self.net.timing.clone(),
            limits: self.net.limits.clone(),
        }
    }

    fn wrap(session: Session) -> Party {
        Party {
            id: session.id(),
            token: session.token(),
            bound: session.bound(),
            state: Arc::clone(session.state()),
            session: std::sync::Mutex::new(Some(session)),
        }
    }

    pub async fn publish(&self, key: Key, service_port: u16) -> Party {
        Self::wrap(
            Session::publish(PublishOpts {
                party: self.party_opts(key, false),
                service_port,
            })
            .await
            .expect("publish"),
        )
    }

    pub async fn publish_ping(&self, key: Key, service_port: u16) -> Party {
        Self::wrap(
            Session::publish(PublishOpts {
                party: self.party_opts(key, true),
                service_port,
            })
            .await
            .expect("publish (ping)"),
        )
    }

    pub async fn try_claim(&self, key: Key) -> Result<Party> {
        Session::claim(ClaimOpts {
            party: self.party_opts(key, false),
        })
        .await
        .map(Self::wrap)
    }

    pub async fn claim(&self, key: Key) -> Party {
        self.try_claim(key).await.expect("claim")
    }

    pub async fn claim_ping(&self, key: Key) -> Party {
        Self::wrap(
            Session::claim(ClaimOpts {
                party: self.party_opts(key, true),
            })
            .await
            .expect("claim (ping)"),
        )
    }

    /// Run a party's session in the background (needed for ping mode, and
    /// for watchdog behaviour); the `Party` keeps its state handle.
    pub fn spawn_run(&self, party: &Party) -> JoinHandle<Result<()>> {
        tokio::spawn(party.take_session().run())
    }

    /// Stop a party's server abruptly, as if its host died: the broker is
    /// not told.
    pub async fn kill(&self, party: Party) {
        let session = party.take_session();
        session.shutdown_token().cancel();
        // Run to completion so the server is closed; the broker keeps
        // heartbeating a dead address until the threshold is hit.
        let _ = tokio::time::timeout(Duration::from_secs(2), session.run()).await;
    }

    pub async fn wait_until(&self, mut pred: impl FnMut(&[PartySummary]) -> bool) {
        loop {
            let snap = self.broker().snapshot();
            if pred(&snap) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn wait_until_true(&self, mut pred: impl FnMut() -> bool) {
        while !pred() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn stop(&self) {
        self.shutdown.cancel();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
