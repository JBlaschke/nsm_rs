use std::net::TcpStream;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, Mutex, Condvar};
use std::thread::sleep;
use std::time::Duration;
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
    pub id: u64,
}

pub trait Event: Any + Send + Sync{
    fn monitor(&mut self) -> std::io::Result<()>;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Clone)]
pub struct Heartbeat {
    pub key: u64,
    pub id: u64,
    pub stream: Arc<Mutex<TcpStream>>,
    pub fail_count: i32
}

impl Event for Heartbeat {

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn monitor(&mut self) -> std::io::Result<()>{
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
            Ok(_x) => trace!("set_read_timeout OK"),
            Err(_e) => trace!("set_read_timeout Error")
        }
    
        let received = match stream_read(&mut loc_stream) {
            Ok(message) => message,
            Err(err) => {
                self.fail_count += 1;
                trace!("Failed to receive data from stream. {:?}", self.fail_count);
                return Err(err);
            }
        };
    
        if received == "" {
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

pub fn event_monitor(state: Arc<(Mutex<State>, Condvar)>) -> std::io::Result<()>{

    trace!("Starting event monitor");
    let (lock, cvar) = &*state;

    let deque_clone = {
        let state_loc = lock.lock().unwrap();
        Arc::clone(& state_loc.deque)
    };

    let data = Arc::new(Mutex::new((0, 0, 0))); // (fail_count, key, id)
    let mut remove_key: i64 = -1;
    // let mut remove_id: i64 = -1;

    loop {
        {
            let mut state_loc = lock.lock().unwrap();
            while !state_loc.running {
                println!("waiting to run");
                state_loc = cvar.wait(state_loc).unwrap();
            }
        }

        let mut shared_data = data.lock().unwrap();
        if shared_data.0 == 10{
            let mut state_loc = lock.lock().unwrap();
            match state_loc.rmv(shared_data.1, shared_data.2){
                Ok(m) => {
                    match m.header {
                        MessageHeader::PUB => {
                            trace!("Removing all clients with same key from deque.");
                            remove_key = shared_data.1 as i64;
                            // remove_id = shared_data.2 as i64;
                        },
                        MessageHeader::CLAIM => trace!("Removed client from Vec"),
                        _ => warn!("Unexpected message returned from remove()"),
                    }
                },
                Err(_e) => {
                    return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,"Unable to remove item from Vec"));
                }
            }
            shared_data.0 = 0;
        }

        let event = {
            let mut loc_deque = deque_clone.lock().unwrap();
            loc_deque.pop_front()
        };

        let mut event = match event {
            Some(tracker) => tracker,
            None => {
                let mut state_loc = lock.lock().unwrap();
                state_loc.running = true;
                cvar.notify_one();
                continue;
            }
        };

        let deque_clone2 = Arc::clone(& deque_clone);
        let data_clone = Arc::clone(&data);
        let state_loc = lock.lock().unwrap();

        state_loc.pool.execute(move || {

            trace!("Passing event to event monitor...");
            let _ = event.monitor();

            if let Some(hb) = event.as_any().downcast_ref::<Heartbeat>() {
                if (hb.key as i64) == remove_key {
                    info!("Dropping all clients with key: {:?}", remove_key);
                }
                else{
                    {
                        let mut data = data_clone.lock().unwrap();
                        data.0 = hb.fail_count;
                        data.1 = hb.key;
                        data.2 = hb.id;
                    }
                    if hb.fail_count < 10 {
                        trace!("Adding client back to VecDeque: {:?}", hb.fail_count);
                        {
                            let mut loc_deque = deque_clone2.lock().unwrap();
                            let _ = loc_deque.push_back(event);
                        }
                    } else {
                        info!("Dropping event");
                    }
                }
            }
        });
        // state_loc.pool.join();
    }
}

#[derive(Clone)]
pub struct State {
    pub clients: HashMap<u64, Vec<Payload>>,
    pub claims: HashMap<u64, Vec<Payload>>,
    pub pool: ThreadPool,
    pub timeout: u64, 
    pub seq: u64,
    pub deque: Arc<Mutex<VecDeque<Box<dyn Event>>>>,
    running: bool,
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
        }
    }

    pub fn add(&mut self, mut p: Payload) {

        let ipstr = only_or_error(& p.service_addr);
        let bind_address = format!("{}:{}", ipstr, p.bind_port);

        trace!("Listener connecting to: {}", bind_address);
        sleep(Duration::from_millis(1000));
        let hb_stream = TcpStream::connect(bind_address).unwrap();
        let shared_hb_stream = Arc::new(Mutex::new(hb_stream));

        let heartbeat = Heartbeat {
            key: p.key,
            id: self.seq,
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

    pub fn rmv(&mut self, k: u64, id: u64) -> std::io::Result<Message>{
        trace!("{:?}", self.clients);
        if let Some(vec) = self.clients.get_mut(&k) {
            if let Some(pos) = vec.iter().position(|item| item.id == id) {
                if pos == 0{
                    if let Some(value) = self.clients.remove(&k){
                        println!("Removed publish: {:?}", value);
                    }
                    trace!("\n{:?}", self.clients);
                    return Ok(Message {
                        header: MessageHeader::PUB,
                        body: "".to_string()
                    });
                }
                else{
                    let value = vec.remove(pos);
                    println!("Removed client: {:?}", value);
                    trace!("\n{:?}", self.clients);
                    return Ok(Message {
                        header: MessageHeader::CLAIM,
                        body: "".to_string()
                    });
                }
            }
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "Failed to remove item from state"));
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
            //currently adds heartbeat event
            state_loc.add(payload);
            state_loc.running = true;
            cvar.notify_one();

            println!("Now state:");
            state_loc.print();
        },
        MessageHeader::CLAIM => {
            info!("Claiming Service: {:?}", payload);

            let (lock, cvar) = &**state;
            let mut state_loc = lock.lock().unwrap();
            let mut loc_stream: &mut TcpStream = &mut *stream.lock().unwrap();
            match state_loc.claim(payload.key){
                Ok(payload) => {
                    sleep(Duration::from_millis(1000));
                    let _ = stream_write(loc_stream, & serialize_message(
                        & Message {
                            header: MessageHeader::ACK,
                            body: "".to_string()
                        }
                    ));
                },
                _ => {
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: MessageHeader::NULL,
                        body: "".to_string()
                    }));
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput, "Failed to claim key"));
                },
            }
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