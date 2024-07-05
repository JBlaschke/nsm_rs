mod hb_connection;
use hb_connection::stream_read;

use std::net::{TcpStream, TcpListener};
use std::sync::{Arc, Mutex};
use std::io::Write;
use std::env;
use rand::Rng;

fn heartbeats(host: &str, bind_port: &str) -> std::io::Result<()> { 

    let request = TcpStream::connect(host)?;
    println!("Connection started on {}", host);

    let listener = TcpListener::bind(bind_port)?;
    println!("Binded to: {}", bind_port);

    let shared_request = Arc::new(Mutex::new(request));
    let loc_request: &mut TcpStream = &mut *shared_request.lock().unwrap();
    println!("Requesting listener to bind");
    let _ = loc_request.write(bind_port.as_bytes());

    let _ = match stream_read(loc_request) {
        Ok(message) => message,
        Err(err) => {return Err(err);}
    };
    println!("Connection initiated");

    let mut rng = rand::thread_rng();

    for stream in listener.incoming(){
        match stream {
            Ok(stream) => {
                let shared_stream = Arc::new(Mutex::new(stream));
                let stream_clone = Arc::clone(&shared_stream);

                loop{

                    let loc_stream: &mut TcpStream = &mut *stream_clone.lock().unwrap();

                    let received = match stream_read(loc_stream) {
                        Ok(message) => message,
                        Err(err) => {return Err(err);}
                    };

                    println!("Heartbeat received: {}", received);

                    //let n: u32 = rng.gen_range(0..2);
                    //if n == 1{
                        println!("Acknowledging heartbeat");
                        let _ = loc_stream.write(received.as_bytes());
                    //}

                }
            }
            Err(e) => {
                eprintln!("Connection failed: {}", e);
            }
        }
    }
    Ok(())
}


fn main() {

    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: {} <name>", args[0]);
        std::process::exit(1);
    }

    if let Err(e) = heartbeats(&args[1], &args[2]) {
        eprintln!("Lub error: {}", e);
    }
}