use crate::connection::{Addr, tcp_server};
use crate::service::{request_handler, event_monitor};
use crate::operations::{AMState, HttpResult};

use std::pin::Pin;
use std::sync::Arc;
use std::future::Future;
use tokio::net::{TcpStream};
use tokio::sync::Mutex;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};


pub async fn listen(state: AMState, host: &String, bind_port: i32) -> () {
    trace!("Entering TCP listen for host: {:?}:{:?}", host, bind_port);

    let state_cl_req = Arc::clone(&state);

    // define handler closure to start request_handler within the tcp server
    // function. Note: this should will return an HTTP response provided the
    // last argument is not none
    let handler = move |stream: Option<Arc<Mutex<TcpStream>>>| {
        let state_clone_inner = Arc::clone(&state_cl_req);
        Box::pin(async move {
            request_handler(&state_clone_inner, stream, None).await
        }) as Pin<Box<dyn Future<Output=HttpResult> + Send>>
    };

    let addr = Addr::new(&host, bind_port);

    info!("Starting listener started on: {}:{}", &addr.host, addr.port);

    // thread handles incoming tcp connections and adds them to State
    // and event queue
    let _thread_handler = tokio::spawn(async move {
        let _ = tcp_server(& addr, handler).await;
    });

    let state_cl_event = Arc::clone(&state);

    // send State into event monitor to handle heartbeat queue
    let event_loop = tokio::spawn(async move {
        let _ = match event_monitor(state_cl_event).await{
            Ok(_resp) => debug!("exited event monitor"),
            Err(_) => debug!("event monitor error")
        };
    });
    let _ = event_loop.await;
}
