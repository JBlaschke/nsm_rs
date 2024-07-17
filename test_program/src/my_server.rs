use std::net::TcpStream;
use std::env;
use std::thread;
use std::io::{Write, Read};
use std::time::Duration;

fn my_server(addr: &str, port: &str) -> std::io::Result<()> { 
    let mut stream = TcpStream::connect(format!("{}:{}", addr, port))?;
    println!("Connection started on {}", format!("{}:{}", addr, port));

    let mut buf = [0; 1024];
    let mut message = String::new();

    loop {
        loop {
            let bytes_read = match stream.read(&mut buf) {
                Ok(n) => n,
                Err(err) => { return Err(err); }
            };
            let s = std::str::from_utf8(&buf[..bytes_read]).unwrap();
            message.push_str(s);
    
            if bytes_read < buf.len() {
                break;
            }
        }
        println!("{}", message);

        let _ = stream.write(message.as_bytes());
        message = "".to_string();

        thread::sleep(Duration::from_secs(2));
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: {} <name>", args[0]);
        std::process::exit(1);
    }

    if let Err(e) = my_server(&args[1], &args[2]) {
        eprintln!("Error: {}", e);
    }
}
