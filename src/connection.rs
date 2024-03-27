use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::fmt;
use serde::{Serialize, Deserialize};


#[derive(Debug)]
pub struct Addr<'a> {
    pub host: &'a String,
    pub port: i32
}


#[derive(Serialize, Deserialize, Debug)]
pub enum MessageHeader {
    HB,
    ACK,
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
    pub header: MessageHeader,
    pub body: String
}


pub fn serialize_message(payload: & Message) -> String {
    serde_json::to_string(payload).unwrap()
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

pub fn send(stream: &mut TcpStream, msg: & Message) -> std::io::Result<Message> {

    stream_write(stream, & serialize_message(msg));

    if matches!(msg.header, MessageHeader::ACK)
    || matches!(msg.header, MessageHeader::HB) {
        return Ok(Message {
            header: MessageHeader::NULL,
            body: "".to_string()
        })
    }

    let response = match stream_read(stream) {
        Ok(message) => deserialize_message(& message),
        Err(err) => {return Err(err);}
    };

    Ok(response)
}

pub fn receive(stream: &mut TcpStream) -> std::io::Result<Message> {

    let response = match stream_read(stream) {
        Ok(message) => deserialize_message(& message),
        Err(err) => {return Err(err);}
    };

    if matches!(response.header, MessageHeader::ACK)
    || matches!(response.header, MessageHeader::HB) {
        return Ok(response);
    }

    stream_write(stream, & serialize_message(
        & Message {
            header: MessageHeader::ACK,
            body: "".to_string()
        }
    ));

    Ok(response)
}


pub fn server(
    addr: &Addr, 
    mut handler: impl FnMut(&mut TcpStream) -> std::io::Result<()>
) -> std::io::Result<()> {
    let listener = TcpListener::bind(format!("{}:{}", addr.host, addr.port))?;

    // accept connections and process them serially
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                // pass the handle_connection function as a function pointer
                handler(&mut stream);
            }
            Err(e) => {
                println!("Error: {}", e);
            }
        }
    }
    Ok(())
}