use std::net::TcpStream;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, Mutex, Condvar};
use std::thread::sleep;
use std::time::Duration;
// use std::thread;
use threadpool::ThreadPool;
use std::collections::VecDeque;
use std::io::{self};
use std::any::Any;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::utils::{only_or_error, epoch};
use crate::connection::{
    MessageHeader, Message, receive, stream_read, stream_write, serialize_message, deserialize_message
};


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Payload {
    pub service_addr: Vec<String>,
    pub service_port: i32,
    pub service_claim: u64,
    pub interface_addr: Vec<String>,
    pub bind_port: i32,
    pub key: u64,
    pub id: u64
}

// #[derive(Clone)]
// pub struct Tracker {
//     pub stream: Arc<Mutex<TcpStream>>,
//     pub fail_count: i32  
// }

pub trait Event: Any + Send + Sync{
    fn monitor(&mut self) -> std::io::Result<()>;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Clone)]
pub struct Heartbeat {
    pub stream: Arc<Mutex<TcpStream>>,
    pub fail_count: i32
}

impl Event for Heartbeat {

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn monitor(&mut self) -> std::io::Result<()>{
        //println!("Starting heartbeat handler");
        let mut loc_stream = match self.stream.lock() {
            Ok(guard) => guard,
            Err(_err) => {
                self.fail_count += 1;
                return Err(io::Error::new(io::ErrorKind::Other, "Mutex lock poisoned"));
            }
        };
    
        let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
            header: MessageHeader::HB,
            body: "".to_string()
        }));
    
        let failure_duration = Duration::from_secs(6); //change to any failure limit
        match loc_stream.set_read_timeout(Some(failure_duration)) {
            Ok(_x) => println!("set_read_timeout OK"),
            Err(_e) => println!("set_read_timeout Error")
        }
    
        let received = match stream_read(&mut loc_stream) {
            Ok(message) => message,
            Err(err) => {
                self.fail_count += 1;
                println!("Failed to receive data from stream. {:?}", self.fail_count);
                return Err(err);
            }
        };
    
        if received == "" {
            //println!("Increasing failcount");
            self.fail_count += 1;
            trace!("Failed to receive HB. {:?}", self.fail_count);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput, "HB Failed")
            );
        } else {    
            self.fail_count = 0;
            trace!("Resetting failcount. {}", self.fail_count);
        }
    
        sleep(Duration::from_millis(2000));

        Ok(())
    }
}

pub fn event_monitor(state: Arc<(Mutex<State>, Condvar)>) -> std::io::Result<()> {
    trace!("Starting event monitor");

    let (lock, cvar) = &*state;
    let mut state_loc = lock.lock().unwrap();

    loop {
        //state_loc.print();
        // Wait for the condition variable to be notified
        while !state_loc.running {
            println!("waiting to run");
            state_loc = cvar.wait(state_loc).unwrap();
        }
        //println!("entering loop");
        // Process events from the deque
        let event = {
            let mut loc_deque = state_loc.deque.lock().unwrap();
            loc_deque.pop_front()
        };

        let mut event = match event {
            Some(tracker) => tracker,
            None => continue,
        };

        let state_clone = Arc::clone(&state);
        let deque_clone = Arc::clone(&state_loc.deque);

        // state_loc.running = false;
        // cvar.notify_one();

        state_loc.pool.execute(move || {
            trace!("Passing event to event monitor...");
            let _ = event.monitor();

            if let Some(hb) = event.as_any().downcast_ref::<Heartbeat>() {
                if hb.fail_count < 10 {
                    // let (lock, cvar) = &*state_clone;
                    // let mut state_loc = lock.lock().unwrap();
                    // state_loc.running = true;
                    trace!("Adding client back to VecDeque: {:?}", hb.fail_count);
                    let mut loc_deque = deque_clone.lock().unwrap();
                    loc_deque.push_back(event);
                } else {
                    info!("Dropping event");
                }
            }
        });
        println!("ending iteration");
        state_loc.running = false;
        cvar.notify_one();
        sleep(Duration::from_millis(1000));
        state_loc.running = true;
        cvar.notify_one();
    }
    Ok(())
}

