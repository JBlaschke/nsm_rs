use crate::network::{get_local_ips, get_matching_ipstr};

use crate::connection::{Message, MessageHeader, Addr, server,
    serialize_message, deserialize_message};

use crate::service::{Payload, State, serialize, request_handler, heartbeat_handler_helper, event_monitor};

use crate::utils::{only_or_error, epoch};

use crate::models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, Send};

use crate::tls::{load_certs, load_private_key, load_ca};

use std::env;
use std::sync::Arc;
use tokio::sync::Notify;
use std::net::SocketAddr;
use hyper::http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Empty};
use hyper::body::{Buf, Bytes, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::client::legacy::Client;
use hyper_rustls::HttpsConnectorBuilder;
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout, Duration, Instant};
use tokio::sync::Mutex;
use std::future::Future;
use rustls::{ServerConfig, RootCertStore};
use tokio_rustls::TlsAcceptor;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

pub fn list_interfaces(inputs: ListInterfaces) -> std::io::Result<()> {
    let ips = get_local_ips();

    if inputs.print_v4 {info!("Listing Matching IPv4 Interfaces");}
    if inputs.print_v6 {info!("Listing Matching IPv6 Interfaces");}

    let mut ipv4_names = Vec::new();
    let mut ipv6_names = Vec::new();

    if inputs.print_v4 {
        if inputs.verbose {println!("IPv4 Interfaces:");}
        for ip in ips.ipv4_addrs {
            let name: & String = & ip.name.unwrap_or_default();
            if ! ipv4_names.contains(name) {
                if inputs.verbose {
                    println!(" - {}", name);
                } else {
                    println!("{}", name);
                }
                ipv4_names.push(name.to_string());
            }
        }
    }

    if inputs.print_v6 {
        if inputs.verbose {println!("IPv6 Interfaces:");}
        for ip in ips.ipv6_addrs {
            let name: & String = & ip.name.unwrap_or_default();
            if ! ipv6_names.contains(name) {
                if inputs.verbose {
                    println!(" - {}", name);
                } else {
                    println!("{}", name);
                }
                ipv6_names.push(name.to_string());
            }
        }
    }

    Ok(())

}

pub fn list_ips(inputs: ListIPs) -> std::io::Result<()> {

    let ips = get_local_ips();
    
    if inputs.print_v4 {info!("Listing Matching IPv4 Addresses");}
    if inputs.print_v6 {info!("Listing Matching IPv6 Addresses");}
    if inputs.print_v4 {
        let ipstr = get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        );
        if inputs.verbose {println!("IPv4 Addresses for {}:", inputs.name);}
        for ip in ipstr {
            if inputs.verbose {
                println!(" - {}", ip);
            } else {
                println!("{}", ip);
            }
        }
    }

    if inputs.print_v6 {
        let ipstr = get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        );
        if inputs.verbose {println!("IPv6 Addresses for {}:", inputs.name);}
        for ip in ipstr {
            if inputs.verbose {
                println!(" - {}", ip);
            } else {
                println!("{}", ip);
            }
        }
    }

    Ok(())

}

pub async fn listen(inputs: Listen) -> Result<Response<Full<Bytes>>, hyper::Error> {
    trace!("Setting up listener...");

    // Set a process wide default crypto provider.
    #[cfg(feature = "ring")]
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "aws-lc-rs")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Load public certificate.
    let cert_path = env::var("LISTEN_CERT_PATH").expect("CERT_PATH not set");
    let certs = load_certs(&cert_path).await.unwrap();
    let key_path = env::var("LISTEN_KEY_PATH").expect("KEY_PATH not set");
    let key = load_private_key(&key_path).await.unwrap();
    
    // Build TLS configuration.
    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| error!("{}", e.to_string())).unwrap();
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

    let ips = get_local_ips();

    // identify local ip address
    let ipstr = if inputs.print_v4 {
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        )
    } else {
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        )
    };
    
    // initialize State struct
    let tls = load_ca(inputs.root_ca).await;
    let state = Arc::new((Mutex::new(State::new(tls)), Notify::new()));
    let state_clone = Arc::clone(& state);

    trace!("Entering event monitor");
    // send State struct holding event queue into event monitor to handle heartbeats
    let event_loop = tokio::task::spawn(async move {
        let _ = match event_monitor(state_clone).await{
            Ok(resp) => println!("exited event monitor"),
            Err(_) => println!("event monitor error")
        };
    });

    let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
        let state_clone = Arc::clone(& state);
        Box::pin(async move {
            request_handler(& state_clone, req).await
        }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, hyper::Error>> + std::marker::Send>>
    }));
    
    let host = only_or_error(& ipstr);
    let server_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();
    let incoming = TcpListener::bind(&server_addr).await.unwrap();
    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));
    info!("Listening on: {}:{}", host.to_string(), inputs.bind_port);
    let service = service_fn(move |req: Request<Incoming>| {
        let handler_clone = Arc::clone(&handler);
        async move {
            let loc_handler = handler_clone.lock().await.clone();
            server(req, loc_handler).await
        }
    });
    loop {
        let (tcp_stream, _remote_addr) = incoming.accept().await.unwrap();
        let tls_acceptor = tls_acceptor.clone();
        let service_clone = service.clone();
        // thread handles incoming connections and adds them to State and event queue
        tokio::task::spawn(async move {
            let tls_stream = match tls_acceptor.accept(tcp_stream).await {
                Ok(tls_stream) => tls_stream,
                Err(err) => {
                    eprintln!("failed to perform tls handshake: {err:#}");
                    return;
                }
            };

            if let Err(err) = Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls_stream), service_clone)
            .await {
                error!("Failed to serve connection: {:?}", err);
            }
        });
    }

    event_loop.await;
    Ok(Response::new(Full::default()))
}

