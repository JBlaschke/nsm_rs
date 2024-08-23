use serde::{Serialize, Deserialize};

/// Lists available interfaces on device
/// Match command line entries with variables in struct
///
/// Run from command line:
/// $ ./target/debug/nsm -o list_interfaces
#[derive(Debug, Deserialize, Serialize)]
pub struct ListInterfaces {
    /// output helpful messages for debugging
    pub verbose: bool,
    /// list version 4 interfaces
    pub print_v4: bool,
    /// list version 6 interfaces
    pub print_v6: bool
}

/// Lists available IP addresses on interface
/// Match command line entries with variables in struct
///
/// ## Example 
/// Run from command line:
/// ``` $ ./target/debug/nsm -n en0 -o list_ips --ip-version 4 ```
#[derive(Debug, Deserialize, Serialize)]
pub struct ListIPs {
    /// output helpful messages for debugging
    pub verbose: bool,
    /// list version 4 IP addresses
    pub print_v4: bool,
    /// list version 6 IP addresses
    pub print_v6: bool,
    /// name of interface
    pub name: String,
    /// filter IP addresses to 1 output when more than 1 available
    pub starting_octets: Option<String>
}

/// Inititate broker
/// Match command line entries with variables in struct
///
/// ## Example 
/// Run from command line:
/// ``` $ ./target/debug/nsm -n en0 --ip-version 4 --operation listen --bind-port 8000 ```
///
/// Port # info:
/// - 8000 : listens for incoming connections from services and clients
#[derive(Debug, Deserialize, Serialize)]
pub struct Listen {
    /// connecting to version 4 address
    pub print_v4: bool,
    /// connection to version 6 address
    pub print_v6: bool,
    /// name of interface
    pub name: String,
    /// filter IP addresses to 1 output when more than 1 available
    pub starting_octets: Option<String>,
    /// port for listening for incoming connections
    pub bind_port: i32
}

/// Connect to broker and discover available address for data connection.
/// Match command line entries with variables in struct
///
/// ## Example 
/// Run from command line:
/// ``` $ ./target/debug/nsm -n en0 --ip-version 4 --operation claim --host 127.0.0.1 --port 8000 --bind-port 8015 --key 1234 ```
///
/// Address info:
/// - use broker's fixed address
///
/// Port # info:
/// - 8000: same port as Listen's #1
/// - 8015: port for sending heartbeats to broker
/// 
/// Key info:
/// - use same key as a published service
#[derive(Debug, Deserialize, Serialize)]
pub struct Claim {
    /// connecting to version 4 address
    pub print_v4: bool,
    /// connection to version 6 address
    pub print_v6: bool,
    /// broker's local IP address
    pub host: String,
    /// same as Listen's bind_port, notify of new connection
    pub port: i32,
    /// name of interface
    pub name: String,
    /// filter IP addresses to 1 output when more than 1 available
    pub starting_octets: Option<String>,
    /// port for sending heartbeats to broker
    pub bind_port: i32,
    /// match to an available published service
    pub key: u64
}


/// Connect to broker and publish address for data connection.
/// Match command line entries with variables in struct
///
/// ## Example 
/// Run from command line:
/// ``` $ ./target/debug/nsm -n en0 --ip-version 4 --operation publish --host 127.0.0.1 --port 8000 --bind-port 8010 --service-port 8020 --key 1234 ```
///
/// Address info:
/// - use broker's fixed address
///
/// Port # info:
/// - 8000: same port as Listen's #1
/// - 8010: port for sending heartbeats to broker
/// - 8020: port for client connection
#[derive(Debug, Deserialize, Serialize)]
pub struct Publish {
    /// connecting to version 4 address
    pub print_v4: bool,
    /// connection to version 6 address
    pub print_v6: bool,
    /// broker's local IP address
    pub host: String,
    /// same as Listen's bind_port, notify of new connection
    pub port: i32,
    /// name of interface
    pub name: String,
    /// filter IP addresses to 1 output when more than 1 available
    pub starting_octets: Option<String>,
    /// port for sending heartbeats to broker
    pub bind_port: i32,
    /// port for service/client connection
    pub service_port: i32,
    /// uniqueley identifies service
    pub key: u64
}

/// Collect heartbeats/messages inside event loop
///
/// Match command line entries with variables in struct
///
/// ## Example 
/// Run from command line:
/// ``` $ ./target/debug/nsm -n en0 --ip-version 4 --operation collect       ```
///
#[derive(Debug, Deserialize, Serialize)]
pub struct Collect {
    /// connecting to version 4 address
    pub print_v4: bool,
    /// connection to version 6 address
    pub print_v6: bool,
    /// claim's local ip address
    pub host: String,
    /// same as Listen's bind_port, notify of new connection
    pub port: i32,
    /// name of interface
    pub name: String,
    /// filter IP addresses to 1 output when more than 1 available
    pub starting_octets: Option<String>,
    /// uniqueley identifies service
    pub key: u64
}

/// Send a message from the client to the service through broker
///
/// Match command line entries with variables in struct
///
/// ## Example 
/// Run from command line:
/// ``` $ ./target/debug/nsm -n en0 --ip-version 4 --operation send       ```
///
#[derive(Debug, Deserialize, Serialize)]
pub struct Send {
    /// connecting to version 4 address
    pub print_v4: bool,
    /// connection to version 6 address
    pub print_v6: bool,
    /// broker's local IP address
    pub host: String,
    /// same as Listen's bind_port, notify of new connection
    pub port: i32,
    /// name of interface
    pub name: String,
    /// filter IP addresses to 1 output when more than 1 available
    pub starting_octets: Option<String>,
    /// contains user-defined message
    pub msg: String,
    /// uniqueley identifies service
    pub key: u64
}