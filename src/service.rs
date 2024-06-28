use std::net::TcpStream;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;
use std::thread;
use threadpool::ThreadPool;
use std::collections::VecDeque;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::utils::{only_or_error, epoch};
use crate::connection::{
    MessageHeader, Message, send, receive
};


#[derive(Serialize, Deserialize, Debug)]
pub struct Payload {
    pub service_addr: Vec<String>,
    pub service_port: i32,
    pub service_claim: u64,
    pub interface_addr: Vec<String>,
    pub bind_port: i32,
    pub key: u64,
    pub id: u64
}

#[derive(Clone)]
pub struct Tracker {
    pub stream: Arc<Mutex<TcpStream>>,
    pub fail_count: i32  
}

// need to add if alive/dead
#[derive(Debug)]
pub struct State {
    pub clients: HashMap<u64, Vec<Payload>>,
    pub claims: HashMap<u64, Vec<Payload>>, //when it was claimed
    pub timeout: u64,
    pub seq: u64,
    pub life: bool
}


impl State {
    pub fn new() -> State {
        State{
            clients: HashMap::new(),
            claims: HashMap::new(),
            timeout: 60,
            seq: 1,
            life: true
        }
    }

    pub fn add(&mut self, mut p: Payload) {
        let cl: &mut Vec<Payload> = self.clients.entry(p.key).or_insert(Vec::new());
        p.id = self.seq;
        cl.push(p);
        self.seq += 1;
    }

    #[allow(dead_code)]
    pub fn claim(&mut self, k:u64) -> Result<&mut Payload, u64> {
        match self.clients.get_mut(& k) {

            Some(value) => {
                for v in value {
                    let current_ecpoch = epoch();
                    if current_ecpoch - v.service_claim > self.timeout {
                        v.service_claim = current_ecpoch;
                        return Ok(v);
                    }
                }
                return Err(1);
            }

            _ => return Err(2)
        }

        // return Err(3);
    }

    pub fn print(&mut self) {
        for (key, values) in & self.clients {
            for v in values {
                println!("{}: {:?}", key, v);
            }
        }
    }
}


pub fn serialize(payload: & Payload) -> String {
    serde_json::to_string(payload).unwrap()
}


pub fn deserialize(payload: & String) -> Payload {
    serde_json::from_str(payload).unwrap()
}

