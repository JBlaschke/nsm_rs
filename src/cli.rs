//! Command-line interface.
//!
//! One binary, one subcommand per operation. The structs here are also the
//! request types of the REST control plane, so the CLI and the API share one
//! definition and one set of validation rules.
//!
//! `store` is a group of four subcommands ([`StoreCommand`]), one per
//! operation on the store a client shares with its service.
//!
//! The transport is taken from the peer address: `host:port` is raw TCP,
//! `http://host:port` and `https://host:port` are HTTP. `listen` and `serve`
//! have no peer address and take `--transport` instead.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use crate::config::{BrokerPolicy, Limits, Timing, TlsPaths};
use crate::net::{Addr, IpVersion, Selector, Transport};
use crate::protocol::{StoreKey, StoreOp};

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
    /// Serve TLS on this party's own listener. Requires --tls-cert and --tls-key.
    #[arg(long)]
    pub tls: bool,

    /// PEM certificate chain this process presents when it serves TLS.
    #[arg(long, value_name = "PEM", env = "CERT_PATH")]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key matching --tls-cert.
    #[arg(long, value_name = "PEM", env = "KEY_PATH")]
    pub tls_key: Option<PathBuf>,

    /// PEM bundle of root certificates used to verify peers.
    #[arg(
        long = "root-ca",
        alias = "root_ca",
        value_name = "PEM",
        env = "ROOT_PATH"
    )]
    pub root_ca: Option<PathBuf>,

    /// Trust the platform certificate store when no --root-ca is given.
    #[arg(long)]
    pub system_roots: bool,
}

impl TlsOpts {
    /// The file locations as the library wants them.
    pub fn paths(&self) -> TlsPaths {
        TlsPaths {
            cert: self.tls_cert.clone(),
            key: self.tls_key.clone(),
            root_ca: self.root_ca.clone(),
            system_roots: self.system_roots,
        }
    }
}

/// Positive number of seconds, fractions allowed.
fn parse_secs(s: &str) -> Result<Duration, String> {
    let secs: f64 = s
        .trim()
        .parse()
        .map_err(|_| format!("{s:?} is not a number of seconds"))?;
    if !secs.is_finite() || secs <= 0.0 {
        return Err("must be a positive number of seconds".to_owned());
    }
    Ok(Duration::from_secs_f64(secs))
}

/// Overrides for heartbeat and request timing, in seconds. Anything not
/// given keeps the default from [`Timing`].
#[derive(Debug, Clone, Default, Args, serde::Serialize, serde::Deserialize)]
pub struct TimingOpts {
    /// Seconds between heartbeats (broker to party, or party pings).
    #[arg(long, value_name = "SECS", value_parser = parse_secs)]
    pub heartbeat_interval: Option<Duration>,
    /// Seconds one heartbeat may take before it counts as failed.
    #[arg(long, value_name = "SECS", value_parser = parse_secs)]
    pub heartbeat_timeout: Option<Duration>,
    /// Consecutive failed heartbeats after which a party is removed.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub fail_threshold: Option<u32>,
    /// Seconds of silence after which a pinging party is removed.
    #[arg(long, value_name = "SECS", value_parser = parse_secs)]
    pub ping_staleness: Option<Duration>,
    /// Seconds without a heartbeat after which a party gives up on its broker.
    #[arg(long, value_name = "SECS", value_parser = parse_secs)]
    pub broker_watchdog: Option<Duration>,
    /// Seconds allowed for one request/response exchange.
    #[arg(long, value_name = "SECS", value_parser = parse_secs)]
    pub request_timeout: Option<Duration>,
    /// Seconds allowed for connecting (and the TLS handshake).
    #[arg(long, value_name = "SECS", value_parser = parse_secs)]
    pub connect_timeout: Option<Duration>,
}

