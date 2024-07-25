//! # Introduction
//! 
//! NERSC Service Mesh manages traffic between service clusters and compute nodes.
//! A "connection broker" with a fixed address passes the address of a service cluster
//! for compute nodes to connect to and run a job.

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
use std::time::Duration;
use std::thread::sleep;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use env_logger::Env;


/// Entry point for service mesh operations
///
/// ## CLIOperations
/// - ListInterfaces - outputs the interfaces on a given device
/// - ListIPs - outputs the IP addresses on some interface
/// - Listen - initiates connection broker, keeps track of services and clients
/// - Claim - connect to broker to receive the address of an available service
/// - Publish - connect to broker to publish address of a new service
/// ### Note
///  see cli module for more details
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

        // # ListInterfaces
        // Lists available interfaces on device
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

        // Lists available IP addresses on interface
        // Match command line entries with variables in struct
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

        // Inititate broker
        // Match command line entries with variables in struct
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
            
            }

        // # Claim
        // Connect to broker and discover available address for data connection.
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

            // connect to broker
            let stream = match connect(& Addr{
                host: inputs.host, port: inputs.port}){
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

            let host = only_or_error(& ipstr);
            let addr = Addr {
                host: host.to_string(),
                port: inputs.bind_port
            };

            // send/receive heartbeats to/from broker
            let _ = server(& addr, heartbeat_handler);
        }
        // # Publish
        // Connect to broker and publish address for data connection.
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

            // send/receive heartbeats to/from broker
            let _ = server(& addr, heartbeat_handler);

        }
    }

    Ok(())
}
