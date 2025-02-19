use std::{env, fs, io};
use std::io::Error;
use std::sync::Arc;
use pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, ServerConfig, RootCertStore};
use hyper_rustls::{HttpsConnectorBuilder, HttpsConnector};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use hyper_util::client::legacy::Client; // TODO: can we do without legacy?
use http_body_util::Full;
use hyper::body::Bytes;
use rustls_native_certs::load_native_certs;
use base64::{engine::general_purpose, Engine as _};
use tokio_rustls::TlsAcceptor;


#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

/// Load public certificate from file.
async fn load_certs() -> Result<Vec<CertificateDer<'static>>, std::io::Error> {
    let filename = env::var("CERT_PATH").expect("CERT_PATH not set");
    // Open certificate file.
    let certfile = fs::File::open(filename.clone())
        .map_err(|e| error!("{}", format!("failed to open {}: {}", filename, e))).unwrap();
    let mut reader = io::BufReader::new(certfile);

    // Load and return certificate.
    rustls_pemfile::certs(&mut reader).collect()
}

// Load private key from file
async fn load_private_key() -> Result<PrivateKeyDer<'static>, std::io::Error> {
    let filename = env::var("KEY_PATH").expect("KEY_PATH not set");
    // Open keyfile.
    let keyfile = fs::File::open(filename.clone())
        .map_err(|e| error!("{}", format!("failed to open {}: {}", filename, e))).unwrap();
    let mut reader = io::BufReader::new(keyfile);

    // Load and return a single private key.
    rustls_pemfile::private_key(&mut reader).map(|key| key.unwrap())
}

/// add certificate to root store
pub async fn load_ca(root_ca: Option<String>) -> Result<RootCertStore, std::io::Error>{
    let root_store = match root_ca {
        Some(ref path) => {
            let file = fs::File::open(path)
                .map_err(
                    |e| error!("{}",
                        format!( "failed to open {}: {}", path, e)
                    )
                ).unwrap();
            let mut reader = io::BufReader::new(file);
            let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
            let mut root_store = RootCertStore::empty();
            root_store.add_parsable_certificates(certs);
            root_store
        }
        None => {
            // let certs = load_certs().await.unwrap();
            let mut root_store = RootCertStore::empty();
            // root_store.add_parsable_certificates(certs);
            // root_store
            let certs = load_native_certs().unwrap(); // This returns Vec<CertificateDer>

            for cert in certs {
                root_store.add(cert).unwrap(); // Add each certificate to the root store
            }
        
            root_store
        },
    };

    Ok(root_store)
}

/// configure tls on the server side
pub async fn tls_config() -> Result<ServerConfig, std::io::Error>{
        // Set a process wide default crypto provider.
        #[cfg(feature = "ring")]
        let _ = rustls::crypto::ring::default_provider().install_default();
        #[cfg(feature = "aws-lc-rs")]
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Load public certificate.
        let certs = load_certs().await.unwrap();
        let key = load_private_key().await.unwrap();

        // Build TLS configuration.
        let mut server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| error!("{}", e.to_string())).unwrap();
        server_config.alpn_protocols = vec![
            b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()
        ];

        Ok(server_config)
}

/// Define an async function for configuring the HTTPS client
pub async fn setup_https_client(root_ca: Option<String>) -> Client<HttpsConnector<HttpConnector>, Full<Bytes>> {
    // Wait for the connector to be configured
    let https_connector = match root_ca {
        Some(certs) => {
            let decoded_certs: Vec<Vec<u8>> = certs
            .lines()
            .map(|line| general_purpose::STANDARD.decode(line).unwrap())
            .collect();

            // Add certificates to the RootCertStore
            let mut root_store = RootCertStore::empty();
            for cert in decoded_certs {
                let der_cert = CertificateDer::from(cert); // Convert Vec<u8> to CertificateDer
                root_store
                .add_parsable_certificates([der_cert]);
            }
            let tls = ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth();
            HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http()
            .enable_http1()
            .build()
        }
        None => {
            HttpsConnectorBuilder::new()
            .with_native_roots().unwrap()
            .https_or_http()
            .enable_http1()
            .build()
        }
    };

    // Build the Client using the HttpsConnector
    Client::builder(TokioExecutor::new()).build(https_connector)
}


pub fn tls_acceptor() -> Result<(ClientConfig, TlsAcceptor), Error> {
    trace!("Start Generating TLS Acceptor");

    let server_config = tls_config().await?;

    let root_path = match env::var("ROOT_PATH") {
        Ok(path) => Some(path),
        Err(_) => None
    };

    let root_store = load_ca(root_path).await?;

    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls_accptor = TlsAcceptor::from(Arc::new(server_config));

    trace!(
        "Done generating TLS Acceptor: ({:?}, {:?})", tls_config, tls_acceptor
    );
    Ok((tls_config, tls_acceptor))
}


