/// Track clients and services and handle events (heartbeats)

use std::collections::HashMap;
use std::sync::Arc;
use std::collections::VecDeque;
use std::io::Error as IoError;
use std::io::ErrorKind::{ConnectionReset, ConnectionAborted, TimedOut};
use lazy_static::lazy_static;
use hyper::body::{Buf, Bytes, Incoming};
use hyper::http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioExecutor;
use hyper_util::client::legacy::Client;
use hyper_rustls::{HttpsConnectorBuilder, HttpsConnector};
use hyper_util::client::legacy::connect::HttpConnector;
use tokio::sync::Mutex;
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::{sleep, Instant, timeout, Duration};
use rustls::ClientConfig;
use serde::{Serialize, Deserialize};


#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::utils::{only_or_error, epoch};
use crate::connection::{MessageHeader, Message, Addr, ComType, connect, send, receive,
     serialize_message, deserialize_message, collect_request, stream_write, stream_read};
use crate::tls::{setup_https_client};

/// Store client or service metadata
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Payload {
    /// local IP address
    pub service_addr: Vec<String>,
    /// used for service-client data connection
    pub service_port: i32,
    /// epoch time of when service was claimed
    pub service_claim: u64,
    pub interface_addr: Vec<String>,
    /// used for sending/receiving heartbeats
    pub bind_port: i32,
    /// unique key identifies system
    pub key: u64,
    /// unique identification of each connection to broker
    pub id: u64,
    /// identifies unique service, clients assume id from connected service
    pub service_id: u64,
    /// path to root store
    pub root_ca: Option<String>,
    /// one or two sided heartbeat
    pub ping: bool,
}

/// Store message contents to send to connected service
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct MsgBody{
    /// message to send to service
    pub msg: String,
    /// identifies unique connection
    pub id: u64,
    /// identifies unique connection
    pub service_id: u64
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
            interval: Duration::from_secs(5),
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
    /// same service_id, used to trace and reassigned a client whose service
    /// dropped from system
    pub service_id: u64,
    /// address of service/client
    pub addr: String,
    /// stream used to send heartbeats to client
    pub stream: Option<Arc<Mutex<TcpStream>>>,
    /// client object for sending heartbeat requests
    pub client: Option<Client<HttpsConnector<HttpConnector>, Full<Bytes>>>,
    /// boolean representing tls activity
    pub tls: bool,
    /// incremented when heartbeat is not received when expected, connection
    /// is dead at 10
    pub fail_counter: FailCounter,
    /// message sent from client, can access using collect()
    pub msg_body: MsgBody,
    /// one- or two-sided heartbeat
    pub ping: bool,
}

