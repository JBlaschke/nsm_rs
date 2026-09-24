//! Legacy REST control plane, formerly the server mode of the `api` binary.
//! Reachable as `nsm serve`. Routes and handlers are unchanged from the
//! pre-cleanup code; the hardening branch replaces this with an axum router
//! with typed requests, a job registry and authentication.

use super::api_builder::{
    handle_claim, handle_collect, handle_list_interfaces, handle_list_ips, handle_publish,
    handle_send,
};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use std::net::SocketAddr;
use hyper::http::{Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;

/// Serve the control plane on `addr` until the process ends.
pub async fn serve(addr: SocketAddr) -> std::io::Result<()> {
    let incoming = TcpListener::bind(&addr).await?;
    info!("Control plane listening on {}", incoming.local_addr()?);
    loop {
        let (stream, _) = incoming.accept().await?;
        tokio::task::spawn(async move {
            let service = service_fn(handle_requests);
            if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                error!("Failed to serve connection: {:?}", err);
            }
        });
    }
}

async fn handle_requests(
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let mut response = Response::new(Full::default());

    match (method, path.as_str()) {
        (Method::GET, p) if p.starts_with("/list_interfaces") => handle_list_interfaces(request).await,
        (Method::GET, p) if p.starts_with("/list_ips") => handle_list_ips(request).await,
        (Method::POST, p) if p.starts_with("/publish") => handle_publish(request).await,
        (Method::GET, p) if p.starts_with("/claim") => handle_claim(request).await,
        (Method::GET, p) if p.starts_with("/collect") => handle_collect(request).await,
        (Method::POST, p) if p.starts_with("/send") => handle_send(request).await,
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
            Ok(response)
        }
    }
}
