mod network;
use network::{get_local_ips, get_matching_ipstr};

mod connection;
use connection::{Message, MessageHeader, connect, Addr, server, send};

mod service;
use service::{Payload, State, serialize, request_handler, heartbeat_handler};

mod utils;
use utils::{only_or_error, epoch};

mod cli;
use cli::{
    init, parse, CLIOperation, ListInterfaces, ListIPs, Listen, Claim, Publish
};

use std::net::TcpStream;


fn main() -> std::io::Result<()> {
    let args = init();
    let ips = get_local_ips();

    match parse(& args) {
        CLIOperation::ListInterfaces(inputs) => {
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

            let mut state: State = State::new();
            let handler =  |stream: &mut TcpStream| {
                return request_handler(&mut state, stream);
            };

            let addr = Addr {
                host: host,
                port: inputs.bind_port
            };

            server(& addr, handler);
        }

        CLIOperation::Claim(inputs) => {
            let payload = serialize(& Payload {
                service_addr: Vec::new(),
                service_port: inputs.port,
                service_claim: epoch(),
                interface_addr: Vec::new(),
                key: inputs.key,
                id: 0
            });

            // let _rec = cwrite(& inputs.host, inputs.port, & payload);
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
                key: inputs.key,
                id: 0
            });

            let mut stream = connect(& Addr{
                host: & inputs.host, port: inputs.port
            })?;
            send(&mut stream, & Message{
                header: MessageHeader::PUB,
                body: payload
            });

            let host = only_or_error(& ipstr);
            let addr = Addr {
                host: host,
                port: inputs.bind_port
            };

            server(& addr, heartbeat_handler);
        }
    }

    Ok(())
}