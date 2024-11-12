//! # Introduction
//! 
//! NERSC Service Mesh manages traffic between service clusters and compute nodes.
//! A "connection broker" with a fixed address passes the address of a service cluster
//! for compute nodes to connect to and run a job.

mod service;
mod models;
mod tls;
mod network;

mod connection;
use connection::ComType;

mod utils;

mod cli;
use cli::{init, parse, CLIOperation};

mod operations;
use operations::{list_interfaces, list_ips, listen, claim, publish, collect, send_msg};

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
#[tokio::main(flavor = "multi_thread", worker_threads = 20)]
async fn main() -> std::io::Result<()> {
    let args = parse(& init());

    let logging_env = Env::default()
        .filter_or("NSM_LOG_LEVEL", "warn")
        .write_style_or("NSM_LOG_STYLE", "always");
    env_logger::init_from_env(logging_env);

    info!("Started NERSC Service MESH");
    trace!("Input args: {:?}", args);

    match args {

        // # ListInterfaces
        // Lists available interfaces on device
        CLIOperation::ListInterfaces(inputs) => {
            let _ = list_interfaces(inputs).await;
        }

        // Lists available IP addresses on interface
        // Match command line entries with variables in struct
        CLIOperation::ListIPs(inputs) => {
            let _ = list_ips(inputs).await;
        }

        // Inititate broker
        // Match command line entries with variables in struct
        CLIOperation::Listen(inputs) => {
            let _ = listen(inputs, ComType::TCP).await;
        }

        // # Claim
        // Connect to broker and discover available address for data connection.
        CLIOperation::Claim(inputs) => {
            let _ = claim(inputs, ComType::TCP).await;
        }
        // # Publish
        // Connect to broker and publish address for data connection.
        CLIOperation::Publish(inputs) => {
            let _ = publish(inputs, ComType::TCP).await;
        }

        CLIOperation::Collect(inputs) => {
            let _ = collect(inputs, ComType::TCP).await;
        }

        CLIOperation::SendMSG(inputs) => {
            let _ = send_msg(inputs, ComType::TCP).await;
        }
    }
    Ok(())
}