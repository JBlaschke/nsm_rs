use crate::connection::{Addr, Message, api_server};
use crate::service::{request_handler, event_monitor};
use crate::tls::{tls_config, tls_acceptor};
use crate::operations::{AMState, HttpResult};

use std::sync::Arc;
use std::net::SocketAddr;
use std::future::Future;
use std::pin::Pin;
use std::marker::Send;
use rustls::ClientConfig;
use hyper::http::Request; 
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::client::legacy::Client; // TODO: can we do without legacy?
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout, Duration, Instant};
use tokio_rustls::TlsAcceptor;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};


// TODO: Use Addr; handle errors
pub async fn listen(
        state: AMState, host: &String, bind_port: i32, tls: bool
    ) -> () {
    trace!("Entering HTTP listen for host: {:?}:{:?}", host, bind_port);

    // initiate tls configuration
    let tls_acceptor: Option<TlsAcceptor> = if tls {
        let server_config = tls_config().await.unwrap();
        Some(TlsAcceptor::from(Arc::new(server_config)))
    } else {
        None
    };

    let state_cl_event = Arc::clone(&state);

    // send State into event monitor to handle heartbeat queue
    let _event_loop = tokio::spawn(async move {
        let _ = match event_monitor(state_cl_event).await{
            Ok(_resp) => trace!("exited event monitor"),
            Err(_) => trace!("event monitor error")
        };
    });

    let state_cl_req = Arc::clone(&state);

    // TODO: too many moves?
    // define handler closure to start request_handler within the api
    // server function
    let handler = Arc::new(Mutex::new(move |req: Request<Incoming>| {
        let state_clone_inner = Arc::clone(&state_cl_req);
        Box::pin(async move {
            request_handler(&state_clone_inner, None, Some(req)).await
        }) as Pin<Box<dyn Future<Output=HttpResult> + Send>>
    }));

    let server_addr: SocketAddr = format!(
        "{}:{}", host.to_string(), bind_port
    ).parse().unwrap();
    
    // bind to host address to listen for requests
    let incoming = TcpListener::bind(&server_addr).await.unwrap();
    info!("Listening on: {}:{}", host.to_string(), bind_port);

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
        let tls_acceptor = if tls {
            tls_acceptor.clone()
        } else {
            None
        };
        // thread handles incoming connections and adds them to State
        // and event queue
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
                            eprintln!("Failed to perform tls handshake: {err:#}");
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


pub fn https_connector(tls: Option<&ClientConfig>) -> () {

    let https_connector = match tls {
        Some(tls_data) => {
            HttpsConnectorBuilder::new()
            .with_tls_config()
            .https_or_http()
            .enable_http1()
            .build()
        },
        None => {
            HttpsConnectorBuilder::new()
            .with_native_roots().unwrap()
            .https_or_http()
            .enable_http1()
            .build()
        }
    };

    Client::builder(TokioExecutor::new()).build(https_connector)
}


// TODO: Infer TLS usage from Addr?
pub async fn publish(
        msg: &Message, host: &Addr, local: &Addr, use_tls: bool
    ) -> () {

    trace!("Entering HTTP publish for host: {}", host);


    // // start tls configuration
    // let tls: Option<rustls::ClientConfig>;
    // // Use the url crate to parse the URL
    // let parsed_url = url::Url::parse(&inputs.host.to_string()).unwrap();
    // let tls_acceptor = if inputs.tls {
    //     trace!("entering tls config");
    //     let server_config = tls_config().await.unwrap();
    //     let root_path = match env::var("ROOT_PATH") {
    //         Ok(path) => Some(path),
    //         Err(_) => None
    //     };
    //     let root_store = load_ca(root_path).await.unwrap();
    //     tls = Some(ClientConfig::builder()
    //         .with_root_certificates(root_store)
    //         .with_no_client_auth());
    //     let _server_name = ServerName::try_from(
    //         parsed_url.host_str().unwrap()
    //     ).map_err(|_|
    //         format!(
    //             "Invalid server DNS name: {}",
    //             parsed_url.host_str().unwrap()
    //         )
    //     ).unwrap();
    //     Some(TlsAcceptor::from(Arc::new(server_config)))
    // } else {
    //     tls = None;
    //     None
    // };

    let (tls, tls_acceptor) = if use_tls {
        tls_acceptor()?
    } else {
        (None, None)
    };

    // connect to broker

    // // Prepare the HTTPS connector
    // let client = if inputs.tls {
    //     let https_connector = HttpsConnectorBuilder::new()
    //     .with_tls_config(tls.clone().unwrap())
    //     .https_or_http()
    //     .enable_http1()
    //     .build();
    //     Client::builder(TokioExecutor::new()).build(https_connector)
    // } else {
    //     let https_connector = HttpsConnectorBuilder::new()
    //     .with_native_roots().unwrap()
    //     .https_or_http()
    //     .enable_http1()
    //     .build();
    //     Client::builder(TokioExecutor::new()).build(https_connector)
    // };

    let client = https_connector(tls);

    let mut read_fail = 0;
    let timeout_duration = Duration::from_millis(6000);
    let request_target = String::from(
        parsed_url.join("request_handler").unwrap()
    );
    // retry sending requests to broker to add itself to event queue and state
    loop {
        sleep(Duration::from_millis(1000)).await;
        trace!("Sending request to {:?}", &request_target);
        let req = Request::builder()
            .method(Method::POST)
            .uri(&request_target)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(serialize_message(&msg.clone()))))
            .unwrap();

        let result = timeout(timeout_duration, client.request(req)).await;
        println!("{:?}", result);
        // wait for response from broker
        match result {
            Ok(Ok(mut resp)) => {
                trace!("Received response: {:?}", resp);
                let m = collect_request(resp.body_mut()).await.unwrap();
                match m.header {
                    MessageHeader::ACK => {
                        info!("Server acknowledged PUB.");
                        trace!("payload - {:?}", m.body.clone());
                        let mut deser_payload = deserialize(&payload.clone());
                        deser_payload.id = m.body.parse().unwrap();
                        deser_payload.service_id = m.body.parse().unwrap();
                        payload = serialize(&deser_payload);
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

    // TODO: right now, this handler only sends pings -- also enable
    // other endpoints in the future.
    let handler = Arc::new(Mutex::new(move |_req: Request<Incoming>| {
        // TODO: We'll need to deal with the two-sided HTTP(S) heatbeat
        // handoff -- I've left the old hearbeat handler code here for
        // reference, but this needs to be upated to be able to handle
        // multiple endpoints
        // Box::pin(async move {
        //     heartbeat_handler_helper(
        //         None, Some(req), None, None, Some(tls_clone)
        //     ).await
        // }) as Pin<Box<dyn Future<Output=HttpResult> + Send>>
        let mut response = Response::new(Full::default());
        *response.status_mut() = StatusCode::BAD_REQUEST;
        *response.body_mut()   = Full::from("Not implemented");
        // TODO: Replace this with real API server
        Box::pin(async move {
            Ok(response)
        }) as Pin<Box<dyn Future<Output=HttpResult> + Send>>
    }));

    if !inputs.ping {
        // Spawn a thread to monitor the heartbeat
        let last_heartbeat_clone: Arc<Mutex<Option<Instant>>> = Arc::clone(
            &GLOBAL_LAST_HEARTBEAT
        );
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
                    std::process::exit(0); // TODO: Don't exist proc insitu
                }
            }
        });
    }

    let handler_addr: SocketAddr = format!(
        "{}:{}", host.to_string(), inputs.bind_port
    ).parse().unwrap();

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

    if inputs.ping {
        tokio::spawn(async move {
            let _ = ping_heartbeat(
                &Arc::new(Mutex::new(payload.clone())),
                Some(&Arc::new(Mutex::new(inputs.host.clone()))),
                tls.clone()
            ).await;
        });
    };

    // infinitely listen for requests
    loop {
        let (tcp_stream, _remote_addr) = incoming.accept().await.unwrap();
        let tls_acceptor = if inputs.tls {
            tls_acceptor.clone()
        } else {
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
                            .serve_connection(
                                TokioIo::new(tls_stream), service_clone
                            ).await {
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
