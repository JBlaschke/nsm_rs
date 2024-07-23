use clap::{Arg, Command, ArgAction, ArgMatches};

/// Parse through command line entry to define variables and initiate functions
pub fn init() -> ArgMatches {
    let args = Command::new("NERSC Service Mesh")
        .version("1.0")
        .author("Johannes Blaschke")
        .about("Manages services meshes with an eye towards HPC")
        .arg(
            Arg::new("operation")
            .short('o')
            .long("operation")
            .value_name("OPERATION")
            .help("Operation to be performed")
            .num_args(1)
            .required(true)
            .value_parser(["list_interfaces", "list_ips", "listen", "claim", "publish"])
        )
        .arg(
            Arg::new("interface_name")
            .short('n')
            .long("name")
            .value_name("NAME")
            .help("Interface Name")
            .num_args(1)
            .required(false)
        )
        .arg(
            Arg::new("ip_start")
            .short('i')
            .long("ip-start")
            .value_name("STARTING OCTETS")
            .help("Only return ip addresses whose starting octets match these")
            .num_args(1)
            .required(false)
        )
        .arg(
            Arg::new("ip_version")
            .long("ip-version")
            .value_name("IP VERSION")
            .help("Output results only matching this IP version")
            .num_args(1)
            .required(false)
            .value_parser(clap::value_parser!(i32))
        )
        .arg(
            Arg::new("bind_port")
            .long("bind-port")
            .value_name("PORT")
            .help("Port to bind the heartbeat server to")
            .num_args(1)
            .required(false)
            .value_parser(clap::value_parser!(i32))
        )
        .arg(
            Arg::new("service_port")
            .long("service-port")
            .value_name("PORT")
            .help("Port exposed by service")
            .num_args(1)
            .required(false)
            .value_parser(clap::value_parser!(i32))
        )
        .arg(
            Arg::new("verbose")
            .short('v')
            .long("verbose")
            .help("Don't output headers")
            .num_args(0)
            .required(false)
            .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("host")
            .long("host")
            .value_name("HOST")
            .help("Host to use during transaction")
            .num_args(1)
            .required(false)
        )
        .arg(
            Arg::new("port")
            .long("port")
            .value_name("PORT")
            .help("Port to use during transaction")
            .num_args(1)
            .required(false)
            .value_parser(clap::value_parser!(i32))
        )
        .arg(
            Arg::new("key")
            .long("key")
            .value_name("KEY")
            .help("Service access key")
            .num_args(1)
            .required(false)
            .value_parser(clap::value_parser!(u64))
        )
        .get_matches();

        return args;
}

/// Lists available interfaces on device
/// Match command line entries with variables in struct
///
/// Run from command line:
/// $ ./target/debug/nsm -o list_interfaces
#[derive(Debug)]
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
/// $ ./target/debug/nsm -n en0 -o list_ips --ip-version 4
#[derive(Debug)]
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
/// $ ./target/debug/nsm -n en0 --ip-version 4 --operation listen --bind-port 8000
///
/// Port # info:
/// - 8000 : listens for incoming connections from services and clients
#[derive(Debug)]
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
/// $ ./target/debug/nsm -n en0 --ip-version 4 --operation claim --host 127.0.0.1 --port 8000 --bind-port 8015 --key 1234
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
#[derive(Debug)]
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

/// Match command line entries with variables in struct
///
/// Connect to broker and publish address for data connection.
///
/// ## Example 
/// Run from command line:
/// $ ./target/debug/nsm -n en0 --ip-version 4 --operation publish --host 127.0.0.1 --port 8000 --bind-port 8010 --service-port 8020 --key 1234
///
/// Address info:
/// - use broker's fixed address
///
/// Port # info:
/// - 8000: same port as Listen's #1
/// - 8010: port for sending heartbeats to broker
/// - 8020: port for client connection
#[derive(Debug)]
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

/// Define possible operations to run in main() based on command line entry
#[derive(Debug)]
pub enum CLIOperation {
    /// list available interfaces on device
    ListInterfaces(ListInterfaces),
    /// list available IP addresses on interface
    ListIPs(ListIPs),
    /// initiate connection broker
    Listen(Listen),
    /// claim a service
    Claim(Claim),
    /// publish a service
    Publish(Publish)
}

