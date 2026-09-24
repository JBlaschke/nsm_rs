use crate::operations::{get_interfaces, claim, publish, collect, send_msg};
use crate::models::{Claim, Publish, Collect, SendMSG};
use crate::network::{get_local_ips, get_matching_ipstr};
use crate::connection::{Addr, ComType, Transport};

use std::str::FromStr;
use std::collections::HashMap;
use hyper::body::{Bytes, Incoming, Buf};
use hyper::http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use url::form_urlencoded;
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

// TODO: can we do this without mutable borrow?
pub async fn handle_publish(mut request: Request<Incoming>) -> HyperResult {    
    info!("Entering handle_publish with request {:?}", request);
    let mut response = Response::new(Full::default());

    let whole_body = match request.body_mut().collect().await {
        Ok(data) => data.aggregate(),
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed while collecting payload: {}", e)
            );
            return Ok(response)
        }
    };
    let data: serde_json::Value = match serde_json::from_reader(
        whole_body.reader()
    ) {
        Ok(data) => data,
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse body as json: {}", e)
            );
            return Ok(response)
        }
    };

    trace!("Received body: {}", data);

    let name = match data.get("name").and_then(|v| v.as_str()) {
        Some(data) => Some(data.trim_matches('"').to_string()),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'name' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_uri = match data.get("host").and_then(|v| v.as_str()) {
        Some(data) => data.trim_matches('"').to_string(),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'host' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_addr = match Addr::from_str(&host_uri) {
        Ok(data) => data,
        Err(e) =>{
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse 'host' as URI: {:?}", e)
            );
            return Ok(response)
        }
    };

    let bind_port = match data.get("bind_port").and_then(|v| v.as_i64()){
        Some(port) => port as i32,
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'bind_port' parameter is required."
            );
            return Ok(response);
        }
    };

    let service_port = match data.get("service_port").and_then(|v| v.as_i64()){
        Some(port) => port as i32,
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'service_port' parameter is required."
            );
            return Ok(response);
        }
    };  

    let key = match data.get("key").and_then(|v| v.as_i64()){
        Some(k) => k as u64,
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'key' parameter is required."
            );
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

    // TODO: tidy up
    let use_tls = host_addr.transport == Transport::HTTPS;

    tokio::spawn(async move{
        let mut task_response: Response<Full<Bytes>> = Response::new(Full::default());
        let _result = match publish(Publish {
            print_v4: data.get("print_v4").map_or(true, |v| v == "true"),
            print_v6: data.get("print_v6").map_or(true, |v| v == "true"),
            host: host_addr,
            name,
            starting_octets,
            bind_port,
            service_port,
            key,
            tls: use_tls,
            root_ca,
            ping : data.get("ping").map_or(false, |v| v == "true"),
        }, ComType::API).await {
            Ok(_output) => {
                *task_response.body_mut() = Full::from(
                    "Request to publish ended"
                )
            },
            Err(e) => {
                *task_response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                *task_response.body_mut() = Full::from(
                    format!("Error processing request: {}", e)
                )
            }
        };
        let _ = Ok::<Response<Full<Bytes>>, hyper::Error>(task_response);
    });
    // TODO: Check if task_response is returned
    *response.body_mut() = Full::from("Successful request to publish");
    Ok(response)
}

// TODO: can we do this without mutable borrow?
pub async fn handle_claim(mut request: Request<Incoming>) -> HyperResult {
    info!("Entering handle_claim with request {:?}", request);
    let mut response = Response::new(Full::default());

    let whole_body = match request.body_mut().collect().await {
        Ok(data) => data.aggregate(),
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed while collecting payload: {}", e)
            );
            return Ok(response)
        }
    };
    let data: serde_json::Value = match serde_json::from_reader(
        whole_body.reader()
    ) {
        Ok(data) => data,
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse body as json: {}", e)
            );
            return Ok(response)
        }
    };

    trace!("Received body: {}", data);

    let name = match data.get("name").and_then(|v| v.as_str()) {
        Some(data) => Some(data.trim_matches('"').to_string()),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'name' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_uri = match data.get("host").and_then(|v| v.as_str()) {
        Some(data) => data.trim_matches('"').to_string(),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'host' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_addr = match Addr::from_str(&host_uri) {
        Ok(data) => data,
        Err(e) =>{
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse 'host' as URI: {:?}", e)
            );
            return Ok(response)
        }
    };

    let bind_port = match data.get("bind_port").and_then(|v| v.as_i64()){
        Some(port) => port as i32,
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'bind_port' parameter is required."
            );
            return Ok(response);
        }
    };

    let key = match data.get("key").and_then(|v| v.as_i64()){
        Some(k) => k as u64,
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'key' parameter is required."
            );
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

    // TODO: tidy up
    let use_tls = host_addr.transport == Transport::HTTPS;

    tokio::spawn(async move{
        let mut task_response: Response<Full<Bytes>> = Response::new(Full::default());
        let _result = match claim(Claim {
            print_v4: data.get("print_v4").map_or(true, |v| v == "true"),
            print_v6: data.get("print_v6").map_or(false, |v| v == "true"),
            host: host_addr,
            name,
            starting_octets,
            bind_port,
            key,
            tls: use_tls,
            root_ca,
            ping: data.get("ping").map_or(false, |v| v == "true"),
        }, ComType::API).await {
            Ok(_output) => {
                *task_response.body_mut() = Full::from(
                    "Request to claim completed"
                )
            },
            Err(e) => {
                *task_response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                *task_response.body_mut() = Full::from(
                    format!("Error processing request: {}", e)
                )
            }
        };
        let _ = Ok::<Response<Full<Bytes>>, hyper::Error>(task_response);
    });
    // TODO: Check if task_response is returned
    *response.body_mut() = Full::from("Successful request to claim");
    Ok(response)
}

