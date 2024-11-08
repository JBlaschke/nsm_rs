/// Handles incoming connections and sending/receiving messages

use std::fmt;
use serde::{Serialize, Deserialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::Arc;
use tokio::time::{timeout, Instant, Duration};
use std::net::SocketAddr;
use hyper::http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming, Buf};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::{TcpStream, TcpListener};
use tokio::sync::{Mutex, Notify};
use std::future::Future;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::operations::GLOBAL_LAST_HEARTBEAT;

/// Store host and port for new connections
#[derive(Debug, Clone)]
pub struct Addr {
    /// IP Address
    pub host: String,
    /// Port number
    pub port: i32
}

/// Specify tcp or api communication
pub enum ComType {
    TCP,
    API
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

/// Connect to address using Addr struct, returns Result of connected TCPStream
pub async fn connect(addr: &Addr) -> std::io::Result<TcpStream> {
    TcpStream::connect(format!("{}:{}", addr.host, addr.port)).await
}

/// Write a message through a TCPStream, returns Result of # of bytes written or err
pub async fn stream_write(stream: &mut TcpStream, msg: & str) -> std::io::Result<usize> {
    match stream.write(msg.as_bytes()).await {
        Ok(n) => Ok(n),
        Err(err) => Err(err)
    }
}

/// Read a message from TCPStream, returns Result of # of bytes read or err
pub async fn stream_read(stream: &mut TcpStream) -> std::io::Result<String>{
    let mut buf = [0; 1024];
    let mut message = String::new();
    let failure_duration = Duration::from_secs(6);
    loop {
        let bytes_read = match timeout(failure_duration, stream.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            Ok(Err(err)) => return Err(err),
            Err(_) => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Read timed out"))
        };
        let s = std::str::from_utf8(&buf[..bytes_read]).unwrap();
        message.push_str(s);

        if bytes_read < buf.len() {
            break;
        }
    }

    Ok(message)
}

/// Sends a message in a TCPStream using stream_write(), checks for response
/// if message not an ACK, returns Result of Message received
pub async fn send(stream: & Arc<Mutex<TcpStream>>, msg: & Message) -> Result<Message, std::io::Error> {

    let loc_stream: &mut TcpStream = &mut *stream.lock().await;
    let _ = stream_write(loc_stream, & serialize_message(msg)).await;

    if matches!(msg.header, MessageHeader::ACK){
        return Ok(Message {
            header: MessageHeader::NULL,
            body: "".to_string()
        })
    }

    let response = match stream_read(loc_stream).await {
        Ok(message) => deserialize_message(& message),
        Err(err) => {return Err(err);}
    };

    Ok(response)
}

/// Receives a message through a TCPStream using stream_read(), writes an ACK
/// if received message not a HB or ACK
pub async fn receive(stream: & Arc<Mutex<TcpStream>>) -> Result<Message, std::io::Error> {

    let loc_stream: &mut TcpStream = &mut *stream.lock().await;

    let response = match stream_read(loc_stream).await {
        Ok(message) => deserialize_message(& message),
        Err(err) => {return Err(err);}
    };

    if matches!(response.header, MessageHeader::ACK)
    || matches!(response.header, MessageHeader::HB) {
        return Ok(response);
    }

    let _ = stream_write(loc_stream, & serialize_message(
        & Message {
            header: MessageHeader::ACK,
            body: "".to_string()
        }
    )).await;

    Ok(response)
}

pub async fn collect_request(request: &mut Incoming) -> Result<Message, std::io::Error>{
    let whole_body = request.collect().await.unwrap().aggregate();
    let mut data: serde_json::Value = serde_json::from_reader(whole_body.reader()).unwrap();
    let json = serde_json::to_string(&data).unwrap();
    let message = deserialize_message(& json);
    Ok(message)
}

/// Binds to stream and listens for incoming connections, then handles connection using specified handler
pub async fn api_server(
    request: Request<Incoming>, 
    mut handler: impl FnMut(Request<Incoming>)
     -> std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, 
     std::io::Error>> + std::marker::Send>> + std::marker::Send + 'static + Clone
) -> Result<Response<Full<Bytes>>, std::io::Error> {

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let mut response = Response::new(Full::default());

    match (method, path.as_str()) {
        (Method::POST, p) if p.starts_with("/request_handler") => {
            info!("received request");
            handler(request).await
        },
        (Method::GET, p) if p.starts_with("/heartbeat_handler") => {
            {
                let mut last_heartbeat = GLOBAL_LAST_HEARTBEAT.lock().await;
                *last_heartbeat = Some(Instant::now());
                println!("Heartbeat received, time updated.");
            }
            info!("received hb request");
            handler(request).await
        },
        _ => {
            // Return 404 not found response.
            *response.status_mut() = StatusCode::NOT_FOUND;
            Ok(response)
        }
    }
}

/// Binds to stream and listens for incoming connections, then handles connection using specified handler
pub async fn tcp_server(
    addr: &Addr, 
    mut handler: impl FnMut(Option<Arc<Mutex<TcpStream>>>)
    -> std::pin::Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, std::io::Error>>
     + std::marker::Send>> + std::marker::Send + 'static + Clone
) -> Result<(), std::io::Error> {
    trace!("Starting server process on: {:?}", addr);

    let listener = TcpListener::bind(format!("{}:{}", addr.host, addr.port)).await.unwrap();
    trace!("Bind to {:?} successful", addr);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                info!("Passing TCP connection to handler...");
                let shared_stream = Arc::new(Mutex::new(stream));
                let _ = handler(Some(shared_stream)).await; 
            },
            Err(e) => {
                error!("Error: {}", e);
            }        
        }
    }

    Ok(())
}