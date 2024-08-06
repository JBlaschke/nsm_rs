/// Track clients and services and handle events (heartbeats)

use std::net::{TcpStream, TcpListener};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, Mutex, Condvar};
use std::thread::sleep;
use std::time::{Duration, Instant};
use std::thread;
use threadpool::ThreadPool;
use std::collections::VecDeque;
use std::io::{self};
use std::any::Any;
use std::fmt;
use lazy_static::lazy_static;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::utils::{only_or_error, epoch};
use crate::connection::{
    MessageHeader, Message, Addr, connect, send, receive, stream_read, stream_write, 
    serialize_message, deserialize_message
};

/// Store client or service metadata
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Payload {
    /// local IP address
    pub service_addr: Vec<String>,
    /// used for service-client data connection
    pub service_port: i32,
    /// epoch time of when service was claimed
    pub service_claim: u64,
    /// 
    pub interface_addr: Vec<String>,
    /// used for sending/receiving heartbeats
    pub bind_port: i32,
    /// unique key identifies system
    pub key: u64,
    /// unique identification of each connection to broker
    pub id: u64,
    /// identifies unique service, clients assume id from connected service
    pub service_id: u64,
}

/// Store message contents to send to connected service
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct MsgBody{
    /// message to send to service
    pub msg: String,
    /// identifies unique service
    pub id: u64,
}

// Initialize the global variable with default values
lazy_static! {
    static ref GLOBAL_MSGBODY: Mutex<MsgBody> = Mutex::new(MsgBody::default());
}

/// Store fail_count for heartbeats and limit increment rate
#[derive(Clone, Debug)]
pub struct FailCounter{
    /// increment when heartbeat is not returned when expected
    pub fail_count: i32,
    /// track time from last fail_count increment
    pub last_increment: Instant,
    /// set interval for incrementing fail_count
    pub interval: Duration,
}

/// create and manipulate FailCounter struct
impl FailCounter{
    /// create new FailCounter struct
    fn new() -> Self {
        Self {
            fail_count: 0,
            last_increment: Instant::now(),
            interval: Duration::from_secs(10),
        }
    }

    /// increment fail_count
    fn increment(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_increment) >= self.interval {
            self.fail_count += 1;
            self.last_increment = now;
        }
    }
}

/// Parent trait of objects added to event queue/loop
/// Intended to encapsulated different types of Events
pub trait Event: Any + Send + Sync{
    /// call from event_monitor for specific Event type
    fn monitor(&mut self) -> std::io::Result<()>;
    /// allows Event object to be downcasted
    fn as_any(&mut self) -> &mut dyn Any;
    /// print event info to debug
    fn describe(&self) -> String;
}

/// Allows for printing out event queue of event objects 
impl fmt::Debug for dyn Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Event: {}", self.describe())
    }
}

/// Heartbeat Event struct holds metadata for a service/client with a heartbeat
#[derive(Clone, Debug)]
pub struct Heartbeat {
    /// same key as payload, needed for rmv()
    pub key: u64,
    /// same id as payload, needed for rmv()
    pub id: u64,
    /// same service_id, used to trace and reassigned a client whose service dropped from system
    pub service_id: u64,
    /// stream used to send heartbeats to client/service
    pub stream: Arc<Mutex<TcpStream>>,
    /// incremented when heartbeat is not received when expected, connection is dead at 10
    pub fail_counter: FailCounter,
    /// message sent from client
    pub msg_body: MsgBody,
}

/// Heartbeats are treated as an Event to handle other types of events in the same queue
impl Event for Heartbeat {

    /// downcast an Event as a Heartbeat
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    /// print ID and fail_count to debug
    fn describe(&self) -> String {
        format!("Heartbeat id: {}, failcount: {}, msg: {}", self.id, self.fail_counter.fail_count, self.msg_body.msg)
    }