impl TimingOpts {
    /// The defaults with these overrides applied.
    pub fn timing(&self) -> Timing {
        let mut t = Timing::default();
        if let Some(v) = self.heartbeat_interval {
            t.heartbeat_interval = v;
        }
        if let Some(v) = self.heartbeat_timeout {
            t.heartbeat_timeout = v;
        }
        if let Some(v) = self.fail_threshold {
            t.fail_threshold = v;
        }
        if let Some(v) = self.ping_staleness {
            t.ping_staleness = v;
        }
        if let Some(v) = self.broker_watchdog {
            t.broker_watchdog = v;
        }
        if let Some(v) = self.request_timeout {
            t.request_timeout = v;
        }
        if let Some(v) = self.connect_timeout {
            t.connect_timeout = v;
        }
        t
    }
}

/// Overrides for size and count limits. Anything not given keeps the
/// default from [`Limits`].
#[derive(Debug, Clone, Default, Args, serde::Serialize, serde::Deserialize)]
pub struct LimitsOpts {
    /// Largest message accepted on any transport, in bytes (at least 1024;
    /// `listen` needs at least `--max-store-bytes` plus 1024, which is 17408
    /// with the default store budget and never less than 1280).
    #[arg(long, value_name = "BYTES", value_parser = clap::value_parser!(u64).range(1024..))]
    pub max_frame_bytes: Option<u64>,
    /// Connections a listener serves concurrently.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub max_connections: Option<u64>,
    /// Registrations (services plus clients) a broker holds at once
    /// (broker only; no effect on `serve`).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub max_registrations: Option<u64>,
    /// Budget of each claim's shared store, counting every entry as its
    /// JSON-encoded key and value plus 64 bytes (256 to 32768, default
    /// 16384; broker only, no effect on `serve`).
    #[arg(long, value_name = "BYTES", value_parser = clap::value_parser!(u64).range(256..=32768))]
    pub max_store_bytes: Option<u64>,
}

impl LimitsOpts {
    /// The defaults with these overrides applied.
    pub fn limits(&self) -> Limits {
        let mut l = Limits::default();
        if let Some(v) = self.max_frame_bytes {
            l.max_frame_bytes = usize::try_from(v).unwrap_or(usize::MAX);
        }
        if let Some(v) = self.max_connections {
            l.max_connections = usize::try_from(v).unwrap_or(usize::MAX);
        }
        if let Some(v) = self.max_registrations {
            l.max_registrations = usize::try_from(v).unwrap_or(usize::MAX);
        }
        if let Some(v) = self.max_store_bytes {
            l.max_store_bytes = usize::try_from(v).unwrap_or(usize::MAX);
        }
        l
    }
}

/// Broker admission policy.
#[derive(Debug, Clone, Default, Args, serde::Serialize, serde::Deserialize)]
pub struct BrokerOpts {
    /// Reject parties whose advertised address differs from the one they
    /// connect from.
    #[arg(long)]
    pub require_matching_host: bool,
    /// Registrations accepted per advertised host.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub max_registrations_per_host: Option<u64>,
}

