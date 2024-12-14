use std::{env, fs, io};
use pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, ServerConfig, RootCertStore};
use hyper_rustls::{HttpsConnectorBuilder, HttpsConnector};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use hyper_util::client::legacy::Client;
use http_body_util::Full;
use hyper::body::Bytes;


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
                .map_err(|e| error!("{}", format!( "failed to open {}: {}", path, e))).unwrap();
            let mut reader = io::BufReader::new(file);
            let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
            let mut root_store = RootCertStore::empty();
            root_store.add_parsable_certificates(certs);
            root_store
        }
        None => {
            let certs = load_certs().await.unwrap();
            let mut root_store = RootCertStore::empty();
            root_store.add_parsable_certificates(certs);
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
        server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

        Ok(server_config)
}

/// Define an async function for configuring the HTTPS client
pub async fn setup_https_client(root_ca: Option<String>) -> Client<HttpsConnector<HttpConnector>, Full<Bytes>> {
    // Wait for the connector to be configured
    let https_connector = match root_ca {
        Some(path) => {
            info!("{:?}", path);
            let root_store = load_ca(Some(path)).await.unwrap();
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
