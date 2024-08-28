use crate::operations::{list_interfaces, list_ips, listen, claim, publish, collect, send_msg};

use crate::models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, Send};

use tiny_http::{Server, Response, Request, Method};
use clap::ArgMatches;
use url::Url;
use std::collections::HashMap;

pub fn handle_list_interfaces(request: Request) {
    let url_str = format!("http://localhost{}", request.url());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();

    let _ = match list_interfaces(ListInterfaces {
        verbose: query_pairs.get("verbose").map_or(false, |v| v == "true"),
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
    }) {
        Ok(output) => {
            let response = Response::from_string(format!("Successful request to list_interfaces"))
            .with_status_code(200);
        let _ = request.respond(response);
        },
        Err(e) => {
            let response = Response::from_string(format!("Error processing request: {}", e))
                .with_status_code(500);
            let _ = request.respond(response);
            return;
        }
    };
}

pub fn handle_list_ips(request: Request) {
    let url_str = format!("http://localhost{}", request.url());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    println!("{:?}", query_pairs);

    let name = match query_pairs.get("name") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'name' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let _ = match list_ips(ListIPs {
        verbose: query_pairs.get("verbose").map_or(false, |v| v == "true"),
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        name,
        starting_octets: query_pairs.get("starting_octets").cloned(),
    }) {
        Ok(output) => {
            let response = Response::from_string(format!("Successful request to list_ips"))
            .with_status_code(200);
        let _ = request.respond(response);
        },
        Err(e) => {
            let response = Response::from_string(format!("Error processing request: {}", e))
                .with_status_code(500);
            let _ = request.respond(response);
            return;
        }
    };

}

pub fn handle_publish(request: Request) {    
    let url_str = format!("http://localhost{}", request.url());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    println!("starting publish ");

    let name = match query_pairs.get("name") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'name' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let host = match query_pairs.get("host") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'host' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let port = match query_pairs.get("port")
        .and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            let response = Response::from_string("Error: 'port' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let bind_port = match query_pairs.get("bind_port")
    .and_then(|p| p.parse::<i32>().ok()) {
    Some(port) => port,
    None => {
        let response = Response::from_string("Error: 'bind_port' parameter is required.")
            .with_status_code(400);
        let _ = request.respond(response);
        return;
    }
    };

    let service_port = match query_pairs.get("service_port")
    .and_then(|p| p.parse::<i32>().ok()) {
    Some(port) => port,
    None => {
        let response = Response::from_string("Error: 'service_port' parameter is required.")
            .with_status_code(400);
        let _ = request.respond(response);
        return;
    }
    };  

    let key = match query_pairs.get("key")
    .and_then(|p| p.parse::<u64>().ok()) {
    Some(k) => k,
    None => {
        let response = Response::from_string("Error: 'key' parameter is required.")
            .with_status_code(400);
        let _ = request.respond(response);
        return;
    }
    };
    
    let result = publish(Publish {
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        host,
        port,
        name,
        starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
        bind_port,
        service_port,
        key,
    });
}

pub fn handle_claim(request: Request) {
    let url_str = format!("http://localhost{}", request.url());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();

    let name = match query_pairs.get("name") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'name' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let host = match query_pairs.get("host") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'host' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let port = match query_pairs.get("port")
        .and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            let response = Response::from_string("Error: 'port' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let bind_port = match query_pairs.get("bind_port")
    .and_then(|p| p.parse::<i32>().ok()) {
    Some(port) => port,
    None => {
        let response = Response::from_string("Error: 'bind_port' parameter is required.")
            .with_status_code(400);
        let _ = request.respond(response);
        return;
    }
    };

    let key = match query_pairs.get("key")
    .and_then(|p| p.parse::<u64>().ok()) {
    Some(k) => k,
    None => {
        let response = Response::from_string("Error: 'key' parameter is required.")
            .with_status_code(400);
        let _ = request.respond(response);
        return;
    }
    };

    let result = claim(Claim {
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        host,
        port,
        name,
        starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
        bind_port,
        key,
    });
}

pub fn handle_collect(request: Request) {
    let url_str = format!("http://localhost{}", request.url());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();

    let name = match query_pairs.get("name") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'name' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let host = match query_pairs.get("host") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'host' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let port = match query_pairs.get("port")
        .and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            let response = Response::from_string("Error: 'port' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let key = match query_pairs.get("key")
    .and_then(|p| p.parse::<u64>().ok()) {
    Some(k) => k,
    None => {
        let response = Response::from_string("Error: 'key' parameter is required.")
            .with_status_code(400);
        let _ = request.respond(response);
        return;
    }
    };

    let result = collect(Collect {
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        host,
        port,
        name,
        starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
        key,
    });

}

pub fn handle_send(request: Request) {
    let url_str = format!("http://localhost{}", request.url());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    
    let name = match query_pairs.get("name") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'name' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let host = match query_pairs.get("host") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'host' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let port = match query_pairs.get("port")
        .and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            let response = Response::from_string("Error: 'port' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let msg = match query_pairs.get("msg") {
        Some(n) => n.to_string(),
        None => {
            let response = Response::from_string("Error: 'msg' parameter is required.")
                .with_status_code(400);
            let _ = request.respond(response);
            return;
        }
    };

    let key = match query_pairs.get("key")
    .and_then(|p| p.parse::<u64>().ok()) {
    Some(k) => k,
    None => {
        let response = Response::from_string("Error: 'key' parameter is required.")
            .with_status_code(400);
        let _ = request.respond(response);
        return;
    }
    };
    let result = send_msg(Send {
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        host,
        port,
        name,
        starting_octets: Some(query_pairs.get("starting_octets").unwrap_or(&"".to_string()).clone()),
        msg,
        key,
    });

}

// fn handle_task_id_update(request: Request) {

// }

// fn handle_task_update(request: Request) {

// }
