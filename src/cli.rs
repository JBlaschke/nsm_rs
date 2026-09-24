//! Command-line interface.
//!
//! One binary, one subcommand per operation. The structs here are also the
//! request types of the REST control plane, so the CLI and the API share one
//! definition and one set of validation rules.
//!
//! The transport is taken from the peer address: `host:port` is raw TCP,
//! `http://host:port` and `https://host:port` are HTTP. `listen` and `serve`
//! have no peer address and take `--transport` instead.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::net::{Addr, IpVersion, Selector};

/// NERSC Service Mesh: publish, claim and broker services across HPC systems.
#[derive(Debug, Parser)]
#[command(name = "nsm", version, about, propagate_version = true)]
pub struct Cli {
    /// Log filter, e.g. `info` or `nsm=debug` (overrides NSM_LOG_LEVEL).
    #[arg(long, global = true, env = "NSM_LOG_LEVEL", value_name = "FILTER")]
    pub log_level: Option<String>,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// Selection of the local address a party advertises.
#[derive(Debug, Clone, Default, Args, serde::Serialize, serde::Deserialize)]
pub struct IfaceOpts {
    /// Interface to take the local address from (e.g. `eth0`, `hsn0`).
    #[arg(short = 'n', long = "name", value_name = "IFACE")]
    pub interface: Option<String>,

    /// Only local addresses whose text starts with this prefix (e.g. `10.128.`).
    #[arg(short = 'i', long = "ip-start", value_name = "PREFIX")]
    pub ip_start: Option<String>,

    /// Address family; both are considered when omitted.
    #[arg(long = "ip-version", value_enum, value_name = "4|6")]
    pub ip_version: Option<IpVersion>,
}

impl IfaceOpts {
    /// The equivalent address filter.
    pub fn selector(&self) -> Selector {
        Selector {
            interface: self.interface.clone(),
            prefix: self.ip_start.clone(),
            version: self.ip_version,
        }
    }
}

/// TLS settings.
#[derive(Debug, Clone, Default, Args, serde::Serialize, serde::Deserialize)]
pub struct TlsOpts {
    /// Serve TLS on this party's own listener (HTTP transport only).
    /// Requires CERT_PATH and KEY_PATH in the environment.
    #[arg(long)]
    pub tls: bool,

    /// PEM bundle of root certificates used to verify peers.
    /// Defaults to the platform trust store.
    #[arg(
        long = "root-ca",
        alias = "root_ca",
        value_name = "PEM",
        env = "ROOT_PATH"
    )]
    pub root_ca: Option<PathBuf>,
}

/// Transport a listener speaks when no peer address implies one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenTransport {
    /// Raw TCP.
    Tcp,
    /// HTTP (TLS when `--tls` is set).
    Http,
}

