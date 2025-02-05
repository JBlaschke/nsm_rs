use crate::connection::{Addr, tcp_server};
use crate::service::{State, request_handler, event_monitor};

use std::sync::Arc;
use std::future::Future;
use http_body_util::Full;
use hyper::http::{Response};
use hyper::body::{Bytes};
use tokio::sync::Notify;
use tokio::net::{TcpStream};
use tokio::sync::Mutex;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

type AMState = Arc<(Mutex<State>, Notify)>;

pub async fn listen(state_clone: AMState, host: &String, bind_port: i32) -> () {
    
    let state_clones = Arc::clone(& state_clone);

    // define handler closure to start request_handler within the tcp
    // server function
    // TODO: this should not need Http responses -- right/
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
        port: bind_port
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
