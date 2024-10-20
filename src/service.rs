/// Track clients and services and handle events (heartbeats)

use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, Condvar};
use tokio::sync::Notify;
use tokio::time::{sleep, Instant, timeout, Duration};
use std::thread;
use threadpool::ThreadPool;
use std::collections::VecDeque;
use std::io::{self, Error as IoError};
use std::any::Any;
use std::fmt;
use lazy_static::lazy_static;
use hyper::body::{Buf, Bytes, Incoming};
use std::net::SocketAddr;
use hyper::http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::client::legacy::Client;
use hyper_rustls::{HttpsConnectorBuilder, HttpsConnector};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::server::conn::auto::Builder;
use tokio::sync::Mutex;
use tokio::net::{TcpListener, TcpStream};
use std::future::Future;
use rustls::ClientConfig;


#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::utils::{only_or_error, epoch};
use crate::connection::{MessageHeader, Message, Addr, ComType, connect, send, receive,
     serialize_message, deserialize_message, collect_request, stream_write, stream_read};

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

// Initialize the global message with default values
lazy_static! {
    static ref GLOBAL_MSGBODY: Mutex<MsgBody> = Mutex::new(MsgBody::default());
}

/// Store fail_count for heartbeats and limit increment rate
#[derive(Clone, Debug)]
pub struct FailCounter{
    /// increment when heartbeat is not returned when expected
    pub fail_count: i32,
    /// track first failure
    pub first_increment: Instant,
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
            first_increment: Instant::now(),
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

/// Heartbeat Event struct holds metadata for a service/client with a heartbeat
#[derive(Clone, Debug)]
pub struct Heartbeat {
    /// same key as payload, needed for rmv()
    pub key: u64,
    /// same id as payload, needed for rmv()
    pub id: u64,
    /// same service_id, used to trace and reassigned a client whose service dropped from system
    pub service_id: u64,
    /// address of service/client
    pub addr: String,
    /// stream used to send heartbeats to client
    pub stream: Option<Arc<Mutex<TcpStream>>>,
    /// client object for sending heartbeat requests
    pub client: Option<Client<HttpsConnector<HttpConnector>, Full<Bytes>>>,
    /// incremented when heartbeat is not received when expected, connection is dead at 10
    pub fail_counter: FailCounter,
    /// message sent from client
    pub msg_body: MsgBody,
}

impl Heartbeat {
    /// send a heartbeat to the service/client and check if entity sent one back,
    /// increment fail_count if heartbeat not received when expected
    async fn monitor(&mut self) -> Result<Response<Full<Bytes>>, std::io::Error>{
        
        trace!("Sending heartbeat containing msg: {}", self.msg_body.msg);
        let mut response = Response::new(Full::default());

        let message = serialize_message(& Message{
            header: MessageHeader::HB,
            body: serde_json::to_string(& self.msg_body.clone()).unwrap()
        });

        let received = match (&self.stream, &self.client){
            (Some(s), None) => {
                let mut loc_stream = s.lock().await;
                
                trace!("Sending heartbeat containing msg: {}", self.msg_body.msg);
                let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                    header: MessageHeader::HB,
                    body: serde_json::to_string(& self.msg_body.clone()).unwrap()
                })).await;
            
                // allots time for reading from stream
                let received = match stream_read(&mut loc_stream).await {
                    Ok(message) => message,
                    Err(ref err) if err.kind() == std::io::ErrorKind::ConnectionReset => {
                        self.fail_counter.increment();
                        trace!("ConnectionReset error");
                        return Err(std::io::Error::new(err.kind(), err.to_string()));
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::ConnectionAborted => {
                        self.fail_counter.increment();
                        trace!("ConnectionAborted error");
                        return Err(std::io::Error::new(err.kind(), err.to_string()));
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::TimedOut => {
                        self.fail_counter.increment();
                        trace!("TimeOut error");
                        return Err(std::io::Error::new(err.kind(), err.to_string()));
                    }
                    Err(err) => {
                        // open connection, read timed out
                        if self.fail_counter.fail_count == 0 {
                            self.fail_counter.first_increment = Instant::now();
                        }
                        self.fail_counter.increment();
                        trace!("Timed out reading from stream. {:?}", self.fail_counter.fail_count);
                        return Err(err);
                    }
                };
                received
            },
            (None, Some(c)) => {
                let req = Request::builder()
                .method(Method::GET)
                .uri(format!("http://{}/heartbeat_handler", self.addr))
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(message)))
                .unwrap();
                