/// Parse through command line arguments and send them to a CLIOperation
pub fn parse(args: & ArgMatches) -> CLIOperation {
    let ip_version =   args.get_one::<i32>("ip_version");
    let verbose    = * args.get_one::<bool>("verbose").unwrap();
    let mut print_v4 = false;
    let mut print_v6 = false;
    if ip_version.is_some() {
        match * ip_version.unwrap() {
            4 => print_v4 = true,
            6 => print_v6 = true,
            _ => panic!(
                "Please specify IP version 4 or 6, or ommit `--ip-version` for both."
            )
        }
    } else {
        print_v4 = true;
        print_v6 = true;
    }

    let operation = args.get_one::<String>("operation").unwrap();

    match operation.as_str() {
        "list_interfaces" => {
            return CLIOperation::ListInterfaces(
                ListInterfaces{
                    verbose: verbose,
                    print_v4: print_v4,
                    print_v6: print_v6
                }
            )
        }
        "list_ips" => {
            assert!(args.contains_id("interface_name"));
            let name = args.get_one::<String>("interface_name").unwrap();
            let starting_octets = args.get_one::<String>("ip_start");
            return CLIOperation::ListIPs(
                ListIPs{
                    verbose: verbose,
                    print_v4: print_v4,
                    print_v6: print_v6,
                    name: name.to_string(),
                    starting_octets: starting_octets.cloned()
                }
            )
        }
        "listen" => {
            assert!(args.contains_id("interface_name"));
            assert!(args.contains_id("bind_port"));
            let port = * args.get_one::<i32>("bind_port").unwrap();
            let name =   args.get_one::<String>("interface_name").unwrap();
            let starting_octets = args.get_one::<String>("ip_start");
            return CLIOperation::Listen(
                Listen{
                    print_v4: print_v4,
                    print_v6:print_v6,
                    name: name.to_string(),
                    starting_octets: starting_octets.cloned(),
                    bind_port: port
                }
            )
        }
        "claim" => {
            assert!(args.contains_id("host"));
            assert!(args.contains_id("port"));
            assert!(args.contains_id("interface_name"));
            assert!(args.contains_id("bind_port"));
            assert!(args.contains_id("key"));
            let host =   args.get_one::<String>("host").unwrap();
            let port = * args.get_one::<i32>("port").unwrap();
            let key  = * args.get_one::<u64>("key").unwrap();
            let name = args.get_one::<String>("interface_name").unwrap();
            let starting_octets =   args.get_one::<String>("ip_start");
            let bind_port       = * args.get_one::<i32>("bind_port").unwrap();
            return CLIOperation::Claim(
                Claim{
                    print_v4: print_v4,
                    print_v6: print_v6,
                    host: host.to_string(),
                    port: port,
                    name: name.to_string(),
                    starting_octets: starting_octets.cloned(),
                    bind_port: bind_port,
                    key: key
                }
            )
        }
        "publish" => {
            assert!(args.contains_id("host"));
            assert!(args.contains_id("port"));
            assert!(args.contains_id("interface_name"));
            assert!(args.contains_id("bind_port"));
            assert!(args.contains_id("service_port"));
            assert!(args.contains_id("key"));
            let host =   args.get_one::<String>("host").unwrap();
            let port = * args.get_one::<i32>("port").unwrap();
            let key  = * args.get_one::<u64>("key").unwrap();
            let name = args.get_one::<String>("interface_name").unwrap();
            let starting_octets =   args.get_one::<String>("ip_start");
            let bind_port       = * args.get_one::<i32>("bind_port").unwrap();
            let service_port    = * args.get_one::<i32>("service_port").unwrap();
            return CLIOperation::Publish(
                Publish {
                    print_v4: print_v4,
                    print_v6: print_v6,
                    host: host.to_string(),
                    port: port,
                    name: name.to_string(),
                    starting_octets: starting_octets.cloned(),
                    bind_port: bind_port,
                    service_port: service_port,
                    key: key
                }
            )
        }
        &_ => panic!()
    }


}