pub async fn publish(inputs: Publish) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // Set a process wide default crypto provider.
    #[cfg(feature = "ring")]
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "aws-lc-rs")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Load public certificate.
    // let cert_path = env::var("PUBLISH_CERT_PATH").expect("CERT_PATH not set");
    // let certs = load_certs(&cert_path).await.unwrap();
    // let key_path = env::var("PUBLISH_KEY_PATH").expect("KEY_PATH not set");
    // let key = load_private_key(&key_path).await.unwrap();
        
    // Build TLS configuration.
    // let mut server_config = ServerConfig::builder()
    //     .with_no_client_auth()
    //     .with_single_cert(certs, key)
    //     .map_err(|e| error!("{}", e.to_string())).unwrap();
    // server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

    let tls = load_ca(inputs.root_ca).await;

    let ips = get_local_ips();

    let (ipstr, all_ipstr) = if inputs.print_v4 {(
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv4_addrs, & inputs.name, & None)
    )} else {(
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv6_addrs, & inputs.name, & None)
    )};

    let payload = serialize(& Payload {
        service_addr: ipstr.clone(),
        service_port: inputs.service_port,
        service_claim: 0,
        interface_addr: all_ipstr,
        bind_port: inputs.bind_port,
        key: inputs.key,
        id: 0,
        service_id: 0,
    });

    // connect to broker
    // Prepare the HTTPS connector
    let https_connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls.clone())
        .https_or_http()
        .enable_http1()
        .build();

    println!("configured https connector: {:?} with tls: {:?}", https_connector, tls.clone());
    // Build the Client using the HttpsConnector
    let client = Client::builder(TokioExecutor::new()).build(https_connector);

    let msg = serialize_message(& Message{
        header: MessageHeader::PUB,
        body: payload
    });

    let mut read_fail = 0;
    let timeout_duration = Duration::from_millis(6000);
    loop {
        sleep(Duration::from_millis(1000)).await;
        trace!("sending request to {}:{}", inputs.host, inputs.port);

        let req = Request::builder()
        .method(Method::POST)
        .uri(format!("https://{}:{}/request_handler", inputs.host, inputs.port))
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(msg.clone())))
        .unwrap();

        let start = Instant::now();
        let result = timeout(timeout_duration, client.request(req)).await;

        match result {
            Ok(Ok(resp)) => {
                trace!("Received response: {:?}", resp);
                let body = resp.collect().await.unwrap().aggregate();
                let mut data: serde_json::Value = serde_json::from_reader(body.reader()).unwrap();
                let json = serde_json::to_string(&data).unwrap();
                let m = deserialize_message(& json);
                match m.header {
                    MessageHeader::ACK => {
                        info!("Server acknowledged PUB.");
                        break;
                    }
                    _ => {
                        warn!("Server responds with unexpected message: {:?}", m)
                    }
                }
            }
            Ok(Err(e)) => {
                read_fail += 1;
                if read_fail > 5 {
                    panic!("Failed to send request to listener")
                }
            },
            Err(_) => {
                read_fail += 1;
                if read_fail > 5 {
                    panic!("Requests to listener timed out")
                }
            },
        }
    }

    // let host = only_or_error(& ipstr);

    // let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
    //     let tls_clone = tls.clone();
    //     Box::pin(async move {
    //         heartbeat_handler_helper(req, None, None, tls_clone).await
    //     }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, hyper::Error>> + std::marker::Send>>
    // }));

    // let handler_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();
    // info!("Starting server on: {}:{}", host.to_string(), inputs.bind_port);
    // // send/receive heartbeats to/from broker
    // let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));
    // let incoming = TcpListener::bind(&handler_addr).await.unwrap();
    // let service = service_fn(move |req: Request<Incoming>| {
    //     let handler_clone = Arc::clone(&handler);
    //     async move {
    //         let loc_handler = handler_clone.lock().await.clone();
    //         server(req, loc_handler).await
    //     }
    // });
    // loop {
    //     let (tcp_stream, _remote_addr) = incoming.accept().await.unwrap();
    //     let tls_acceptor = tls_acceptor.clone();
    //     let service_clone = service.clone();
    //     tokio::task::spawn(async move {
    //         let tls_stream = match tls_acceptor.accept(tcp_stream).await {
    //             Ok(tls_stream) => tls_stream,
    //             Err(err) => {
    //                 eprintln!("failed to perform tls handshake: {err:#}");
    //                 return;
    //             }
    //         };

    //         if let Err(err) = Builder::new(TokioExecutor::new())
    //         .serve_connection(TokioIo::new(tls_stream), service_clone)
    //         .await {
    //             error!("Failed to serve connection: {:?}", err);
    //         }
    //     });
    // }

    Ok(Response::new(Full::default()))

}

