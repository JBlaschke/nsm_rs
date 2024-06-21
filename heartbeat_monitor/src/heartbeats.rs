mod hb_connection;
use hb_connection::stream_read;

use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::io::Write;
use std::env;
use std::{thread, time};

fn heartbeats(host: &str, msg: &str) -> std::io::Result<()> { 

    let stream = TcpStream::connect(host)?;
    println!("Connection started");

    let stream_mut = Arc::new(Mutex::new(stream));
    let stream_clone = Arc::clone(&stream_mut);


    loop{

        let loc_stream: &mut TcpStream = &mut stream_clone.lock().unwrap();

        println!("Writing to stream");
        let _ = loc_stream.write(msg.as_bytes());
        println!("Message sent");

        let received = match stream_read(loc_stream) {
            Ok(message) => message,
            Err(err) => {return Err(err);}
        };
        println!("{}", received);

        thread::sleep(time::Duration::from_secs(5)); //change to any rate

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