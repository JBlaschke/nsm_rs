use crate::connection::{
    Addr, Message, MessageHeader, tcp_server, connect, send
};
use crate::service::{heartbeat_handler_helper, request_handler, event_monitor};
use crate::operations::{AMState, HttpResult};

use std::io::{ErrorKind, Error};
use std::pin::Pin;
use std::sync::Arc;
use std::future::Future;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};


// TODO: don't use host + ip => use Addr instead; handle errors
pub async fn listen(state: AMState, host: &String, bind_port: i32) -> () {
    let addr = Addr::new(&host, bind_port);

    trace!("Starting TCP listen for host: {}", addr);

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

    info!("Starting listener on: {}", addr);

    // thread handles incoming tcp connections and adds them to State
    // and event queue
    let _thread_handler = tokio::spawn(async move {
        let _ = tcp_server(&addr, handler).await;
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


pub async fn publish(
        payload: &mut String, host: &Addr, local: &Addr
    ) -> Result<(), Error> {

    trace!("Starting TCP publish for host: {}", local);

    let msg = &Message{
        header: MessageHeader::PUB,
        body: payload.clone()
    };

    // connect to broker
    let stream = match connect(host).await {
        Ok(s) => s,
        Err(e) => {
            error!("Encountered {} while connecting to: {}", e, host);
            return Err(e);
        }
    };
    let stream_mut = Arc::new(Mutex::new(stream));
    // send broker identification and add itself to event queue and state
    let ack = send(&stream_mut, msg).await;

    // check for successful connection
    match ack {
        Ok(m) => {
            trace!("Received response: {:?}", m);
            match m.header {
                MessageHeader::ACK => {
                    info!("Server acknowledged PUB.")
                }
                _ => {
                    error!("Server responds with unexpected header: {:?}", m);
                    return Err(Error::new(
                        ErrorKind::InvalidInput, "Invalid Header"
                    ));
                }
            }
        }
        Err(e) => {
            error!("Encountered error: {:?}", e);
            return Err(e);
        }
    }

    // define closure to send connections from server to heartbeat handler
    let handler = move |stream: Option<Arc<Mutex<TcpStream>>>| {
        Box::pin(async move {
            return heartbeat_handler_helper(
                stream, None, None, None, None
            ).await
        }) as Pin<Box<dyn Future<Output=HttpResult> + Send>>
    };

    // send/receive heartbeats to/from broker
    let _ = tcp_server(local, handler).await;

    Ok(())
}