// TODO: can we do this without mutable borrow?
pub async fn handle_collect(mut request: Request<Incoming>) -> HyperResult {
    info!("Entering handle_collect with request {:?}", request);
    let mut response = Response::new(Full::default());

    let whole_body = match request.body_mut().collect().await {
        Ok(data) => data.aggregate(),
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed while collecting payload: {}", e)
            );
            return Ok(response)
        }
    };
    let data: serde_json::Value = match serde_json::from_reader(
        whole_body.reader()
    ) {
        Ok(data) => data,
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse body as json: {}", e)
            );
            return Ok(response)
        }
    };

    // TODO: Collect should not need name
    let name = match data.get("name").and_then(|v| v.as_str()) {
        Some(data) => Some(data.trim_matches('"').to_string()),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'name' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_uri = match data.get("host").and_then(|v| v.as_str()) {
        Some(data) => data.trim_matches('"').to_string(),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'host' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_addr = match Addr::from_str(&host_uri) {
        Ok(data) => data,
        Err(e) =>{
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse 'host' as URI: {:?}", e)
            );
            return Ok(response)
        }
    };

    // // TODO: Collect should not need 'bind_port'
    // let bind_port = match data.get("bind_port").and_then(|v| v.as_i64()){
    //     Some(port) => port as i32,
    //     None => {
    //         *response.status_mut() = StatusCode::BAD_REQUEST;
    //         *response.body_mut() = Full::from(
    //             "Error: 'bind_port' parameter is required."
    //         );
    //         return Ok(response);
    //     }
    // };

    let key = match data.get("key").and_then(|v| v.as_i64()){
        Some(k) => k as u64,
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'key' parameter is required."
            );
            return Ok(response);
        }
    };

    // TODO: Collect should not need 'starting_octets'
    let starting_octets = match data.get("starting_octets").and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    let root_ca = match data.get("root_ca").and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    // TODO: tidy up
    let use_tls = host_addr.transport == Transport::HTTPS;

    let result = match collect(Collect{
        print_v4: data.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: data.get("print_v6").map_or(false, |v| v == "true"),
        host: host_addr,
        name,
        starting_octets,
        key,
        tls: use_tls,
        root_ca,
    }, ComType::API).await {
        Ok(_output) => {
            *response.body_mut() = Full::from("Successful request to collect");
        },
        Err(e) => {
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            *response.body_mut() = Full::from(
                format!("Error processing request: {}", e)
            );
        }
    };
    // TODO: Check if task_response is returned
    *response.body_mut() = Full::from(format!(
        "Successful request to collect: {:?}", result
    ));
    Ok(response)
}

// TODO: can we do this without mutable borrow?
pub async fn handle_send(mut request: Request<Incoming>) -> HyperResult {
    info!("Entering handle_send with request {:?}", request);
    let mut response = Response::new(Full::default());

    let whole_body = match request.body_mut().collect().await {
        Ok(data) => data.aggregate(),
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed while collecting payload: {}", e)
            );
            return Ok(response)
        }
    };
    let data: serde_json::Value = match serde_json::from_reader(
        whole_body.reader()
    ) {
        Ok(data) => data,
        Err(e) => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse body as json: {}", e)
            );
            return Ok(response)
        }
    };

    trace!("Received body: {}", data);

    // TODO: Send should not need name
    let name = match data.get("name").and_then(|v| v.as_str()) {
        Some(data) => Some(data.trim_matches('"').to_string()),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'name' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_uri = match data.get("host").and_then(|v| v.as_str()) {
        Some(data) => data.trim_matches('"').to_string(),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'host' parameter is required."
            );
            return Ok(response);
        }
    };

    let host_addr = match Addr::from_str(&host_uri) {
        Ok(data) => data,
        Err(e) =>{
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                format!("Failed to parse 'host' as URI: {:?}", e)
            );
            return Ok(response)
        }
    };

    let msg = match data.get("msg").and_then(|v| v.as_str()) {
        Some(data) => data.trim_matches('"').to_string(),
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'msg' parameter is required."
            );
            return Ok(response);
        }
    };

    let key = match data.get("key").and_then(|v| v.as_i64()){
        Some(k) => k as u64,
        None => {
            *response.status_mut() = StatusCode::BAD_REQUEST;
            *response.body_mut() = Full::from(
                "Error: 'key' parameter is required."
            );
            return Ok(response);
        }
    };

    // TODO: Send should not need 'starting_octets'
    let starting_octets = match data.get("starting_octets").and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    let root_ca = match data.get("root_ca").and_then(|v| v.as_str()) {
        Some(s) => Some(s.trim_matches('"').to_string()),
        None => None
    };

    // TODO: tidy up
    let use_tls = host_addr.transport == Transport::HTTPS;
    let _result = match send_msg(SendMSG{
        print_v4: data.get("print_v4").map_or(true, |v| v == "true"),
        print_v6: data.get("print_v6").map_or(false, |v| v == "true"),
        host: host_addr,
        name,
        starting_octets,
        msg,
        key,
        tls: use_tls,
        root_ca,
    }, ComType::API).await {
        Ok(_output) => {
            *response.body_mut() = Full::from("Successful request to send");
        },
        Err(e) => {
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            *response.body_mut() = Full::from(format!(
                "Error processing request: {}", e
            ));
        }
    };

    // TODO: Check if task_response is returned
    Ok(response)
}

// fn handle_task_id_update(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {

// }

// fn handle_task_update(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {

// }