    /// send a heartbeat to the service/client and check if entity sent one back,
    /// increment fail_count if heartbeat not received when expected
    fn monitor(&mut self) -> std::io::Result<()>{
        let mut loc_stream = match self.stream.lock() {
            Ok(guard) => guard,
            Err(_err) => {
                self.fail_counter.increment();
                return Err(io::Error::new(io::ErrorKind::Other, "Mutex lock poisoned"));
            }
        };
        
        trace!("Sending heartbeat containing msg: {}", self.msg_body.msg);
        let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
            header: MessageHeader::HB,
            body: serde_json::to_string(& self.msg_body.clone()).unwrap()
        }));
    
        let failure_duration = Duration::from_secs(6); //change to any failure limit
        // allots time for reading from stream
        match loc_stream.set_read_timeout(Some(failure_duration)) {
            Ok(_x) => trace!("set_read_timeout OK"),
            Err(_e) => warn!("set_read_timeout Error")
        }
    
        let received = match stream_read(&mut loc_stream) {
            Ok(message) => message,
            Err(err) => {
                self.fail_counter.increment();
                trace!("Failed to receive data from stream. {:?}", self.fail_counter.fail_count);
                return Err(err);
            }
        };
    
        if received == "" {
            self.fail_counter.increment();
            trace!("Failed to receive HB. {:?}", self.fail_counter.fail_count);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput, "HB Failed")
            );
        } else {    
            trace!("Received: {:?}", received);
            self.fail_counter.fail_count = 0;
            trace!("Resetting failcount. {}", self.fail_counter.fail_count);
        }
    
        sleep(Duration::from_millis(2000)); //change to any time interval, must match time in heartbeat_handler()

        Ok(())
    }
}