//only wants to receive PUB and CLAIM
//only used in listen
pub fn request_handler(
    state: & Arc<Mutex<State>>, pool: & ThreadPool, deque: & Arc<Mutex<VecDeque<Tracker>>>,
     stream: & Arc<Mutex<TcpStream>>
) -> std::io::Result<()> {
    trace!("Starting request handler");

    let message = receive(stream)?;

    let payload = match message.header {
        MessageHeader::HB => panic!("Unexpected HB message encountered!"),
        MessageHeader::ACK => panic!("Unexpected ACK message encountered!"),
        MessageHeader::PUB => deserialize(& message.body),
        MessageHeader::CLAIM => deserialize(& message.body),
        MessageHeader::NULL => panic!("Unexpected NULL message encountered!"),
    };

    let ipstr = only_or_error(& payload.service_addr);
    let bind_address = format!("{}:{}", ipstr, payload.bind_port);

    info!("Request handler received: {:?}", payload);
    match message.header {
        MessageHeader::PUB => {
            info!("Publishing Service: {:?}", payload);
            let mut state_loc = state.lock().unwrap();
            state_loc.add(payload);

            println!("Now state:");
            state_loc.print();

            //listener connects to bind port here
            info!("Listener connecting to: {}", bind_address);
            let hb_stream = TcpStream::connect(bind_address).unwrap();
            let shared_hb_stream = Arc::new(Mutex::new(hb_stream));

            //add to queue here
            let tracker = Tracker {
                stream: shared_hb_stream,
                fail_count: 0
            };
            println!("Adding new connection to queue");
            {
                let mut loc_deque = deque.lock().unwrap();
                let _ = loc_deque.push_back(tracker);
            }
        },
        MessageHeader::CLAIM => {
            info!("Claiming Service: {:?}", payload);
            //hold mutex on shared state
            //mutex is released once out of scope
            let mut state_loc = state.lock().unwrap();
            state_loc.add(payload);

            println!("Now state:");
            state_loc.print();

            //listener connects to bind port here
            // info!("Listener connecting to: {}", bind_address);
            // let hb_stream = TcpStream::connect(bind_address).unwrap();
            // let shared_hb_stream = Arc::new(Mutex::new(hb_stream));
            // let ack = send(& shared_hb_stream, & Message{
            //     header: MessageHeader::HB,
            //     body: message.body
            // });
            // info!("Connection successful");

            // //add to queue here
            // let tracker = Tracker {
            //     stream: shared_hb_stream,
            //     fail_count: 0
            // };
            // println!("Adding new connection to queue");
            // {
            //     let mut loc_deque = deque.lock().unwrap();
            //     let _ = loc_deque.push_back(tracker);
            // }
        }
        _ => {panic!("This should not be reached!");}
    }

    match message.header {
        MessageHeader::PUB => {
            info!("Starting HB monitor thread");
            let thread_state = Arc::clone(& state);
            // let thread_stream = Arc::new(stream);
            // let monitor = thread::spawn(move || {
            //         heartbeat_monitor(&thread_state, & thread_stream);
            // });
            let deque_clone = Arc::clone(& deque);
            loop {
        
                //println!("Popping tracker from VecDeque");
                let popped_tracker = {
                    let mut loc_deque = deque_clone.lock().unwrap();
                    loc_deque.pop_front()
                };
        
                let mut popped_tracker = match popped_tracker {
                    Some(tracker) => tracker,
                    None => continue
                };

                let thread_state2 = Arc::clone(& state);
                let deque_clone2 = Arc::clone(& deque);
                pool.execute(move || {
        
                    //println!("Passing TCP connection to handler...");
                    let _ = heartbeat_monitor(&thread_state2, &mut popped_tracker);
                    //println!("Connection handled");
        
                    if popped_tracker.fail_count < 10 {
                        //println!("Adding client back to VecDeque: {:?}", popped_client.fail_count);
                        {
                            let mut loc_deque = deque_clone2.lock().unwrap();
                            let _ = loc_deque.push_back(popped_tracker.clone());
                        }
                    } else {
                        println!("Dropping tracker");
                    }
        
                });
            }
        }
        _ => {}
    }

    Ok(())
}


pub fn heartbeat_monitor(
    state: & Arc<Mutex<State>>, tracker: &mut Tracker
) {
    info!("HB Monitor is checking for a HB.");
    // let mut data = state.lock().unwrap();
    let ack = send(& tracker.stream, & Message{
        header: MessageHeader::HB,
        body: "".to_string()
    });
    match ack {
        Ok(ref m) => {
            trace!("Received response: {:?}", m);
            match m.header {
                MessageHeader::NULL => {
                    info!("Publish acknowledged HB.")
                }
                _ => {
                    warn!("Publish responds with unexpected message: {:?}", m);
                    tracker.fail_count += 1
                }
            }
        }
        Err(ref e) => {
            error!("Encountered error: {:?}", e);
            tracker.fail_count += 1;
        }
    }
    info!("HB returned: {:?}", ack);
    sleep(Duration::from_millis(2000));
}

pub fn heartbeat_handler(stream: & Arc<Mutex<TcpStream>>) -> std::io::Result<()> {
    trace!("Starting heartbeat handler");

    let request = receive(stream)?;
    if ! matches!(request.header, MessageHeader::HB) {
        warn!(
            "Non-heartbeat request sent to heartbeat_handler: {}",
            request.header
        );
        info!("Dropping non-heartbeat request");
    } else {
        info!("Heartbeat handler received {:?}", request);
        send(stream, & Message{header: MessageHeader::HB, body: request.body})?;
        trace!("Heartbeat handler has returned heartbeat request");
    }
    Ok(())
}