                // allots time for reading response
                let timeout_duration = Duration::from_secs(10);
                let start = Instant::now();
                let received = match timeout(timeout_duration, c.request(req)).await {
                    Ok(Ok(mut resp)) => {
                        trace!("Received response: {:?}", resp);
                        let msg = collect_request(resp.body_mut()).await.unwrap();
                        serialize_message(&msg)
                    }
                    Ok(Err(e)) => {
                        if self.fail_counter.fail_count == 0 {
                            self.fail_counter.first_increment = Instant::now();
                        }
                        self.fail_counter.increment();
                        trace!("Timed out reading from stream. {:?}", self.fail_counter.fail_count);
                        *response.status_mut() = StatusCode::BAD_REQUEST;
                        return Ok(response);            
                    }
                    Err(_) => {
                        *response.status_mut() = StatusCode::REQUEST_TIMEOUT;
                        return Ok(response); 
                    }
                };
                received
            },
            _ => panic!("Unexpected state: no stream or client.")
        };
        
        if received == "" {
            // broken connection
            if self.fail_counter.fail_count == 0 {
                self.fail_counter.first_increment = Instant::now();
            }
            self.fail_counter.increment();
            trace!("Failed to receive HB. {:?}", self.fail_counter.fail_count);
            *response.status_mut() = StatusCode::BAD_REQUEST;
        } else {    
            trace!("Received: {:?}", received);
            self.fail_counter.fail_count = 0;
            trace!("Resetting failcount. {}", self.fail_counter.fail_count);
            *response.status_mut() = StatusCode::OK;
        }

        Ok(response)
    }
}