impl Heartbeat {
    /// send a heartbeat to the service/client and check if entity sent one
    /// back, increment fail_count if heartbeat not received when expected
    async fn monitor(&mut self) -> Result<Response<Full<Bytes>>, IoError> {

        let mut response = Response::new(Full::default());
        *response.status_mut() = StatusCode::OK;

        // check ping status
        if false == self.ping {
            trace!(
                "HB in two-sided mode. Sending HB containing msg: {}", 
                self.msg_body.msg
            );
    
            let message = serialize_message(& Message{
                header: MessageHeader::HB,
                body: serde_json::to_string(& self.msg_body.clone()).unwrap()
            });

            // enter tcp or api mode
            let received = match (&self.stream, &self.client){
                (Some(s), None) => {

                    trace!("TCP stream present => Sending HB over TCP.");

                    let mut loc_stream = s.lock().await;
                    
                    // send heartbeat
                    let _ = stream_write(
                        &mut loc_stream,
                        &serialize_message(
                            &Message {
                                header: MessageHeader::HB,
                                body: serde_json::to_string(
                                    &self.msg_body.clone()).unwrap()
                            }
                        )
                    ).await;
                
                    // allots time for reading from stream
                    let received = match stream_read(&mut loc_stream).await {
                        Ok(message) => message,
                        Err(ref err) if err.kind() == ConnectionReset => {
                            self.fail_counter.increment();
                            trace!("ConnectionReset error");
                            *response.status_mut() = StatusCode::BAD_REQUEST;
                            return Ok(response); 
                        }
                        Err(ref err) if err.kind() == ConnectionAborted => {
                            self.fail_counter.increment();
                            trace!("ConnectionAborted error");
                            *response.status_mut() = StatusCode::BAD_REQUEST;
                            return Ok(response); 
                        }
                        Err(ref err) if err.kind() == TimedOut => {
                            self.fail_counter.increment();
                            trace!("TimeOut error");
                            *response.status_mut() = StatusCode::REQUEST_TIMEOUT;
                            return Ok(response); 
                        }
                        Err(_err) => {
                            // open connection, read timed out
                            if self.fail_counter.fail_count == 0 {
                                self.fail_counter.first_increment = Instant::now();
                            }
                            self.fail_counter.increment();
                            trace!(
                                "Timed out reading from stream. {:?}",
                                self.fail_counter.fail_count
                            );
                            *response.status_mut() = StatusCode::REQUEST_TIMEOUT;
                            return Ok(response); 
                        }
                    };
                    received
                },
                (None, Some(c)) => {

                    trace!("TCP stream present => Sending HB over TCP.");

                    // send heartbeat request with/without tls
                    let req = match self.tls{
                        true => Request::builder()
                        .method(Method::GET)
                        .uri(format!("https://{}/heartbeat_handler", self.addr))
                        .header(hyper::header::CONTENT_TYPE, "application/json")
                        .body(Full::new(Bytes::from(message)))
                        .unwrap(),
                        false => Request::builder()
                        .method(Method::GET)
                        .uri(format!("http://{}/heartbeat_handler", self.addr))
                        .header(hyper::header::CONTENT_TYPE, "application/json")
                        .body(Full::new(Bytes::from(message)))
                        .unwrap(),
                    };
                    
                    // allots time for reading response
                    let timeout_duration = Duration::from_secs(3);
                    let received = match timeout(timeout_duration, c.request(req)).await {
                        Ok(Ok(mut resp)) => {
                            trace!("Received response: {:?}", resp);
                            let msg = collect_request(resp.body_mut()).await.unwrap();
                            serialize_message(&msg)
                        }
                        Ok(Err(_e)) => {
                            if self.fail_counter.fail_count == 0 {
                                self.fail_counter.first_increment = Instant::now();
                            }
                            self.fail_counter.increment();
                            warn!(
                                "Timed out reading from stream. {:?}",
                                self.fail_counter.fail_count
                            );
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
        
            // check for valid response
            if received == "" {
                // broken connection, increment fail count
                if self.fail_counter.fail_count == 0 {
                    self.fail_counter.first_increment = Instant::now();
                }
                self.fail_counter.increment();
                trace!("Failed to receive HB. {:?}", self.fail_counter.fail_count);
                *response.status_mut() = StatusCode::BAD_REQUEST
            } else { 
                // connection is alive, reset fail count   
                trace!("Received: {:?}", received);
                self.fail_counter.fail_count = 0;
                trace!("Resetting failcount. {}", self.fail_counter.fail_count);
                *response.status_mut() = StatusCode::OK
            }
        } else {
            // check timing of last heartbeat ping
            if Instant::now() - self.fail_counter.last_increment > Duration::from_secs(60) {
                *response.status_mut() = StatusCode::GONE;
            }            
            else {
                *response.status_mut() = StatusCode::ACCEPTED
            }
        }
        Ok(response)
    }
}

/// Function loops through State's deque (event queue) to handle events using
/// multithreading 
pub async fn event_monitor(
        state: Arc<(Mutex<State>, Notify)>
    )->Result<Response<Full<Bytes>>, IoError> {

    trace!("Starting event monitor");
    let (lock, notify) = &*state;

    let deque_clone = {
        let state_loc = lock.lock().await;
        Arc::clone(& state_loc.deque)
    };

    // (fail_count, key, id, service_id, fail_id)
    let data = Arc::new(Mutex::new((0, 0, 0, 0, 0))); 
    let mut service_id: i64 = -1;

    // loop runs while events are in the queue
    loop {
        {
            let mut state_loc = lock.lock().await;
            // loop is paused while there are no events to handle
            while !state_loc.running {
                // trace!("waiting to run");
                let notify_result = timeout(
                    Duration::from_millis(200), notify.notified()
                ).await;
                match notify_result {
                    Ok(()) => {
                        //trace!("Processing next events");
                    }
                    Err(_) => {
                        // Timeout occurred, modify the state
                        // trace!("Timeout occurred, modifying running state...");
                        state_loc.running = true;
                        notify.notify_one();
                    }
                }
            }
        }

        let mut shared_data = data.lock().await;
        // if connection is dead, remove it
        // check if the fail_count has reached 10 or if a ping has failed
        if shared_data.0 == 10 || shared_data.4 != 0{
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
                    warn!("Failed to remove item from vec");
                }
            }
            // reset fail_count and fail_id trackers
            shared_data.0 = 0;
            shared_data.4 = 0;
        }
        {
            // reset a published service's service_claim once its client has been removed
            let mut state_loc = lock.lock().await;
            if let Some(vec) = state_loc.clients.get_mut(&shared_data.1) {
                if let Some(pos) = vec.iter().position(
                    |item| item.service_id == shared_data.4 as u64
                        && item.service_id == item.id
                    ) { if let Some(publish) = vec.get_mut(pos) {
                        trace!("Client disconnected. Changing service_claim");
                        publish.service_claim = 0;
                    }
                }
            }
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
            // send/receive heartbeat for popped item
            let response = hb.monitor().await;
            // check status of heartbeat
            let fail_id: i64 = match response {
                Ok(resp) => {
                    let status = resp.status();
                    if status == StatusCode::ACCEPTED {
                        trace!("ping is alive");
                        0
                    }
                    else if status != StatusCode::OK {
                        trace!("monitor status not ok");
                        hb.service_id as i64
                    }
                    else {
                        trace!("monitor status is ok");
                        0
                    }
                }
                Err(e) => {
                    trace!("Error occurred in monitor: {}", e);
                    hb.service_id as i64
                }
            };

            // check heartbeat metadata to see if entity should be added back
            // to event queue or if client should claim a new service
            let mut data = data_clone.lock().await;
            *data = (
                hb.fail_counter.fail_count,
                hb.key,
                hb.id,
                hb.service_id,
                fail_id as i64
            );
            if data.0 < 10 && data.4 == 0 {
                trace!(
                    "Adding back to VecDeque: id: {:?}, fail_count: {:?}",
                    data.2, data.0
                );
                if hb.service_id == (service_id as u64){
                    trace!("Connecting to new service");
                    match state_clone.claim(hb.key).await{
                        Ok(p) => {
                            trace!("Claimed new published service w/ payload: {:?}", p);
                            // create new Heartbeat object with new service_id
                            hb = Heartbeat {
                                key: hb.key,
                                id: hb.id,
                                service_id: hb.service_id,
                                addr: hb.addr.clone(),
                                stream: hb.stream.clone(),
                                client: hb.client.clone(),
                                tls: hb.tls.clone(),
                                fail_counter: FailCounter::new(),
                                msg_body: hb.msg_body.clone(),
                                ping: hb.ping.clone(),
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
                    let _ = loc_deque.push_back(hb);
                    trace!("Deque status {:?}", loc_deque);
                }
            } else {
                // service or client no longer connected to broker
                info!("Dropping event");
            }
        });
        sleep(Duration::from_millis(200)).await;
    }
}

/// Keep track of all connected clients/services, holds event loop and threadpool
#[derive(Clone, Debug)]
pub struct State {
    /// hashmap - key: key, value: clients/services payloads
    pub clients: HashMap<u64, Vec<Payload>>,
    pub timeout: u64, 
    /// increments for each new connection to broker, used for id in payload
    pub seq: u64,
    /// event queue
    pub deque: Arc<Mutex<VecDeque<Heartbeat>>>,
    /// true when event queue contains events to handle, false when event queue is empty
    pub running: bool,
}


impl State {
    /// inititate State struct with empty and default variables
    pub fn new(_tls: Option<ClientConfig>) -> State { // TODO: Should use default constructor instead?
        State{
            clients: HashMap::new(),
            timeout: 60,
            seq: 1,
            deque: Arc::new(Mutex::new(VecDeque::new())),
            running: false,
        }
    }

    /// adds new services/clients to State struct and creates Event object to
    /// add to event loop 
    pub async fn add(&mut self, mut p: Payload, service_id: u64, com: ComType) -> Result<Heartbeat, std::io::Error>{
        let ipstr = only_or_error(&p.service_addr);
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
                            let _ = sleep(Duration::from_millis(1000));
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
                let client = setup_https_client(p.root_ca.clone()).await;
                (None, Some(client))
            }
        };

        let tls = match p.root_ca{
            Some(ref _t) => true,
            None => false
        };

        // check if item already exists in the queue when trying to reconnect from poor connection
        if let Some(vec) = self.clients.get_mut(&p.key) {
            // find value in clients with matching id
            if let Some(pos) = vec.iter().position(|item| item.service_addr == p.service_addr
                 && item.service_port == p.service_port) {
                let item = &vec[pos];
                let mut counter = 0;
                while counter < 10 {
                    {
                        let mut deque_loc = self.deque.lock().await;
                        trace!("searching for item {:?}", deque_loc);
                        if let Some(hb) = deque_loc.iter_mut().find_map(|e| {
                            if e.id == item.id {
                                trace!("During 'add': found id {}", e.id);
                                Some(e)
                            } else {
                                counter+=1;
                                None
                            }
                        }) {
                            // alter heartbeat with new metadata
                            if Instant::now().duration_since(hb.fail_counter.first_increment) < Duration::from_secs(60){
                                hb.addr = bind_address.clone();
                                hb.stream = stream;
                                hb.client = client;
                                hb.tls = tls;
                                hb.ping = item.ping;
                                trace!("Altering hb {:?}", hb);
                                p.id = item.id;
                                p.service_id = item.service_id;
                                let immutable_hb: &Heartbeat = hb;
                                return Ok(immutable_hb.clone());
                            }
                        } else {
                            debug!(
                                "During 'add': could not find service {}", 
                                item.id
                            );
                        }
                    }
                }
                if counter == 10 {
                    warn!("'add' giving up after {} attempts", counter);
                }
            }
        }

        let mut temp_id = self.seq;
        // service_id is same as id for a service - unclaimed service starts with service_id = 0
        // client inherits its service's id for service_id
        if service_id != 0{
            temp_id = service_id;
        }
        // create new heartbeat with metadata from payload
        let heartbeat = Heartbeat {
            key: p.key,
            id: self.seq,
            service_id: temp_id,
            addr: bind_address.clone(),
            stream,
            client,
            tls,
            fail_counter: FailCounter::new(),
            msg_body: MsgBody::default(),
            ping: p.ping
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

// TODO: use temples rather than matching input args to discriminate between the
// http and tcp paths.

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
        MessageHeader::HB => Payload::default(),
        MessageHeader::ACK => panic!("Unexpected ACK message encountered!"),
        MessageHeader::PUB => deserialize(& message.body),
        MessageHeader::CLAIM => deserialize(& message.body),
        MessageHeader::COL => panic!("Unexpected COL message encountered!"),
        MessageHeader::MSG => Payload::default(),
        MessageHeader::NULL => panic!("Unexpected NULL message encountered!"),
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
                (Some(_s), None) => {
                    let _ = state_loc.add(payload, 0, ComType::TCP).await; // add service to clients hashmap and event loop 
                    state_loc.running = true; // set running to true for event loop
                    notify.notify_one(); // notify event loop of new Event
        
                    println!("Now state:");
                    state_loc.print().await; // print state of clients hashmap
                },
                (None, Some(_r)) => {
                    // send acknowledgment of successful request
                    let json = serialize_message( & Message {
                        header: MessageHeader::ACK,
                        body: state_loc.seq.to_string() // address extracted in main to print to client
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

            let (lock, _notify) = &**state;

            // let mut service_id = 0;
            let mut claim_fail = 0; // initiate counter for connection failure

            // loop solves race case when client starts faster than service can be published
            loop {
                let mut state_loc = lock.lock().await;
                match state_loc.claim(payload.key).await{
                    Ok(p) => {
                        trace!("found key");
                        let mut service_id = (*p).service_id; // capture claimed service's service_id for add()
                        // send acknowledgment of successful service claim with service's payload containing its address
                        let json = serialize_message( & Message {
                            header: MessageHeader::ACK,
                            body: serialize(p) // address extracted in main to print to client
                        });
                        match (stream, request) {
                            (Some(s), None) => {
                                let loc_stream: &mut TcpStream = &mut *s.lock().await;
                                service_id = (*p).service_id; // capture claimed service's service_id for add()
                                // send acknowledgment of successful service claim with service's payload containing its address
                                let _ = stream_write(loc_stream, & json).await;
                                let _ = state_loc.add(payload, service_id, ComType::TCP).await;
                            },
                            (None, Some(_r)) => {
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
                        trace!("looking for key, not found: {:?}", e);
                        claim_fail += 1;
                        if claim_fail <= 5{
                            sleep(Duration::from_millis(300)).await;
                            continue;
                        }
                        let json = serialize_message( & Message {
                            header: MessageHeader::NULL,
                            body: "".to_string()
                        });
                        // notify main() of failure to claim an available service
                        
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
                            (None, Some(_r)) => {
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
            let mut state_loc = lock.lock().await;
            state_loc.print().await; // print state of clients hashmap
        },
        MessageHeader::MSG => {
            let msg_body: MsgBody = serde_json::from_str(& message.body).unwrap();
            trace!("Sending Message: {:?}", msg_body);

            let (lock, _notify) = &**state;

            let mut counter = 0;
            // retry and check queue for heartbeat with a matching id
            // alter message in heartbeat with new message
            while counter < 50 {
                {
                    let state_loc = lock.lock().await;
                    let mut deque = state_loc.deque.lock().await;
                    trace!("deque status {:?}", deque);
                    if let Some(hb) = deque.iter_mut().find_map(|e| {
                        if e.id == msg_body.id {
                            trace!("During 'MSG': found id {}", e.id);
                            Some(e)
                        } else {
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
                            (None, Some(_r)) => {
                                response = Response::builder()
                                .status(StatusCode::OK)
                                .header(hyper::header::CONTENT_TYPE, "application/json")
                                .body(Full::new(Bytes::from(json))).unwrap(); 
                            },
                            _ => {}
                        }
                        break;
                    } else {
                        debug!(
                            "During 'MSG': could not find service {}",
                            msg_body.id
                        );
                        counter += 1;
                    }
                }
                sleep(Duration::from_millis(300)).await;
            }
            if counter == 50 {
                warn!("'MSG' giving up after {} attempts", counter);
            }
        },
        MessageHeader::HB => {
            let hb_body: MsgBody = serde_json::from_str(&message.body).unwrap();
            trace!("Processing ping heartbeat: {:?}", hb_body);

            let (lock, _notify) = &**state;

            let mut counter = 0;
            // retry and check queue for heartbeat with a matching id
            // reset last heartbeat timer for ping heartbeats
            while counter < 50 {
                {
                    let state_loc = lock.lock().await;
                    let mut deque = state_loc.deque.lock().await;
                    trace!("searching deque {:?}", deque);
                    // only alter a client's heartbeat
                    if let Some(hb) = deque.iter_mut().find_map(|e| {
                        if e.service_id == hb_body.service_id && e.id == hb_body.id {
                            trace!(
                                "During 'HB': found service ({}, {})",
                                e.service_id, e.id
                            );
                            Some(e)
                        } else {
                            trace!(
                                "During 'HB': id {} != {}, {} != {}",
                                e.service_id, hb_body.service_id,
                                e.id, hb_body.id
                            );
                            None
                        }
                    }) {
                        // reset hearbeat timer 
                        info!("altering last heartbeat: {:?}", hb);
                        hb.fail_counter.last_increment = Instant::now();
                        let json = serialize_message( & Message {
                            header: MessageHeader::HB,
                            body: message.body.clone()
                        });
                        match (stream, request) {
                            (None, Some(_r)) => {
                                response = Response::builder()
                                .status(StatusCode::OK)
                                .header(hyper::header::CONTENT_TYPE, "application/json")
                                .body(Full::new(Bytes::from(json))).unwrap(); 
                            },
                            _ => {}
                        }
                        break;
                    } else{
                        trace!(
                            "During 'HB': could not find service({}, {})",
                            hb_body.service_id, hb_body.id
                        );
                        counter += 1;
                    }
                }
                sleep(Duration::from_millis(702)).await;
            }
            if counter == 50 {
                warn!("'HB' giving up after {} attempts", counter);
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
    let binding = Arc::new(Mutex::new("".to_string()));
    let payload = payload.unwrap_or(&binding);
    let payload_clone = payload.lock().await.clone();

    let empty_addr = Arc::new(Mutex::new(Addr::new(&"".to_string(), 0)));
    // client uses listener's address to send msg, service does not need addr
    let addr = addr.unwrap_or(&empty_addr);
    let addr_clone = addr.lock().await.clone();

    // send/receive heartbeats/messages to/from listener
    // loop for tcp constantly checks for heartbeats in stream
    // one handler call for each http request
    
    // enter tcp or api server
    response = match (&stream, &mut request) {
        (Some(_s), None) => {
            let _ = tokio::spawn(async move {
                // infinite heartbeat loop for tcp connection
                loop {
                    match heartbeat_handler(&stream, &mut request, &payload_clone, addr_clone.clone(), None).await {
                        Ok(_resp) => {
                            trace!("Heartbeat received, time updated.");
                        },
                        Err(_e) => error!("Heartbeat handler returned an error")
                    }
                }
            });
            response
        },
        (None, Some(_r)) => {
            // enter heartbeat handler once for every api request
            response = match heartbeat_handler(&stream, &mut request, &payload_clone, addr_clone, tls).await {
                Ok(resp) => {
                    resp
                },
                Err(_e) => {
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

/// heartbeat handler for one-sided heartbeats
pub async fn ping_heartbeat(
        payload: &Arc<Mutex<String>>, 
        address: Option<&Arc<Mutex<Addr>>>,
        tls: Option<ClientConfig>
    )->Result<Response<Full<Bytes>>, std::io::Error> {

    sleep(Duration::from_millis(2000)).await;
    // let mut response = Response::new(Full::default());

    // retrieve service's payload from client or set an empty message body
    let payload_loc = payload.lock().await.clone();

    let empty_addr = Arc::new(Mutex::new(Addr::new(&"".to_string(),0)));
    // client uses listener's address to send msg, service does not need addr
    let addr = address.unwrap_or(&empty_addr);
    let addr_loc = addr.lock().await.clone();

    info!("Initiated service sending pings to {:?}", addr_loc.clone());

    // use service id to identify client
    let mut service_id = 0;
    let mut id = 0;
    if *payload_loc != "".to_string() {
        service_id = deserialize(&payload_loc).service_id;
        id = deserialize(&payload_loc).id;
    }
    let timeout_duration = Duration::from_secs(10); // Set timeout to 10 seconds

    // setup http connection with/without tls
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
    let client: Client<HttpsConnector<HttpConnector>, Full<Bytes>> = Client::builder(
        TokioExecutor::new()
    ).build(https_connector);

    let msg_body = MsgBody{
        msg: "".to_string(),
        id,
        service_id
    };

    let msg = serialize_message( & Message{
            header: MessageHeader::HB,
            body: serde_json::to_string(& msg_body).unwrap()
    });
    tokio::spawn( async move {
        // send heartbeats to broker as long as it is alive
        loop {
            let mut read_fail = 0;
            loop {
                sleep(Duration::from_millis(1000)).await;
                trace!("ping_heartbeat: Sent message to listener on {:?}", addr_loc);
        
                let req = match tls.clone() {
                    Some(_t) => {
                        Request::builder()
                        .method(Method::POST)
                        .uri(format!(
                                "https://{}:{}/request_handler",
                                addr_loc.host, addr_loc.port
                        ))
                        .header(hyper::header::CONTENT_TYPE, "application/json")
                        .body(Full::new(Bytes::from(msg.clone())))
                        .unwrap()
                    }
                    None => {
                        Request::builder()
                        .method(Method::POST)
                        .uri(format!(
                                "http://{}:{}/request_handler",
                                addr_loc.host, addr_loc.port
                        ))
                        .header(hyper::header::CONTENT_TYPE, "application/json")
                        .body(Full::new(Bytes::from(msg.clone())))
                        .unwrap()
                    }
                };

                let result = timeout(timeout_duration, client.request(req)).await;
                
                // check for valid response, quit process if broker is dead
                match result {
                    Ok(Ok(resp)) => {
                        trace!("Received response: {:?}", resp);
                        let body = resp.collect().await.unwrap().aggregate();
                        let data: serde_json::Value = serde_json::from_reader(
                            body.reader()
                        ).unwrap();
                        let json = serde_json::to_string(&data).unwrap();
                        let m = deserialize_message(& json);
                        match m.header {
                            MessageHeader::HB => info!("Request acknowledged: {:?}", m),
                            _ => warn!("Server responds with unexpected message: {:?}", m),
                        }
                        break;
                    }
                    Ok(Err(e)) => {
                        read_fail += 1;
                        warn!("Failed to send request to listener: {}", e);
                        if read_fail > 10 {
                            warn!("Shutting down pings");
                            std::process::exit(0); // TODO: Don't exist proc insitu
                        }
                    }
                    Err(e) => {
                        read_fail += 1;
                        warn!("Failed to send request to listener: {}", e);
                        if read_fail > 10 {
                            warn!("Request timed out");
                            std::process::exit(0); // TODO: Don't exist proc insitu
                        }
                    }
                }
            }
            let _ = sleep(Duration::from_millis(10000));
        }
    });
    let response = Response::builder()
    .status(StatusCode::OK)
    .header(hyper::header::CONTENT_TYPE, "application/json")
    .body(Full::new(Bytes::from(serialize_message(& Message{
        header: MessageHeader::ACK,
        body: "".to_string()
    })))).unwrap();
    Ok(response)
}

/// Receive a heartbeat from broker and send one back to show life of connection.
/// If a client, HB message contains payload of connected service to use in Collect()
pub async fn heartbeat_handler(stream: &Option<Arc<Mutex<TcpStream>>>, 
    request: &mut Option<Request<Incoming>>, payload: &String,
     addr: Addr, tls: Option<ClientConfig>)
    -> Result<Response<Full<Bytes>>, std::io::Error> {
    trace!("Starting heartbeat handler on {}", addr);

    // enter tcp or api mode
    // receive message from broker
    let message = match (stream, &mut *request) {
        (Some(s), None) => {
            let loc_stream: &mut TcpStream = &mut *s.lock().await;
            match stream_read(loc_stream).await {
                Ok(message) => {
                    trace!("{:?}", message);
                    deserialize_message(& message)
                },
                Err(ref err) if err.kind() == std::io::ErrorKind::ConnectionReset => {
                    trace!("ConnectionReset error");
                    std::process::exit(0); // TODO: Don't exist proc insitu
                }
                Err(ref err) if err.kind() == std::io::ErrorKind::ConnectionAborted => {
                    trace!("ConnectionAborted error");
                    std::process::exit(0); // TODO: Don't exist proc insitu
                }
                Err(ref err) if err.kind() == std::io::ErrorKind::TimedOut => {
                    trace!("TimeOut error");
                    std::process::exit(0); // TODO: Don't exist proc insitu
                }
                Err(_err) => {
                    trace!("Unknown error reading from stream");
                    std::process::exit(0); // TODO: Don't exist proc insitu
                }
            }
        },
        (None, Some(r)) => collect_request((&mut *r).body_mut()).await.unwrap(), 
        _ => panic!("Unexpected state: no stream or request.")
    };
    // set service_id to know which service to send msg to
    let mut service_id = 0;
    if *payload != "".to_string() {
        service_id = deserialize(payload).service_id;
    }
    let timeout_duration = Duration::from_secs(10); // Set timeout to 10 seconds

    // receive heartbeat/message
    let mut response = Response::new(Full::default());

    // if a MSG: connect to listener to alter the msg value in the service's heartbeat
    trace!("Heartbeat handler received {:?}", message);

    // match message header
    if matches!(message.header, MessageHeader::MSG){
        // enter tcp or api mode
        match (stream, &mut *request) {
            (Some(_s), None) => {
                // create new connection to broker
                let listen_stream = match connect(&addr).await{
                    Ok(s) => s,
                    Err(_e) => {
                        return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,"MSG connection unsuccessful."));
                        }
                };
                let stream_mut = Arc::new(Mutex::new(listen_stream));
                let msg_body = MsgBody{
                    msg: message.body.clone(),
                    id: 0,
                    service_id
                };
                // relay message from send_msg() to broker
                let _ack = send(& stream_mut, & Message{
                    header: MessageHeader::MSG,
                    body: serde_json::to_string(& msg_body).unwrap()
                }).await;
            },
            (None, Some(_r)) => {
                // Prepare the HTTPS connector
                trace!("initiating request to listener");
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
                    id: 0,
                    service_id
                };
            
                let msg = serialize_message( & Message{
                        header: MessageHeader::MSG,
                        body: serde_json::to_string(& msg_body).unwrap()
                });
                let mut read_fail = 0;
                loop {
                    sleep(Duration::from_millis(1000)).await;
                    trace!("heartbeat_handler: Sent message to listener on {:?}", addr);

                    // relay message from send_msg() to broker with new request
                    let req = match tls.clone() {
                        Some(_t) => {
                            Request::builder()
                            .method(Method::POST)
                            .uri(format!(
                                    "https://{}:{}/request_handler",
                                    addr.host, addr.port
                            ))
                            .header(hyper::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(Bytes::from(msg.clone())))
                            .unwrap()
                        }
                        None => {
                            Request::builder()
                            .method(Method::POST)
                            .uri(format!(
                                    "http://{}:{}/request_handler",
                                    addr.host, addr.port
                            ))
                            .header(hyper::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(Bytes::from(msg.clone())))
                            .unwrap()
                        }
                    };

                    let result = timeout(timeout_duration, client.request(req)).await;
                
                    // check for valid response
                    match result {
                        Ok(Ok(resp)) => {
                            trace!("Received response: {:?}", resp);
                            let body = resp.collect().await.unwrap().aggregate();
                            let data: serde_json::Value = serde_json::from_reader(body.reader()).unwrap();
                            let json = serde_json::to_string(&data).unwrap();
                            let m = deserialize_message(& json);
                            match m.header {
                                MessageHeader::MSG => info!("Request acknowledged: {:?}", m),
                                _ => warn!("Server responds with unexpected message: {:?}", m),
                            }
                            // response = Response::builder()
                            // .status(StatusCode::OK)
                            // .header(hyper::header::CONTENT_TYPE, "application/json")
                            // .body(Full::new(Bytes::from(serialize_message(& Message{
                            //     header: MessageHeader::ACK,
                            //     body: "".to_string()
                            // })))).unwrap();
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
    } else if matches!(message.header, MessageHeader::COL) {
        // payload is empty for services, retrieve and send msg waiting in global variable
        if *payload == "".to_string(){
            // send msg back
            let msg = GLOBAL_MSGBODY.lock().await;
            // enter tcp or api mode
            let _ = match (&stream, &mut *request) {
                (Some(s), None) => {
                    // return message to collect()
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: serde_json::to_string(&* msg.msg).unwrap()
                    })).await;  
                },
                (None, Some(_r)) => {
                    // return message to collect()
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
                    // return published service payload to client
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: payload.clone()
                    })).await;
                },
                (None, Some(_r)) => {
                    // return published service payload to client
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
    } else if matches!(message.header, MessageHeader::HB) {
        let msg_body: MsgBody = serde_json::from_str(&message.body.clone()).unwrap();
        // default HB/MSG response to send what was received
        if msg_body.msg.is_empty() {
            let _ = match (&stream, &mut *request) {
                    // return acknowledgment heartbeat
                    (Some(s), None) => {
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: payload.clone(),
                    })).await;
                },
                (None, Some(_r)) => {
                    // return acknowledgment heartbeat
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
        } else {
            // store msg received from listener into global msg variable
            let mut msg = GLOBAL_MSGBODY.lock().await;
            *msg = serde_json::from_str(& message.body).unwrap();
            // send new message to broker
            let _ = match (&stream, &mut *request) {
                (Some(s), None) => {
                    let mut loc_stream: &mut TcpStream = &mut *s.lock().await;
                    let _ = stream_write(&mut loc_stream, & serialize_message(& Message{
                        header: message.header,
                        body: message.body
                    })).await;
                },
                (None, Some(_r)) => {
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
