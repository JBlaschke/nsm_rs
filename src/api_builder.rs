use crate::operations::{get_interfaces, claim, publish, collect, send_msg};
use crate::models::{Claim, Publish, Collect, SendMSG};
use crate::network::{get_local_ips, get_matching_ipstr};
use crate::connection::ComType;

use std::collections::HashMap;
use hyper::body::{Bytes, Incoming, Buf};
use hyper::http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use url::{form_urlencoded, Url};
use serde_json;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

type HyperResult = Result<Response<Full<Bytes>>, hyper::Error>;

pub async fn handle_list_interfaces(request: Request<Incoming>) -> HyperResult {
    info!("Entering handle_list_interfaces with request: {:?}", request);
    let mut response = Response::new(Full::default());

    let params: HashMap<String, String> = request.uri().query().map(|v| {
        form_urlencoded::parse(v.as_bytes())
            .into_owned()
            .collect()
    }).unwrap_or_else(HashMap::new);

    let get_v4 = params.get("v4").map_or(true, |v| v == "true");
    let get_v6 = params.get("v6").map_or(false, |v| v == "true");
    let (ipv4_names, ipv6_names) = get_interfaces(get_v4, get_v6).await;
    let mut response_data: HashMap<&str, Vec<String>> = HashMap::new();
    if get_v4 { response_data.insert("ipv4_interfaces", ipv4_names); }
    if get_v6 { response_data.insert("ipv6_interfaces", ipv6_names); }

    trace!("Responding with data: {:?}", response_data);

    match serde_json::to_string(&response_data)  {
        Ok(output) => {
            *response.status_mut() = StatusCode::OK;
            *response.body_mut()   = Full::from(output);
        },
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Error processing request: {}", e)
            );
        }
    };

    Ok(response)
}

pub async fn handle_list_ips(request: Request<Incoming>) -> HyperResult {
    info!("Entering handle_list_interfaces with request {:?}", request);
    let mut response = Response::new(Full::default());

    let params: HashMap<String, String> = request.uri().query().map(|v| {
        form_urlencoded::parse(v.as_bytes())
            .into_owned()
            .collect()
    }).unwrap_or_else(HashMap::new);

    let get_v4 = params.get("get_v4").map_or(true, |v| v == "true");
    let get_v6 = params.get("get_v6").map_or(false, |v| v == "true");
    let starting_octets = params.get("starting_octets").map(String::from);
    let name = match params.get("name") {
        Some(n) => Some(n.to_string()),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'name' parameter is required."
            );
            return Ok(response);
        }
    };

    let ips = get_local_ips().await;
    let mut response_data: HashMap<&str, Vec<String>> = HashMap::new();
    if get_v4 {
        response_data.insert("ipv4_addresses",
            get_matching_ipstr(&ips.ipv4_addrs, &name, &starting_octets).await
        );
    }
    if get_v6 {
        response_data.insert("ipv6_addresses",
            get_matching_ipstr(&ips.ipv6_addrs, &name, &starting_octets).await
        );
    }

    trace!("Responding with data: {:?}", response_data);

    match serde_json::to_string(&response_data)  {
        Ok(output) => {
            *response.status_mut() = StatusCode::OK;
            *response.body_mut()   = Full::from(output);
        },
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Error processing request: {}", e)
            );
        }
    };

    Ok(response)
}

