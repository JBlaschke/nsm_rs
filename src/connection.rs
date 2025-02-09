/// Handles incoming connections and sending/receiving messages

use crate::operations::GLOBAL_LAST_HEARTBEAT;

use std::fmt;
use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use std::marker::Send;
use std::io::Error;
use serde::{Serialize, Deserialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Instant, Duration};
use tokio::net::{TcpStream, TcpListener};
use tokio::sync::Mutex;
use hyper::http::{Method, Request, Response, StatusCode};
use hyper::body::{Bytes, Incoming, Buf};
use http_body_util::{BodyExt, Full};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};


/// Specify the transport layer used by address specifiers
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum Transport {
    SOCKET,
    HTTP,
    HTTPS
}


/// Store host and port for new connections
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Addr {
    /// Transport
    pub transport: Transport,
    /// Address
    pub host: String,
    /// Port number
    pub port: i32
}


impl Addr {
    pub fn new(host: &String, port: i32) -> Self {
        Self {
            transport: Transport::SOCKET,
            host: host.to_string(),
            port: port
        }
    }
}


// also implements ToString
impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.transport {
            Transport::SOCKET => write!(f, "{}:{}", self.host, self.port),
            Transport::HTTP => write!(f, "http://{}:{}", self.host, self.port),
            Transport::HTTPS => write!(f, "https://{}:{}", self.host, self.port)
        }
    }
}


/// Error to indicate that a string representation of an address could not be
/// parsed -- namely because the address string does not follow the pattern:
/// <IP>:<Port> or http://<Name>:<Port> or https://<Name>:<Port> -- Port MUST be
/// specified
#[derive(Debug, PartialEq, Eq)]
pub struct ParseAddrError;


impl std::str::FromStr for Addr {
    type Err = ParseAddrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Determin the transport type and number of leadign characters by the
        // presence of a "https://" or "http://"
        let (transport, nchars) = if s.starts_with("http://") {
            (Transport::HTTP, 7)
        } else if s.starts_with("https://") {
            (Transport::HTTPS, 8)
        } else {
            (Transport::SOCKET, 0)
        };

        // remove the leading characters from name string
        let mut chars = s.chars();
        for _ in 1..=nchars {chars.next();}
        if s.ends_with("/") { chars.next_back(); } // remove possible trailing /
        let spec:Vec<&str> = chars.as_str().split(":").collect();

        // get port (last element in split), and ensure that it's an i32
        let port: i32 = match spec.last().ok_or(ParseAddrError)?.parse::<i32>(){
            Ok(i) => i,
            Err(_) => {
                return Err(ParseAddrError);
            }
        };

        // get remaining elements in the spec (splitted vector) -- and re-join
        // them with ":" -- eg. if an IPv6 address is specified then it can
        // contain ":", which would have been removed by the split
        let host: String = spec[..spec.len()-1]
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<String>>()
            .join(":");

        // ensure that the host does not have any remaining /
        if host.contains("/") { return Err(ParseAddrError); }

        Ok(Addr{transport: transport, host: host, port:port})
    }
}


/// Specify tcp or api communication
#[derive(Debug, Clone)]
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

/// Reformat a request body into a string
pub async fn collect_request(request: &mut Incoming) -> Result<Message, Error> {
    let whole_body = request.collect().await.unwrap().aggregate();
    let data: serde_json::Value = serde_json::from_reader(
        whole_body.reader()
    ).unwrap();
    let json = serde_json::to_string(&data).unwrap();
    let message = deserialize_message(& json);
    Ok(message)
}


/// Binds to stream and listens for incoming connections, then handles
/// connection using specified handler
pub async fn api_server(
        request: Request<Incoming>, 
        mut handler: impl FnMut(Request<Incoming>) -> Pin<Box<
                dyn Future<Output=Result<Response<Full<Bytes>>, Error>> + Send
            >> + Send + 'static + Clone
    ) -> Result<Response<Full<Bytes>>, Error> {

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let mut response = Response::new(Full::default());

    match (method, path.as_str()) {
        (Method::POST, p) if p.starts_with("/request_handler") => {
            handler(request).await
        },
        (Method::GET, p) if p.starts_with("/heartbeat_handler") => {
            {
                let mut last_heartbeat = GLOBAL_LAST_HEARTBEAT.lock().await;
                *last_heartbeat = Some(Instant::now());
            }
            handler(request).await
        },
        _ => {
            // Return 404 not found response.
            *response.status_mut() = StatusCode::NOT_FOUND;
            Ok(response)
        }
    }
}

/// Binds to stream and listens for incoming connections, then handles
/// connection using specified handler
pub async fn tcp_server(
        addr: &Addr, 
        mut handler: impl FnMut(Option<Arc<Mutex<TcpStream>>>) -> Pin<Box<
                dyn Future<Output = Result<Response<Full<Bytes>>, Error>> + Send
            >> + Send + 'static + Clone
    ) -> Result<(), std::io::Error> {

    trace!("Starting server process on: {:?}", addr);

    let listener = TcpListener::bind(format!("{}:{}", addr.host, addr.port)).await.unwrap();
    trace!("Bind to {:?} successful", addr);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                info!("Passing TCP connection to handler...");
                let shared_stream = Arc::new(Mutex::new(stream));
                let _ = handler(Some(shared_stream)).await; 
            },
            Err(e) => {
                error!("Error: {}", e);
            }        
        }
    }

    // Ok(())
}
