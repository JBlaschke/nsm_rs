use crate::operations::{list_interfaces, list_ips, claim, publish, collect, send_msg};

use crate::models::{ListInterfaces, ListIPs, Claim, Publish, Collect, Send};

use crate::connection::ComType;

use hyper::http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;
use std::collections::HashMap;
use url::Url;

pub async fn handle_list_interfaces(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let mut response = Response::new(Full::default());

    let url_str = format!("http://localhost{}", request.uri().to_string());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();

    let _ = match list_interfaces(ListInterfaces {
        verbose: query_pairs.get("verbose").map_or(false, |v| v == "true"),
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
    }).await {
        Ok(_output) => {
            *response.body_mut() = Full::from("Successful request to list_interfaces")
        },
        Err(e) => {
            *response.body_mut() = Full::from(format!("Error processing request: {}", e))
        }
    };
    Ok(response)
}

pub async fn handle_list_ips(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let mut response = Response::new(Full::default());

    let url_str = format!("http://localhost{}", request.uri().to_string());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    println!("{:?}", query_pairs);

    let name = match query_pairs.get("name") {
        Some(n) => n.to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'name' parameter is required.");
            return Ok(response);
        }
    };

    let _ = match list_ips(ListIPs {
        verbose: query_pairs.get("verbose").map_or(false, |v| v == "true"),
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        name,
        starting_octets: query_pairs.get("starting_octets").cloned(),
    }).await {
        Ok(_output) => {
            *response.body_mut() = Full::from("Successful request to list_interfaces")
        },
        Err(e) => {
            *response.body_mut() = Full::from(format!("Error processing request: {}", e))
        }
    };
    Ok(response)
}

pub async fn handle_publish(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {    
    let mut response = Response::new(Full::default());

    let url_str = format!("http://localhost{}", request.uri().path().to_string());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    println!("starting publish ");

    let name = match query_pairs.get("name") {
        Some(n) => n.to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'name' parameter is required.");
            return Ok(response);
        }
    };

    let host = match query_pairs.get("host") {
        Some(n) => n.to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'host' parameter is required.");
            return Ok(response);
        }
    };

    let port = match query_pairs.get("port")
        .and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            *response.body_mut() = Full::from("Error: 'port' parameter is required.");
            return Ok(response);
        }
    };

    let bind_port = match query_pairs.get("bind_port")
    .and_then(|p| p.parse::<i32>().ok()) {
    Some(port) => port,
    None => {
        *response.body_mut() = Full::from("Error: 'bind_port' parameter is required.");
        return Ok(response);
    }
    };

    let service_port = match query_pairs.get("service_port")
    .and_then(|p| p.parse::<i32>().ok()) {
    Some(port) => port,
    None => {
        *response.body_mut() = Full::from("Error: 'service_port' parameter is required.");
        return Ok(response);
    }
    };  

    let key = match query_pairs.get("key")
    .and_then(|p| p.parse::<u64>().ok()) {
    Some(k) => k,
    None => {
        *response.body_mut() = Full::from("Error: 'key' parameter is required.");
        return Ok(response);
    }
    };
    
    let _result = publish(Publish {
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        host,
        port,
        name,
        starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
        bind_port,
        service_port,
        key,
        root_ca: Some(query_pairs.get("root_ca").unwrap_or(&"".to_string()).clone()),
    }, ComType::API);
    Ok(response)
}

