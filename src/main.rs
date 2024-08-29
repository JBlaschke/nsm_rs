//! # Introduction
//! 
//! NERSC Service Mesh manages traffic between service clusters and compute nodes.
//! A "connection broker" with a fixed address passes the address of a service cluster
//! for compute nodes to connect to and run a job.

mod network;
mod connection;
mod service;
mod utils;
mod models; 

mod operations;
use operations::{list_interfaces, list_ips, listen, claim, publish, collect, send_msg};

mod cli;
use cli::{init, parse, CLIOperation};

use tiny_http::{Server, Response, Request, Method};
use clap::ArgMatches;

mod api;
use api::{handle_claim, handle_collect, handle_publish, handle_list_interfaces,
    handle_list_ips, handle_send};

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

    let matches = init();

    if matches.contains_id("operation") {
        //cli entry
        let args = parse(& matches);

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
                let _ = list_interfaces(inputs);
            }

            // Lists available IP addresses on interface
            // Match command line entries with variables in struct
            CLIOperation::ListIPs(inputs) => {
                let _ = list_ips(inputs);
            }

            // Inititate broker
            // Match command line entries with variables in struct
            CLIOperation::Listen(inputs) => {
                let _ = listen(inputs);
            }

            // # Claim
            // Connect to broker and discover available address for data connection.
            CLIOperation::Claim(inputs) => {
                let _ = claim(inputs);
            }
            // # Publish
            // Connect to broker and publish address for data connection.
            CLIOperation::Publish(inputs) => {
                println!("entering publish");
                let _ = publish(inputs);
            }

            CLIOperation::Collect(inputs) => {
                let _ = collect(inputs);
            }

            CLIOperation::Send(inputs) => {
                let _ = send_msg(inputs);
            }
        }
        return Ok(());
    }
    else {
        // api entry
        println!("Starting service on 0.0.0.0.1:8080");
        let server = Server::http("0.0.0.0:8080").unwrap();

        for request in server.incoming_requests() {
            let method = request.method().clone();
            let path = request.url();

            match method {
                Method::Get if path.starts_with("/list_interfaces") => {
                    let _ = handle_list_interfaces(request);
                },
                Method::Get if path.starts_with("/list_ips") => {
                    let _ = handle_list_ips(request);
                },
                Method::Post if path.starts_with("/publish") => {
                    let _ = handle_publish(request);
                },
                Method::Get if path.starts_with("/claim") => {
                    let _ = handle_claim(request);
                },
                Method::Get if path.starts_with("/collect") => {
                    let _ = handle_collect(request);
                },
                Method::Post if path.starts_with("/send") => {
                    let _ = handle_send(request);
                },
                // (Method::Get, "/task_update") => {
                //     let _ = handle_task_update(request);
                // },
                _ => {
                    let _response = Response::from_string("Unsupported HTTP method").with_status_code(405);
                }
            }
        }
    }

    return Ok(());
}