pub async fn handle_publish(mut request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {    
    let mut response = Response::new(Full::default());
    let whole_body = request.body_mut().collect().await.unwrap().aggregate();
    let data: serde_json::Value = serde_json::from_reader(whole_body.reader()).unwrap();

    let name = match data.get("name").and_then(|v| v.as_str()) {
        Some(n) => Some(n.trim_matches('"').to_string()),
        None => {
            // *response.body_mut() = Full::from("Error: 'name' parameter is required.");
            // return Ok(response);
            None
        }
    };

    let host = match data.get("host").and_then(|v| v.as_str()) {
        Some(n) => n.trim_matches('"').to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'host' parameter is required.");
            return Ok(response);
        }
    };

    let port = match data.get("port").and_then(|v| v.as_i64()){
        Some(port) => port as i32,
        None => {
            *response.body_mut() = Full::from("Error: 'port' parameter is required.");
            return Ok(response);
        }
    };

    let bind_port = match data.get("bind_port").and_then(|v| v.as_i64()){
        Some(port) => port as i32,
        None => {
            *response.body_mut() = Full::from("Error: 'bind_port' parameter is required.");
            return Ok(response);
        }
    };

    let service_port = match data.get("service_port").and_then(|v| v.as_i64()){
        Some(port) => port as i32,
        None => {
            *response.body_mut() = Full::from("Error: 'service_port' parameter is required.");
            return Ok(response);
        }
    };  

    let key = match data.get("key").and_then(|v| v.as_i64()){
        Some(k) => k as u64,
        None => {
            *response.body_mut() = Full::from("Error: 'key' parameter is required.");
            return Ok(response);
        }
    };

    let starting_octets = match data.get("starting_octets").and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    let root_ca = match data.get("root_ca")
        .and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    tokio::spawn(async move{
        let mut task_response: Response<Full<Bytes>> = Response::new(Full::default());
        let _result = match publish(Publish {
            print_v4: data.get("print_v4").map_or(true, |v| v == "true"),
            print_v6: data.get("print_v6").map_or(true, |v| v == "true"),
            host,
            port,
            name,
            starting_octets,
            bind_port,
            service_port,
            key,
            tls : data.get("tls").map_or(false, |v| v == "true"),
            root_ca,
            ping : data.get("ping").map_or(false, |v| v == "true"),
        }, ComType::API).await {
            Ok(_output) => {
                *task_response.body_mut() = Full::from("Request to publish ended")
            },
            Err(e) => {
                *task_response.body_mut() = Full::from(format!("Error processing request: {}", e))
            }
        };
        let _ = Ok::<Response<Full<Bytes>>, hyper::Error>(task_response);
    });

    *response.body_mut() = Full::from("Successful request to publish");
    Ok(response)
}

pub async fn handle_claim(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let mut response = Response::new(Full::default());

    let url_str = format!("http://localhost{}", request.uri().to_string());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    println!("{:?}", query_pairs);
    let name = match query_pairs.get("name") {
        Some(n) => Some(n.to_string()),
        None => {
            // *response.body_mut() = Full::from("Error: 'name' parameter is required.");
            // return Ok(response);
            None
        }
    };

    let host = match query_pairs.get("host") {
        Some(n) => n.to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'host' parameter is required.");
            return Ok(response);
        }
    };

    let port = match query_pairs.get("port").and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            *response.body_mut() = Full::from("Error: 'port' parameter is required.");
            return Ok(response);
        }
    };

    let bind_port = match query_pairs.get("bind_port").and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            *response.body_mut() = Full::from("Error: 'bind_port' parameter is required.");
            return Ok(response);
        }
    };

    let key = match query_pairs.get("key").and_then(|p| p.parse::<u64>().ok()) {
        Some(k) => k,
        None => {
            *response.body_mut() = Full::from("Error: 'key' parameter is required.");
            return Ok(response);
        }
    };

    let starting_octets = match query_pairs.get("starting_octets") {
        Some(s) => Some(s.to_string()),
        None => None
    };

    let root_ca = match query_pairs.get("root_ca") {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    tokio::spawn(async move{
        let mut task_response: Response<Full<Bytes>> = Response::new(Full::default());
        let _result = match claim(Claim {
            print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
            print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
            host,
            port,
            name,
            starting_octets,
            bind_port,
            key,
            tls : query_pairs.get("tls").map_or(false, |v| v == "true"),
            root_ca,
            ping : query_pairs.get("ping").map_or(false, |v| v == "true"),
        }, ComType::API).await {
            Ok(_output) => {
                *task_response.body_mut() = Full::from("Request to claim completed")
            },
            Err(e) => {
                *task_response.body_mut() = Full::from(format!("Error processing request: {}", e))
            }
        };
        let _ = Ok::<Response<Full<Bytes>>, hyper::Error>(task_response);
    });

    *response.body_mut() = Full::from("Successful request to claim");
    Ok(response)
}

