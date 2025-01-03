use crate::network::{get_local_ips, get_matching_ipstr};

use crate::connection::{ComType, Message, MessageHeader, Addr, api_server, tcp_server,
    serialize_message, deserialize_message, send, connect, collect_request, stream_read};

use crate::service::{Payload, State, serialize, request_handler,
     heartbeat_handler_helper, ping_heartbeat, event_monitor};

use crate::utils::{only_or_error, epoch};

use crate::models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, SendMSG};

use crate::tls::{tls_config, load_ca, setup_https_client};

use std::env;
use std::sync::Arc;
use tokio::sync::Notify;
use std::net::SocketAddr;
use hyper::http::{Method, Request, Response};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::client::legacy::Client;
use hyper_rustls::HttpsConnectorBuilder;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout, Duration, Instant};
use tokio::sync::Mutex;
use std::future::Future;
use tokio_rustls::TlsAcceptor;
use rustls::client::ClientConfig;
use rustls::pki_types::ServerName;
use lazy_static::lazy_static;


#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

lazy_static! {
    pub static ref GLOBAL_LAST_HEARTBEAT: Arc<Mutex<Option<Instant>>>= Arc::new(Mutex::new(None));
}

/// # ListInterfaces
/// Lists available interfaces on device
pub async fn list_interfaces(inputs: ListInterfaces) -> std::io::Result<()> {
    let ips = get_local_ips().await;

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

/// #List IPs
/// Lists available IP addresses on interface
/// specify version 4 or 6 to filter to one address
pub async fn list_ips(inputs: ListIPs) -> std::io::Result<()> {

    let ips = get_local_ips().await;
    
    if inputs.print_v4 {info!("Listing Matching IPv4 Addresses");}
    if inputs.print_v6 {info!("Listing Matching IPv6 Addresses");}
    if inputs.print_v4 {
        let ipstr = get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ).await;
        if inputs.verbose {println!("IPv4 Addresses for {:?}:", inputs.name);}
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
        ).await;
        if inputs.verbose {println!("IPv6 Addresses for {:?}:", inputs.name);}
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

/// # Listen
/// Inititate broker with tcp or api communication
pub async fn listen(inputs: Listen, com: ComType) -> Result<Response<Full<Bytes>>, std::io::Error> {
    trace!("Setting up listener...");
    let ips = get_local_ips().await;

    // identify local ip address
    let ipstr = if inputs.print_v4 {
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ).await
    } else {
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        ).await
    };
    let host = only_or_error(& ipstr);

    // initiate state 
    let state = Arc::new((Mutex::new(State::new(None)), Notify::new()));
    let state_clone = Arc::clone(& state);
   
    // enter tcp or api integration
    match com{
        ComType::TCP => {
            let state_clones = Arc::clone(& state_clone);

            // define handler closure to start request_handler within the tcp server function
            let handler =  move |stream: Option<Arc<Mutex<TcpStream>>>| {
                let state_clone_inner = Arc::clone(& state_clones);
                Box::pin(async move {
                    request_handler(& state_clone_inner, stream, None).await
                }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
            };

            let addr = Addr {
                host: host.to_string(),
                port: inputs.bind_port
            };

            info!("Starting listener started on: {}:{}", & addr.host, addr.port);

            // thread handles incoming tcp connections and adds them to State and event queue
            let _thread_handler = tokio::spawn(async move {
                let _ = tcp_server(& addr, handler).await;
            });

            let state_clone_cl = Arc::clone(& state_clone);

            // send State into event monitor to handle heartbeat queue
            let event_loop = tokio::spawn(async move {
                let _ = match event_monitor(state_clone_cl).await{
                    Ok(_resp) => trace!("exited event monitor"),
                    Err(_) => trace!("event monitor error")
                };
            });
            let _ = event_loop.await;
        },
        ComType::API => {
            // initiate tls configuration
            let tls_acceptor : Option<TlsAcceptor> = if inputs.tls {
                let server_config = tls_config().await.unwrap();
                Some(TlsAcceptor::from(Arc::new(server_config)))
            }
            else {
                None
            };

            let state_clone = Arc::clone(& state);

            // send State into event monitor to handle heartbeat queue
            let _event_loop = tokio::spawn(async move {
                let _ = match event_monitor(state_clone).await{
                    Ok(_resp) => trace!("exited event monitor"),
                    Err(_) => trace!("event monitor error")
                };
            });

            // define handler closure to start request_handler within the api server function
            let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
                let state_clone = Arc::clone(& state);
                Box::pin(async move {
                    request_handler(& state_clone, None, Some(req)).await
                }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
            }));

            let server_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();
            
            // bind to host address to listen for requests
            let incoming = TcpListener::bind(&server_addr).await.unwrap();
            info!("Listening on: {}:{}", host.to_string(), inputs.bind_port);

            // define api service closure to route incoming requests
            let service = service_fn(move |req: Request<Incoming>| {
                let handler_clone = Arc::clone(&handler);
                async move {
                    let loc_handler = handler_clone.lock().await.clone();
                    api_server(req, loc_handler).await
                }
            });

            // infinitely listen for requests
            loop {
                let (tcp_stream, _remote_addr) = incoming.accept().await.unwrap();
                let service_clone = service.clone();
                let tls_acceptor = if inputs.tls {
                    tls_acceptor.clone()
                }
                else {
                    None
                };
                // thread handles incoming connections and adds them to State and event queue
                tokio::task::spawn(async move {
                    // check for tls
                    match tls_acceptor {
                        Some(tls_acc) => {
                            match tls_acc.accept(tcp_stream).await {
                                Ok(tls_stream) => {
                                    if let Err(err) = Builder::new(TokioExecutor::new())
                                    .serve_connection(TokioIo::new(tls_stream), service_clone)
                                    .await {
                                        error!("Failed to serve connection: {:?}", err);
                                    }
                                },
                                Err(err) => {
                                    eprintln!("failed to perform tls handshake: {err:#}");
                                    return;
                                }
                            }
                        },
                        None => {
                            if let Err(err) = Builder::new(TokioExecutor::new())
                            .serve_connection(TokioIo::new(tcp_stream), service_clone)
                            .await {
                                error!("Failed to serve connection: {:?}", err);
                            }
                        }
                    };
                });
            }
            // let _ = event_loop.await;
        }
    };
    
    Ok(Response::new(Full::default()))
}

