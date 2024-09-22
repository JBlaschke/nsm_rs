use std::{env, fs, io};
use pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, ServerConfig, RootCertStore};
use hyper_rustls::ConfigBuilderExt;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

// Load public certificate from file.
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

pub async fn load_ca(root_ca: Option<String>) -> Result<ClientConfig, std::io::Error>{
    let mut ca = match root_ca {
        Some(ref path) => {
            let f = fs::File::open(path)
                .map_err(|e| error!("{}", format!( "failed to open {}: {}", path, e))).unwrap();
            let rd = io::BufReader::new(f);
            Some(rd)
        }
        None => None,
    };

    // Prepare the TLS client config
    let tls = match ca {
        Some(ref mut rd) => {
            // println!("using ca");
            // Read trust roots
            let certs = rustls_pemfile::certs(rd).collect::<Result<Vec<_>, _>>().unwrap();
            let mut roots = RootCertStore::empty();
            roots.add_parsable_certificates(certs);
            // TLS client config using the custom CA store for lookups
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        }
        // Default TLS client config with native roots
        None => rustls::ClientConfig::builder()
                .with_native_roots().unwrap()
                .with_no_client_auth(),
    };
    Ok(tls)
}

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