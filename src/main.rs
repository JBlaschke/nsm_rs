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

mod api;
use api::{handle_claim, handle_collect, handle_publish, handle_list_interfaces,
    handle_list_ips, handle_send};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use env_logger::Env;

use std::net::SocketAddr;
use clap::ArgMatches;
use hyper::http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;

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

#[tokio::main]
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
                println!("matched listen");
                let _ = listen(inputs).await;
            }

            // # Claim
            // Connect to broker and discover available address for data connection.
            CLIOperation::Claim(inputs) => {
                let _ = claim(inputs).await;
            }
            // # Publish
            // Connect to broker and publish address for data connection.
            CLIOperation::Publish(inputs) => {
                println!("entering publish");
                let _ = publish(inputs).await;
            }

            CLIOperation::Collect(inputs) => {
                let _ = collect(inputs).await;
            }

            CLIOperation::Send(inputs) => {
                let _ = send_msg(inputs).await;
            }
        }
        return Ok(());
    }
    else {
        // api entry
        println!("Starting service on 0.0.0.0.1:8080");
        let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        let incoming = TcpListener::bind(&addr).await?;
        loop {
            let (stream, _) = incoming.accept().await?;
    
            tokio::task::spawn(async move {
                let service = service_fn(move |req| handle_requests(req));
    
                if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await {
                    println!("Failed to serve connection: {:?}", err);
                }
            });
        }
    }

    return Ok(());
}

async fn handle_requests(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let mut response = Response::new(Full::default());

    match (method, path.as_str()) {
        (Method::GET, p) if p.starts_with("/list_interfaces") => {
            handle_list_interfaces(request).await
        },
        (Method::GET, p) if p.starts_with("/list_ips") => {
            handle_list_ips(request).await
        },
        (Method::POST, p) if p.starts_with("/publish") => {
            handle_publish(request).await
        },
        (Method::GET, p) if p.starts_with("/claim") => {
            handle_claim(request).await
        },
        (Method::GET, p) if p.starts_with("/collect") => {
            handle_collect(request).await
        },
        (Method::POST, p) if p.starts_with("/send") => {
            handle_send(request).await
        },
        // (Method::Get, "/task_update") => {
        //     let _ = handle_task_update(request);
        // },
        _ => {
            // Return 404 not found response.
            *response.status_mut() = StatusCode::NOT_FOUND;
            Ok(response)
        }
    }
}