pub async fn handle_claim(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // let url_str = format!("http://localhost{}", request.uri().path().to_string());
    // let parsed_url = Url::parse(&url_str).unwrap();

    // // Extract query parameters into a HashMap
    // let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();

    // let name = match query_pairs.get("name") {
    //     Some(n) => n.to_string(),
    //     None => {
    //         let response = Response::from_string("Error: 'name' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let host = match query_pairs.get("host") {
    //     Some(n) => n.to_string(),
    //     None => {
    //         let response = Response::from_string("Error: 'host' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let port = match query_pairs.get("port")
    //     .and_then(|p| p.parse::<i32>().ok()) {
    //     Some(port) => port,
    //     None => {
    //         let response = Response::from_string("Error: 'port' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let bind_port = match query_pairs.get("bind_port")
    // .and_then(|p| p.parse::<i32>().ok()) {
    // Some(port) => port,
    // None => {
    //     let response = Response::from_string("Error: 'bind_port' parameter is required.")
    //         .with_status_code(400);
    //     let _ = request.respond(response);
    //     return;
    // }
    // };

    // let key = match query_pairs.get("key")
    // .and_then(|p| p.parse::<u64>().ok()) {
    // Some(k) => k,
    // None => {
    //     let response = Response::from_string("Error: 'key' parameter is required.")
    //         .with_status_code(400);
    //     let _ = request.respond(response);
    //     return;
    // }
    // };

    // let _result = claim(Claim {
    //     print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
    //     print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
    //     host,
    //     port,
    //     name,
    //     starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
    //     bind_port,
    //     key,
    //     root_ca: Some(query_pairs.get("root_ca").unwrap_or(&"".to_string()).clone()),

    // }, ComType::API);
    let mut response = Response::new(Full::default());
    Ok(response)
}

pub async fn handle_collect(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // let url_str = format!("http://localhost{}", request.uri().path().to_string());
    // let parsed_url = Url::parse(&url_str).unwrap();

    // // Extract query parameters into a HashMap
    // let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();

    // let name = match query_pairs.get("name") {
    //     Some(n) => n.to_string(),
    //     None => {
    //         let response = Response::from_string("Error: 'name' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let host = match query_pairs.get("host") {
    //     Some(n) => n.to_string(),
    //     None => {
    //         let response = Response::from_string("Error: 'host' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let port = match query_pairs.get("port")
    //     .and_then(|p| p.parse::<i32>().ok()) {
    //     Some(port) => port,
    //     None => {
    //         let response = Response::from_string("Error: 'port' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let key = match query_pairs.get("key")
    // .and_then(|p| p.parse::<u64>().ok()) {
    // Some(k) => k,
    // None => {
    //     let response = Response::from_string("Error: 'key' parameter is required.")
    //         .with_status_code(400);
    //     let _ = request.respond(response);
    //     return;
    // }
    // };

    // let _result = collect(Collect {
    //     print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
    //     print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
    //     host,
    //     port,
    //     name,
    //     starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
    //     key,
    //     root_ca: Some(query_pairs.get("root_ca").unwrap_or(&"".to_string()).clone()),
    // }, ComType::API);
    let mut response = Response::new(Full::default());
    Ok(response)
}

pub async fn handle_send(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // let url_str = format!("http://localhost{}", request.uri().path().to_string());
    // let parsed_url = Url::parse(&url_str).unwrap();

    // // Extract query parameters into a HashMap
    // let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    
    // let name = match query_pairs.get("name") {
    //     Some(n) => n.to_string(),
    //     None => {
    //         let response = Response::from_string("Error: 'name' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let host = match query_pairs.get("host") {
    //     Some(n) => n.to_string(),
    //     None => {
    //         let response = Response::from_string("Error: 'host' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let port = match query_pairs.get("port")
    //     .and_then(|p| p.parse::<i32>().ok()) {
    //     Some(port) => port,
    //     None => {
    //         let response = Response::from_string("Error: 'port' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let msg = match query_pairs.get("msg") {
    //     Some(n) => n.to_string(),
    //     None => {
    //         let response = Response::from_string("Error: 'msg' parameter is required.")
    //             .with_status_code(400);
    //         let _ = request.respond(response);
    //         return;
    //     }
    // };

    // let key = match query_pairs.get("key")
    // .and_then(|p| p.parse::<u64>().ok()) {
    // Some(k) => k,
    // None => {
    //     let response = Response::from_string("Error: 'key' parameter is required.")
    //         .with_status_code(400);
    //     let _ = request.respond(response);
    //     return;
    // }
    // };
    // let _result = send_msg(Send {
    //     print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
    //     print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
    //     host,
    //     port,
    //     name,
    //     starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
    //     msg,
    //     key,
    //     root_ca: Some(query_pairs.get("root_ca").unwrap_or(&"".to_string()).clone()),
    // }, ComType::API);
    let mut response = Response::new(Full::default());
    Ok(response)
}

// fn handle_task_id_update(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {

// }

// fn handle_task_update(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {

// }
