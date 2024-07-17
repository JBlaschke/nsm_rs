use std::net::TcpListener;
use std::env;
use std::thread;
use std::io::{Write, Read};
use std::time::Duration;

fn my_client(addr: &str, port: &str) -> std::io::Result<()> { 
    let listener = TcpListener::bind(format!("{}:{}", addr, port))?;
    println!("Binded to: {}", format!("{}:{}", addr, port));

    let mut buf = [0; 1024];
    let mut message = "some data".to_string();

    for stream in listener.incoming(){
        match stream {
            Ok(mut stream) => {
                loop {

                    let _ = stream.write(message.as_bytes());
                    message = "".to_string();

                    loop {
                        let bytes_read = match stream.read(&mut buf) {
                            Ok(n) => n,
                            Err(err) => { return Err(err); }
                        };
                        let s = std::str::from_utf8(&buf[..bytes_read]).unwrap();
                        message.push_str(s);
                        println!("{}", message);
                
                        if bytes_read < buf.len() {
                            break;
                        }
                    }

                    thread::sleep(Duration::from_secs(2));
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

    if let Err(e) = my_client(&args[1], &args[2]) {
        eprintln!("Error: {}", e);
    }
}
