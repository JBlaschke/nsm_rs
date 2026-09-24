//! rustls configuration from PEM files.
//!
//! Everything TLS-related that the transports need is built here from a
//! [`TlsPaths`], so the trust model lives in one place:
//!
//! - **Server identity** comes from `cert` (a PEM certificate chain, leaf
//!   first) and `key` (the matching PEM private key). A process can only act
//!   as a TLS server when both are configured ([`TlsPaths::has_server_identity`]).
//! - **Peer verification** uses the trust anchors in `root_ca`, a PEM bundle
//!   chosen by the operator. Only when `root_ca` is `None` does the client fall
//!   back to the platform trust store, and a store that is missing or empty is
//!   a configuration error, never a panic. Trust anchors never arrive over the
//!   wire: the old protocol let the party being verified ship the CA that
//!   verified it, which authenticated nobody (audit S21, decision D10).
//! - **No downgrade.** A connector produced here speaks TLS on every
//!   connection. Transports must never pair it with a plaintext fallback such
//!   as hyper's `https_or_http` (audit S20); whether a peer is dialled with TLS
//!   is decided by the address scheme and the configuration, not by whether a
//!   CA blob happened to be present.
//! - **ALPN** is whatever the caller asks for: `["http/1.1"]` for the HTTP
//!   transport, `[]` for framed TCP. Nothing advertises `h2` or the
//!   non-existent `http/1.0` any more (audit S23).
//! - **No client authentication yet.** Servers accept any client and clients
//!   present no certificate. Mutual TLS, with authorisation bound to the client
//!   certificate, is a listed follow-up (`docs/PLAN.md`, D10).
//!
//! The rustls [`CryptoProvider`] is selected by the crate features
//! (`aws-lc-rs`, the default, or `ring`) and installed once per process by
//! [`install_default_provider`]; `main` calls it before anything else.
//!
//! HPC compute nodes and minimal container images frequently have no system
//! trust store at all. On such hosts [`root_store`] with `None` fails with
//! [`Error::Config`], and the operator must pass `--root-ca` (or `ROOT_PATH`)
//! pointing at the CA that issued the broker's and parties' certificates.

use std::path::Path;
use std::sync::Arc;

use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::{self, PemObject};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, warn};

use crate::config::TlsPaths;
use crate::error::{Error, Result};

/// Install the process-wide rustls [`CryptoProvider`] selected by the crate
/// features, exactly once.
///
/// With the `aws-lc-rs` feature (the default) this installs aws-lc-rs; with
/// only `ring` it installs ring. If both features are enabled aws-lc-rs wins
/// and the second, failing installation is ignored on purpose. With neither
/// feature the function does nothing and [`server_config`]/[`client_config`]
/// return a configuration error instead of panicking.
///
/// Calling this more than once is harmless: rustls keeps the first provider
/// and the later attempts return an error that is discarded here.
pub fn install_default_provider() {
    #[cfg(feature = "aws-lc-rs")]
    {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
    #[cfg(feature = "ring")]
    {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// The installed provider, installing the feature-selected one on demand so
/// that a caller who forgot [`install_default_provider`] gets an error or a
/// working configuration rather than rustls' panic.
fn provider() -> Result<Arc<CryptoProvider>> {
    if let Some(provider) = CryptoProvider::get_default() {
        return Ok(Arc::clone(provider));
    }
    install_default_provider();
    CryptoProvider::get_default()
        .map(Arc::clone)
        .ok_or_else(|| {
            Error::config(
                "no rustls crypto provider is compiled in; build with the `aws-lc-rs` or `ring` feature",
            )
        })
}

/// Fill `buf` with cryptographically secure random bytes from the installed
/// crypto provider (used for registration tokens).
pub fn fill_random(buf: &mut [u8]) -> Result<()> {
    provider()?
        .secure_random
        .fill(buf)
        .map_err(|_| Error::config("the crypto provider could not produce random bytes"))
}

/// Map a PEM error for `path` onto the crate error, keeping I/O errors (a
/// missing or unreadable file) as [`Error::Io`] so callers can tell them from
/// malformed contents.
fn pem_error(path: &Path, err: pem::Error) -> Error {
    debug!(path = %path.display(), error = %err, "cannot read PEM file");
    match err {
        pem::Error::Io(io) => Error::Io(io),
        other => Error::Pem(other),
    }
}

/// Load every certificate from the PEM file at `path`, in file order.
///
/// Sections of other kinds (a private key kept in the same file, say) are
/// skipped. A file that contains no certificate at all is an
/// [`Error::Config`]; a missing file is [`Error::Io`]; malformed PEM is
/// [`Error::Pem`].
pub fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|e| pem_error(path, e))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| pem_error(path, e))?;
    if certs.is_empty() {
        return Err(Error::config(format!(
            "no certificates in {}",
            path.display()
        )));
    }
    debug!(path = %path.display(), count = certs.len(), "loaded certificates");
    Ok(certs)
}

