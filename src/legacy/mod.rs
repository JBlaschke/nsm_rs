//! The pre-cleanup implementation, moved here unchanged apart from module
//! paths so that the single `nsm` binary keeps working while the new backend
//! is built. Everything in this module is scheduled for deletion by the
//! `cleanup/03-common-backend` branch; do not add to it.
//!
//! [`run`] is the only entry point new code may use: it maps the new CLI
//! types onto the old operation inputs.

pub mod api_builder;
pub mod connection;
pub mod mode_api;
pub mod mode_tcp;
pub mod models;
pub mod network;
pub mod operations;
pub mod rest;
pub mod service;
pub mod tls;
pub mod utils;

use crate::cli::{Command, IfaceOpts, ListenTransport, TlsOpts};
use crate::net::{Addr, IpVersion, Transport};

/// Convert the new address type into the legacy one.
impl From<&Addr> for connection::Addr {
    fn from(a: &Addr) -> Self {
        connection::Addr {
            transport: match a.transport {
                Transport::Tcp => connection::Transport::SOCKET,
                Transport::Http => connection::Transport::HTTP,
                Transport::Https => connection::Transport::HTTPS,
            },
            host: a.host.clone(),
            port: i32::from(a.port),
        }
    }
}

fn com_type(addr: &Addr) -> connection::ComType {
    if addr.transport.is_http() {
        connection::ComType::API
    } else {
        connection::ComType::TCP
    }
}

/// The legacy code models "which family" as two booleans; both true means
/// "IPv4 preferred" because every consumer checks `print_v4` first.
fn family(v: Option<IpVersion>) -> (bool, bool) {
    match v {
        None => (true, true),
        Some(IpVersion::V4) => (true, false),
        Some(IpVersion::V6) => (false, true),
    }
}

fn root_ca(tls: &TlsOpts) -> Option<String> {
    tls.root_ca.as_ref().map(|p| p.to_string_lossy().into_owned())
}

/// Run one CLI command against the legacy operations.
pub async fn run(cmd: Command) -> crate::Result<()> {
    match cmd {
        Command::ListInterfaces { ip_version, verbose } => {
            let (print_v4, print_v6) = family(ip_version);
            operations::list_interfaces(models::ListInterfaces { verbose, print_v4, print_v6 }).await?;
        }
        Command::ListIps { iface, verbose } => {
            let (print_v4, print_v6) = family(iface.ip_version);
            operations::list_ips(models::ListIPs {
                verbose,
                print_v4,
                print_v6,
                name: iface.interface,
                starting_octets: iface.ip_start,
            })
            .await?;
        }
        Command::Listen { bind_port, transport, iface, tls } => {
            let (print_v4, print_v6) = family(iface.ip_version);
            let com = match transport {
                ListenTransport::Tcp => connection::ComType::TCP,
                ListenTransport::Http => connection::ComType::API,
            };
            operations::listen(
                models::Listen {
                    print_v4,
                    print_v6,
                    name: iface.interface,
                    starting_octets: iface.ip_start,
                    bind_port: i32::from(bind_port),
                    tls: tls.tls,
                    root_ca: root_ca(&tls),
                },
                com,
            )
            .await?;
        }
        Command::Publish { broker, bind_port, service_port, key, ping, iface, tls } => {
            let (print_v4, print_v6) = family(iface.ip_version);
            let com = com_type(&broker);
            operations::publish(
                models::Publish {
                    print_v4,
                    print_v6,
                    host: (&broker).into(),
                    name: iface.interface,
                    starting_octets: iface.ip_start,
                    bind_port: i32::from(bind_port),
                    service_port: i32::from(service_port),
                    key,
                    tls: tls.tls || broker.transport.is_tls(),
                    root_ca: root_ca(&tls),
                    ping,
                },
                com,
            )
            .await?;
        }
        Command::Claim { broker, bind_port, key, ping, iface, tls } => {
            let (print_v4, print_v6) = family(iface.ip_version);
            let com = com_type(&broker);
            operations::claim(
                models::Claim {
                    print_v4,
                    print_v6,
                    host: (&broker).into(),
                    name: iface.interface,
                    starting_octets: iface.ip_start,
                    bind_port: i32::from(bind_port),
                    key,
                    tls: tls.tls || broker.transport.is_tls(),
                    root_ca: root_ca(&tls),
                    ping,
                },
                com,
            )
            .await?;
        }
        Command::Collect { party, key, iface, tls } => {
            let (print_v4, print_v6) = family(iface.ip_version);
            let com = com_type(&party);
            operations::collect(
                models::Collect {
                    print_v4,
                    print_v6,
                    host: (&party).into(),
                    name: iface.interface,
                    starting_octets: iface.ip_start,
                    key: key.unwrap_or(0),
                    tls: tls.tls || party.transport.is_tls(),
                    root_ca: root_ca(&tls),
                },
                com,
            )
            .await?;
        }
        Command::Send { party, msg, key, iface, tls } => {
            let (print_v4, print_v6) = family(iface.ip_version);
            let com = com_type(&party);
            operations::send_msg(
                models::SendMSG {
                    print_v4,
                    print_v6,
                    host: (&party).into(),
                    name: iface.interface,
                    starting_octets: iface.ip_start,
                    msg,
                    key: key.unwrap_or(0),
                    tls: tls.tls || party.transport.is_tls(),
                    root_ca: root_ca(&tls),
                },
                com,
            )
            .await?;
        }
        Command::Serve { bind } => {
            rest::serve(bind).await?;
        }
    }
    Ok(())
}

// Keep the flatten-only import used in a type position above from being
// reported as unused when the module is compiled with warnings allowed.
#[allow(dead_code)]
fn _uses(_: IfaceOpts) {}