pub async fn claim(inputs: Claim) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // Set a process wide default crypto provider.
    #[cfg(feature = "ring")]
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "aws-lc-rs")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Load public certificate.
    let certs = load_certs("examples/sample.pem").await.unwrap();  //change location
    // Load private key.
    let key = load_private_key("examples/sample.rsa").await.unwrap(); //change location
        
    // Build TLS configuration.
    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| error!("{}", e.to_string())).unwrap();
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()];

    let tls = load_ca(inputs.root_ca).await;

    let ips = get_local_ips();

    let (ipstr, _all_ipstr) = if inputs.print_v4 {(
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv4_addrs, & inputs.name, & None)
    )} else {(
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv6_addrs, & inputs.name, & None)
    )};

    let payload = serialize(& Payload {
        service_addr: ipstr.clone(),
        service_port: inputs.port,
        service_claim: epoch(),
        interface_addr: Vec::new(),
        bind_port: inputs.bind_port,
        key: inputs.key,
        id: 0,
        service_id: 0,
    });

    // connect to broker
    // Prepare the HTTPS connector
    let https_connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls.clone())
        .https_or_http()
        .enable_http1()
        .build();
        let client = Client::builder(TokioExecutor::new()).build(https_connector);
    let msg = serialize_message(& Message{
        header: MessageHeader::CLAIM,
        body: payload
    });

    let mut read_fail = 0;
    let service_payload = Arc::new(Mutex::new("".to_string()));
    let timeout_duration = Duration::from_millis(6000);

    loop {
        sleep(Duration::from_millis(1000)).await;
        trace!("sending request to {}:{}", inputs.host, inputs.port);

        let req = Request::builder()
        .method(Method::POST)
        .uri(format!("http://{}:{}/request_handler", inputs.host, inputs.port))
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(msg.clone())))
        .unwrap();

        let start = Instant::now();
        let result = timeout(timeout_duration, client.request(req)).await;
    
        // check for successful connection to a published service
        match result {
            Ok(Ok(resp)) => {
                trace!("Received response: {:?}", resp);
                let body = resp.collect().await.unwrap().aggregate();
                let mut data: serde_json::Value = serde_json::from_reader(body.reader()).unwrap();
                let json = serde_json::to_string(&data).unwrap();
                let m = deserialize_message(& json);
                // print service's address to client
                if matches!(m.header, MessageHeader::ACK){
                    info!("Server acknowledged CLAIM.");
                    println!("{}", m.body);
                    let mut service_payload_loc = service_payload.lock().await;
                    *service_payload_loc = m.body;
                    break;
                } else{
                    read_fail += 1;
                    if read_fail > 5 {
                        panic!("Key not found. Try a different key.")
                    }
                }
            }
            Ok(Err(e)) => {
                read_fail += 1;
                if read_fail > 5 {
                    panic!("Failed to send request to listener")
                }
            },
            Err(_) => {
                read_fail += 1;
                if read_fail > 5 {
                    panic!("Requests to listener timed out")
                }
            },
        }
    }

    let broker_addr = Arc::new(Mutex::new(Addr{
        host: inputs.host,
        port: inputs.port
    }));

    let host = only_or_error(& ipstr);

    let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
        let service_payload_value = Arc::clone(&service_payload);
        let broker_addr_value = Arc::clone(&broker_addr);
        let tls_clone = tls.clone();
        Box::pin(async move {
            heartbeat_handler_helper(req, Some(&service_payload_value), Some(broker_addr_value), tls_clone).await
        }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, hyper::Error>> + std::marker::Send>>
    }));

    let handler_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();

    info!("Starting server on: {}:{}", host.to_string(), inputs.bind_port);

    // send/receive heartbeats to/from broker
    let tls_acceptor = TlsAcceptor::from(Arc::new(server_config));
    let incoming = TcpListener::bind(&handler_addr).await.unwrap();
    let service = service_fn(move |req: Request<Incoming>| {
        let handler_clone = Arc::clone(&handler);
        async move {
            let loc_handler = handler_clone.lock().await.clone();
            server(req, loc_handler).await
        }
    });
    loop {
        let (tcp_stream, _remote_addr) = incoming.accept().await.unwrap();
        let tls_acceptor = tls_acceptor.clone();
        let service_clone = service.clone();
        tokio::task::spawn(async move {
            let tls_stream = match tls_acceptor.accept(tcp_stream).await {
                Ok(tls_stream) => tls_stream,
                Err(err) => {
                    eprintln!("failed to perform tls handshake: {err:#}");
                    return;
                }
            };

            if let Err(err) = Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls_stream), service_clone)
            .await {
                error!("Failed to serve connection: {:?}", err);
            }
        });
    }

    Ok(Response::new(Full::default()))
}

