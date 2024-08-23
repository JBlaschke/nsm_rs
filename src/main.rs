//! # Introduction
//! 
//! NERSC Service Mesh manages traffic between service clusters and compute nodes.
//! A "connection broker" with a fixed address passes the address of a service cluster
//! for compute nodes to connect to and run a job.

mod network;
mod connection;
mod service;
mod utils;

mod operations;
use operations::{list_interfaces, list_ips, listen, claim, publish, collect, send_msg};

mod models;
use models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, Send};

mod cli;
use cli::{init, parse, CLIOperation};

use actix_web::{web, App, HttpServer, HttpResponse, Responder, HttpRequest};
use clap::ArgMatches;


#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use env_logger::Env;

async fn handle_list_interfaces(query: web::Query<ListInterfaces>) -> impl Responder {
    let inputs = query.into_inner();
    
    let result = list_interfaces(ListInterfaces {
        verbose: inputs.verbose,
        print_v4: inputs.print_v4,
        print_v6: inputs.print_v6,
    });

    match result {
        Ok(_) => HttpResponse::Ok().body("Successfully listed interfaces"),
        Err(err) => {
            HttpResponse::InternalServerError().body(format!("Error: {}", err))
        }
    }
}

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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
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
                let _ = publish(inputs);
            }

            CLIOperation::Collect(inputs) => {
                let _ = collect(inputs);
            }

            CLIOperation::Send(inputs) => {
                let _ = send_msg(inputs);
            }
        }
        Ok(())
    }
    else {
        // api entry
        HttpServer::new(|| {
            App::new()
                .route("/list_interfaces", web::get().to(handle_list_interfaces))
        })
        .bind("127.0.0.1:11000")?
        .run()
        .await
    }
}