/// Load the first private key (PKCS#8, SEC1 or PKCS#1) from the PEM file at
/// `path`.
///
/// A missing file is [`Error::Io`]; a file without a key section or with
/// malformed PEM is [`Error::Pem`].
pub fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let key = PrivateKeyDer::from_pem_file(path).map_err(|e| pem_error(path, e))?;
    debug!(path = %path.display(), "loaded private key");
    Ok(key)
}

/// The trust anchors used to verify peers.
///
/// With `Some(path)` the PEM bundle at `path` is parsed and every certificate
/// that is a usable trust anchor is added; certificates rustls cannot use are
/// counted and logged at `warn`, and it is an [`Error::Config`] when none
/// remain.
///
/// With `None` the platform trust store is loaded through
/// [`rustls_native_certs::load_native_certs`]. Each problem it reports is
/// logged at `warn` and an empty result is an [`Error::Config`]: HPC compute
/// nodes and minimal container images often have no system store, and
/// operators there must pass `--root-ca` instead. Note that with the platform
/// store *any* public CA can issue a certificate the mesh will accept, so an
/// explicit `--root-ca` is the recommended configuration for mesh traffic.
pub fn root_store(root_ca: Option<&Path>, system_roots: bool) -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    match root_ca {
        Some(path) => {
            let certs = load_certs(path)?;
            let (added, ignored) = store.add_parsable_certificates(certs);
            if ignored > 0 {
                warn!(
                    path = %path.display(),
                    ignored,
                    "root CA bundle contains certificates that cannot be used as trust anchors"
                );
            }
            if added == 0 {
                return Err(Error::config(format!(
                    "no usable root certificates in {}",
                    path.display()
                )));
            }
            debug!(path = %path.display(), added, "loaded root certificates");
        }
        None => {
            if !system_roots {
                return Err(Error::config(
                    "verifying TLS peers needs --root-ca (or --system-roots to trust the \
                     platform certificate store)",
                ));
            }
            let native = rustls_native_certs::load_native_certs();
            for err in &native.errors {
                warn!(error = %err, "problem loading the platform trust store");
            }
            let (added, ignored) = store.add_parsable_certificates(native.certs);
            if ignored > 0 {
                warn!(
                    ignored,
                    "platform trust store contains certificates that cannot be used as trust anchors"
                );
            }
            if added == 0 {
                return Err(Error::config(
                    "no usable root certificates in the platform trust store; \
                     pass --root-ca with the CA bundle that issued the peers' certificates",
                ));
            }
            debug!(
                added,
                "loaded root certificates from the platform trust store"
            );
        }
    }
    Ok(store)
}

/// ALPN protocol identifiers in the form rustls stores them.
fn alpn_protocols(alpn: &[&str]) -> Vec<Vec<u8>> {
    alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
}

/// A server configuration presenting the identity in `paths`, accepting any
/// client (no client authentication yet) and offering exactly the given ALPN
/// protocols (`["http/1.1"]` for HTTP, `[]` for framed TCP).
///
/// It is an [`Error::Config`] unless [`TlsPaths::has_server_identity`] holds;
/// the message names the missing flag(s). File and parse errors come from
/// [`load_certs`] and [`load_private_key`]; a key that does not match the
/// certificate is an [`Error::Tls`].
pub fn server_config(paths: &TlsPaths, alpn: &[&str]) -> Result<Arc<ServerConfig>> {
    let (cert_path, key_path) = match (paths.cert.as_deref(), paths.key.as_deref()) {
        (Some(cert), Some(key)) => (cert, key),
        (cert, key) => {
            let mut missing = Vec::new();
            if cert.is_none() {
                missing.push("--tls-cert (CERT_PATH)");
            }
            if key.is_none() {
                missing.push("--tls-key (KEY_PATH)");
            }
            return Err(Error::config(format!(
                "a TLS server needs a certificate and a private key; missing: {}",
                missing.join(" and ")
            )));
        }
    };
    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;
    let mut config = ServerConfig::builder_with_provider(provider()?)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    config.alpn_protocols = alpn_protocols(alpn);
    Ok(Arc::new(config))
}

/// A client configuration that verifies servers against [`root_store`] built
/// from `paths.root_ca`, presents no client certificate, and offers exactly
/// the given ALPN protocols.
///
/// Errors are those of [`root_store`].
pub fn client_config(paths: &TlsPaths, alpn: &[&str]) -> Result<Arc<ClientConfig>> {
    let roots = root_store(paths.root_ca.as_deref(), paths.system_roots)?;
    let mut config = ClientConfig::builder_with_provider(provider()?)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn_protocols(alpn);
    Ok(Arc::new(config))
}

