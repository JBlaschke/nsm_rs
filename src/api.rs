mod network;
mod connection;
mod route;
mod utils;

mod operations;
use operations::{list_interfaces};

mod models;
use models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, Send};

use actix_web::{web, App, HttpServer, HttpResponse, Responder, HttpRequest};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use env_logger::Env;

async fn handle_list_interfaces(query: Option<web::Query<ListInterfaces>>) -> impl Responder {
    let inputs = query.into_inner();
    
    // Provide defaults if fields are None
    let verbose = inputs.verbose.unwrap_or(false);
    let print_v4 = inputs.print_v4.unwrap_or(false);
    let print_v6 = inputs.print_v6.unwrap_or(false);
    
    let result = list_interfaces(ListInterfaces {
        verbose,
        print_v4,
        print_v6,
    });

    let result = list_interfaces(inputs);
    HttpResponse::Ok().body(result);
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new()
            .route("/list_interfaces", web::get().to(handle_list_interfaces))
    })    
    .bind("127.0.0.1:9000")?
    .run()
    .await
}