/// Function loops through State's deque (event queue) to handle events using multithreading 
pub fn event_monitor(state: Arc<(Mutex<State>, Condvar)>) -> std::io::Result<()>{

    trace!("Starting event monitor");
    let (lock, cvar) = &*state;

    let deque_clone = {
        let state_loc = lock.lock().unwrap();
        Arc::clone(& state_loc.deque)
    };

    let data = Arc::new(Mutex::new((0, 0, 0, 0))); // (fail_count, key, id, service_id)
    let mut service_id: i64 = -1;
    let mut failed_client = 0;

    // loop runs while events are in the queue
    loop {
        {
            let mut state_loc = lock.lock().unwrap();
            // loop is paused while there are no events to handle
            while !state_loc.running {
                trace!("waiting to run");
                let (new_state_loc, result) = cvar.wait_timeout(state_loc, Duration::from_secs(1)).unwrap();
                state_loc = new_state_loc;
                if result.timed_out() {
                    // Set running to true and notify
                    state_loc.running = true;
                    cvar.notify_one();
                }
            }
        }

        let mut shared_data = data.lock().unwrap();
        // if connection is dead, remove it
        if shared_data.0 == 10{
            let mut state_loc = lock.lock().unwrap();
            match state_loc.rmv(shared_data.1, shared_data.2, shared_data.3){
                Ok(m) => {
                    match m.header {
                        MessageHeader::PUB => {
                            trace!("Removed service from Vec.");
                            // set service_id to track client and claim new service
                            service_id = m.body.parse().unwrap();
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

        // pop event from queue 
        let event = {
            let mut loc_deque = deque_clone.lock().unwrap();
            loc_deque.pop_front()
        };

        let mut event = match event {
            Some(tracker) => tracker,
            None => {
                // pause loop to wait for new events
                let mut state_loc = lock.lock().unwrap();
                state_loc.running = false;
                cvar.notify_one();
                continue;
            }
        };

        let deque_clone2 = Arc::clone(& deque_clone);
        let data_clone = Arc::clone(&data);
        let mut state_loc = lock.lock().unwrap();
        let mut state_clone = state_loc.clone();
        state_loc.running = true;
        cvar.notify_one();

        // use a worker from threadpool to handle events with multithreading
        state_loc.pool.execute(move || {

            trace!("Passing event to event monitor...");
            let _ = event.monitor();

            // check heartbeat metadata to see if entity should be added back to event queue 
            // or if client should be claim a new service
            if let Some(hb) = event.as_any().downcast_mut::<Heartbeat>() {
                let mut data = data_clone.lock().unwrap();
                *data = (hb.fail_counter.fail_count, hb.key, hb.id, hb.service_id);
                if data.0 < 10 {
                    trace!("Adding back to VecDeque: id: {:?}, fail_count: {:?}", data.2, data.0);
                    if hb.service_id == (service_id as u64){
                        trace!("Connecting to new service");
                        match state_clone.claim(hb.key){
                            Ok(p) => {
                                trace!("Claimed new service w/ payload: {:?}", p);
                                // create new Heartbeat object with new service_id
                                event = Box::new(Heartbeat {
                                    key: hb.key,
                                    id: hb.id,
                                    service_id: hb.service_id,
                                    stream: Arc::clone(&hb.stream),
                                    fail_counter: FailCounter::new(),
                                    msg_body: hb.msg_body.clone(),
                                });
                                {
                                    // add updated client back to queue 
                                    let mut loc_deque = deque_clone2.lock().unwrap();
                                    let _ = loc_deque.push_back(event);
                                }
                            }
                            Err(_err) => {
                                // no more available services to claim
                                trace!("Could not connect to new service");
                                data.0 = 10;
                            }
                        }
                    }
                    else{
                        // add event back to queue 
                        let mut loc_deque = deque_clone2.lock().unwrap();
                        let _ = loc_deque.push_back(event);
                        trace!("{:?}", loc_deque);
                    }
                } else {
                    // service or client no longer connected to broker
                    info!("Dropping event");
                }
            }
        });
    }
}

/// Keep track of all connected clients/services, holds event loop and threadpool
#[derive(Clone)]
pub struct State {
    /// hashmap - key: key, value: clients/services payloads
    pub clients: HashMap<u64, Vec<Payload>>,
    pub claims: HashMap<u64, Vec<Payload>>,
    /// pool of threads to handle events in event queue
    pub pool: ThreadPool,
    pub timeout: u64, 
    /// increments for each new connection to broker, used for id in payload
    pub seq: u64,
    /// event queue
    pub deque: Arc<Mutex<VecDeque<Box<dyn Event>>>>,
    /// true when event queue contains events to handle, false when event queue is empty
    running: bool,
}


impl State {
    /// inititates State struct with empty and default variables
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

    /// adds new services/clients to State struct and creates Event object to add to event loop 
    pub fn add(&mut self, mut p: Payload, service_id: u64) -> std::io::Result<Heartbeat>{

        let ipstr = only_or_error(& p.service_addr);
        let bind_address = format!("{}:{}", ipstr, p.bind_port);

        trace!("Listener connecting to: {}", bind_address);
        let mut bind_fail = 0;
        // broker connects to service/client bind_port for heartbeats
        // loop solves connection race case 
        let hb_stream = loop {
            match TcpStream::connect(bind_address.clone()) {
                Ok(stream) => break stream,
                Err(err) => {
                    bind_fail += 1;
                    if bind_fail > 5{
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput, "Service failed to connect to bind port"));
                    }
                    trace!("{}",format!("Retrying connection: {}", err));
                    sleep(Duration::from_millis(1000));
                    continue;
                }
            }
        };
        let shared_hb_stream = Arc::new(Mutex::new(hb_stream));
        
        let mut temp_id = self.seq;
        // service_id is same as id for a service - unclaimed service starts with service_id = 0
        // client inherits its service's id for service_id
        if service_id != 0{
            temp_id = service_id;
        }
        let heartbeat = Heartbeat {
            key: p.key,
            id: self.seq,
            service_id: temp_id,
            stream: shared_hb_stream,
            fail_counter: FailCounter::new(),
            msg_body: MsgBody::default(),
        };
        trace!("Adding new connection to queue");
        {
            // push Heartbeat event to event queue
            let mut loc_deque = self.deque.lock().unwrap();
            let _ = loc_deque.push_back(Box::new(heartbeat.clone()));
        }

        // check key with clients hashmap in State
        // add new key if key does not exist or set cl to exising key
        let cl: &mut Vec<Payload> = self.clients.entry(p.key).or_insert(Vec::new());
        p.id = self.seq;
        p.service_id = temp_id;
        cl.push(p); // push payload to cl key in clients hashmap
        self.seq += 1; // set up seq for next connection
        return Ok(heartbeat);
    }

    /// remove service/client from clients hashmap in State when Heartbeat fails
    pub fn rmv(&mut self, k: u64, id: u64, service_id: u64) -> std::io::Result<Message>{
        trace!("Current state: {:?}", self.clients);

        let mut removed_item = None;
        // find key in hashmap
        if let Some(vec) = self.clients.get_mut(&k) {
            // find value in clients with matching id
            if let Some(pos) = vec.iter().position(|item| item.id == id) {
                let item = vec.remove(pos); // remove item with matching idi
                println!("Removed item: {:?}", item);
                // if no entities in key entry, remove key from clients
                if vec.is_empty(){
                    self.clients.remove(&k); 
                }
                removed_item = Some(item);
            }
        }
        if let Some(item) = removed_item {
            trace!("Now state: {:?}", self.clients);
            // check if a service was removed from clients
            if item.service_id == item.id{
                return Ok(Message {
                    header: MessageHeader::PUB,
                    body: item.service_id.to_string()
                });
            }
            // if a client was removed, reset service's service_claim to 0
            // makes service available for a new client
            if let Some(vec) = self.clients.get_mut(&k) {
                if let Some(pos) = vec.iter().position(|publish| publish.service_id == service_id) {
                    if let Some(publish) = vec.get_mut(pos) {
                        publish.service_claim = 0;
                    }
                }
            }
            return Ok(Message {
                header: MessageHeader::CLAIM,
                body: "".to_string()
            }); 
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "Failed to remove item from state"));
    }

    /// search clients hashmap for an available service with matching key,
    /// set service's service_claim equal to ecpoch time to match the new client
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

    /// print all entries in clients hashmap
    pub fn print(&mut self) {
        for (key, values) in & self.clients {
            for v in values {
                println!("{}: {:?}", key, v);
            }
        }
    }

}

/// Serialize Payload struct into JSON String
pub fn serialize(payload: & Payload) -> String {
    serde_json::to_string(payload).unwrap()
}

/// Deserialize JSON String into Payload struct
pub fn deserialize(payload: & String) -> Payload {
    serde_json::from_str(payload).unwrap()
}

/// Broker handles incoming connections, adds entity to its State struct,
/// connects client to available services, and notifies event loop of new Events
pub fn request_handler(
    state: &Arc<(Mutex<State>, Condvar)>, stream: & Arc<Mutex<TcpStream>>
) -> std::io::Result<()> {
    trace!("Starting request handler");

    // receive Payload from incoming connection
    let message = receive(stream)?;

    // check type of Message, only want to handle PUB or CLAIM Message
    let payload = match message.header {
        MessageHeader::HB => panic!("Unexpected HB message encountered!"),
        MessageHeader::ACK => panic!("Unexpected ACK message encountered!"),
        MessageHeader::PUB => deserialize(& message.body),
        MessageHeader::CLAIM => deserialize(& message.body),
        MessageHeader::COL => panic!("Unexpected COL message encountered!"),
        MessageHeader::MSG => Payload::default(),
        MessageHeader::NULL => panic!("Unexpected NULL message encountered!"),
    };

    trace!("Request handler received: {:?}", payload);
    // handle services (PUB), clients (CLAIM), messages (MSG) appropriately
    match message.header {
        MessageHeader::PUB => {
            trace!("Publishing Service: {:?}", payload);
            let (lock, cvar) = &**state;
            let mut state_loc = lock.lock().unwrap();

            let _ = state_loc.add(payload, 0); // add service to clients hashmap and event loop 
            state_loc.running = true; // set running to true for event loop
            cvar.notify_one(); // notify event loop of new Event

            println!("Now state:");
            state_loc.print(); // print state of clients hashmap
        },
        MessageHeader::CLAIM => {
            trace!("Claiming Service: {:?}", payload);

            let (lock, _cvar) = &**state;
            let mut state_loc = lock.lock().unwrap();
            let mut loc_stream: &mut TcpStream = &mut *stream.lock().unwrap();
            let mut service_id = 0;
            let mut claim_fail = 0; // initiate counter for connection failure
            // loop solves race case when client starts faster than service can be published
            loop {
                // 
                match state_loc.claim(payload.key){
                    Ok(p) => {
                        service_id = (*p).service_id; // capture claimed service's service_id for add()
                        // send acknowledgment of successful service claim with service's payload containing its address
                        let _ = stream_write(loc_stream, & serialize_message(
                            & Message {
                                header: MessageHeader::ACK,
                                body: serialize(p) // address extracted in main to print to client
                            }
                        ));
                        break;
                    },
                    _ => {
                        claim_fail += 1;
                        if claim_fail <= 5{
                            sleep(Duration::from_millis(1000));
                            continue;
                        }
                        // notify main() of failure to claim an available service
                        let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                            header: MessageHeader::NULL,
                            body: "".to_string()
                        }));
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput, "Failed to claim key"));
                    },
                }
            }
            let _ = state_loc.add(payload, service_id); // add client to clients hashmap and event loop

            println!("Now state:");
            state_loc.print(); // print state of clients hashmap
        },
        MessageHeader::MSG => {
            let msg_body: MsgBody = serde_json::from_str(& message.body).unwrap();
            trace!("Sending Message: {:?}", msg_body);

            let (lock, _cvar) = &**state;
            let mut state_loc = lock.lock().unwrap();

            let mut counter = 0;
            while counter < 10 {
                {
                    let mut deque = state_loc.deque.lock().unwrap();
                    if let Some(hb) = deque.iter_mut().find_map(|e| {
                        e.as_any().downcast_mut::<Heartbeat>().filter(|hb| hb.service_id == msg_body.id) }) {
                            hb.msg_body = msg_body.clone();
                            trace!("Altering hb message {:?}", hb);
                            break;
                    }
                    else{
                        counter += 1;
                    }
                }
                sleep(Duration::from_millis(1000));
            }
            if counter == 10 {
                warn!("Could not find matching service");
            }

        }
        _ => {panic!("This should not be reached!");}
    }
    Ok(())
}