/// # Publish
/// Connect to broker and publish address for data connection.
pub async fn publish(inputs: Publish, com: ComType) -> Result<Response<Full<Bytes>>, std::io::Error> {

    let ips = get_local_ips().await;
    let (ipstr, all_ipstr) = if inputs.print_v4 {(
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ).await,
        get_matching_ipstr(& ips.ipv4_addrs, & inputs.name, & None).await
    )} else {(
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        ).await,
        get_matching_ipstr(& ips.ipv6_addrs, & inputs.name, & None).await
    )};

    // define payload with metadata
    let payload = serialize(& Payload {
        service_addr: ipstr.clone(),
        service_port: inputs.service_port,
        service_claim: 0,
        interface_addr: all_ipstr,
        bind_port: inputs.bind_port,
        key: inputs.key,
        id: 0,
        service_id: 0,
        root_ca: inputs.root_ca,
        ping: inputs.ping

    });
    let host = only_or_error(& ipstr);

    let msg = & Message{
        header: MessageHeader::PUB,
        body: payload.clone()
    };

    let broker_addr = Arc::new(Mutex::new(Addr{
        host: inputs.host.clone(),
        port: inputs.port
    }));
    
    // enter tcp or api integration
    match com{
        ComType::TCP => {
            // connect to broker
            let stream = match connect(& Addr{
                host: inputs.host, port: inputs.port}).await{
                    Ok(s) => s,
                    Err(_e) => {
                        return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,"Connection unsuccessful"));
                        }
            };
            let stream_mut = Arc::new(Mutex::new(stream));
            // send broker identification and add itself to event queue and state
            let ack = send(& stream_mut, msg).await;

            // check for successful connection
            match ack {
                Ok(m) => {
                    trace!("Received response: {:?}", m);
                    match m.header {
                        MessageHeader::ACK => {
                            info!("Server acknowledged PUB.")
                        }
                        _ => {
                            warn!("Server responds with unexpected message: {:?}", m)
                        }
                    }
                }
                Err(e) => {
                    error!("Encountered error: {:?}", e);
                }
            }

            // define closure to send connections from server to heartbeat handler
            let handler =  move |stream: Option<Arc<Mutex<TcpStream>>>| {
                Box::pin(async move {
                    return heartbeat_handler_helper(stream, None, None, None, None).await
                }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
            };

            let addr = Addr {
                host: host.to_string(),
                port: inputs.bind_port
            };
            // send/receive heartbeats to/from broker
            let _ = tcp_server(& addr, handler).await;
        },
        ComType::API => {
            // start tls configuration
            let tls : Option<rustls::ClientConfig>;
            let tls_acceptor = if inputs.tls {
                trace!("entering tls config");
                let server_config = tls_config().await.unwrap();
                let root_path = match env::var("ROOT_PATH") {
                    Ok(path) => Some(path),
                    Err(_) => None
                };
                let root_store = load_ca(root_path).await.unwrap();
                tls = Some(ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth());
                let _server_name = ServerName::try_from(inputs.host.clone())
                    .map_err(|_| format!("Invalid server DNS name: {}", inputs.host.clone())).unwrap();
                Some(TlsAcceptor::from(Arc::new(server_config)))
            }
            else {
                tls = None;
                None
            };

            // connect to broker
            // Prepare the HTTPS connector
            let client = if inputs.tls {
                let https_connector = HttpsConnectorBuilder::new()
                .with_tls_config(tls.clone().unwrap())
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            }
            else {
                let https_connector = HttpsConnectorBuilder::new()
                .with_native_roots().unwrap()
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            };

            let mut read_fail = 0;
            let timeout_duration = Duration::from_millis(6000);
            // retry sending requests to broker to add itself to event queue and state
            loop {
                sleep(Duration::from_millis(1000)).await;
                trace!("sending request to {}:{}", inputs.host, inputs.port);
                let req = if inputs.tls {
                    Request::builder()
                    .method(Method::POST)
                    .uri(format!("https://{}:{}/request_handler", inputs.host, inputs.port))
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                    .unwrap()
                }
                else {
                    Request::builder()
                    .method(Method::POST)
                    .uri(format!("http://{}:{}/request_handler", inputs.host, inputs.port))
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                    .unwrap()
                };

                let result = timeout(timeout_duration, client.request(req)).await;
                // wait for response from broker
                match result {
                    Ok(Ok(mut resp)) => {
                        trace!("Received response: {:?}", resp);
                        let m = collect_request(resp.body_mut()).await.unwrap();
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
                    Ok(Err(_e)) => {
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

            // define closure to send connections from server to heartbeat handler
            let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
                let _broker_addr_value = Arc::clone(&broker_addr);
                if inputs.tls {
                    let tls_clone = tls.clone().unwrap();
                    Box::pin(async move {
                        heartbeat_handler_helper(None, Some(req), None, None, Some(tls_clone)).await
                    }) as std::pin::Pin<Box<dyn Future<Output = 
                    Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
                }
                else {
                    Box::pin(async move {
                        heartbeat_handler_helper(None, Some(req), None, None, None).await
                    }) as std::pin::Pin<Box<dyn Future<Output = 
                    Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
                }
            }));

            // Spawn a thread to monitor the heartbeat
            let last_heartbeat_clone: Arc<Mutex<Option<Instant>>> = Arc::clone(&GLOBAL_LAST_HEARTBEAT);
            tokio::spawn(async move {
                sleep(Duration::from_millis(5000)).await;
                loop {
                    sleep(Duration::from_millis(500)).await;
                    let elapsed = {
                        let timer_loc = last_heartbeat_clone.lock().await;
                        if let Some(time) = *timer_loc {
                            time.elapsed()
                        } else {
                            continue;
                        }
                    };
                    if elapsed > Duration::from_secs(10) {
                        trace!("No heartbeat received for 10 seconds, exiting...");
                        std::process::exit(0);
                    }
                }
            });

            let handler_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();
            info!("Starting server on: {}:{}", host.to_string(), inputs.bind_port);
            // bind to address to send/receive heartbeats to/from broker
            let incoming = TcpListener::bind(&handler_addr).await.unwrap();

            // define closure to listen for incoming requests
            let service = service_fn(move |req: Request<Incoming>| {
                let handler_clone = Arc::clone(&handler);
                async move {
                    let loc_handler = handler_clone.lock().await.clone();
                    api_server(req, loc_handler).await
                }
            });
            // infinitely listen for requests
            loop {
                let (tcp_stream, _remote_addr) = incoming.accept().await.unwrap();
                let tls_acceptor = if inputs.tls {
                    tls_acceptor.clone()
                }
                else {
                    None
                };
                let service_clone = service.clone();
                tokio::task::spawn(async move {
                    // enter service with or without tls
                    match tls_acceptor {
                        Some(tls_acc) => {
                            match tls_acc.accept(tcp_stream).await {
                                Ok(tls_stream) => {
                                    if let Err(err) = Builder::new(TokioExecutor::new())
                                    .serve_connection(TokioIo::new(tls_stream), service_clone)
                                    .await {
                                        error!("Failed to serve connection: {:?}", err);
                                    }
                                },
                                Err(err) => {
                                    eprintln!("failed to perform tls handshake: {err:#}");
                                    return;
                                }
                            }
                        },
                        None => {
                            if let Err(err) = Builder::new(TokioExecutor::new())
                            .serve_connection(TokioIo::new(tcp_stream), service_clone)
                            .await {
                                error!("Failed to serve connection: {:?}", err);
                            }
                        }
                    };
                });
            }

        }
    }
    Ok(Response::new(Full::default()))

}

