use crate::network::{get_local_ips, get_matching_ipstr};

use crate::connection::{Message, MessageHeader, Addr, server,
    serialize_message, deserialize_message};

use crate::service::{Payload, State, serialize, request_handler, heartbeat_handler_helper, event_monitor};

use crate::utils::{only_or_error, epoch};

use crate::models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, Send};

use std::thread;
use std::sync::{Arc, Mutex, Condvar};
use std::thread::sleep;
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
use tokio::time::{timeout, Duration, Instant};
use std::future::Future;

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

    let ips = get_local_ips();

    trace!("Start setting up listener...");

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
    
    let host = only_or_error(& ipstr);

    // initialize State struct
    let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
    let state_clone = Arc::clone(& state);

    let addr = Arc::new(Mutex::new(Addr {
        host: host.to_string(),
        port: inputs.bind_port
    }));

    let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
        let state_clone_inner = Arc::clone(& state_clone);
        Box::pin(async move {
            request_handler(& state_clone_inner, req).await
        }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, hyper::Error>> + std::marker::Send>>
    }));

    let server_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();

    info!("Starting listener started on: {}:{}", host.to_string(), inputs.bind_port);

    // thread handles incoming connections and adds them to State and event queue
    tokio::task::spawn(async move {
        let incoming = TcpListener::bind(&server_addr).await.unwrap();
        loop {
            let (stream, _) = incoming.accept().await.unwrap();
            let loc_addr = {
                addr.lock().unwrap().clone()
            };
            let loc_handler = {
                handler.lock().unwrap().clone()
            };
            let service = service_fn(move |req| {
                let addr_clone = loc_addr.clone();
                let handler_clone = loc_handler.clone();
                async move {
                    server(req, addr_clone, handler_clone).await
                }
            });

            if let Err(err) = Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(stream), service)
            .await {
                error!("Failed to serve connection: {:?}", err);
            }
        }
    });

    trace!("entering event_monitor");
    // send State struct holding event queue into event monitor to handle heartbeats
    let _ = match event_monitor(state).await{
        Ok(resp) => println!("exited event monitor"),
        Err(_) => println!("event monitor error")
    };

    Ok(Response::new(Full::default()))
}

pub async fn publish(inputs: Publish) -> Result<Response<Full<Bytes>>, hyper::Error> {

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
        .with_native_roots().unwrap()
        .https_or_http()
        .enable_http1()
        .build();

    // Build the Client using the HttpsConnector
    let client = Client::builder(TokioExecutor::new()).build(https_connector);

    let msg = serialize_message(& Message{
        header: MessageHeader::PUB,
        body: payload
    });

    let mut read_fail = 0;
    let timeout_duration = Duration::from_millis(6000);
    loop {
        sleep(Duration::from_millis(1000));
        trace!("sending request to {}:{}", inputs.host, inputs.port);

        let req = Request::builder()
        .method(Method::POST)
        .uri(format!("http://{}:{}/request_handler", inputs.host, inputs.port))
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
            _ => {
                read_fail += 1;
                if read_fail > 5 {
                    panic!("Failed to send request to listener")
                }
            },
        }
    }

    let host = only_or_error(& ipstr);
    let addr = Arc::new(Mutex::new(Addr {
        host: host.to_string(),
        port: inputs.bind_port
    }));

    let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
        Box::pin(async move {
            heartbeat_handler_helper(req, None, None).await
        }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, hyper::Error>> + std::marker::Send>>
    }));

    let handler_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();

    info!("Starting server on: {}:{}", host.to_string(), inputs.bind_port);

    // send/receive heartbeats to/from broker
    let incoming = TcpListener::bind(&handler_addr).await.unwrap();
    loop {
        let (stream, _) = incoming.accept().await.unwrap();
        let loc_addr = addr.lock().unwrap().clone();
        let loc_handler = handler.lock().unwrap().clone();
        let service = service_fn(move |req| {
            let addr_clone = loc_addr.clone();
            let handler_clone = loc_handler.clone();
            async move {
                server(req, addr_clone, handler_clone).await
            }
        });

        if let Err(err) = Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(stream), service)
        .await {
            error!("Failed to serve connection: {:?}", err);
        }
    }

    Ok(Response::new(Full::default()))

}

