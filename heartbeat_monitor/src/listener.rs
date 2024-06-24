mod hb_connection;
use hb_connection::stream_read;

use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::io::Write;
use std::thread;
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
                    let client = Client {
                        conn: shared_stream,
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

        // println!("Popping client from VecDeque");
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
    }

    Ok(())
}

fn handle_connection(cli: &mut Client) -> std::io::Result<()>{
    //println!("Starting heartbeat handler");

    let failure_duration = Duration::from_secs(10); //change to any failure limit

    let loc_stream: &mut TcpStream = &mut *cli.conn.lock().unwrap();
    loc_stream.set_read_timeout(Some(failure_duration))?;

    let received = match stream_read(loc_stream) {
        Ok(message) => message,
        Err(err) => {
            println!("Failed to receive data from stream");
            return Err(err);
        }
    };

    if received == "" {
        cli.fail_count += 1;
        // println!("Failed to receive HB. {:?}", cli.fail_count);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "HB Failed")
        );
    } else {    
        let ack = "Received message";
        cli.fail_count = 0;
        let _ = loc_stream.write(ack.as_bytes());
        println!("{}", received);
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
