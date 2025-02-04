
use crate::network::{get_local_ips, get_matching_ipstr};
use crate::connection::{
    ComType, Message, MessageHeader, Addr, api_server, tcp_server,
    serialize_message, deserialize_message, send, connect, collect_request,
    stream_read
};
use crate::service::{
    Payload, State, serialize, deserialize, request_handler,
    heartbeat_handler_helper, ping_heartbeat, event_monitor
};
use crate::utils::{only_or_error, epoch};
use crate::models::{
    ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, SendMSG
};
use crate::tls::{tls_config, load_ca, setup_https_client};

use std::{env, fs};
use std::sync::Arc;
use std::net::SocketAddr;
use std::future::Future;
use std::pin::Pin;
use std::marker::Send;
use std::io::Error;
use base64::{engine::general_purpose, Engine as _};
use http_body_util::Full;
use hyper::http::{Method, Request, Response};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::client::legacy::Client;
use hyper_rustls::HttpsConnectorBuilder;
use tokio::sync::Notify;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout, Duration, Instant};
use tokio::sync::Mutex;
use tokio_rustls::TlsAcceptor;
use rustls::client::ClientConfig;
use rustls::pki_types::ServerName;
use lazy_static::lazy_static;


pub async fn listen(state_clone) -> () {
    
    let state_clones = Arc::clone(& state_clone);

    // define handler closure to start request_handler within the tcp
    // server function
    let handler =  move |stream: Option<Arc<Mutex<TcpStream>>>| {
        let state_clone_inner = Arc::clone(& state_clones);
        Box::pin(async move {
            request_handler(& state_clone_inner, stream, None).await
        }) as std::pin::Pin<Box<
            dyn Future<Output = Result<Response<Full<Bytes>>,
            std::io::Error>> + std::marker::Send
        >>
    };

    let addr = Addr {
        host: host.to_string(),
        port: inputs.bind_port
    };

    info!("Starting listener started on: {}:{}", &addr.host, addr.port);

    // thread handles incoming tcp connections and adds them to State
    // and event queue
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
}