pub fn heartbeat_handler_helper(stream: & Arc<Mutex<TcpStream>>, payload: Option<&String>, 
    addr: Option<&Addr>) -> std::io::Result<()> {
    let stream_clone = Arc::clone(&stream);

    // retrieve service's payload from client or set an empty message body
    let binding = "".to_string();
    let payload = payload.unwrap_or(&binding);
    let payload_clone = payload.clone();

    let empty_addr = Addr{
        host: "".to_string(),
        port: 0
    };
    let addr = addr.unwrap_or(&empty_addr);
    let addr_clone = addr.clone();

    let _ = thread::spawn(move || {
        let _ = heartbeat_handler(&stream_clone, &payload_clone, &addr_clone);
    });
    return Ok(());
}

/// Receive a heartbeat from broker and send one back to show life of connection.
/// If a client, HB message contains payload of connected service to use in Collect()
pub fn heartbeat_handler(stream: & Arc<Mutex<TcpStream>>, payload: &String, addr: &Addr)
  -> std::io::Result<()> {
    trace!("Starting heartbeat handler");

    let mut service_id = 0;
    if *payload != "".to_string() {
        service_id = deserialize(payload).service_id;
    }

    loop{
        let mut loc_stream: &mut TcpStream = &mut *stream.lock().unwrap();

        let request = match stream_read(loc_stream) {
            Ok(message) => deserialize_message(& message),
            Err(err) => {return Err(err);}
        };

        if matches!(request.header, MessageHeader::MSG){
            let listen_stream = match connect(& addr){
                Ok(s) => s,
                Err(_e) => {
                    return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,"MSG connection unsuccessful."));
                    }
            };
            let stream_mut = Arc::new(Mutex::new(listen_stream));
            let msg_body = MsgBody{
                msg: request.body.clone(),
                id: service_id
            };
            println!("{:?}", msg_body);
            let ack = send(& stream_mut, & Message{
                header: MessageHeader::MSG,
                body: serde_json::to_string(& msg_body).unwrap()
            });
        }
        trace!("Heartbeat handler received {:?}", request);
        if matches!(request.header, MessageHeader::COL) {
            if *payload == "".to_string(){
                // send msg back
                let mut msg = GLOBAL_MSGBODY.lock().unwrap();
                let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                    header: request.header,
                    body: serde_json::to_string(&* msg.msg).unwrap()
                }));
            }
            else {
                // send payload back
                let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                    header: request.header,
                    body: payload.clone()
                }));
            }
            trace!("Heartbeat handler has returned request");
        }
        else if matches!(request.header, MessageHeader::HB | MessageHeader::MSG){
            let msg_body: MsgBody = serde_json::from_str(& request.body.clone()).unwrap();
            if msg_body.msg.is_empty(){
                let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                    header: request.header,
                    body: payload.clone(),
                }));
            }
            else{
                let mut msg = GLOBAL_MSGBODY.lock().unwrap();
                *msg = serde_json::from_str(& request.body).unwrap();
                let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                    header: request.header,
                    body: request.body
                }));
            }   
            trace!("Heartbeat handler has returned request");
        }
        else {
            warn!(
                "Unexpected request sent to heartbeat_handler: {}",
                request.header
            );
            info!("Dropping request");
        }

        sleep(Duration::from_millis(2000)); // change to any time interval, must match time in monitor()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// check if function returns an error when a client claims an unused key
    #[test]
    fn test_claim_key_DNE() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _cvar) = &*state;
        
        // add published services to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let mut p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12020,
                service_claim: 0,
                interface_addr: Vec::new(),
                bind_port: 12010,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            let seq = state_loc.seq;
            let cl: &mut Vec<Payload> = state_loc.clients.entry(p.key).or_insert(Vec::new());
            p.id = seq;
            p.service_id = seq;
            cl.push(p);
            state_loc.seq += 1;
        }
        // claim published services with matching keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let mut state_loc = lock.lock().unwrap();
            let _ = state_loc.claim(k.floor() as u64);
        }
        // try to claim a key that does not exist in State
        let mut state_loc = lock.lock().unwrap();
        let result = state_loc.claim(2000);
        assert!(result.is_err());
    }

    /// check if function returns an error when a client claims a used key with no more available services
    #[test]
    fn test_claim_filled_services() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _cvar) = &*state;
        
        // add published services to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let mut p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12020,
                service_claim: 0,
                interface_addr: Vec::new(),
                bind_port: 12010,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            let seq = state_loc.seq;
            let cl: &mut Vec<Payload> = state_loc.clients.entry(p.key).or_insert(Vec::new());
            p.id = seq;
            p.service_id = seq;
            cl.push(p);
            state_loc.seq += 1;
        }
        // claim published services with matching keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let mut state_loc = lock.lock().unwrap();
            let _ = state_loc.claim(k.floor() as u64);
        }
        // try to claim existing key without available service
        let mut state_loc = lock.lock().unwrap();
        let result = state_loc.claim(1000);
        assert!(result.is_err());
    }

    /// Test adding services and clients to State
    #[test]
    fn test_add() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _cvar) = &*state;

        let _listener = TcpListener::bind("127.0.0.1:12010");
        
        // add published services to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12020,
                service_claim: 0,
                interface_addr: Vec::new(),
                bind_port: 12010,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            let result = state_loc.add(p, 0);
            assert!(result.is_ok());
        }

        sleep(Duration::from_millis(500));
        let _listener = TcpListener::bind("127.0.0.1:12015");
    
        // add clients to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12000,
                service_claim: epoch(),
                interface_addr: Vec::new(),
                bind_port: 12015,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            let mut service_id = 0;
            match state_loc.claim(p.key){
                Ok(pl) => service_id = (*pl).service_id,
                _ => panic!("Error: Failed to claim key")
            };
            let result = state_loc.add(p, service_id);
            assert!(result.is_ok());
        }

    }

    /// test if rmv() properly removes clients
    #[test]
    fn test_rmv_client() {
    
        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _cvar) = &*state;

        sleep(Duration::from_millis(500));
        let _listener = TcpListener::bind("127.0.0.1:12010");
        
        // add published services to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12020,
                service_claim: 0,
                interface_addr: Vec::new(),
                bind_port: 12010,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            state_loc.add(p, 0);
        }

        sleep(Duration::from_millis(500));
        let _listener = TcpListener::bind("127.0.0.1:12015");
    
        // add clients to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12000,
                service_claim: epoch(),
                interface_addr: Vec::new(),
                bind_port: 12015,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            let mut service_id = 0;
            match state_loc.claim(p.key){
                Ok(pl) => service_id = (*pl).service_id,
                _ => panic!("Error: Failed to claim key")
            };
            state_loc.add(p, service_id);
        }
        {
            // State contains all services and clients
            let mut state_loc = lock.lock().unwrap();
            state_loc.print();
        }
        // remove all clients from State
        for x in 1..21{
            let mut state_loc = lock.lock().unwrap();
            let k: f64 = (1000 + x/2) as f64;
            // id = x+20 : services' ids are 1-21
            // service_id = x : client's service_ids match services' ids
            let result = state_loc.rmv(k.floor() as u64, x + 20, x);
            assert!(result.is_ok());
        }
        {
            // State only contains clients
            let mut state_loc = lock.lock().unwrap();
            state_loc.print();
        }
    }
    
    // test if rmv() properly removes services
    #[test]
    fn test_rmv_service() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _cvar) = &*state;

        sleep(Duration::from_millis(500));
        let _listener = TcpListener::bind("127.0.0.1:12010");
        
        // add published services to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12020,
                service_claim: 0,
                interface_addr: Vec::new(),
                bind_port: 12010,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            state_loc.add(p, 0);
        }

        sleep(Duration::from_millis(500));
        let _listener = TcpListener::bind("127.0.0.1:12015");
    
        // add clients to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12000,
                service_claim: epoch(),
                interface_addr: Vec::new(),
                bind_port: 12015,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            let mut service_id = 0;
            match state_loc.claim(p.key){
                Ok(pl) => service_id = (*pl).service_id,
                _ => panic!("Error: Failed to claim key")
            };
            state_loc.add(p, service_id);
        }
        {
            // State contains all services and clients
            let mut state_loc = lock.lock().unwrap();
            state_loc.print();
        }
        // remove all services from State
        for x in 1..21{
            let mut state_loc = lock.lock().unwrap();
            let k: f64 = (1000 + x/2) as f64;
            // id = x: services' ids are 1-21
            // service_id = x : id == service_id
            let result = state_loc.rmv(k.floor() as u64, x, x);
            assert!(result.is_ok());
        }
        {
            // State only contains clients
            let mut state_loc = lock.lock().unwrap();
            state_loc.print();
        }
    }

    // test if rmv() properly removes clients and services
    #[test]
    fn test_rmv_clients_and_services() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _cvar) = &*state;

        sleep(Duration::from_millis(500));
        let _listener = TcpListener::bind("127.0.0.1:12010");
        
        // add published services to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12020,
                service_claim: 0,
                interface_addr: Vec::new(),
                bind_port: 12010,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            state_loc.add(p, 0);
        }

        sleep(Duration::from_millis(500));
        let _listener = TcpListener::bind("127.0.0.1:12015");
    
        // add clients to State struct with two copies of 10 keys
        for x in 1..21{
            let k: f64 = (1000 + x/2) as f64;
            let p = Payload {
                service_addr: vec!["127.0.0.1".to_string()],
                service_port: 12000,
                service_claim: epoch(),
                interface_addr: Vec::new(),
                bind_port: 12015,
                key: k.floor() as u64,
                id: 0,
                service_id: 0,
            };
            let mut state_loc = lock.lock().unwrap();
            let mut service_id = 0;
            match state_loc.claim(p.key){
                Ok(pl) => service_id = (*pl).service_id,
                _ => panic!("Error: Failed to claim key")
            };
            state_loc.add(p, service_id);
        }
        {
            // State contains all services and clients
            let mut state_loc = lock.lock().unwrap();
            state_loc.print();
        }
        // remove all services from State
        for x in 1..21{
            let mut state_loc = lock.lock().unwrap();
            let k: f64 = (1000 + x/2) as f64;
            // id = x: services' ids are 1-21
            // service_id = x : id == service_id
            let result = state_loc.rmv(k.floor() as u64, x, x);
            assert!(result.is_ok());
            let result = state_loc.rmv(k.floor() as u64, x + 20, x);
            assert!(result.is_ok());
        }
        {
            // State only contains clients
            let mut state_loc = lock.lock().unwrap();
            state_loc.print();
        }
    }

    /// test if request_handler() panics when receiving a NULL message
    #[test]
    #[should_panic]
    fn test_request_handler_null() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));

        let listener = TcpListener::bind("127.0.0.1:11000").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:11000").unwrap();

        let _ = stream_write(&mut stream, & serialize_message(& Message{
            header: MessageHeader::NULL,
            body: "".to_string()
        }));

        for s in listener.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let _ = request_handler(&state, &shared_stream);
                }
                Err(e) => {
                    panic!("Error: {}", e);
                }
            }
        }
    }

    /// test if request_handler() panics when receiving an ACK message
    #[test]
    #[should_panic]
    fn test_request_handler_ack() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));

        let listener = TcpListener::bind("127.0.0.1:13000").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:13000").unwrap();

        let _ = stream_write(&mut stream, & serialize_message(& Message{
            header: MessageHeader::ACK,
            body: "".to_string()
        }));

        for s in listener.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let _ = request_handler(&state, &shared_stream);
                }
                Err(e) => {
                    panic!("Error: {}", e);
                }
            }
        }
    }

    /// test if request_handler() panics when receiving a HB message
    #[test]
    #[should_panic]
    fn test_request_handler_HB() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));

        let listener = TcpListener::bind("127.0.0.1:14000").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:14000").unwrap();

        let _ = stream_write(&mut stream, & serialize_message(& Message{
            header: MessageHeader::HB,
            body: "".to_string()
        }));

        for s in listener.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let _ = request_handler(&state, &shared_stream);
                }
                Err(e) => {
                    panic!("Error: {}", e);
                }
            }
        }
    }

    /// test if request_handler() panics when no service to claim
    #[test]
    #[should_panic]
    fn test_request_handler_CLAIM() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));

        sleep(Duration::from_millis(500));
        let listener = TcpListener::bind("127.0.0.1:15000").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:15000").unwrap();

        let p = Payload {
            service_addr: vec!["127.0.0.1".to_string()],
            service_port: 15000,
            service_claim: epoch(),
            interface_addr: Vec::new(),
            bind_port: 15015,
            key: 1000,
            id: 0,
            service_id: 0,
        };

        let _ = stream_write(&mut stream, & serialize_message(
            & Message {
                header: MessageHeader::CLAIM,
                body: serialize(&p)
            }
        ));

        for s in listener.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let stream_clone = Arc::clone(&shared_stream);
                    let state_clone = state.clone();
                    let thread_handler = thread::spawn(move || {
                        let _ = request_handler(&state_clone, &stream_clone);
                    });
                    let _ = thread_handler.join();
                    loop {
                        sleep(Duration::from_millis(1000));
                        let message = match stream_read(&mut stream) {
                            Ok(m) => deserialize_message(& m),
                            Err(err) => panic!("server correctly dropped claim")
                        };
                        trace!("{:?}", message);
                        // print service's address to client
                        if matches!(message.header, MessageHeader::ACK){
                            panic!("server should not have acknowledged claim");
                        }
                    }
                }
                Err(e) => {
                    panic!("Error: {}", e);
                }
            }
        }
    }

    /// test if request_handler() successfully completes with PUB message
    #[test]
    fn test_request_handler_PUB() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));

        sleep(Duration::from_millis(500));
        let listener = TcpListener::bind("127.0.0.1:16000").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:16000").unwrap();

        let p = Payload {
            service_addr: vec!["127.0.0.1".to_string()],
            service_port: 16020,
            service_claim: epoch(),
            interface_addr: Vec::new(),
            bind_port: 16010,
            key: 1000,
            id: 0,
            service_id: 0,
        };

        let _ = stream_write(&mut stream, & serialize_message(
            & Message {
                header: MessageHeader::PUB,
                body: serialize(&p)
            }
        ));

        for s in listener.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let stream_clone = Arc::clone(&shared_stream);
                    let state_clone = state.clone();
                    let thread_handler = thread::spawn(move || {
                        let result = request_handler(&state_clone, &stream_clone);
                        assert!(result.is_ok());
                    });
                    let _ = thread_handler.join();
                    break;
                }
                Err(e) => {
                    panic!("Error: {}", e);
                }
            }
        }
    }

    /// test if heartbeat handler properly processes non-heartbeat messages
    #[test]
    #[should_panic]
    fn test_heartbeat_handler_incorrect_message() {

        let publisher = TcpListener::bind("127.0.0.1:17000").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:17000").unwrap();

        let _ = stream_write(&mut stream, & serialize_message(
            & Message {
                header: MessageHeader::PUB,
                body: "".to_string()
            }
        ));
        for s in publisher.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let stream_clone = Arc::clone(&shared_stream);
                    let thread_handler = thread::spawn(move || {
                        let _ = heartbeat_handler(&stream_clone);
                    });
                    sleep(Duration::from_millis(1000));
                    let failure_duration = Duration::from_secs(10);
                    match stream.set_read_timeout(Some(failure_duration)) {
                        Ok(_x) => trace!("set_read_timeout OK"),
                        Err(_e) => trace!("set_read_timeout Error")
                    }
                    let _ = match stream_read(&mut stream) {
                        Ok(m) => deserialize_message(& m),
                        Err(err) => panic!("server correctly dropped claim")
                    };
                }
                Err(e) => {
                    panic!("Error: {}", e);
                }
            }
        }

    }

    /// test if heartbeat handler returns a heartbeat message
    #[test]
    fn test_heartbeat_handler_correct_message() {

        let publisher = TcpListener::bind("127.0.0.1:17010").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:17010").unwrap();

        let _ = stream_write(&mut stream, & serialize_message(
            & Message {
                header: MessageHeader::HB,
                body: "".to_string()
            }
        ));
        for s in publisher.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let stream_clone = Arc::clone(&shared_stream);
                    let thread_handler = thread::spawn(move || {
                        let _ = heartbeat_handler(&stream_clone);
                    });
                    sleep(Duration::from_millis(1000));
                    let failure_duration = Duration::from_secs(5);
                    match stream.set_read_timeout(Some(failure_duration)) {
                        Ok(_x) => trace!("set_read_timeout OK"),
                        Err(_e) => trace!("set_read_timeout Error")
                    }
                    let message = match stream_read(&mut stream) {
                        Ok(m) => deserialize_message(& m),
                        Err(err) => panic!("server correctly dropped claim")
                    };
                    let header = match message.header{
                        MessageHeader::HB => "HB",
                        _ => panic!("heartbeat not returned properly")
                    };
                    assert!(header == "HB");
                    break;
                }
                Err(e) => {
                    panic!("Error: {}", e);
                }
            }
        }

    }

}