/// # Claim
/// Connect to broker and discover available address for data connection.
pub async fn claim(inputs: Claim, com: ComType) -> Result<Response<Full<Bytes>>, std::io::Error> {
    let ips = get_local_ips().await;

    let (ipstr, _all_ipstr) = if inputs.print_v4 {(
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ).await,
        get_matching_ipstr(& ips.ipv4_addrs, & inputs.name, & None).await
    )} else {(
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        ).await,
        get_matching_ipstr(& ips.ipv6_addrs, & inputs.name, & None).await
    )};

    let host = only_or_error(& ipstr);

    // define payload with metadata to send to broker
    let payload = serialize(& Payload {
        service_addr: ipstr.clone(),
        service_port: inputs.port,
        service_claim: epoch(),
        interface_addr: Vec::new(),
        bind_port: inputs.bind_port,
        key: inputs.key,
        id: 0,
        service_id: 0,
        root_ca: inputs.root_ca.clone(),
        ping: inputs.ping
    });

    let broker_addr = Arc::new(Mutex::new(Addr{
        host: inputs.host.clone(),
        port: inputs.port
    }));

    let msg = & Message{
        header: MessageHeader::CLAIM,
        body: payload.clone()
    };

    let mut read_fail = 0;
    let service_payload = Arc::new(Mutex::new("".to_string()));
    let timeout_duration = Duration::from_millis(6000);
    
    // start tcp or api process
    match com {
        ComType::TCP => {
            // connect to broker
            let stream = match connect(& Addr{
                host: inputs.host.clone(),
                port: inputs.port
            }).await{
                    Ok(s) => s,
                    Err(_e) => {
                        return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,"Connection unsuccessful. Try another key"));
                        }
            };
            let stream_mut = Arc::new(Mutex::new(stream));
            // send message to broker with metadata to add to event queue and state
            let ack = send(& stream_mut, msg).await;

            // check for successful connection to a published service
            match ack {
                Ok(m) => {
                    trace!("Received response: {:?}", m);
                    match m.header {
                        MessageHeader::ACK => {
                            let mut read_fail = 0;
                            let loc_stream = &mut stream_mut.lock().await;
                            // loop handles connection race case
                            loop {
                                let _ = sleep(Duration::from_millis(1000));
                                let message = match stream_read(loc_stream).await {
                                    Ok(m) => deserialize_message(& m),
                                    Err(err) => {return Err(err);}
                                };
                                trace!("{:?}", message);
                                // print service's address to client
                                if matches!(message.header, MessageHeader::ACK){
                                    info!("Server acknowledged CLAIM.");
                                    println!("Received payload: {}", message.body);
                                    let mut service_payload_loc = service_payload.lock().await;
                                    *service_payload_loc = message.body;
                                    break;
                                }
                                else{
                                    read_fail += 1;
                                    if read_fail > 5 {
                                        return Err(std::io::Error::new(
                                            std::io::ErrorKind::InvalidInput,"Key not found"));
                                    }
                                }
                            }
                        }
                        _ => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                format!("Server responds with unexpected message: {:?}", m),
                            ));
                        }
                    }
                }
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Encountered error: {:?}", e),
                    ));
                }
            }
            let payload_clone = Arc::clone(&service_payload);
            let broker_clone = Arc::clone(&broker_addr);

            // define closure to send incoming requests to heartbeat monitor
            let handler =  move |stream: Option<Arc<Mutex<TcpStream>>>| {
                let inner_payload = Arc::clone(&payload_clone);
                let inner_broker = Arc::clone(&broker_clone);
                // Shared state to track the last heartbeat time
                Box::pin(async move {
                    let payload_clone = Arc::clone(&inner_payload);
                    let broker_clone = Arc::clone(&inner_broker);
                    heartbeat_handler_helper(stream, None, Some(& payload_clone), Some(&broker_clone), None).await
                }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
            };

            let addr = Addr {
                host: host.to_string(),
                port: inputs.bind_port
            };

            // send/receive heartbeats to/from broker
            let _ = tcp_server(& addr, handler).await;
        },
        ComType::API => {
            // initialize tls configuration
            let tls : Option<rustls::ClientConfig>;
            let tls_acceptor = if inputs.tls {
                let server_config = tls_config().await.unwrap();
                let root_path = match env::var("ROOT_PATH") {
                    Ok(path) => Some(path),
                    Err(_) => None
                };
                let root_store = load_ca(root_path).await.unwrap();
                tls = Some(ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth());
                let _server_name = ServerName::try_from(inputs.host.clone())
                    .map_err(|_| format!("Invalid server DNS name: {}", inputs.host.clone())).unwrap();
                Some(TlsAcceptor::from(Arc::new(server_config)))
            }
            else {
                tls = None;
                None
            };

           // connect to broker
            // Prepare the HTTPS connector
            let client = if inputs.tls {
                let https_connector = HttpsConnectorBuilder::new()
                .with_tls_config(tls.clone().unwrap())
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            }
            else {
                let https_connector = HttpsConnectorBuilder::new()
                .with_native_roots().unwrap()
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            };

            // retry sending requests to broker with metadata to add to event queue and state
            loop {
                sleep(Duration::from_millis(1000)).await;
                trace!("sending request to {}:{}", inputs.host, inputs.port);

                let req = if inputs.tls {
                    Request::builder()
                    .method(Method::POST)
                    .uri(format!("https://{}:{}/request_handler", inputs.host, inputs.port))
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                    .unwrap()
                }
                else {
                    Request::builder()
                    .method(Method::POST)
                    .uri(format!("http://{}:{}/request_handler", inputs.host, inputs.port))
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                    .unwrap()
                };

                let result = timeout(timeout_duration, client.request(req)).await;

                // check for successful connection to a published service
                match result {
                    Ok(Ok(mut resp)) => {
                        trace!("Received response: {:?}", resp);
                        let m = collect_request(resp.body_mut()).await.unwrap();
                        // print service's address to client
                        if matches!(m.header, MessageHeader::ACK){
                            info!("Server acknowledged CLAIM.");
                            println!("Received payload: {}", m.body);
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
                    Ok(Err(_e)) => {
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
            
            // define closure to start heartbeats
            let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
                let service_payload_value = Arc::clone(&service_payload);
                let broker_addr_value = Arc::clone(&broker_addr);
                // check for tls config
                if inputs.tls {
                    let tls_clone = tls.clone().unwrap();
                    // check for one- or two-sided heartbeat
                    if inputs.ping {
                        Box::pin(async move {
                            ping_heartbeat(&service_payload_value, Some(&broker_addr_value), Some(tls_clone)).await
                        }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
                    }
                    else {
                        Box::pin(async move {
                            heartbeat_handler_helper(None, Some(req), Some(&service_payload_value), Some(&broker_addr_value), Some(tls_clone)).await
                        }) as std::pin::Pin<Box<dyn Future<Output = 
                        Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
                    }
                }
                else {
                    // check for one- or two-sided heartbeat
                    if inputs.ping {
                        Box::pin(async move {
                            ping_heartbeat(&service_payload_value, Some(&broker_addr_value), None).await
                        }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
                    }
                    else {
                        Box::pin(async move {
                            heartbeat_handler_helper(None, Some(req), Some(&service_payload_value), Some(&broker_addr_value), None).await
                        }) as std::pin::Pin<Box<dyn Future<Output = 
                        Result<Response<Full<Bytes>>, std::io::Error>> + std::marker::Send>>
                    }
                }
            }));

            let handler_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();

            info!("Starting server on: {}:{}", host.to_string(), inputs.bind_port);

            if !inputs.ping {
                // Spawn a thread to monitor the heartbeat
                let last_heartbeat_clone: Arc<Mutex<Option<Instant>>> = Arc::clone(&GLOBAL_LAST_HEARTBEAT);
                tokio::spawn(async move {
                    sleep(Duration::from_millis(5000)).await;
                    loop {
                        sleep(Duration::from_millis(500)).await;
                        let elapsed = {
                            let timer_loc = last_heartbeat_clone.lock().await;
                            if let Some(time) = *timer_loc {
                                time.elapsed()
                            } else {
                                continue;
                            }
                        };
                        if elapsed > Duration::from_secs(10) {
                            trace!("No heartbeat received for 10 seconds, exiting...");
                            std::process::exit(0);
                        }
                    }
                });
            }
            // send/receive heartbeats to/from broker
            let incoming = TcpListener::bind(&handler_addr).await.unwrap();
            let service = service_fn(move |req: Request<Incoming>| {
                let handler_clone = Arc::clone(&handler);
                async move {
                    let loc_handler = handler_clone.lock().await.clone();
                    api_server(req, loc_handler).await
                }
            });
            
            // send request to start one-sided heartbeat 
            if inputs.ping {
                tokio::spawn(async move {
                    let client = setup_https_client(inputs.root_ca.clone()).await;
                    let mut read_fail = 0;
                    loop {
                        sleep(Duration::from_millis(1000)).await;
                        trace!("sending request to {}:{}", inputs.host, inputs.bind_port);
        
                        let req = if inputs.tls {
                            Request::builder()
                            .method(Method::GET)
                            .uri(format!("https://{}:{}/heartbeat_handler", inputs.host, inputs.bind_port))
                            .header(hyper::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(Bytes::from("".to_string())))
                            .unwrap()
                        }
                        else {
                            Request::builder()
                            .method(Method::GET)
                            .uri(format!("http://{}:{}/heartbeat_handler", inputs.host, inputs.bind_port))
                            .header(hyper::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(Bytes::from("".to_string())))
                            .unwrap()
                        };
        
                        let result = timeout(timeout_duration, client.request(req)).await;
        
                        // check for successful connection to a published service
                        match result {
                            Ok(Ok(resp)) => {
                                trace!("Received ping response: {:?}", resp);
                                break;
                            }
                            Ok(Err(_e)) => {
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
                });
            }
            // infinitely wait for incoming requests
            loop {
                let (tcp_stream, _remote_addr) = incoming.accept().await.unwrap();
                let tls_acceptor = if inputs.tls {
                    tls_acceptor.clone()
                }
                else {
                    None
                };
                let service_clone = service.clone();
                tokio::task::spawn(async move {
                    // enter service with or without tls
                    match tls_acceptor {
                        Some(tls_acc) => {
                            match tls_acc.accept(tcp_stream).await {
                                Ok(tls_stream) => {
                                    if let Err(err) = Builder::new(TokioExecutor::new())
                                    .serve_connection(TokioIo::new(tls_stream), service_clone)
                                    .await {
                                        error!("Failed to serve connection: {:?}", err);
                                    }
                                },
                                Err(err) => {
                                    eprintln!("failed to perform tls handshake: {err:#}");
                                    return;
                                }
                            }
                        },
                        None => {
                            if let Err(err) = Builder::new(TokioExecutor::new())
                            .serve_connection(TokioIo::new(tcp_stream), service_clone)
                            .await {
                                error!("Failed to serve connection: {:?}", err);
                            }
                        }
                    };
                });
            }
        }
    }


    Ok(Response::new(Full::default()))
}

/// # Claim
/// Connect to client to obtain the address of the published service
/// Connect to published service to obtain message sent from the client
pub async fn collect(inputs: Collect, com: ComType) -> Result<(), std::io::Error> {
    // Set a process wide default crypto provider.
    #[cfg(feature = "ring")]
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "aws-lc-rs")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let msg = & Message{
        header: MessageHeader::COL,
        body: "".to_string()
    };
    // enter tcp or api process
    match com {
        ComType::TCP => {
            let addr = Addr {
                host: inputs.host,
                port: inputs.port
            };

            // connect to published service or client, using bind ports
            let stream = match connect(& addr).await{
                Ok(s) => s,
                Err(_e) => {
                    return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,"Connection unsuccessful"));
                    }
            };        
            trace!("Connection to {:?} successful", addr);

            let stream_mut = Arc::new(Mutex::new(stream));

            // send message with COL header to signal collection
            let received = send(& stream_mut, msg).await;
        
            // check for a valid return 
            let _payload = match received {
                Ok(message) => {
                    if message.body.is_empty() {
                        panic!("Payload not found in heartbeat.");
                    }
                    println!("{:?}", message.body);
                }
                Err(_err) => return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput, "Failed to collect message."))
            };
        },
        ComType::API => {
            // configure tls
            let tls : Option<rustls::ClientConfig> = if inputs.tls {
                let root_path = env::var("ROOT_PATH").expect("ROOT_PATH not set");
                let root_store = load_ca(Some(root_path)).await.unwrap();
                Some(ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth())
                }
            else {
                None
            };

            // connect to bind port of service or client
            // Prepare the HTTPS connector
            let client = if inputs.tls {
                let https_connector = HttpsConnectorBuilder::new()
                .with_tls_config(tls.clone().unwrap())
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            }
            else {
                let https_connector = HttpsConnectorBuilder::new()
                .with_native_roots().unwrap()
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            };

            let timeout_duration = Duration::from_millis(6000);
        
            // send request to server
            let req = if inputs.tls {
                Request::builder()
                .method(Method::GET)
                .uri(format!("https://{}:{}/heartbeat_handler", inputs.host, inputs.port))
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                .unwrap()
            }
            else {
                Request::builder()
                .method(Method::GET)
                .uri(format!("http://{}:{}/heartbeat_handler", inputs.host, inputs.port))
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                .unwrap()
            };
        
            let result = timeout(timeout_duration, client.request(req)).await;
        
            trace!("sending request to {}:{}", inputs.host, inputs.port);
        
            // check valid response and return message to terminal
            match result {
                Ok(Ok(mut resp)) => {
                    trace!("Received response: {:?}", resp);
                    let m = collect_request(resp.body_mut()).await.unwrap();
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
        }
    }

    Ok(())
}

/// # Send 
/// Connect to client to send a message through the broker to the internal published service
pub async fn send_msg(inputs: SendMSG, com: ComType) -> Result<(), std::io::Error> {
    // Set a process wide default crypto provider.
    #[cfg(feature = "ring")]
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "aws-lc-rs")]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let msg = & Message{
        header: MessageHeader::MSG,
        body: serde_json::to_string(&inputs.msg).unwrap()
    };

    // start tcp or api process
    match com {
        ComType::TCP => {
            let addr = Addr {
                host: inputs.host,
                port: inputs.port
            };
            // connect to bind port of service or client
            let stream = match connect(& addr).await{
                Ok(s) => s,
                Err(_e) => {
                    return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,"Connection unsuccessful"));
                }
            };  

            let stream_mut = Arc::new(Mutex::new(stream));

            // send message to client
            let received = send(& stream_mut, msg).await;
        
            let _ = match received {
                Ok(message) => trace!("Received {:?}", message),
                Err(_err) => error!("Failed to collect message.")
            };
        },
        ComType::API => {
            // setup tls configuration
            let tls : Option<rustls::ClientConfig> = if inputs.tls {
                // let server_config = tls_config().await.unwrap();
                let root_path = env::var("ROOT_PATH").expect("ROOT_PATH not set");
                let root_store = load_ca(Some(root_path)).await.unwrap();
                Some(ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth())
                // Some(TlsAcceptor::from(Arc::new(server_config)))
            }
            else {
                None
            };

            // setup http connection
            let client = if inputs.tls {
                let https_connector = HttpsConnectorBuilder::new()
                .with_tls_config(tls.clone().unwrap())
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            }
            else {
                let https_connector = HttpsConnectorBuilder::new()
                .with_native_roots().unwrap()
                .https_or_http()
                .enable_http1()
                .build();
                Client::builder(TokioExecutor::new()).build(https_connector)
            };
            let timeout_duration = Duration::from_millis(6000);
            // send request to client with message in the body
            trace!("sending request to {}:{}", inputs.host, inputs.port);
            let req = if inputs.tls {
                Request::builder()
                .method(Method::POST)
                .uri(format!("https://{}:{}/request_handler", inputs.host, inputs.port))
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                .unwrap()
            }
            else {
                Request::builder()
                .method(Method::POST)
                .uri(format!("http://{}:{}/request_handler", inputs.host, inputs.port))
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(serialize_message(& msg.clone()))))
                .unwrap()
            };
        
            let result = timeout(timeout_duration, client.request(req)).await;
        
            // check for a valid response
            match result {
                Ok(Ok(mut resp)) => {
                    trace!("Received response: {:?}", resp);
                    let m = collect_request(resp.body_mut()).await.unwrap();
                    match m.header {
                        MessageHeader::ACK => info!("Request acknowledged: {:?}", m),
                        _ => warn!("Server responds with unexpected message: {:?}", m),
                    }
                }
                Ok(Err(_e)) => error!("Failed to collect message."),
                Err(_e) => error!("Timed out reading response")
            }
        }
    }

    Ok(())    
}