/// The operations.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// List network interfaces on this host.
    #[command(alias = "list_interfaces")]
    ListInterfaces {
        /// Address family to consider.
        #[arg(long = "ip-version", value_enum, value_name = "4|6")]
        ip_version: Option<IpVersion>,
        /// Print section headers.
        #[arg(short, long)]
        verbose: bool,
    },

    /// List IP addresses on this host, optionally filtered by interface and prefix.
    #[command(alias = "list_ips")]
    ListIps {
        /// Local address selection.
        #[command(flatten)]
        iface: IfaceOpts,
        /// Print section headers.
        #[arg(short, long)]
        verbose: bool,
    },

    /// Run the broker.
    Listen {
        /// Port to accept registrations and relay messages on.
        #[arg(long, value_name = "PORT")]
        bind_port: u16,
        /// Transport to serve.
        #[arg(long, value_enum, default_value = "tcp")]
        transport: ListenTransport,
        /// Local address selection.
        #[command(flatten)]
        iface: IfaceOpts,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
    },

    /// Announce a service to the broker and keep it registered.
    Publish {
        /// Broker address (`host:port`, `http://host:port` or `https://host:port`).
        broker: Addr,
        /// Port this party listens on for the broker's heartbeats.
        #[arg(long, value_name = "PORT")]
        bind_port: u16,
        /// Port the actual service accepts connections on.
        #[arg(long, value_name = "PORT")]
        service_port: u16,
        /// Rendezvous key shared with the clients that may claim this service.
        #[arg(long)]
        key: u64,
        /// Send one-sided heartbeats to the broker instead of answering its heartbeats.
        #[arg(long)]
        ping: bool,
        /// Local address selection.
        #[command(flatten)]
        iface: IfaceOpts,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
    },

    /// Ask the broker for a service with a given key and stay paired with it.
    Claim {
        /// Broker address.
        broker: Addr,
        /// Port this party listens on for the broker's heartbeats.
        #[arg(long, value_name = "PORT")]
        bind_port: u16,
        /// Rendezvous key of the wanted service.
        #[arg(long)]
        key: u64,
        /// Send one-sided heartbeats to the broker instead of answering its heartbeats.
        #[arg(long)]
        ping: bool,
        /// Local address selection.
        #[command(flatten)]
        iface: IfaceOpts,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
    },

    /// Fetch what a party is holding: a service's last received message, or a
    /// client's paired service address.
    Collect {
        /// The party's heartbeat address.
        party: Addr,
        /// Accepted for compatibility; not used.
        #[arg(long, hide = true)]
        key: Option<u64>,
        /// Local address selection.
        #[command(flatten)]
        iface: IfaceOpts,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
    },

    /// Send a message to a client, to be relayed through the broker to its service.
    Send {
        /// The client's heartbeat address.
        party: Addr,
        /// Message text.
        #[arg(long, value_name = "TEXT")]
        msg: String,
        /// Accepted for compatibility; not used.
        #[arg(long, hide = true)]
        key: Option<u64>,
        /// Local address selection.
        #[command(flatten)]
        iface: IfaceOpts,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
    },

    /// Run the REST control plane that exposes the operations above over HTTP.
    Serve {
        /// Address to bind. Loopback by default; anything else should sit behind
        /// authentication (see the hardening branch of the cleanup plan).
        #[arg(long, default_value = "127.0.0.1:8080", value_name = "ADDR")]
        bind: SocketAddr,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("nsm").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn publish_parses_positional_broker_and_flags() {
        let cli = parse(&[
            "publish",
            "https://broker:12000",
            "--bind-port",
            "12010",
            "--service-port",
            "9000",
            "--key",
            "1234",
            "-n",
            "en0",
            "--ip-version",
            "4",
        ]);
        match cli.command {
            Command::Publish {
                broker,
                bind_port,
                service_port,
                key,
                ping,
                iface,
                tls,
            } => {
                assert_eq!(broker.to_string(), "https://broker:12000");
                assert_eq!(
                    (bind_port, service_port, key, ping),
                    (12010, 9000, 1234, false)
                );
                assert_eq!(iface.interface.as_deref(), Some("en0"));
                assert_eq!(iface.ip_version, Some(IpVersion::V4));
                assert!(!tls.tls);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn old_snake_case_names_still_work() {
        assert!(matches!(
            parse(&["list_interfaces"]).command,
            Command::ListInterfaces { .. }
        ));
        assert!(matches!(
            parse(&["list_ips", "-n", "lo"]).command,
            Command::ListIps { .. }
        ));
    }

    #[test]
    fn missing_required_flag_is_a_usage_error_not_a_panic() {
        let err = Cli::try_parse_from(["nsm", "listen"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
        let err =
            Cli::try_parse_from(["nsm", "claim", "broker:1", "--bind-port", "1"]).unwrap_err();
        assert!(err.to_string().contains("--key"));
    }

    #[test]
    fn bad_address_and_port_are_reported() {
        let err = Cli::try_parse_from(["nsm", "collect", "not-an-address"]).unwrap_err();
        assert!(err.to_string().contains("port is required"), "{err}");
        let err = Cli::try_parse_from(["nsm", "listen", "--bind-port", "70000"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn listen_defaults_to_tcp_and_serve_to_loopback() {
        match parse(&["listen", "--bind-port", "1"]).command {
            Command::Listen { transport, .. } => assert_eq!(transport, ListenTransport::Tcp),
            other => panic!("{other:?}"),
        }
        match parse(&["serve"]).command {
            Command::Serve { bind } => assert!(bind.ip().is_loopback()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn root_ca_accepts_both_spellings() {
        for flag in ["--root-ca", "--root_ca"] {
            match parse(&["send", "c:1", "--msg", "hi", flag, "/ca.pem"]).command {
                Command::Send { tls, .. } => assert_eq!(
                    tls.root_ca.as_deref(),
                    Some(std::path::Path::new("/ca.pem"))
                ),
                other => panic!("{other:?}"),
            }
        }
    }
}