pub async fn claim(inputs: Claim) -> Result<Response<Full<Bytes>>, hyper::Error> {

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
        .with_native_roots().unwrap()
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
    let timeout_duration = Duration::from_millis(2000);

    loop {
        sleep(Duration::from_millis(1000));
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
                    let mut service_payload_loc = service_payload.lock().unwrap();
                    *service_payload_loc = m.body;
                    break;
                } else{
                    panic!("Key not found. Try a different key.")
                }
            }
            _ => {
                read_fail += 1;
                if read_fail > 5 {
                    panic!("Failed to send request to listener")
                }
            },
        }
    }

    let broker_addr = Arc::new(Mutex::new(Addr{
        host: inputs.host,
        port: inputs.port
    }));

    let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
        let service_payload_value = {
            let shared_service_payload = Arc::clone(&service_payload);
            let service_payload_loc = shared_service_payload.lock().unwrap();
            service_payload_loc.clone()
        };
        let broker_addr_value = {
            let shared_broker_addr = Arc::clone(&broker_addr);
            let broker_addr_loc = shared_broker_addr.lock().unwrap();
            broker_addr_loc.clone()
        };
        Box::pin(async move {
            heartbeat_handler_helper(req, Some(&service_payload_value), Some(broker_addr_value)).await
        }) as std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, hyper::Error>> + std::marker::Send>>
    }));

    let host = only_or_error(& ipstr);
    let addr = Arc::new(Mutex::new(Addr {
        host: host.to_string(),
        port: inputs.bind_port
    }));

    let handler_addr: SocketAddr = format!("{}:{}", host.to_string(), inputs.bind_port).parse().unwrap();

    info!("Starting server on: {}:{}", host.to_string(), inputs.bind_port);

    // send/receive heartbeats to/from broker
    let incoming = TcpListener::bind(&handler_addr).await.unwrap();

    // // send/receive heartbeats to/from broker
    loop {
        let (stream, _) = incoming.accept().await.unwrap();
        let loc_addr = addr.lock().unwrap().clone();
        let loc_handler = handler.lock().unwrap().clone();
        let service = service_fn(move |req| {
            let addr_clone = loc_addr.clone();
            let handler_clone = loc_handler.clone();
            async move {
                server(req, addr_clone, handler_clone).await
            }
        });

        if let Err(err) = Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(stream), service)
        .await {
            error!("Failed to serve connection: {:?}", err);
        }
    }

    Ok(Response::new(Full::default()))
}

pub async fn collect(inputs: Collect) -> Result<Response<Full<Bytes>>, hyper::Error> {

    // // connect to bind port of service or client
    // let client = Client::new();
    // let msg = serialize_message(& Message{
    //     header: MessageHeader::COL,
    //     body: "".to_string()
    // });

    // println!("sending request to {}:{}", inputs.host, inputs.port);
    // let response = client
    // .post(format!("http://{}:{}/request_handler", inputs.host, inputs.port))
    // .body(msg)
    // .send();

    // match response {
    //     Ok(resp) => {
    //         trace!("Received response: {:?}", resp);
    //         let body = resp.text().unwrap();
    //         let m = deserialize_message(& body);
    //         match m.header {
    //             MessageHeader::ACK => {
    //                 info!("Request acknowledged.");
    //                 if m.body.is_empty() {
    //                     panic!("Payload not found in heartbeat.");
    //                 }
    //                 println!("{:?}", m.body);
    //             },
    //             _ => warn!("Server responds with unexpected message: {:?}", m),
    //         }
    //     }
    //     Err(_e) => return Err(std::io::Error::new(
    //         std::io::ErrorKind::InvalidInput, "Failed to collect message.")),
    // }

    Ok(Response::new(Full::default()))
}

pub async fn send_msg(inputs: Send) -> Result<Response<Full<Bytes>>, hyper::Error> {

    // // connect to bind port of service or client
    // let client = Client::new();
    // let msg = serialize_message(& Message{
    //     header: MessageHeader::MSG,
    //     body: inputs.msg
    // });

    // println!("sending request to {}:{}", inputs.host, inputs.port);
    // let response = client
    // .post(format!("http://{}:{}/request_handler", inputs.host, inputs.port))
    // .body(msg)
    // .send();

    // match response {
    //     Ok(resp) => {
    //         trace!("Received response: {:?}", resp);
    //         let body = resp.text().unwrap();
    //         let m = deserialize_message(& body);
    //         match m.header {
    //             MessageHeader::ACK => info!("Request acknowledged: {:?}", m),
    //             _ => warn!("Server responds with unexpected message: {:?}", m),
    //         }
    //     }
    //     Err(_e) => return Err(std::io::Error::new(
    //         std::io::ErrorKind::InvalidInput, "Failed to collect message.")),
    // }

    Ok(Response::new(Full::default()))    
}