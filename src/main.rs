mod network;
use network::{get_local_ips, get_matching_ipstr};

mod connection;
use connection::{Message, MessageHeader, connect, Addr, server, send, stream_read, deserialize_message};

mod service;
use service::{Payload, State, serialize, request_handler, heartbeat_handler, event_monitor};

mod utils;
use utils::{only_or_error, epoch};

mod cli;
use cli::{init, parse, CLIOperation};
use std::thread;

use std::net::TcpStream;
use std::sync::{Arc, Mutex, Condvar};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use env_logger::Env;


fn main() -> std::io::Result<()> {
    let args = parse(& init());

    let logging_env = Env::default()
        .filter_or("NSM_LOG_LEVEL", "warn")
        .write_style_or("NSM_LOG_STYLE", "always");
    env_logger::init_from_env(logging_env);

    info!("Started NERSC Service MESH");
    trace!("Input args: {:?}", args);

    let ips = get_local_ips();

    match args {
        CLIOperation::ListInterfaces(inputs) => {
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
        }

        CLIOperation::ListIPs(inputs) => {
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
        }

        CLIOperation::Listen(inputs) => {
            trace!("Start setting up listener...");

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

            let _thread_handler = thread::spawn(move || {
                let _ = server(& addr, handler);
            });
            println!("entering event_monitor");
            let _ = match event_monitor(state){
                Ok(()) => println!("exited event monitor"),
                Err(_) => println!("event monitor error")
            };
            
            }

        CLIOperation::Claim(inputs) => {
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

            let stream = match connect(& Addr{
                host: inputs.host, port: inputs.port}){
                    Ok(s) => s,
                    Err(_e) => {
                        return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,"Connection unsuccessful> Try another key"));
                        }
            };
            let stream_mut = Arc::new(Mutex::new(stream));
            let ack = send(& stream_mut, & Message{
                header: MessageHeader::CLAIM,
                body: _payload
            });
            match ack {
                Ok(m) => {
                    trace!("Received response: {:?}", m);
                    match m.header {
                        MessageHeader::ACK => {
                            let loc_stream: &mut TcpStream = &mut stream_mut.lock().unwrap();
                            let claim_key = match stream_read(loc_stream) {
                                Ok(message) => deserialize_message(& message),
                                Err(err) => {return Err(err);}
                            };
                            println!("{:?}", claim_key);
                            if matches!(claim_key.header, MessageHeader::ACK){
                                info!("Server acknowledged CLAIM.");
                            }
                            else{
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput,"Key not found"));
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

            let host = only_or_error(& ipstr);
            let addr = Addr {
                host: host.to_string(),
                port: inputs.bind_port
            };
            let _ = server(& addr, heartbeat_handler);
        }

        CLIOperation::Publish(inputs) => {
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
                        
            let _ = server(& addr, heartbeat_handler);

        }
    }

    Ok(())
}