impl BrokerOpts {
    /// The defaults with these overrides applied.
    pub fn policy(&self) -> BrokerPolicy {
        let mut p = BrokerPolicy {
            require_matching_host: self.require_matching_host,
            ..BrokerPolicy::default()
        };
        if let Some(v) = self.max_registrations_per_host {
            p.max_registrations_per_host = usize::try_from(v).unwrap_or(usize::MAX);
        }
        p
    }
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
        /// Transport to serve (`tls` and `https` need --tls-cert and --tls-key).
        #[arg(long, value_enum, default_value = "tcp")]
        transport: Transport,
        /// Local address selection.
        #[command(flatten)]
        iface: IfaceOpts,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
        /// Limit overrides.
        #[command(flatten)]
        limits: LimitsOpts,
        /// Admission policy.
        #[command(flatten)]
        policy: BrokerOpts,
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
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
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
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },

    /// Fetch the last text a party received from its peer.
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
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },

    /// Print the service a client is paired with (its `host:port`), as of now.
    Peer {
        /// The client's heartbeat address.
        party: Addr,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },

    /// Send a message to a party, to be relayed through the broker to its peer.
    Send {
        /// The party's heartbeat address: a client's, for its service; a
        /// service's, for the client holding it.
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
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },

    /// Read and write the store a client shares with its service, through
    /// either party.
    Store {
        /// The operation.
        #[command(subcommand)]
        op: StoreCommand,
    },

    /// Run the REST control plane that exposes the operations above over HTTP.
    Serve {
        /// Address to bind. Loopback by default; binding anything else
        /// requires --token.
        #[arg(long, default_value = "127.0.0.1:8080", value_name = "ADDR")]
        bind: SocketAddr,
        /// Bearer token clients must present (`Authorization: Bearer <TOKEN>`).
        #[arg(long, env = "NSM_TOKEN", value_name = "TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// TLS material handed to the parties this server starts (`--tls`
        /// itself has no effect here; a job asks for TLS in its request body).
        #[command(flatten)]
        tls: TlsOpts,
        /// Timing overrides for the parties this server starts.
        #[command(flatten)]
        timing: TimingOpts,
        /// Limit overrides for the parties this server starts.
        #[command(flatten)]
        limits: LimitsOpts,
    },
}

/// The operations of `nsm store`. `PARTY` is either party's heartbeat
/// address: the client's or its service's, which share one store; `KEY` is a
/// store key, unrelated to the rendezvous `--key`.
#[derive(Debug, Subcommand)]
pub enum StoreCommand {
    /// Print the value stored under a key (exit 3 when the key is not set).
    Get {
        /// Either party's heartbeat address.
        party: Addr,
        /// Store key: 1 to 128 characters from A-Z a-z 0-9 . _ - : /, not
        /// starting with -.
        key: StoreKey,
        /// Print the broker's reply as one line of JSON instead.
        #[arg(long)]
        json: bool,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },

    /// Store a value under a key, replacing what was there, and print the
    /// write's version.
    Put {
        /// Either party's heartbeat address.
        party: Addr,
        /// Store key: 1 to 128 characters from A-Z a-z 0-9 . _ - : /, not
        /// starting with -.
        key: StoreKey,
        /// The value: any text, including empty text.
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        value: String,
        /// Print the broker's reply as one line of JSON instead.
        #[arg(long)]
        json: bool,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },

    /// Remove a key; succeeds whether or not it was set.
    Delete {
        /// Either party's heartbeat address.
        party: Addr,
        /// Store key: 1 to 128 characters from A-Z a-z 0-9 . _ - : /, not
        /// starting with -.
        key: StoreKey,
        /// Print the broker's reply as one line of JSON.
        #[arg(long)]
        json: bool,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },

    /// Print every key in the store, one per line.
    List {
        /// Either party's heartbeat address.
        party: Addr,
        /// Print the broker's reply, with every value, as one line of JSON
        /// instead.
        #[arg(long)]
        json: bool,
        /// TLS options.
        #[command(flatten)]
        tls: TlsOpts,
        /// Timing overrides.
        #[command(flatten)]
        timing: TimingOpts,
    },
}

