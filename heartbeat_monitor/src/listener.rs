mod hb_connection;
use hb_connection::stream_read;

use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::io::{self, Write};
use std::{thread, time};
use std::time::Duration;
use threadpool::ThreadPool;
use std::collections::VecDeque;

use std::env;

#[derive(Clone)]
pub struct Client{
    pub conn: Arc<Mutex<TcpStream>>,
    pub fail_count: i32
}

fn listener(host: &str) -> std::io::Result<()>{

    let listener = TcpListener::bind(host).unwrap();
    println!("Bind successful");
    
    let pool = ThreadPool::new(10); //change to any thread limit
    let deque = Arc::new(Mutex::new(VecDeque::new()));
    let deque_clone = Arc::clone(& deque);

    let _ = thread::spawn(move|| {

        println!("Deque thread entered");

        for stream in listener.incoming() {

            println!("Request received on {:?}, processing...", stream);
            match stream {
                Ok(stream) => {

                    let shared_stream = Arc::new(Mutex::new(stream));
                    let loc_stream: &mut TcpStream = &mut *shared_stream.lock().unwrap();
                    let bind_port = match stream_read(loc_stream) {
                        Ok(message) => message,
                        Err(err) => err.to_string()
                    };
                    println!("Attempting to connect to: {}", bind_port);
                    let hb_stream = TcpStream::connect(bind_port).unwrap();
                    println!("Connection successful");

                    let _ = loc_stream.write(b"Connected to bind_port");

                    let shared_hb_stream = Arc::new(Mutex::new(hb_stream));
                    let client = Client {
                        conn: shared_hb_stream,
                        fail_count: 0
                    };

                    println!("Adding new connection to queue");
                    {
                        let mut loc_deque = deque_clone.lock().unwrap();
                        let _ = loc_deque.push_back(client);
                    }

                }
                Err(e) => {
                    eprintln!("Connection failed: {}", e);
                }
            }
            println!("Moving to next incoming stream");
        }
    });
    
    let deque_clone = Arc::clone(& deque);
    loop {

        //println!("Popping client from VecDeque");
        let popped_client = {
            let mut loc_deque = deque_clone.lock().unwrap();
            loc_deque.pop_front()
        };

        let mut popped_client = match popped_client {
            Some(client) => client,
            None => continue
        };

        let deque_clone2 = Arc::clone(& deque);
        pool.execute(move || {

            //println!("Passing TCP connection to handler...");
            let _ = handle_connection(&mut popped_client);
            //println!("Connection handled");

            if popped_client.fail_count < 10 {
                //println!("Adding client back to VecDeque: {:?}", popped_client.fail_count);
                {
                    let mut loc_deque = deque_clone2.lock().unwrap();
                    let _ = loc_deque.push_back(popped_client.clone());
                }
            } else {
                println!("Dropping client")
            }

        });
        thread::sleep(time::Duration::from_secs(1)); //change to any rate
    }

    Ok(())
}

fn handle_connection(cli: &mut Client) -> std::io::Result<()>{
    //println!("Starting heartbeat handler");

    let mut loc_stream = match cli.conn.lock() {
        Ok(guard) => guard,
        Err(err) => {
            cli.fail_count += 1;
            return Err(io::Error::new(io::ErrorKind::Other, "Mutex lock poisoned"));
        }
    };

    // let loc_stream: &mut TcpStream = &mut *cli.conn.lock().unwrap();

    let hb_msg = "lub";
    println!("Writing to stream: {}", hb_msg);
    let _ = loc_stream.write(hb_msg.as_bytes());
    println!("Message sent");

    let failure_duration = Duration::from_secs(3); //change to any failure limit
    match loc_stream.set_read_timeout(Some(failure_duration)) {
        Ok(x) => println!("set_read_timeout OK"),
        Err(e) => println!("set_read_timeout Error")
    }

    let received = match stream_read(&mut loc_stream) {
        Ok(message) => message,
        Err(err) => {
            println!("Failed to receive data from stream");
            cli.fail_count += 1;
            println!("Failed to receive HB. {:?}", cli.fail_count);
            return Err(err);
        }
    };
    //println!("Message received");

    if received == "" {
        //println!("Increasing failcount");
        cli.fail_count += 1;
        println!("Failed to receive HB. {:?}", cli.fail_count);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "HB Failed")
        );
    } else {    
        cli.fail_count = 0;
        println!("Resetting failcount. {}", cli.fail_count);
    }

    Ok(())
}


fn main() {

    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <name>", args[0]);
        std::process::exit(1);
    }

    if let Err(e) = listener(&args[1]) {
        eprintln!("Listener error: {}", e);
    }
}