/// Function loops through State's deque (event queue) to handle events using multithreading 
pub async fn event_monitor(state: Arc<(Mutex<State>, Notify)>) -> Result<Response<Full<Bytes>>, std::io::Error>{

    trace!("Starting event monitor");
    let (lock, notify) = &*state;

    let deque_clone = {
        let state_loc = lock.lock().await;
        Arc::clone(& state_loc.deque)
    };

    let data = Arc::new(Mutex::new((0, 0, 0, 0))); // (fail_count, key, id, service_id)
    let mut service_id: i64 = -1;
    let mut failed_client = 0;

    // loop runs while events are in the queue
    loop {
        {
            let mut state_loc = lock.lock().await;
            // loop is paused while there are no events to handle
            while !state_loc.running {
                trace!("waiting to run");
                let notify_result = timeout(Duration::from_millis(200), notify.notified()).await;
                match notify_result {
                    Ok(()) => {
                        trace!("Processing next events");
                    }
                    Err(_) => {
                        // Timeout occurred, modify the state
                        trace!("Timeout occurred, modifying running state...");
                        state_loc.running = true;
                        notify.notify_one();
                    }
                }
            }
        }

        let mut shared_data = data.lock().await;
        // if connection is dead, remove it
        if shared_data.0 == 10{
            let mut state_loc = lock.lock().await;
            match state_loc.rmv(shared_data.1, shared_data.2, shared_data.3).await{
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
                    let mut response = Response::new(Full::default());
                    *response.status_mut() = StatusCode::BAD_REQUEST;
                    return Ok(response)
                }
            }
            shared_data.0 = 0;
        }

        // pop event from queue 
        let event = {
            let mut loc_deque = deque_clone.lock().await;
            loc_deque.pop_front()
        };

        let mut hb = match event {
            Some(tracker) => tracker,
            None => {
                // pause loop to wait for new events
                let mut state_loc = lock.lock().await;
                state_loc.running = false;
                notify.notify_one();
                continue;
            }
        };

        let deque_clone2 = Arc::clone(& deque_clone);
        let data_clone = Arc::clone(&data);
        let mut state_loc = lock.lock().await;
        let mut state_clone = state_loc.clone();
        state_loc.running = true;
        notify.notify_one();

        // use a worker from threadpool to handle events with multithreading
        tokio::spawn(async move {
            sleep(Duration::from_millis(1000)).await;

            trace!("Passing event to event monitor...");
            let _ = hb.monitor().await;

            // check heartbeat metadata to see if entity should be added back to event queue 
            // or if client should be claim a new service
            let mut data = data_clone.lock().await;
            *data = (hb.fail_counter.fail_count, hb.key, hb.id, hb.service_id);
            if data.0 < 10 {
                trace!("Adding back to VecDeque: id: {:?}, fail_count: {:?}", data.2, data.0);
                if hb.service_id == (service_id as u64){
                    trace!("Connecting to new service");
                    match state_clone.claim(hb.key).await{
                        Ok(p) => {
                            trace!("Claimed new service w/ payload: {:?}", p);
                            // create new Heartbeat object with new service_id
                            hb = Heartbeat {
                                key: hb.key,
                                id: hb.id,
                                service_id: hb.service_id,
                                addr: hb.addr.clone(),
                                stream: hb.stream.clone(),
                                client: hb.client.clone(),
                                fail_counter: FailCounter::new(),
                                msg_body: hb.msg_body.clone(),
                            };
                            {
                                // add updated client back to queue 
                                let mut loc_deque = deque_clone2.lock().await;
                                let _ = loc_deque.push_back(hb);
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
                    let mut loc_deque = deque_clone2.lock().await;
                    println!("{:?}", hb);
                    let _ = loc_deque.push_back(hb);
                    trace!("Deque status {:?}", loc_deque);
                }
            } else {
                // service or client no longer connected to broker
                info!("Dropping event");
            }
        });
        sleep(Duration::from_millis(100)).await;
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
    pub deque: Arc<Mutex<VecDeque<Heartbeat>>>,
    /// true when event queue contains events to handle, false when event queue is empty
    pub running: bool,
    /// tls client configuration to send heartbeat messages
    pub tls: Option<ClientConfig>
}


impl State {
    /// inititate State struct with empty and default variables
    pub fn new(tls: Option<ClientConfig>) -> State {
        State{
            clients: HashMap::new(),
            claims: HashMap::new(),
            pool: ThreadPool::new(30),
            timeout: 60,
            seq: 1,
            deque: Arc::new(Mutex::new(VecDeque::new())),
            running: false,
            tls
        }
    }

    /// adds new services/clients to State struct and creates Event object to add to event loop 
    pub async fn add(&mut self, mut p: Payload, service_id: u64, com: ComType) -> Result<Heartbeat, std::io::Error>{

        println!("adding service");

        let ipstr = only_or_error(& p.service_addr);
        let bind_address = format!("{}:{}", ipstr, p.bind_port);

        let (stream, client) = match com {
            ComType::TCP => {
                let mut bind_fail = 0;
                // broker connects to service/client bind_port for heartbeats
                // loop solves connection race case 
                let hb_stream = loop {
                    match TcpStream::connect(bind_address.clone()).await {
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
                (Some(shared_hb_stream), None)
            },
            ComType::API => {
                // connect to broker
                // Prepare the HTTPS connector

                let https_connector = match self.tls.clone() {
                    Some(t) => {
                        HttpsConnectorBuilder::new()
                        .with_tls_config(t)
                        .https_or_http()
                        .enable_http1()
                        .build()
                    }
                    None => {
                        HttpsConnectorBuilder::new()
                        .with_native_roots().unwrap()
                        .https_or_http()
                        .enable_http1()
                        .build()
                    }
                };

                // Build the Client using the HttpsConnector
                let client = Client::builder(TokioExecutor::new()).build(https_connector);     
                (None, Some(client))
            }
        };

        if let Some(vec) = self.clients.get_mut(&p.key) {
            // find value in clients with matching id
            if let Some(pos) = vec.iter().position(|item| item.service_addr == p.service_addr
                 && item.service_port == p.service_port) {
                let item = &vec[pos];
                let mut counter = 0;
                println!("{:?}", item);
                while counter < 10 {
                    {
                        let mut deque_loc = self.deque.lock().await;
                        println!("searching for item {:?}", deque_loc);
                        if let Some(hb) = deque_loc.iter_mut().find_map(|e| {
                            if e.id == item.id {
                                println!("found id");
                                Some(e)
                            } else {
                                println!("id not found");
                                counter+=1;
                                None
                            }
                        }) {
                            if Instant::now().duration_since(hb.fail_counter.first_increment) < Duration::from_secs(60){
                                hb.addr = bind_address.clone();
                                hb.stream = stream;
                                hb.client = client;
                                trace!("Altering hb {:?}", hb);
                                p.id = item.id;
                                p.service_id = item.service_id;
                                let immutable_hb: &Heartbeat = hb;
                                return Ok(immutable_hb.clone());
                            }
                        }
                    }
                    sleep(Duration::from_millis(5000)).await;
                }
                if counter == 10 {
                    warn!("Could not find matching service");
                }
            }
        }

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
            addr: bind_address.clone(),
            stream,
            client,
            fail_counter: FailCounter::new(),
            msg_body: MsgBody::default(),
        };
        trace!("Adding new connection to queue");
        {
            // push Heartbeat event to event queue
            let mut loc_deque = self.deque.lock().await;
            let _ = loc_deque.push_back(heartbeat.clone());
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
    pub async fn rmv(&mut self, k: u64, id: u64, service_id: u64) -> Result<Message, IoError>{
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
        // Convert the IoError to a HyperError
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "Failed to remove item from state"));
    }

    /// search clients hashmap for an available service with matching key,
    /// set service's service_claim equal to ecpoch time to match the new client
    #[allow(dead_code)]
    pub async fn claim(&mut self, k:u64) -> Result<&mut Payload, u64> {
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
    pub async fn print(&mut self) {
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
pub async fn request_handler(
    state: &Arc<(Mutex<State>, Notify)>, stream: Option<Arc<Mutex<TcpStream>>>, 
    mut request: Option<Request<Incoming>>
) -> Result<Response<Full<Bytes>>, std::io::Error> {
    info!("Starting request handler");

    // receive Payload from incoming connection
    let message = match (&stream, &mut request) {
        (Some(s), None) => receive(&s).await.unwrap(),
        (None, Some(r)) => collect_request(r.body_mut()).await.unwrap(), 
        _ => panic!("Unexpected state: no stream or request.")
    };


    // // check type of Message, only want to handle PUB or CLAIM Message
    let payload = match message.header {
        MessageHeader::HB => panic!("Unexpected HB message encountered!"),
        MessageHeader::ACK => panic!("Unexpected ACK message encountered!"),
        MessageHeader::PUB => deserialize(& message.body),
        MessageHeader::CLAIM => deserialize(& message.body),
        MessageHeader::COL => panic!("Unexpected COL message encountered!"),
        MessageHeader::MSG => Payload::default(),
        MessageHeader::NULL => Payload::default(),
    };

    trace!("Request handler received: {:?}", payload);
    let mut response = Response::new(Full::default());

    // handle services (PUB), clients (CLAIM), messages (MSG) appropriately
    match message.header {
        MessageHeader::PUB => {
            trace!("Publishing Service: {:?}", payload);

            let (lock, notify) = &**state;
            let mut state_loc = lock.lock().await;

            match (stream, request) {
                (Some(s), None) => {
                    let _ = state_loc.add(payload, 0, ComType::TCP).await; // add service to clients hashmap and event loop 
                    state_loc.running = true; // set running to true for event loop
                    notify.notify_one(); // notify event loop of new Event
        
                    println!("Now state:");
                    state_loc.print().await; // print state of clients hashmap
                },
                (None, Some(r)) => {
                    // send acknowledgment of successful request
                    let json = serialize_message( & Message {
                        header: MessageHeader::ACK,
                        body: "".to_string()
                    });
                    response = Response::builder()
                        .status(StatusCode::OK)
                        .header(hyper::header::CONTENT_TYPE, "application/json")
                        .body(Full::new(Bytes::from(json))).unwrap();
                    
                    // add service to clients hashmap and event loop
                    let _ = state_loc.add(payload, 0, ComType::API).await;
                    state_loc.running = true; // set running to true for event loop
                    notify.notify_one(); // notify event loop of new Event

                    // print state of clients hashmap
                    println!("Now state:");
                    state_loc.print().await;
                }, 
                _ => panic!("Unexpected state: no stream or request.")
            };
        },
        MessageHeader::CLAIM => {
            trace!("Claiming Service: {:?}", payload);

            let (lock, notify) = &**state;
            let mut state_loc = lock.lock().await;

            let mut service_id = 0;
            let mut claim_fail = 0; // initiate counter for connection failure
            // loop solves race case when client starts faster than service can be published
            loop {
                println!("{:?}", state_loc.clients.clone());
                match state_loc.claim(payload.key).await{
                    Ok(p) => {
                        println!("found key");
                        service_id = (*p).service_id; // capture claimed service's service_id for add()
                        // send acknowledgment of successful service claim with service's payload containing its address
                        let json = serialize_message( & Message {
                            header: MessageHeader::ACK,
                            body: serialize(p) // address extracted in main to print to client
                        });
                        match (stream, request) {
                            (Some(s), None) => {
                                let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                                service_id = (*p).service_id; // capture claimed service's service_id for add()
                                // send acknowledgment of successful service claim with service's payload containing its address
                                let _ = stream_write(loc_stream, & json).await;
                                let _ = state_loc.add(payload, service_id, ComType::TCP).await;
                            },
                            (None, Some(r)) => {
                                response = Response::builder()
                                    .status(StatusCode::OK)
                                    .header(hyper::header::CONTENT_TYPE, "application/json")
                                    .body(Full::new(Bytes::from(json))).unwrap();
                                let _ = state_loc.add(payload, service_id, ComType::API).await;
                            },
                            _ => panic!("Unexpected state: no stream or request.")
                        }
                        break;
                    },
                    Err(e) => {
                        println!("looking for key, not found: {:?}", e);
                        claim_fail += 1;
                        if claim_fail <= 5{
                            sleep(Duration::from_millis(200)).await;
                            continue;
                        }
                        let json = serialize_message( & Message {
                            header: MessageHeader::NULL,
                            body: "".to_string()
                        });
                        // notify main() of failure to claim an available service
                        println!("bad request");
                        
                        match (stream, request) {
                            (Some(s), None) => {
                                let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                                let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                                    header: MessageHeader::NULL,
                                    body: "".to_string()
                                }));
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput, "Failed to claim key"
                                ).into());
                            },
                            (None, Some(r)) => {
                                response = Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .header(hyper::header::CONTENT_TYPE, "application/json")
                                .body(Full::new(Bytes::from(json))).unwrap();
                            },
                            _ => panic!("Unexpected state: no stream or request.")
                        }
                        return Ok(response);
                    },
                }
            }
            
            println!("Now state:");
            state_loc.print().await; // print state of clients hashmap
        },
        MessageHeader::MSG => {
            info!("adding msg to queue");
            let msg_body: MsgBody = serde_json::from_str(& message.body).unwrap();
            trace!("Sending Message: {:?}", msg_body);

            let (lock, notify) = &**state;
            let mut state_loc = lock.lock().await;

            let mut counter = 0;
            while counter < 10 {
                {
                    let mut deque = state_loc.deque.lock().await;
                    if let Some(hb) = deque.iter_mut().find_map(|e| {
                        if e.id == msg_body.id {
                            println!("found id");
                            Some(e)
                        } else {
                            println!("id not found");
                            None
                        }
                    }) {
                        hb.msg_body = msg_body.clone();
                        trace!("Altering hb message {:?}", hb);
                        let json = serialize_message( & Message {
                            header: MessageHeader::MSG,
                            body: message.body.clone()
                        });
                        match (stream, request) {
                            (None, Some(r)) => {
                                response = Response::builder()
                                .status(StatusCode::OK)
                                .header(hyper::header::CONTENT_TYPE, "application/json")
                                .body(Full::new(Bytes::from(json))).unwrap(); 
                            },
                            _ => {}
                        }
                        break;
                    }
                    else{
                        counter += 1;
                    }
                }
                sleep(Duration::from_millis(1000)).await;
            }
            if counter == 10 {
                warn!("Could not find matching service");
            }

        }
        _ => {panic!("This should not be reached!");}
    }
    Ok(response)
}

pub async fn heartbeat_handler_helper(stream: Option<Arc<Mutex<TcpStream>>>, 
    mut request: Option<Request<Incoming>>, payload: Option<&Arc<Mutex<String>>>, 
    addr: Option<&Arc<Mutex<Addr>>>, tls: Option<ClientConfig>) -> Result<Response<Full<Bytes>>, std::io::Error> {

    let mut response = Response::new(Full::default());

    // retrieve service's payload from client or set an empty message body
    println!("{:?}", payload);
    let binding = Arc::new(Mutex::new("".to_string()));
    let payload = payload.unwrap_or(&binding);
    let payload_clone = payload.lock().await.clone();

    let empty_addr = Arc::new(Mutex::new(Addr{
        host: "".to_string(),
        port: 0
    }));
    // client uses listener's address to send msg, service does not need addr
    let addr = addr.unwrap_or(&empty_addr);
    let addr_clone = addr.lock().await.clone();

    // send/receive heartbeats/messages to/from listener
    // loop for tcp constantly checks for heartbeats in stream
    // one handler call for each http request
    
    response = match (&stream, &mut request) {
        (Some(s), None) => {
            // Shared state to track the last heartbeat time
            let last_heartbeat: Arc<Mutex<Option<Instant>>>= Arc::new(Mutex::new(None));

            // Spawn a thread to monitor the heartbeat
            let last_heartbeat_clone = Arc::clone(&last_heartbeat);
            let _ = tokio::spawn(async move {
                loop {
                    thread::sleep(Duration::from_millis(500));
                    let elapsed = {
                        let timer_loc = last_heartbeat_clone.lock().await;
                        if let Some(time) = *timer_loc {
                            time.elapsed()
                        } else {
                            continue;
                        }
                    };
                    if elapsed > Duration::from_secs(10) {
                        trace!("No heartbeat received for 10 seconds, exiting...");
                        std::process::exit(0);
                    }
                }
            });
            let _ = tokio::spawn(async move {
                loop {
                    match heartbeat_handler(&stream, &mut request, &payload_clone, addr_clone.clone(), None).await {
                        Ok(resp) => {
                            let mut last_heartbeat = last_heartbeat.lock().await;
                            *last_heartbeat = Some(Instant::now());
                            trace!("Heartbeat received, time updated.");
                        },
                        Err(e) => error!("Heartbeat handler returned an error")
                    }
                }
            });
            response
        },
        (None, Some(r)) => {
            response = match heartbeat_handler(&stream, &mut request, &payload_clone, addr_clone, tls).await {
                Ok(resp) => {
                    resp
                },
                Err(e) => {
                    Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Full::new(Bytes::from("Heartbeat handler error".to_string())))
                        .unwrap()
                }
            };
            response
        }, 
        _ => Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from("Heartbeat handler error".to_string())))
                .unwrap()
    };
    // });
    // response = hb_handler.await.unwrap();
    return Ok(response);
}

