/// Handles incoming connections and sending/receiving messages

use std::fmt;
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::net::SocketAddr;
use hyper::http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming, Buf};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};
use std::future::Future;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

/// Store host and port for new connections
#[derive(Debug, Clone)]
pub struct Addr {
    /// IP Address
    pub host: String,
    /// Port number
    pub port: i32
}

/// Identify type of Message to handle it properly
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum MessageHeader {
    /// heartbeat
    HB,
    /// acknowledgement
    ACK,
    /// body contains Publish payload
    PUB,
    /// body contains Claim payload
    CLAIM,
    /// collect message
    COL,
    /// user-defined message
    MSG,
    /// empty message
    NULL
}


impl fmt::Display for MessageHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MessageHeader::HB    => write!(f, "HB"),
            MessageHeader::ACK   => write!(f, "ACK"),
            MessageHeader::PUB   => write!(f, "PUB"),
            MessageHeader::CLAIM => write!(f, "CLAIM"),
            MessageHeader::COL   => write!(f, "COL"),
            MessageHeader::MSG   => write!(f, "MSG"),
            MessageHeader::NULL  => write!(f, "NULL"),
       }
    }
}

/// Send messages in body that are identified by MessageHeader
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    /// Specify type of Message
    pub header: MessageHeader,
    /// Contains contents of Message
    pub body: String
}

/// Serialize Message struct into JSON String 
pub fn serialize_message(payload: & Message) -> String {
    serde_json::to_string(payload).unwrap()
}

/// Deserialize JSON String into Message struct
pub fn deserialize_message(payload: & String) -> Message {
    serde_json::from_str(payload).unwrap()
}

/// Binds to stream and listens for incoming connections, then handles connection using specified handler
pub async fn server(
    request: Request<Incoming>, 
    mut handler: impl FnMut(Request<Incoming>)-> std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, hyper::Error>> + std::marker::Send>> + std::marker::Send + 'static + Clone
) -> Result<Response<Full<Bytes>>, hyper::Error> {

    // // Shared state to track the last heartbeat time
    // let last_heartbeat: Arc<Mutex<Option<Instant>>>= Arc::new(Mutex::new(None));

    // // Spawn a thread to monitor the heartbeat
    // let last_heartbeat_clone = Arc::clone(&last_heartbeat);
    // thread::spawn(move || {
    //     loop {
    //         thread::sleep(Duration::from_millis(500));
    //         let elapsed = {
    //             let timer_loc = last_heartbeat_clone.lock().unwrap();
    //             if let Some(time) = *timer_loc {
    //                 time.elapsed()
    //             } else {
    //                 continue;
    //             }
    //         };
    //         if elapsed > Duration::from_secs(10) {
    //             trace!("No heartbeat received for 10 seconds, exiting...");
    //             std::process::exit(0);
    //         }
    //     }
    // });
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let mut response = Response::new(Full::default());

    match (method, path.as_str()) {
        (Method::POST, p) if p.starts_with("/request_handler") => {
            info!("received request");
            handler(request).await
        },
        (Method::GET, p) if p.starts_with("/heartbeat_handler") => {
            // {
            //     let mut last_heartbeat = last_heartbeat.lock().unwrap();
            //     *last_heartbeat = Some(Instant::now());
            //     println!("Heartbeat received, time updated.");
            // }
            info!("received hb request");
            handler(request).await
        },
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
            Ok(response)
        }
    }
}