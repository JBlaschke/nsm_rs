use crate::operations::{list_interfaces, list_ips, listen, claim, publish, collect, send_msg};

use crate::models::{ListInterfaces, ListIPs, Listen, Claim, Publish, Collect, Send};

use actix_web::{web, App, HttpServer, HttpResponse, Responder, HttpRequest};
use clap::ArgMatches;

pub async fn handle_list_interfaces(query: web::Query<ListInterfaces>) -> impl Responder {
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

pub async fn handle_list_ips(query: web::Query<ListIPs>) -> impl Responder {
    let inputs = query.into_inner();
    
    let result = list_ips(ListIPs {
        verbose: inputs.verbose,
        print_v4: inputs.print_v4,
        print_v6: inputs.print_v6,
        name: inputs.name,
        starting_octets: inputs.starting_octets,
    });

    match result {
        Ok(_) => HttpResponse::Ok().body("Successfully listed addresses"),
        Err(err) => {
            HttpResponse::InternalServerError().body(format!("Error: {}", err))
        }
    }
}

pub async fn handle_listen(data: web::Json<Listen>) -> impl Responder {
    let inputs = data.into_inner();
    
    let result = listen(Listen {
        print_v4: inputs.print_v4,
        print_v6: inputs.print_v6,
        name: inputs.name,
        starting_octets: inputs.starting_octets,
        bind_port: inputs.bind_port,
    });

    match result {
        Ok(_) => HttpResponse::Ok().body("Successfully posted Listener"),
        Err(err) => {
            HttpResponse::InternalServerError().body(format!("Error: {}", err))
        }
    }
}

pub async fn handle_publish(data: web::Json<Publish>) -> impl Responder {
    let inputs = data.into_inner();
    
    let result = publish(Publish {
        print_v4: inputs.print_v4,
        print_v6: inputs.print_v6,
        host: inputs.host,
        port: inputs.port,
        name: inputs.name,
        starting_octets: inputs.starting_octets,
        bind_port: inputs.bind_port,
        service_port: inputs.service_port,
        key: inputs.key
    });

    match result {
        Ok(_) => HttpResponse::Ok().body("Successfully published service"),
        Err(err) => {
            HttpResponse::InternalServerError().body(format!("Error: {}", err))
        }
    }
}

pub async fn handle_claim(data: web::Json<Claim>) -> impl Responder {
    let inputs = data.into_inner();
    
    let result = claim(Claim {
        print_v4: inputs.print_v4,
        print_v6: inputs.print_v6,
        host: inputs.host,
        port: inputs.port,
        name: inputs.name,
        starting_octets: inputs.starting_octets,
        bind_port: inputs.bind_port,
        key: inputs.key
    });

    match result {
        Ok(_) => HttpResponse::Ok().body("Successfully claimed service"),
        Err(err) => {
            HttpResponse::InternalServerError().body(format!("Error: {}", err))
        }
    }
}

pub async fn handle_collect(query: web::Query<Collect>) -> impl Responder {
    let inputs = query.into_inner();
    
    let result = collect(Collect {
        print_v4: inputs.print_v4,
        print_v6: inputs.print_v6,
        host: inputs.host,
        port: inputs.port,
        name: inputs.name,
        starting_octets: inputs.starting_octets,
        key: inputs.key
    });

    match result {
        Ok(_) => HttpResponse::Ok().body("Successfully sent collect request"),
        Err(err) => {
            HttpResponse::InternalServerError().body(format!("Error: {}", err))
        }
    }
}

pub async fn handle_send(data: web::Json<Send>) -> impl Responder {
    let inputs = data.into_inner();
    
    let result = send_msg(Send {
        print_v4: inputs.print_v4,
        print_v6: inputs.print_v6,
        host: inputs.host,
        port: inputs.port,
        name: inputs.name,
        starting_octets: inputs.starting_octets,
        msg: inputs.msg,
        key: inputs.key
    });

    match result {
        Ok(_) => HttpResponse::Ok().body("Successfully sent message"),
        Err(err) => {
            HttpResponse::InternalServerError().body(format!("Error: {}", err))
        }
    }
}

// async fn handle_task_id_update() -> impl Responder {

// }

// async fn handle_task_update() -> impl Responder {

// }