/// Receive a heartbeat from broker and send one back to show life of connection.
/// If a client, HB message contains payload of connected service to use in Collect()
pub async fn heartbeat_handler(stream: &Option<Arc<Mutex<TcpStream>>>, 
    mut request: &mut Option<Request<Incoming>>, payload: &String,
     addr: Addr, tls: Option<ClientConfig>)
    -> Result<Response<Full<Bytes>>, std::io::Error> {
    trace!("Starting heartbeat handler");

    let message = match (stream, &mut *request) {
        (Some(s), None) => {
            let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
            match stream_read(loc_stream).await {
                Ok(message) => {
                    println!("{:?}", message);
                    deserialize_message(& message)
                },
                Err(ref err) if err.kind() == std::io::ErrorKind::ConnectionReset => {
                    trace!("ConnectionReset error");
                    std::process::exit(0);
                }
                Err(ref err) if err.kind() == std::io::ErrorKind::ConnectionAborted => {
                    trace!("ConnectionAborted error");
                    std::process::exit(0);
                }
                Err(ref err) if err.kind() == std::io::ErrorKind::TimedOut => {
                    trace!("TimeOut error");
                    std::process::exit(0);
                }
                Err(err) => {
                    trace!("Unknown error reading from stream");
                    std::process::exit(0);
                }
            }
        },
        (None, Some(r)) => collect_request((&mut *r).body_mut()).await.unwrap(), 
        _ => panic!("Unexpected state: no stream or request.")
    };
    // set service_id to know which service to send msg to
    let mut service_id = 0;
    println!("{:?}", payload);
    if *payload != "".to_string() {
        service_id = deserialize(payload).service_id;
    }
    println!("{:?}", service_id);
    let timeout_duration = Duration::from_secs(10); // Set timeout to 10 seconds

    // allots time for reading from stream
    let start_time = Instant::now();

    // receive heartbeat/message
    let mut response = Response::new(Full::default());

    // if a MSG: connect to listener to alter the msg value in the service's heartbeat
    trace!("Heartbeat handler received {:?}", message);

    if matches!(message.header, MessageHeader::MSG){
        match (stream, &mut *request) {
            (Some(s), None) => {
                let listen_stream = match connect(& addr).await{
                    Ok(s) => s,
                    Err(_e) => {
                        return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,"MSG connection unsuccessful."));
                        }
                };
                let stream_mut = Arc::new(Mutex::new(listen_stream));
                let msg_body = MsgBody{
                    msg: message.body.clone(),
                    id: service_id
                };
                let ack = send(& stream_mut, & Message{
                    header: MessageHeader::MSG,
                    body: serde_json::to_string(& msg_body).unwrap()
                }).await;
            },
            (None, Some(r)) => {
                // Prepare the HTTPS connector
                let https_connector = match tls.clone() {
                    Some(t) => {
                        HttpsConnectorBuilder::new()
                        .with_tls_config(t)
                        .https_or_http()
                        .enable_http1()
                        .build()
                    }
                    None => {
                        HttpsConnectorBuilder::new()
                        .with_native_roots().unwrap()
                        .https_or_http()
                        .enable_http1()
                        .build()
                    }
                };

                let client: Client<HttpsConnector<HttpConnector>, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(https_connector);

                let msg_body = MsgBody{
                    msg: message.body.clone(),
                    id: service_id
                };
            
                let msg = serialize_message( & Message{
                        header: MessageHeader::MSG,
                        body: serde_json::to_string(& msg_body).unwrap()
                });
                let mut read_fail = 0;
                loop {
                    sleep(Duration::from_millis(1000)).await;
                    trace!("sent message to listener on {:?}", addr);

                    let req = match tls.clone() {
                        Some(t) => {
                            Request::builder()
                            .method(Method::POST)
                            .uri(format!("https://{}:{}/request_handler", addr.host, addr.port))
                            .header(hyper::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(Bytes::from(msg.clone())))
                            .unwrap()
                        }
                        None => {
                            Request::builder()
                            .method(Method::POST)
                            .uri(format!("http://{}:{}/request_handler", addr.host, addr.port))
                            .header(hyper::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(Bytes::from(msg.clone())))
                            .unwrap()
                        }
                    };

                    let start = Instant::now();
                    let result = timeout(timeout_duration, client.request(req)).await;
                
                    match result {
                        Ok(Ok(resp)) => {
                            trace!("Received response: {:?}", resp);
                            let body = resp.collect().await.unwrap().aggregate();
                            let mut data: serde_json::Value = serde_json::from_reader(body.reader()).unwrap();
                            let json = serde_json::to_string(&data).unwrap();
                            let m = deserialize_message(& json);
                            match m.header {
                                MessageHeader::MSG => info!("Request acknowledged: {:?}", m),
                                _ => warn!("Server responds with unexpected message: {:?}", m),
                            }
                            response = Response::builder()
                            .status(StatusCode::OK)
                            .header(hyper::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(Bytes::from(serialize_message(& Message{
                                header: MessageHeader::ACK,
                                body: "".to_string()
                            })))).unwrap();
                            break;
                        }
                        Ok(Err(_e)) => {
                            read_fail += 1;
                            if read_fail > 5 {
                                panic!("Failed to send request to listener")
                            }
                        }
                        Err(_e) => {
                            read_fail += 1;
                            if read_fail > 5 {
                                panic!("Request timed out")
                            }
                        }
                    };
                }
                response = Response::builder()
                .status(StatusCode::OK)
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(serialize_message(& Message{
                    header: MessageHeader::ACK,
                    body: message.body.clone()
                })))).unwrap();
            },
            _ => panic!("Unexpected state: no stream or request.")
        }
    }
    else if matches!(message.header, MessageHeader::COL) {
        // payload is empty for services, retrieve and send msg waiting in global variable
        if *payload == "".to_string(){
            // send msg back
            let msg = GLOBAL_MSGBODY.lock().await;
            let _ = match (&stream, &mut *request) {
                (Some(s), None) => {
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: serde_json::to_string(&* msg.msg).unwrap()
                    })).await;  
                },
                (None, Some(r)) => {
                    response = Response::builder()
                    .status(StatusCode::OK)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& Message{
                        header: MessageHeader::ACK,
                        body: serde_json::to_string(&* msg.msg).unwrap()
                    })))).unwrap()
                }, 
                _ => panic!("Unexpected state: no stream or request.")
            };
        }
        else {
            // payload not empty for clients, send payload w/ address of the paired service
            let _ = match (&stream, &mut *request) {
                (Some(s), None) => {
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: payload.clone()
                    })).await;
                },
                (None, Some(r)) => {
                    response = Response::builder()
                    .status(StatusCode::OK)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& Message{
                        header: MessageHeader::ACK,
                        body: payload.clone()
                    })))).unwrap();
                }, 
                _ => panic!("Unexpected state: no stream or request.")
            };
        }
        trace!("Heartbeat handler has returned request");
    }
    else if matches!(message.header, MessageHeader::HB){
        println!("{:?}", message.clone());
        let msg_body: MsgBody = serde_json::from_str(& message.body.clone()).unwrap();
        // default HB/MSG response to send what was received
        if msg_body.msg.is_empty(){
            let _ = match (&stream, &mut *request) {
                (Some(s), None) => {
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: payload.clone(),
                    })).await;
                },
                (None, Some(r)) => {
                    response = Response::builder()
                    .status(StatusCode::OK)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& Message{
                        header: message.header,
                        body: payload.clone()
                    })))).unwrap();
                }, 
                _ => panic!("Unexpected state: no stream or request.")
            };
        }
        else{
            // store msg received from listener into global msg variable
            let mut msg = GLOBAL_MSGBODY.lock().await;
            *msg = serde_json::from_str(& message.body).unwrap();
            let _ = match (&stream, &mut *request) {
                (Some(s), None) => {
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: message.body
                    })).await;
                },
                (None, Some(r)) => {
                    response = Response::builder()
                    .status(StatusCode::OK)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serialize_message(& Message{
                        header: message.header,
                        body: message.body
                    })))).unwrap();
                }, 
                _ => panic!("Unexpected state: no stream or request.")
            };
        }   
        trace!("Heartbeat handler has returned request");
    }
    else {
        warn!(
            "Unexpected request sent to heartbeat_handler: {}",
            message.header
        );
        info!("Dropping request");
        *response.status_mut() = StatusCode::BAD_REQUEST;
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_logger() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    /// check if function returns an error when a client claims an unused key
    #[test]
    fn test_claim_key_DNE() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _notify) = &*state;
        
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
            let mut state_loc = lock.lock().await;
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
            let mut state_loc = lock.lock().await;
            let _ = state_loc.claim(k.floor() as u64);
        }
        // try to claim a key that does not exist in State
        let mut state_loc = lock.lock().await;
        let result = state_loc.claim(2000);
        assert!(result.is_err());
    }

    /// check if function returns an error when a client claims a used key with no more available services
    #[test]
    fn test_claim_filled_services() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _notify) = &*state;
        
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
            let mut state_loc = lock.lock().await;
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
            let mut state_loc = lock.lock().await;
            let _ = state_loc.claim(k.floor() as u64);
        }
        // try to claim existing key without available service
        let mut state_loc = lock.lock().await;
        let result = state_loc.claim(1000);
        assert!(result.is_err());
    }

    /// Test adding services and clients to State
    #[test]
    fn test_add() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _notify) = &*state;

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
            let mut state_loc = lock.lock().await;
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
            let mut state_loc = lock.lock().await;
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
        let (lock, _notify) = &*state;

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
            let mut state_loc = lock.lock().await;
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
            let mut state_loc = lock.lock().await;
            let mut service_id = 0;
            match state_loc.claim(p.key){
                Ok(pl) => service_id = (*pl).service_id,
                _ => panic!("Error: Failed to claim key")
            };
            state_loc.add(p, service_id);
        }
        {
            // State contains all services and clients
            let mut state_loc = lock.lock().await;
            state_loc.print();
        }
        // remove all clients from State
        for x in 1..21{
            let mut state_loc = lock.lock().await;
            let k: f64 = (1000 + x/2) as f64;
            // id = x+20 : services' ids are 1-21
            // service_id = x : client's service_ids match services' ids
            let result = state_loc.rmv(k.floor() as u64, x + 20, x);
            assert!(result.is_ok());
        }
        {
            // State only contains clients
            let mut state_loc = lock.lock().await;
            state_loc.print();
        }
    }
    
    // test if rmv() properly removes services
    #[test]
    fn test_rmv_service() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _notify) = &*state;

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
            let mut state_loc = lock.lock().await;
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
            let mut state_loc = lock.lock().await;
            let mut service_id = 0;
            match state_loc.claim(p.key){
                Ok(pl) => service_id = (*pl).service_id,
                _ => panic!("Error: Failed to claim key")
            };
            state_loc.add(p, service_id);
        }
        {
            // State contains all services and clients
            let mut state_loc = lock.lock().await;
            state_loc.print();
        }
        // remove all services from State
        for x in 1..21{
            let mut state_loc = lock.lock().await;
            let k: f64 = (1000 + x/2) as f64;
            // id = x: services' ids are 1-21
            // service_id = x : id == service_id
            let result = state_loc.rmv(k.floor() as u64, x, x);
            assert!(result.is_ok());
        }
        {
            // State only contains clients
            let mut state_loc = lock.lock().await;
            state_loc.print();
        }
    }

    // test if rmv() properly removes clients and services
    #[test]
    fn test_rmv_clients_and_services() {

        let state = Arc::new((Mutex::new(State::new()), Condvar::new()));
        let (lock, _notify) = &*state;

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
            let mut state_loc = lock.lock().await;
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
            let mut state_loc = lock.lock().await;
            let mut service_id = 0;
            match state_loc.claim(p.key){
                Ok(pl) => service_id = (*pl).service_id,
                _ => panic!("Error: Failed to claim key")
            };
            state_loc.add(p, service_id);
        }
        {
            // State contains all services and clients
            let mut state_loc = lock.lock().await;
            state_loc.print();
        }
        // remove all services from State
        for x in 1..21{
            let mut state_loc = lock.lock().await;
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
            let mut state_loc = lock.lock().await;
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
                        let _ = heartbeat_handler_helper(&stream_clone, None, None);
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

        init_logger();

        let publisher = TcpListener::bind("127.0.0.1:17010").unwrap();
        sleep(Duration::from_millis(500));
        let mut stream = TcpStream::connect("127.0.0.1:17010").unwrap();

        let _ = stream_write(&mut stream, & serialize_message(
            & Message {
                header: MessageHeader::HB,
                body: serde_json::to_string(&MsgBody::default()).unwrap()
            }
        ));
        for s in publisher.incoming(){
            match s {
                Ok(s) => {
                    let shared_stream = Arc::new(Mutex::new(s));
                    let stream_clone = Arc::clone(&shared_stream);
                    let thread_handler = thread::spawn(move || {
                        let _ = heartbeat_handler_helper(&stream_clone, None, None);
                    });
                    sleep(Duration::from_millis(1000));
                    let failure_duration = Duration::from_secs(5);
                    match stream.set_read_timeout(Some(failure_duration)) {
                        Ok(_x) => trace!("set_read_timeout OK"),
                        Err(_e) => trace!("set_read_timeout Error")
                    }
                    let message = match stream_read(&mut stream) {
                        Ok(m) => deserialize_message(& m),
                        Err(err) => panic!("server dropped claim")
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

        /// test if faulty connection restarts 
        #[test]
        fn test_connect_with_retry() {
            init_logger();

            let state = Arc::new((Mutex::new(State::new()), Condvar::new()));

            sleep(Duration::from_millis(500));
            let publisher = TcpListener::bind("127.0.0.1:18000").unwrap();
            sleep(Duration::from_millis(500));
            let mut stream = TcpStream::connect("127.0.0.1:18000").unwrap();

            let shared_stream = Arc::new(Mutex::new(stream));
            let hb = Heartbeat {
                key: 1000,
                id: 1,
                service_id: 1,
                addr: "127.0.0.1:18000".to_string(),
                stream: Arc::clone(&shared_stream),
                fail_counter: FailCounter::new(),
                msg_body: MsgBody::default(),
            };
            let mut loc_stream = hb.stream.lock().await;
            let _ = stream_write(&mut loc_stream, & serialize_message(
                & Message {
                    header: MessageHeader::HB,
                    body: serde_json::to_string(&MsgBody::default()).unwrap()
                }
            ));
    
            let thread_handler = thread::spawn(move || {
                for s in publisher.incoming(){
                    match s {
                        Ok(s) => {
                            trace!("new connection");
                            let shared_s = Arc::new(Mutex::new(s));
                            let mut loc_s = shared_s.lock().await;
                            let response_a = match stream_read(&mut loc_s) {
                                Ok(m) => deserialize_message(& m),
                                Err(err) => panic!("failed to read from stream")
                            };
                            trace!("Received on stream : {:?}", response_a);

                            sleep(Duration::from_millis(5000));
                        }
                        Err(e) => {
                            panic!("Error: {}", e);
                        }
                    }
                }
            });

            drop(shared_stream);
            *loc_stream = connect_with_retry("127.0.0.1:18000".to_string()).unwrap();

            trace!("writing to same stream");
            let _ = stream_write(&mut loc_stream, & serialize_message(
                & Message {
                    header: MessageHeader::HB,
                    body: serde_json::to_string(&MsgBody::default()).unwrap()
                }
            ));

            sleep(Duration::from_millis(30000));

        }
}