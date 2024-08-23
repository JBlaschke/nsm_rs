use crate::network::{get_local_ips, get_matching_ipstr};

use crate::connection::{Message, MessageHeader, connect, Addr, server, send, stream_read, deserialize_message};

use crate::service::{Payload, State, serialize, request_handler, heartbeat_handler_helper, event_monitor};

use crate::utils::{only_or_error, epoch};

use crate::models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, Send};

use std::thread;
use std::net::TcpStream;
use std::sync::{Arc, Mutex, Condvar};
use std::time::Duration;
use std::thread::sleep;
use actix_web::{web, App, HttpServer, HttpResponse, Responder, HttpRequest};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use env_logger::Env;

pub fn list_interfaces(inputs: ListInterfaces) -> std::io::Result<()> {
    
    let ips = get_local_ips();

    if inputs.print_v4 {info!("Listing Matching IPv4 Interfaces");}
    if inputs.print_v6 {info!("Listing Matching IPv6 Interfaces");}

    let mut ipv4_names = Vec::new();
    let mut ipv6_names = Vec::new();

    if inputs.print_v4 {
        if inputs.verbose {println!("IPv4 Interfaces:");}
        for ip in ips.ipv4_addrs {
            let name: & String = & ip.name.unwrap_or_default();
            if ! ipv4_names.contains(name) {
                if inputs.verbose {
                    println!(" - {}", name);
                } else {
                    println!("{}", name);
                }
                ipv4_names.push(name.to_string());
            }
        }
    }

    if inputs.print_v6 {
        if inputs.verbose {println!("IPv6 Interfaces:");}
        for ip in ips.ipv6_addrs {
            let name: & String = & ip.name.unwrap_or_default();
            if ! ipv6_names.contains(name) {
                if inputs.verbose {
                    println!(" - {}", name);
                } else {
                    println!("{}", name);
                }
                ipv6_names.push(name.to_string());
            }
        }
    }

    Ok(())

}

pub fn list_ips(inputs: ListIPs) -> std::io::Result<()> {

    let ips = get_local_ips();
    
    if inputs.print_v4 {info!("Listing Matching IPv4 Addresses");}
    if inputs.print_v6 {info!("Listing Matching IPv6 Addresses");}
    if inputs.print_v4 {
        let ipstr = get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        );
        if inputs.verbose {println!("IPv4 Addresses for {}:", inputs.name);}
        for ip in ipstr {
            if inputs.verbose {
                println!(" - {}", ip);
            } else {
                println!("{}", ip);
            }
        }
    }

    if inputs.print_v6 {
        let ipstr = get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        );
        if inputs.verbose {println!("IPv6 Addresses for {}:", inputs.name);}
        for ip in ipstr {
            if inputs.verbose {
                println!(" - {}", ip);
            } else {
                println!("{}", ip);
            }
        }
    }

    Ok(())

}

pub fn listen(inputs: Listen) -> std::io::Result<()> {

    let ips = get_local_ips();

    trace!("Start setting up listener...");

    // identify local ip address
    let ipstr = if inputs.print_v4 {
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        )
    } else {
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        )
    };
    
    let host = only_or_error(& ipstr);

    // initialize State struct
    let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
    let state_clone = Arc::clone(& state);
    
    let handler =  move |stream: &Arc<Mutex<TcpStream>>| {
        return request_handler(& state_clone, stream);
    };

    let addr = Addr {
        host: host.to_string(),
        port: inputs.bind_port
    };

    info!("Starting listener started on: {}:{}", & addr.host, addr.port);

    // thread handles incoming connections and adds them to State and event queue
    let _thread_handler = thread::spawn(move || {
        let _ = server(& addr, handler);
    });

    trace!("entering event_monitor");
    // send State struct holding event queue into event monitor to handle heartbeats
    let _ = match event_monitor(state){
        Ok(()) => println!("exited event monitor"),
        Err(_) => println!("event monitor error")
    };

    Ok(())

}

pub fn publish(inputs: Publish) -> std::io::Result<()> {

    let ips = get_local_ips();

    let (ipstr, all_ipstr) = if inputs.print_v4 {(
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv4_addrs, & inputs.name, & None)
    )} else {(
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv6_addrs, & inputs.name, & None)
    )};

    let payload = serialize(& Payload {
        service_addr: ipstr.clone(),
        service_port: inputs.service_port,
        service_claim: 0,
        interface_addr: all_ipstr,
        bind_port: inputs.bind_port,
        key: inputs.key,
        id: 0,
        service_id: 0,
    });

    // connect to broker
    let stream = match connect(& Addr{
        host: inputs.host, port: inputs.port}){
            Ok(s) => s,
            Err(_e) => {
                return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,"Connection unsuccessful"));
                }
    };
    let stream_mut = Arc::new(Mutex::new(stream));
    let ack = send(& stream_mut, & Message{
        header: MessageHeader::PUB,
        body: payload
    });

    // check for successful connection
    match ack {
        Ok(m) => {
            trace!("Received response: {:?}", m);
            match m.header {
                MessageHeader::ACK => {
                    info!("Server acknowledged PUB.")
                }
                _ => {
                    warn!("Server responds with unexpected message: {:?}", m)
                }
            }
        }
        Err(e) => {
            error!("Encountered error: {:?}", e);
        }
    }

    drop(stream_mut);

    let host = only_or_error(& ipstr);
    let addr = Addr {
        host: host.to_string(),
        port: inputs.bind_port
    };

    let handler =  move |stream: &Arc<Mutex<TcpStream>>| {
        return heartbeat_handler_helper(stream, None, None);
    };

    // send/receive heartbeats to/from broker
    let _ = server(& addr, handler);

    Ok(())

}