impl StoreCommand {
    /// The request as the operations layer takes it: the party to ask, the
    /// operation, the TLS and timing options, and whether to print the reply
    /// as JSON.
    pub fn into_parts(self) -> (Addr, StoreOp, TlsOpts, TimingOpts, bool) {
        match self {
            StoreCommand::Get {
                party,
                key,
                json,
                tls,
                timing,
            } => (party, StoreOp::Get { key }, tls, timing, json),
            StoreCommand::Put {
                party,
                key,
                value,
                json,
                tls,
                timing,
            } => (
                party,
                StoreOp::Put {
                    key,
                    value,
                    if_version: None,
                },
                tls,
                timing,
                json,
            ),
            StoreCommand::Delete {
                party,
                key,
                json,
                tls,
                timing,
            } => (
                party,
                StoreOp::Delete {
                    key,
                    if_version: None,
                },
                tls,
                timing,
                json,
            ),
            StoreCommand::List {
                party,
                json,
                tls,
                timing,
            } => (party, StoreOp::List, tls, timing, json),
        }
    }
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
                timing: _,
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
    fn peer_takes_a_party_address_and_the_shared_options() {
        let cli = parse(&["peer", "http://10.0.0.9:41232", "--request-timeout", "2"]);
        match cli.command {
            Command::Peer { party, tls, timing } => {
                assert_eq!(party.to_string(), "http://10.0.0.9:41232");
                assert!(!tls.tls);
                assert_eq!(timing.timing().request_timeout, Duration::from_secs(2));
            }
            other => panic!("{other:?}"),
        }
        let err = Cli::try_parse_from(["nsm", "peer"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    fn store(args: &[&str]) -> (Addr, StoreOp, TlsOpts, TimingOpts, bool) {
        match parse(&[&["store"], args].concat()).command {
            Command::Store { op } => op.into_parts(),
            other => panic!("{other:?}"),
        }
    }

    fn store_key(text: &str) -> StoreKey {
        text.parse().unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    #[test]
    fn store_subcommands_become_store_ops() {
        let (party, op, tls, timing, json) = store(&["get", "10.0.0.9:41232", "input/path"]);
        assert_eq!(party.to_string(), "10.0.0.9:41232");
        assert_eq!(
            op,
            StoreOp::Get {
                key: store_key("input/path")
            }
        );
        assert!(!tls.tls && !json);
        assert_eq!(timing.timing(), Timing::default());

        let (party, op, _, timing, json) = store(&[
            "put",
            "http://10.0.0.7:41231",
            "step",
            "--value",
            "5",
            "--json",
            "--request-timeout",
            "2",
        ]);
        assert_eq!(party.to_string(), "http://10.0.0.7:41231");
        assert_eq!(
            op,
            StoreOp::Put {
                key: store_key("step"),
                value: "5".into(),
                if_version: None,
            }
        );
        assert!(json);
        assert_eq!(timing.timing().request_timeout, Duration::from_secs(2));

        let (_, op, _, _, json) = store(&["delete", "c:1", "step", "--json"]);
        assert_eq!(
            op,
            StoreOp::Delete {
                key: store_key("step"),
                if_version: None,
            }
        );
        assert!(json);

        let (_, op, tls, _, json) = store(&["list", "c:1", "--root-ca", "/ca.pem"]);
        assert_eq!(op, StoreOp::List);
        assert!(!json);
        assert_eq!(
            tls.root_ca.as_deref(),
            Some(std::path::Path::new("/ca.pem"))
        );
    }

    #[test]
    fn store_put_takes_any_value_including_hyphens_and_empty_text() {
        for value in ["-5", "--json", "", "two words", "line\nbreak"] {
            let (_, op, _, _, json) = store(&["put", "c:1", "step", "--value", value]);
            assert_eq!(
                op,
                StoreOp::Put {
                    key: store_key("step"),
                    value: value.into(),
                    if_version: None,
                },
                "{value:?}"
            );
            assert!(!json, "{value:?} is the value, not a flag");
        }
        let err = Cli::try_parse_from(["nsm", "store", "put", "c:1", "step"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
        assert!(err.to_string().contains("--value"), "{err}");
    }

    #[test]
    fn an_invalid_store_key_is_a_usage_error() {
        for bad in ["", "two words", "-x", "a*b", "ü"] {
            let err = Cli::try_parse_from(["nsm", "store", "get", "c:1", bad]).unwrap_err();
            assert!(
                matches!(
                    err.kind(),
                    clap::error::ErrorKind::ValueValidation
                        | clap::error::ErrorKind::UnknownArgument
                ),
                "{bad:?}: {err}"
            );
            assert_eq!(err.exit_code(), 2, "{bad:?}");
        }
        let err = Cli::try_parse_from(["nsm", "store", "get", "c:1", "a*b"]).unwrap_err();
        assert!(err.to_string().contains("a store key is"), "{err}");
        for missing in [&["store", "get", "c:1"][..], &["store", "list"], &["store"]] {
            let err = Cli::try_parse_from(std::iter::once("nsm").chain(missing.iter().copied()))
                .unwrap_err();
            assert_eq!(err.exit_code(), 2, "{missing:?}");
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
            Command::Listen { transport, .. } => assert_eq!(transport, Transport::Tcp),
            other => panic!("{other:?}"),
        }
        match parse(&["listen", "--bind-port", "1", "--transport", "https"]).command {
            Command::Listen { transport, .. } => assert_eq!(transport, Transport::Https),
            other => panic!("{other:?}"),
        }
        match parse(&["serve"]).command {
            Command::Serve { bind, token, .. } => {
                assert!(bind.ip().is_loopback());
                assert!(token.is_none());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn timing_and_limit_overrides_parse_and_validate() {
        match parse(&[
            "listen",
            "--bind-port",
            "1",
            "--heartbeat-interval",
            "0.5",
            "--fail-threshold",
            "3",
            "--max-frame-bytes",
            "4096",
            "--max-store-bytes",
            "2048",
            "--require-matching-host",
            "--max-registrations-per-host",
            "5",
        ])
        .command
        {
            Command::Listen {
                timing,
                limits,
                policy,
                ..
            } => {
                let t = timing.timing();
                assert_eq!(t.heartbeat_interval, Duration::from_millis(500));
                assert_eq!(t.fail_threshold, 3);
                assert_eq!(t.request_timeout, Timing::default().request_timeout);
                assert_eq!(limits.limits().max_frame_bytes, 4096);
                assert_eq!(limits.limits().max_store_bytes, 2048);
                assert_eq!(
                    limits.limits().max_registrations,
                    Limits::default().max_registrations
                );
                let p = policy.policy();
                assert!(p.require_matching_host);
                assert_eq!(p.max_registrations_per_host, 5);
            }
            other => panic!("{other:?}"),
        }
        for bad in [
            &["listen", "--bind-port", "1", "--heartbeat-interval", "0"][..],
            &["listen", "--bind-port", "1", "--heartbeat-interval", "soon"],
            &["listen", "--bind-port", "1", "--max-frame-bytes", "10"],
            &["listen", "--bind-port", "1", "--fail-threshold", "0"],
            &["listen", "--bind-port", "1", "--max-store-bytes", "255"],
            &["listen", "--bind-port", "1", "--max-store-bytes", "32769"],
        ] {
            let err =
                Cli::try_parse_from(std::iter::once("nsm").chain(bad.iter().copied())).unwrap_err();
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn max_store_bytes_accepts_its_whole_range() {
        for (given, expected) in [
            (None, 16 * 1024),
            (Some("256"), 256),
            (Some("32768"), 32768),
        ] {
            let mut args = vec!["listen", "--bind-port", "1"];
            args.extend(
                given
                    .map(|v| ["--max-store-bytes", v])
                    .into_iter()
                    .flatten(),
            );
            match parse(&args).command {
                Command::Listen { limits, .. } => {
                    assert_eq!(limits.limits().max_store_bytes, expected, "{given:?}")
                }
                other => panic!("{other:?}"),
            }
        }
        // `serve` accepts it with the other limits and ignores it; only a
        // broker has stores.
        match parse(&["serve", "--max-store-bytes", "1024"]).command {
            Command::Serve { limits, .. } => assert_eq!(limits.limits().max_store_bytes, 1024),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tls_opts_become_tls_paths() {
        match parse(&[
            "listen",
            "--bind-port",
            "1",
            "--tls-cert",
            "/c.pem",
            "--tls-key",
            "/k.pem",
        ])
        .command
        {
            Command::Listen { tls, .. } => {
                let p = tls.paths();
                assert!(p.has_server_identity());
                assert_eq!(p.root_ca, None);
                assert!(!p.system_roots);
            }
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
