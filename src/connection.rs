use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::fmt;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, Mutex};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

#[derive(Debug)]
pub struct Addr {
    pub host: String,
    pub port: i32
}


#[derive(Serialize, Deserialize, Debug)]
pub enum MessageHeader {
    HB,
    ACK, //acknowledgement
    PUB,
    CLAIM,
    NULL
}


impl fmt::Display for MessageHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MessageHeader::HB    => write!(f, "HB"),
            MessageHeader::ACK   => write!(f, "ACK"),
            MessageHeader::PUB   => write!(f, "PUB"),
            MessageHeader::CLAIM => write!(f, "CLAIM"),
            MessageHeader::NULL  => write!(f, "NULL"),
       }
    }
}


#[derive(Serialize, Deserialize, Debug)]
pub struct Message {
    pub header: MessageHeader, //tells how to interprate message
    pub body: String
}


pub fn serialize_message(payload: & Message) -> String {
    serde_json::to_string(payload).unwrap()  //serializes any struct into a string
                                             //sending messages across message as a string
                                             //flexibility
}


pub fn deserialize_message(payload: & String) -> Message {
    serde_json::from_str(payload).unwrap()
}


pub fn connect(addr: &Addr) -> std::io::Result<TcpStream> {
    TcpStream::connect(format!("{}:{}", addr.host, addr.port))
}


pub fn stream_write(stream: &mut TcpStream, msg: & str) -> std::io::Result<usize> {
    // stream.write_all(msg.as_bytes())?;
    match stream.write(msg.as_bytes()) {
        Ok(n) => Ok(n),
        Err(err) => Err(err)
    }
}


pub fn stream_read(stream: &mut TcpStream) -> std::io::Result<String>{
    let mut buf = [0; 1024];
    let mut message = String::new();

    loop {
        // let bytes_read = stream.read(&mut buf)?;
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

//writes to stream with TCP object
//looks at stream and reads answer back to make sure other side got message
//maybe multithreaded
pub fn send(stream: & Arc<Mutex<TcpStream>>, msg: & Message) -> std::io::Result<Message> {

    let loc_stream: &mut TcpStream = &mut *stream.lock().unwrap();
    let _ = stream_write(loc_stream, & serialize_message(msg));

    if matches!(msg.header, MessageHeader::ACK)
    || matches!(msg.header, MessageHeader::HB) {
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

//receives data from stream and writes acknowledgment message
//not multithreaded
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


pub fn server(
    addr: &Addr, 
    mut handler: impl FnMut(& Arc<Mutex<TcpStream>>) -> std::io::Result<()>
) -> std::io::Result<()> {
    println!("Starting server process on: {:?}", addr);

    let listener = TcpListener::bind(format!("{}:{}", addr.host, addr.port))?;
    trace!("Bind to {:?} successful", addr);

    // accept connections and process them serially
    for stream in listener.incoming() {
        info!("Request received on {:?}, processing...", stream);
        match stream {
            Ok(stream) => {
                trace!("Passing TCP connection to handler...");
                //mutex avoids race conditions
                let shared_stream = Arc::new(Mutex::new(stream)); //multiple servers can listen in the same place
                let _ = handler(& shared_stream); 
            }
            Err(e) => {
                println!("Error: {}", e);
            }
        }
    }
    Ok(())
}


// pub fn listen_server(
//     addr: &Addr, 
//     handler: impl FnMut(& Arc<Mutex<TcpStream>>) -> std::io::Result<()>
//         + std::marker::Send + 'static
// ) -> std::io::Result<()> {
//     trace!("Starting server process on: {:?}", addr);

//     let listener = TcpListener::bind(format!("{}:{}", addr.host, addr.port))?;
//     trace!("Bind to {:?} successful", addr);

//     let pool = ThreadPool::new(30);

//     trace!("Setting up VecDeque");
//     let deque = Arc::new(Mutex::new(VecDeque::new()));
//     let deque_clone = Arc::clone(& deque);

//     let _ = thread::spawn(move|| {
//         // accept connections and process them serially
//         for stream in listener.incoming() {
//             info!("Request received on {:?}, processing...", stream);
//             match stream {
//                 Ok(stream) => {
//                     trace!("Passing TCP connection to handler...");
//                     //mutex avoids race conditions
//                     let shared_stream = Arc::new(Mutex::new(stream)); //multiple servers can listen in the same place
//                     {
//                         let mut loc_deque = deque_clone.lock().unwrap();
//                         let _ = loc_deque.push_back(shared_stream);
//                     }  
//                 }
//                 Err(e) => {
//                     println!("Error: {}", e);
//                 }
//             }
//         }
//     });

//     let deque_clone = Arc::clone(& deque);
//     let dyn_handler = Arc::new(Mutex::new(handler));

//     //set up heartbeat handler here
//     loop {

//         let popped_client = {
//             let mut loc_deque = deque_clone.lock().unwrap();
//             loc_deque.pop_front()
//         };

//         let popped_client = match popped_client {
//             Some(client) => client,
//             None => continue
//         };

//         let deque_clone2 = Arc::clone(& deque_clone);
//         let handler_clone = Arc::clone(& dyn_handler);

//         pool.execute(move || {
            
//             let mut handler = handler_clone.lock().unwrap();
//             let _ = handler(& popped_client.clone()).unwrap();

//             //if popped_client.fail_count < 10 {
//                 let deque_clone3 = Arc::clone(& deque_clone2);
//                 {
//                     let mut loc_deque = deque_clone3.lock().unwrap();
//                     let _ = loc_deque.push_back(popped_client.clone());
//                 }
//             //}

//         });
//     }

//     Ok(())
// }