pub async fn collect(inputs: Collect) -> Result<(), std::io::Error> {
    // Set a process wide default crypto provider.
    #[cfg(feature = "ring")]
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "aws-lc-rs")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let tls = load_ca(inputs.root_ca).await;

    // connect to bind port of service or client
    // Prepare the HTTPS connector
    let https_connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http1()
        .build();
    let client = Client::builder(TokioExecutor::new()).build(https_connector);
    let msg = serialize_message(& Message{
        header: MessageHeader::COL,
        body: "".to_string()
    });
    let timeout_duration = Duration::from_millis(6000);

    let req = Request::builder()
    .method(Method::GET)
    .uri(format!("http://{}:{}/heartbeat_handler", inputs.host, inputs.port))
    .header(hyper::header::CONTENT_TYPE, "application/json")
    .body(Full::new(Bytes::from(msg.clone())))
    .unwrap();

    let start = Instant::now();
    let result = timeout(timeout_duration, client.request(req)).await;

    println!("sending request to {}:{}", inputs.host, inputs.port);

    match result {
        Ok(Ok(resp)) => {
            trace!("Received response: {:?}", resp);
            let body = resp.collect().await.unwrap().aggregate();
            let mut data: serde_json::Value = serde_json::from_reader(body.reader()).unwrap();
            let json = serde_json::to_string(&data).unwrap();
            let m = deserialize_message(& json);
            match m.header {
                MessageHeader::ACK => {
                    info!("Request acknowledged.");
                    if m.body.is_empty() {
                        panic!("Payload not found in heartbeat.");
                    }
                    println!("{:?}", m.body);
                },
                _ => warn!("Server responds with unexpected message: {:?}", m),
            }
        }
        Ok(Err(_e)) => panic!("Failed to collect message."),
        Err(_e) => panic!("Timed out reading response")
    }

    Ok(())
}

pub async fn send_msg(inputs: Send) -> Result<(), std::io::Error> {
    // Set a process wide default crypto provider.
    #[cfg(feature = "ring")]
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "aws-lc-rs")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let tls = load_ca(inputs.root_ca).await;

    // connect to bind port of service or client
    let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http1()
        .build();
    let client = Client::builder(TokioExecutor::new()).build(https_connector);
    let msg = serialize_message(& Message{
        header: MessageHeader::MSG,
        body: serde_json::to_string(&inputs.msg).unwrap()
    });
    let timeout_duration = Duration::from_millis(6000);

    println!("sending request to {}:{}", inputs.host, inputs.port);
    let req = Request::builder()
    .method(Method::GET)
    .uri(format!("http://{}:{}/heartbeat_handler", inputs.host, inputs.port))
    .header(hyper::header::CONTENT_TYPE, "application/json")
    .body(Full::new(Bytes::from(msg.clone())))
    .unwrap();

    let start = Instant::now();
    let result = timeout(timeout_duration, client.request(req)).await;

    match result {
        Ok(Ok(resp)) => {
            trace!("Received response: {:?}", resp);
            let body = resp.collect().await.unwrap().aggregate();
            let mut data: serde_json::Value = serde_json::from_reader(body.reader()).unwrap();
            let json = serde_json::to_string(&data).unwrap();
            let m = deserialize_message(& json);
            match m.header {
                MessageHeader::ACK => info!("Request acknowledged: {:?}", m),
                _ => warn!("Server responds with unexpected message: {:?}", m),
            }
        }
        Ok(Err(_e)) => panic!("Failed to collect message."),
        Err(_e) => panic!("Timed out reading response")
    }

    Ok(())    
}