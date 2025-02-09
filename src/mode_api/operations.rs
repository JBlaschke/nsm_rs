use crate::connection::api_server;
use crate::service::{request_handler, event_monitor};
use crate::tls::tls_config;
use crate::operations::{AMState, HttpResult};

use std::sync::Arc;
use std::net::SocketAddr;
use std::future::Future;
use std::pin::Pin;
use std::marker::Send;
use hyper::http::Request; 
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_rustls::TlsAcceptor;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};


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