/// A [`TlsAcceptor`] for accepted TCP streams, built from [`server_config`].
pub fn acceptor(paths: &TlsPaths, alpn: &[&str]) -> Result<TlsAcceptor> {
    Ok(TlsAcceptor::from(server_config(paths, alpn)?))
}

/// A [`TlsConnector`] for outgoing TCP streams, built from [`client_config`].
/// Every connection made through it is TLS; there is no plaintext fallback.
pub fn connector(paths: &TlsPaths, alpn: &[&str]) -> Result<TlsConnector> {
    Ok(TlsConnector::from(client_config(paths, alpn)?))
}

/// The name a client verifies the server's certificate against.
///
/// `host` may be an IPv4 or IPv6 literal (the brackets of the `[::1]:port`
/// form are tolerated) or a DNS name; anything else is an [`Error::Config`]
/// naming the offending host.
pub fn server_name(host: &str) -> Result<ServerName<'static>> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    ServerName::try_from(bare)
        .map(|name| name.to_owned())
        .map_err(|_| {
            Error::config(format!(
                "invalid server name {host:?}: expected an IP address or a DNS name"
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Duration;

    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);

    /// A fresh CA and a leaf for `localhost`/`127.0.0.1` signed by it, written
    /// as `ca.pem`, `cert.pem` and `key.pem` into a temporary directory. The
    /// directory must outlive the returned paths.
    fn test_certs() -> (TempDir, TlsPaths) {
        install_default_provider();
        let dir = tempfile::tempdir().expect("tempdir");

        let ca_key = KeyPair::generate().expect("CA key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "nsm test CA");
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

        let paths = TlsPaths {
            cert: Some(cert),
            key: Some(key),
            root_ca: Some(ca),
            system_roots: false,
        };
        (dir, paths)
    }

    #[test]
    fn install_default_provider_is_idempotent() {
        install_default_provider();
        install_default_provider();
        #[cfg(any(feature = "aws-lc-rs", feature = "ring"))]
        assert!(CryptoProvider::get_default().is_some());
    }

    #[test]
    fn loads_certificates_and_key() {
        let (_dir, paths) = test_certs();
        let certs = load_certs(paths.cert.as_deref().expect("cert path")).expect("load certs");
        assert_eq!(certs.len(), 1);
        load_private_key(paths.key.as_deref().expect("key path")).expect("load key");
        let roots = root_store(paths.root_ca.as_deref(), false).expect("root store");
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn missing_files_are_io_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist.pem");
        assert!(matches!(load_certs(&missing), Err(Error::Io(_))));
        assert!(matches!(load_private_key(&missing), Err(Error::Io(_))));
        assert!(matches!(
            root_store(Some(&missing), false),
            Err(Error::Io(_))
        ));
    }

    #[test]
    fn files_without_pem_blocks_are_errors_not_panics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let junk = dir.path().join("junk.pem");
        std::fs::write(&junk, "this is not PEM\n").expect("write junk");
        assert!(matches!(load_certs(&junk), Err(Error::Config(_))));
        assert!(matches!(load_private_key(&junk), Err(Error::Pem(_))));
        assert!(matches!(
            root_store(Some(&junk), false),
            Err(Error::Config(_))
        ));

        let truncated = dir.path().join("truncated.pem");
        std::fs::write(&truncated, "-----BEGIN CERTIFICATE-----\nAAAA\n").expect("write");
        assert!(load_certs(&truncated).is_err());
        assert!(load_private_key(&truncated).is_err());
    }

    #[test]
    fn server_config_requires_certificate_and_key() {
        install_default_provider();
        match server_config(&TlsPaths::default(), &["http/1.1"]) {
            Err(Error::Config(msg)) => {
                assert!(
                    msg.contains("--tls-cert") && msg.contains("--tls-key"),
                    "{msg}"
                );
            }
            other => panic!("expected a configuration error, got {other:?}"),
        }

        let (_dir, paths) = test_certs();
        let cert_only = TlsPaths {
            key: None,
            ..paths.clone()
        };
        match server_config(&cert_only, &[]) {
            Err(Error::Config(msg)) => {
                assert!(
                    msg.contains("--tls-key") && !msg.contains("--tls-cert"),
                    "{msg}"
                );
            }
            other => panic!("expected a configuration error, got {other:?}"),
        }
    }

    #[test]
    fn server_and_client_configs_carry_requested_alpn() {
        let (_dir, paths) = test_certs();
        let server = server_config(&paths, &["http/1.1"]).expect("server config");
        assert_eq!(server.alpn_protocols, vec![b"http/1.1".to_vec()]);
        let raw = server_config(&paths, &[]).expect("server config");
        assert!(raw.alpn_protocols.is_empty());

        let client = client_config(&paths, &["http/1.1"]).expect("client config");
        assert_eq!(client.alpn_protocols, vec![b"http/1.1".to_vec()]);
        acceptor(&paths, &[]).expect("acceptor");
        connector(&paths, &[]).expect("connector");
    }

    #[test]
    fn platform_trust_store_is_an_error_or_a_store_never_a_panic() {
        match root_store(None, false) {
            Err(Error::Config(msg)) => assert!(msg.contains("--root-ca"), "{msg}"),
            other => panic!("the platform store must be opt-in, got {other:?}"),
        }
        match root_store(None, true) {
            Ok(store) => assert!(!store.is_empty()),
            Err(Error::Config(_)) => {}
            Err(other) => panic!("unexpected error from the platform store: {other}"),
        }
    }

    #[tokio::test]
    async fn handshake_over_loopback_negotiates_alpn_and_carries_data() {
        let (_dir, paths) = test_certs();
        let acceptor = acceptor(&paths, &["http/1.1"]).expect("acceptor");
        let connector = connector(&paths, &["http/1.1"]).expect("connector");
        let name = server_name("localhost").expect("server name");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = async {
            let (tcp, _) = listener.accept().await?;
            let mut tls = acceptor.accept(tcp).await?;
            let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await?;
            assert_eq!(&buf, b"ping");
            tls.write_all(b"pong").await?;
            tls.shutdown().await?;
            Ok::<_, io::Error>(alpn)
        };
        let client = async {
            let tcp = TcpStream::connect(addr).await?;
            let mut tls = connector.connect(name, tcp).await?;
            let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
            tls.write_all(b"ping").await?;
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await?;
            assert_eq!(&buf, b"pong");
            Ok::<_, io::Error>(alpn)
        };

        let (server_alpn, client_alpn) =
            tokio::time::timeout(HANDSHAKE_DEADLINE, async { tokio::join!(server, client) })
                .await
                .expect("handshake timed out");
        assert_eq!(
            server_alpn.expect("server side"),
            Some(b"http/1.1".to_vec())
        );
        assert_eq!(
            client_alpn.expect("client side"),
            Some(b"http/1.1".to_vec())
        );
    }

    #[tokio::test]
    async fn client_with_a_foreign_root_rejects_the_server() {
        let (_server_dir, server_paths) = test_certs();
        let (_other_dir, other_paths) = test_certs();
        let client_paths = TlsPaths {
            root_ca: other_paths.root_ca.clone(),
            ..TlsPaths::default()
        };
        let acceptor = acceptor(&server_paths, &[]).expect("acceptor");
        let connector = connector(&client_paths, &[]).expect("connector");
        let name = server_name("localhost").expect("server name");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let server = async {
            let (tcp, _) = listener.accept().await?;
            // Expected to fail once the client aborts the handshake; either
            // outcome is acceptable on this side.
            let _stream = acceptor.accept(tcp).await?;
            Ok::<_, io::Error>(())
        };
        let client = async {
            let tcp = TcpStream::connect(addr).await?;
            connector.connect(name, tcp).await.map(drop)
        };

        let (_server_result, client_result) =
            tokio::time::timeout(HANDSHAKE_DEADLINE, async { tokio::join!(server, client) })
                .await
                .expect("handshake timed out");
        let err = client_result.expect_err("a server signed by an unknown CA must be rejected");
        // Both throwaway CAs carry the same subject name, so webpki may report
        // `BadSignature` (name matched, key did not) rather than
        // `UnknownIssuer`; either way it must be a certificate rejection.
        let rejected = err
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
            .is_some_and(|e| matches!(e, rustls::Error::InvalidCertificate(_)));
        assert!(
            rejected,
            "expected a certificate verification error, got: {err}"
        );
    }

    #[test]
    fn server_name_accepts_ip_literals_and_dns_names() {
        assert!(matches!(
            server_name("127.0.0.1"),
            Ok(ServerName::IpAddress(_))
        ));
        assert!(matches!(server_name("::1"), Ok(ServerName::IpAddress(_))));
        assert!(matches!(server_name("[::1]"), Ok(ServerName::IpAddress(_))));
        assert!(matches!(
            server_name("broker.example"),
            Ok(ServerName::DnsName(_))
        ));
        assert!(matches!(
            server_name("localhost"),
            Ok(ServerName::DnsName(_))
        ));
    }

    #[test]
    fn server_name_rejects_garbage() {
        match server_name("not a host name!") {
            Err(Error::Config(msg)) => assert!(msg.contains("not a host name!"), "{msg}"),
            other => panic!("expected a configuration error, got {other:?}"),
        }
        assert!(server_name("").is_err());
    }
}
