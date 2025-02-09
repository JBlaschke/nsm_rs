use crate::connection::Addr;
use crate::models::{
    ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, SendMSG
};

use std::str::FromStr;
use clap::{Arg, Command, ArgAction, ArgMatches};


/// Parse through command line entry to define variables and initiate functions
pub fn init() -> ArgMatches {
    let args = Command::new("NERSC Service Mesh")
        .version("1.0")
        .author("Johannes Blaschke")
        .about("Manages services meshes with an eye towards HPC")
        .arg(
            Arg::new("operation")
            .value_name("OPERATION")
            .help("Operation to be performed")
            .index(1)
            .required(true)
            .value_parser([
                "list_interfaces",
                "list_ips",
                "listen",
                "claim",
                "publish",
                "collect",
                "send"
            ])
        )
        .arg(
            Arg::new("host")
            .value_name("HOST")
            .help("Host with which to transact")
            .index(2)
            .required(false)
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
            Arg::new("key")
            .long("key")
            .value_name("KEY")
            .help("Service access key")
            .num_args(1)
            .required(false)
            .value_parser(clap::value_parser!(u64))
        )
        .arg(
            Arg::new("msg")
            .long("msg")
            .value_name("MSG")
            .help("Message to send to service")
            .num_args(1)
            .required(false)
        )
        .arg(
            Arg::new("root_ca")
            .long("root_ca")
            .value_name("CA")
            .help("Root certificate manager store path")
            .num_args(1)
            .required(false)
        )
        .arg(
            Arg::new("ping")
            .long("ping")
            .value_name("SET PING")
            .help("Send heartbeats one way to the listener")
            .num_args(0)
            .action(clap::ArgAction::SetTrue)
            .required(false)
        )
        .get_matches();

        return args;
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
    Publish(Publish),
    /// collect outputs from claim and start service-client connection
    Collect(Collect),
    /// send message from client to service containing a message
    SendMSG(SendMSG),
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
            _ => panic!("Only IP versions 4 and 6 supported")
        }
    } else {
        print_v4 = true;
        print_v6 = true;
    }
    let tls = * args.get_one::<bool>("tls").unwrap();

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
            let name            = args.get_one::<String>("interface_name");
            let starting_octets = args.get_one::<String>("ip_start");

            return CLIOperation::ListIPs(
                ListIPs{
                    verbose: verbose,
                    print_v4: print_v4,
                    print_v6: print_v6,
                    name: name.cloned(),
                    starting_octets: starting_octets.cloned()
                }
            )
        }

        "listen" => {
            assert!(args.contains_id("bind_port"));

            let port            = * args.get_one::<i32>("bind_port").unwrap();
            let name            =   args.get_one::<String>("interface_name");
            let starting_octets =   args.get_one::<String>("ip_start");
            let root_ca         =   args.get_one::<String>("root_ca");

            return CLIOperation::Listen(
                Listen{
                    print_v4: print_v4,
                    print_v6:print_v6,
                    name: name.cloned(),
                    starting_octets: starting_octets.cloned(),
                    bind_port: port,
                    tls: tls,
                    root_ca: root_ca.cloned()
                }
            )
        }

        "claim" => {
            assert!(args.contains_id("host"));
            assert!(args.contains_id("bind_port"));
            assert!(args.contains_id("key"));

            let host = Addr::from_str(
                args.get_one::<String>("host").unwrap()
            ).unwrap();
            let key             = * args.get_one::<u64>("key").unwrap();
            let name            =   args.get_one::<String>("interface_name");
            let starting_octets =   args.get_one::<String>("ip_start");
            let bind_port       = * args.get_one::<i32>("bind_port").unwrap();
            let root_ca         =   args.get_one::<String>("root_ca");
            let ping            = * args.get_one::<bool>("ping").unwrap();

            return CLIOperation::Claim(
                Claim{
                    print_v4: print_v4,
                    print_v6: print_v6,
                    host: host,
                    name: name.cloned(),
                    starting_octets: starting_octets.cloned(),
                    bind_port: bind_port,
                    key: key,
                    tls: tls,
                    root_ca: root_ca.cloned(),
                    ping: ping
                }
            )
        }

        "publish" => {
            assert!(args.contains_id("host"));
            assert!(args.contains_id("bind_port"));
            assert!(args.contains_id("service_port"));
            assert!(args.contains_id("key"));

            let host = Addr::from_str(
                args.get_one::<String>("host").unwrap()
            ).unwrap();
            let key             = * args.get_one::<u64>("key").unwrap();
            let name            =   args.get_one::<String>("interface_name");
            let starting_octets =   args.get_one::<String>("ip_start");
            let bind_port       = * args.get_one::<i32>("bind_port").unwrap();
            let service_port    = * args.get_one::<i32>("service_port").unwrap();
            let root_ca         =   args.get_one::<String>("root_ca");
            let ping            = * args.get_one::<bool>("ping").unwrap();

            return CLIOperation::Publish(
                Publish {
                    print_v4: print_v4,
                    print_v6: print_v6,
                    host: host,
                    name: name.cloned(),
                    starting_octets: starting_octets.cloned(),
                    bind_port: bind_port,
                    service_port: service_port,
                    key: key,
                    tls: tls,
                    root_ca: root_ca.cloned(),
                    ping: ping
                }
            )
        }

        "collect" => {
            assert!(args.contains_id("host"));

            let host = Addr::from_str(
                args.get_one::<String>("host").unwrap()
            ).unwrap();
            let name            =   args.get_one::<String>("interface_name");
            let starting_octets =   args.get_one::<String>("ip_start");
            let key             = * args.get_one::<u64>("key").unwrap();
            let root_ca         =   args.get_one::<String>("root_ca");

            return CLIOperation::Collect(
                Collect{
                    print_v4: print_v4,
                    print_v6: print_v6,
                    host: host,
                    name: name.cloned(),
                    starting_octets: starting_octets.cloned(),
                    key: key,
                    tls: tls,
                    root_ca: root_ca.cloned()
                }
            )
        }

        "send" => {
            assert!(args.contains_id("host"));
            assert!(args.contains_id("key"));

            let host = Addr::from_str(
                args.get_one::<String>("host").unwrap()
            ).unwrap();
            let key             = * args.get_one::<u64>("key").unwrap();
            let name            =   args.get_one::<String>("interface_name");
            let starting_octets =   args.get_one::<String>("ip_start");
            let msg             =   args.get_one::<String>("msg").unwrap();
            let root_ca         =   args.get_one::<String>("root_ca");

            return CLIOperation::SendMSG(
                SendMSG {
                    print_v4: print_v4,
                    print_v6: print_v6,
                    host: host,
                    name: name.cloned(),
                    starting_octets: starting_octets.cloned(),
                    msg: msg.to_string(),
                    key: key,
                    tls: tls,
                    root_ca: root_ca.cloned()
                }
            )
        }

        _ => panic!()
    }
}