#[derive(Clone)]
pub struct State {
    pub clients: HashMap<u64, Vec<Payload>>,
    pub claims: HashMap<u64, Vec<Payload>>,
    pub pool: ThreadPool,
    pub timeout: u64, 
    pub seq: u64,
    pub deque: Arc<Mutex<VecDeque<Box<dyn Event>>>>,
    pub running: bool,
    // pub life: Arc<Mutex<bool>>,
}


impl State {
    pub fn new() -> State {
        State{
            clients: HashMap::new(),
            claims: HashMap::new(),
            pool: ThreadPool::new(30),
            timeout: 60,
            seq: 1,
            deque: Arc::new(Mutex::new(VecDeque::new())),
            running: false,
            // life: Arc::new(Mutex::new(true)),
        }
    }

    // pub fn stop_event(&self) {
    //     *self.life.lock().unwrap() = false;
    // }

    // pub fn start_event(&self) {
    //     *self.life.lock().unwrap() = true;
    // }

    pub fn add(&mut self, mut p: Payload) {

        let ipstr = only_or_error(& p.service_addr);
        let bind_address = format!("{}:{}", ipstr, p.bind_port);

        trace!("Listener connecting to: {}", bind_address);
        sleep(Duration::from_millis(1000));
        let hb_stream = TcpStream::connect(bind_address).unwrap();
        let shared_hb_stream = Arc::new(Mutex::new(hb_stream));

        //add to queue here
        let heartbeat = Heartbeat {
            stream: shared_hb_stream,
            fail_count: 0
        };
        trace!("Adding new connection to queue");
        {
            let mut loc_deque = self.deque.lock().unwrap();
            let _ = loc_deque.push_back(Box::new(heartbeat));
        }

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
    state: &Arc<(Mutex<State>, Condvar)>, stream: & Arc<Mutex<TcpStream>>
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

    info!("Request handler received: {:?}", payload);
    match message.header {
        MessageHeader::PUB => {
            info!("Publishing Service: {:?}", payload);
            let (lock, cvar) = &**state;
            let mut state_loc = lock.lock().unwrap();
            //decide later how to make add apply to different messages/objects

            state_loc.add(payload);
            state_loc.running = true;
            cvar.notify_one();

            println!("Now state:");
            state_loc.print();
        },
        MessageHeader::CLAIM => {
            info!("Claiming Service: {:?}", payload);
            //hold mutex on shared state
            //mutex is released once out of scope
            let (lock, cvar) = &**state;
            let mut state_loc = lock.lock().unwrap();
            state_loc.add(payload);

            println!("Now state:");
            state_loc.print();
        }
        _ => {panic!("This should not be reached!");}
    }

    Ok(())
}

pub fn heartbeat_handler(stream: & Arc<Mutex<TcpStream>>) -> std::io::Result<()> {
    trace!("Starting heartbeat handler");

    loop{
        let mut loc_stream: &mut TcpStream = &mut *stream.lock().unwrap();

        let request = match stream_read(loc_stream) {
            Ok(message) => deserialize_message(& message),
            Err(err) => {return Err(err);}
        };

        trace!("{:?}", request);
        if ! matches!(request.header, MessageHeader::HB) {
            warn!(
                "Non-heartbeat request sent to heartbeat_handler: {}",
                request.header
            );
            info!("Dropping non-heartbeat request");
        } else {
            trace!("Heartbeat handler received {:?}", request);
            let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                header: MessageHeader::HB,
                body: request.body
            }));
            trace!("Heartbeat handler has returned heartbeat request");
        }
        sleep(Duration::from_millis(2000));
    }
}