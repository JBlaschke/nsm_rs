/// Handles incoming connections and sending/receiving messages

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::fmt;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, Mutex};
use rand::Rng;
use std::thread;
use std::time::Duration;
use tiny_http::{Server, Response, Request, Method};


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
#[derive(Serialize, Deserialize, Debug)]
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
pub fn connect(addr: &Addr) -> std::io::Result<TcpStream> {

    TcpStream::connect(format!("{}:{}", addr.host, addr.port))
}

// pub fn connect_with_retry(addr: String) -> std::io::Result<TcpStream> {
//     let mut retry_delay = Duration::from_secs(1);
//     let max_delay = Duration::from_secs(5);
//     let mut rng = rand::thread_rng();
//     let mut failcount = 0;

//     while failcount < 3 {
//         match TcpStream::connect(addr.clone()) {
//             Ok(stream) => {
//                 trace!("Reset stream connection");
//                 return Ok(stream);
//             }
//             Err(e) => {
//                 warn!("Failed to connect: {:?}", e);
                
//                 failcount += 1;
//                 // Exponential backoff with jitter
//                 let jitter: u64 = rng.gen_range(0..1000); // Random jitter in milliseconds
//                 let total_delay = retry_delay + Duration::from_millis(jitter);
                
//                 thread::sleep(total_delay);
//                 retry_delay = std::cmp::min(retry_delay * 2, max_delay);
//             }
//         }
//     }
//     // If the loop exits without establishing a connection, return an error
//     Err(std::io::Error::new(
//         std::io::ErrorKind::InvalidInput, "Failed to connect after multiple attempts"))
// }


/// Write a message through a TCPStream, returns Result of # of bytes written or err
pub fn stream_write(stream: &mut TcpStream, msg: & str) -> std::io::Result<usize> {
    match stream.write(msg.as_bytes()) {
        Ok(n) => Ok(n),
        Err(err) => Err(err)
    }
}

/// Read a message from TCPStream, returns Result of # of bytes read or err
pub fn stream_read(stream: &mut TcpStream) -> std::io::Result<String>{
    let mut buf = [0; 1024];
    let mut message = String::new();
    loop {
        let bytes_read = match stream.read(&mut buf) {
            Ok(n) => n,
            Err(err) => { return Err(err); }
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
pub fn send(request: & Arc<Mutex<TcpStream>>, msg: & Message) -> std::io::Result<Message> {

    let loc_stream: &mut TcpStream = &mut *request.lock().unwrap();
    let _ = stream_write(loc_stream, & serialize_message(msg));

    if matches!(msg.header, MessageHeader::ACK){
        return Ok(Message {
            header: MessageHeader::NULL,
            body: "".to_string()
        })
    }

    let response = match stream_read(loc_stream) {
        Ok(message) => deserialize_message(& message),
        Err(err) => {return Err(err);}
    };

    Ok(response)
}

/// Receives a message through a TCPStream using stream_read(), writes an ACK
/// if received message not a HB or ACK
pub fn receive(stream: & Arc<Mutex<TcpStream>>) -> std::io::Result<Message> {

    let loc_stream: &mut TcpStream = &mut *stream.lock().unwrap();

    let response = match stream_read(loc_stream) {
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
    ));

    Ok(response)
}

/// Binds to stream and listens for incoming connections, then handles connection using specified handler
pub fn server(
    addr: &Addr, 
    mut handler: impl FnMut(Request) -> std::io::Result<()> + std::marker::Send + 'static + Clone
) -> std::io::Result<()> {
    trace!("Starting server process on: {:?}", addr);

    let server = Server::http(format!("{}:{}", addr.host, addr.port)).unwrap();

    for request in server.incoming_requests() {
        println!("request received");
        
        let (method, path) = {
            (request.method().clone(), request.url().to_string())
        };

        match method {
            Method::Post if path.starts_with("/request_handler") => {
                println!("entering handler");
                let _ = handler(request);
            },
            Method::Post if path.starts_with("/heartbeat_handler") => {
                let _ = handler(request);
            },
            _ => {
                let response = Response::from_string("Unsupported HTTP method").with_status_code(405);
            }
        }
    }

    // let listener = TcpListener::bind(format!("{}:{}", addr.host, addr.port))?;
    // trace!("Bind to {:?} successful", addr);

    // for stream in listener.incoming() {
    //     info!("Request received on {:?}, processing...", stream);
    //     match stream {
    //         Ok(stream) => {
    //             trace!("Passing TCP connection to handler...");
    //             let shared_stream = Arc::new(Mutex::new(stream));
    //             let _ = handler(& shared_stream); 
    //         }
    //         Err(e) => {
    //             println!("Error: {}", e);
    //         }
    //     }
    // }
    Ok(())
}