pub fn claim(inputs: Claim) -> std::io::Result<()> {

    let ips = get_local_ips();

    let (ipstr, _all_ipstr) = if inputs.print_v4 {(
        get_matching_ipstr(
            & ips.ipv4_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv4_addrs, & inputs.name, & None)
    )} else {(
        get_matching_ipstr(
            & ips.ipv6_addrs, & inputs.name, & inputs.starting_octets
        ),
        get_matching_ipstr(& ips.ipv6_addrs, & inputs.name, & None)
    )};

    let _payload = serialize(& Payload {
        service_addr: ipstr.clone(),
        service_port: inputs.port,
        service_claim: epoch(),
        interface_addr: Vec::new(),
        bind_port: inputs.bind_port,
        key: inputs.key,
        id: 0,
        service_id: 0,
    });

    // connect to broker
    let broker_addr = Addr{
        host: inputs.host,
         port: inputs.port
    };
    let stream = match connect(& broker_addr){
            Ok(s) => s,
            Err(_e) => {
                return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,"Connection unsuccessful. Try another key"));
                }
    };
    let stream_mut = Arc::new(Mutex::new(stream));
    let ack = send(& stream_mut, & Message{
        header: MessageHeader::CLAIM,
        body: _payload
    });

    let mut service_payload = "".to_string();

    // check for successful connection to a published service
    match ack {
        Ok(m) => {
            trace!("Received response: {:?}", m);
            match m.header {
                MessageHeader::ACK => {
                    let mut read_fail = 0;
                    let loc_stream: &mut TcpStream = &mut stream_mut.lock().unwrap();
                    // loop handles connection race case
                    loop {
                        sleep(Duration::from_millis(1000));
                        let message = match stream_read(loc_stream) {
                            Ok(m) => deserialize_message(& m),
                            Err(err) => {return Err(err);}
                        };
                        trace!("{:?}", message);
                        // print service's address to client
                        if matches!(message.header, MessageHeader::ACK){
                            info!("Server acknowledged CLAIM.");
                            println!("{}", message.body);
                            service_payload = message.body;
                            break;
                        }
                        else{
                            read_fail += 1;
                            if read_fail > 5 {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput,"Key not found"));
                            }
                        }
                    }
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Server responds with unexpected message: {:?}", m),
                    ));
                }
            }
        }
        Err(e) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Encountered error: {:?}", e),
            ));
        }
    }
    let handler =  move |stream: &Arc<Mutex<TcpStream>>| {
        return heartbeat_handler_helper(stream, Some(&service_payload), Some(&broker_addr));
    };

    let host = only_or_error(& ipstr);
    let addr = Addr {
        host: host.to_string(),
        port: inputs.bind_port
    };

    // send/receive heartbeats to/from broker
    let _ = server(& addr, handler);

    Ok(())

}

pub fn collect(inputs: Collect) -> std::io::Result<()> {

    let ips = get_local_ips();

    let addr = Addr {
        host: inputs.host,
        port: inputs.port
    };

    // connect to bind port of service or client
    let stream = match connect(& addr){
            Ok(s) => s,
            Err(_e) => {
                return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,"Connection unsuccessful"));
                }
    };        
    trace!("Connection to {:?} successful", addr);

    let stream_mut = Arc::new(Mutex::new(stream));

    let received = send(& stream_mut, & Message{
        header: MessageHeader::COL,
        body: "".to_string()
    });

    println!("reading HB");
    let _payload = match received {
        Ok(message) => {
            if message.body.is_empty() {
                panic!("Payload not found in heartbeat.");
            }
            println!("{:?}", message.body);
        }
        Err(_err) => return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "Failed to collect message."))
    };

    Ok(())

}

pub fn send_msg(inputs: Send) -> std::io::Result<()> {

    let ips = get_local_ips();

    let addr = Addr {
        host: inputs.host,
        port: inputs.port
    };
    // connect to bind port of service or client
    let mut stream = match connect(& addr){
        Ok(s) => s,
        Err(_e) => {
            return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,"Connection unsuccessful"));
            }
    };  

    let stream_mut = Arc::new(Mutex::new(stream));

    let received = send(& stream_mut, & Message{
        header: MessageHeader::MSG,
        body: inputs.msg
    });

    println!("reading msg");
    let _ = match received {
        Ok(message) => {
            println!("{:?}", message);
        }
        Err(_err) => return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "Failed to collect message."))
    };

    Ok(())
    
}