pub async fn handle_collect(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let mut response = Response::new(Full::default());

    let url_str = format!("http://localhost{}", request.uri().to_string());
    let parsed_url = Url::parse(&url_str).unwrap();

    // Extract query parameters into a HashMap
    let query_pairs: HashMap<_, _> = parsed_url.query_pairs().into_owned().collect();
    println!("{:?}", query_pairs);
    let name = match query_pairs.get("name") {
        Some(n) => Some(n.to_string()),
        None => {
            // *response.body_mut() = Full::from("Error: 'name' parameter is required.");
            // return Ok(response);
            None
        }
    };

    let host = match query_pairs.get("host") {
        Some(n) => n.to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'host' parameter is required.");
            return Ok(response);
        }
    };

    let port = match query_pairs.get("port").and_then(|p| p.parse::<i32>().ok()) {
        Some(port) => port,
        None => {
            *response.body_mut() = Full::from("Error: 'port' parameter is required.");
            return Ok(response);
        }
    };

    let key = match query_pairs.get("key").and_then(|p| p.parse::<u64>().ok()) {
        Some(k) => k,
        None => {
            *response.body_mut() = Full::from("Error: 'key' parameter is required.");
            return Ok(response);
        }
    };

    let starting_octets = match query_pairs.get("starting_octets") {
        Some(s) => Some(s.to_string()),
        None => None
    };

    let root_ca = match query_pairs.get("root_ca") {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    let _result = match collect(Collect{
        print_v4: query_pairs.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: query_pairs.get("print_v6").map_or(false, |v| v == "true"),
        host,
        port,
        name,
        starting_octets,
        key,
        tls : query_pairs.get("tls").map_or(false, |v| v == "true"),
        root_ca,
    }, ComType::API).await {
        Ok(_output) => {
            *response.body_mut() = Full::from("Successful request to collect");
        },
        Err(e) => {
            *response.body_mut() = Full::from(format!("Error processing request: {}", e));
        }
    };

    *response.body_mut() = Full::from("Successful request to collect");
    Ok(response)
}

pub async fn handle_send(mut request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let mut response = Response::new(Full::default());
    println!("entered handler");
    let whole_body = request.body_mut().collect().await.unwrap().aggregate();
    let data: serde_json::Value = serde_json::from_reader(whole_body.reader()).unwrap();

    println!("Received body: {}", data);
    let name = match data.get("name").and_then(|v| v.as_str()) {
        Some(n) => Some(n.trim_matches('"').to_string()),
        None => {
            // *response.body_mut() = Full::from("Error: 'name' parameter is required.");
            // return Ok(response);
            None
        }
    };

    let host = match data.get("host").and_then(|v| v.as_str()) {
        Some(n) => n.trim_matches('"').to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'host' parameter is required.");
            return Ok(response);
        }
    };

    let port = match data.get("port").and_then(|v| v.as_i64()){
        Some(port) => port as i32,
        None => {
            *response.body_mut() = Full::from("Error: 'port' parameter is required.");
            return Ok(response);
        }
    };

    let msg = match data.get("msg").and_then(|v| v.as_str()) {
        Some(n) => n.trim_matches('"').to_string(),
        None => {
            *response.body_mut() = Full::from("Error: 'msg' parameter is required.");
            return Ok(response);
        }
    };

    let key = match data.get("key").and_then(|v| v.as_i64()){
        Some(k) => k as u64,
        None => {
            *response.body_mut() = Full::from("Error: 'key' parameter is required.");
            return Ok(response);
        }
    };

    let starting_octets = match data.get("starting_octets").and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    let root_ca = match data.get("root_ca").and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    let _result = match send_msg(SendMSG{
        print_v4: data.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: data.get("print_v6").map_or(true, |v| v == "true"),
        host,
        port,
        name,
        starting_octets,
        msg,
        key,
        tls : data.get("tls").map_or(false, |v| v == "true"),
        root_ca,
    }, ComType::API).await {
        Ok(_output) => {
            *response.body_mut() = Full::from("Successful request to send");
        },
        Err(e) => {
            *response.body_mut() = Full::from(format!("Error processing request: {}", e));
        }
    };

    Ok(response)
}

// fn handle_task_id_update(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {

// }

// fn handle_task_update(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {

// }
