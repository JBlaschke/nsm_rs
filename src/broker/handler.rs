//! The request handler mounted on the broker's listener.
//!
//! One `match` over [`Message`], written once for every transport. Anything
//! the broker does not expect is a [`Message::Nack`]; nothing here panics on
//! peer input, and the registry lock is never held across an `.await`.

use std::future::Future;
use std::sync::Arc;

use tokio::time::{sleep, Instant};
use tracing::{debug, trace};

use super::monitor::Broker;
use crate::net::Addr;
use crate::protocol::Message;
use crate::transport::{Handler, PeerInfo};
use crate::{Error, Result};

/// [`Handler`] for the broker.
#[derive(Debug, Clone)]
pub struct BrokerHandler {
    broker: Arc<Broker>,
}

impl BrokerHandler {
    /// A handler over the given broker state.
    pub fn new(broker: Arc<Broker>) -> Self {
        BrokerHandler { broker }
    }

    async fn dispatch(&self, msg: Message, peer: PeerInfo) -> Result<Message> {
        trace!(kind = msg.kind(), remote = %peer.remote, "broker request");
        match msg {
            Message::Publish {
                key,
                service_port,
                bind_addr,
                ping,
            } => {
                if bind_addr.port == 0 {
                    return Ok(Message::nack("bind_addr must carry the actual port"));
                }
                if peer.remote.ip().to_string() != bind_addr.host {
                    debug!(remote = %peer.remote, advertised = %bind_addr.host, "party advertises an address other than the one it connected from");
                }
                let service_addr = Addr::tcp(bind_addr.host.clone(), service_port);
                let now = Instant::now();
                match self
                    .broker
                    .with_registry(|r| r.publish(key, service_addr, bind_addr, ping, now))
                {
                    Ok(id) => {
                        self.broker.watch(id);
                        Ok(Message::Registered { id })
                    }
                    Err(Error::Rejected(reason)) => Ok(Message::nack(reason)),
                    Err(e) => Err(e),
                }
            }

            Message::Claim {
                key,
                bind_addr,
                ping,
            } => {
                if bind_addr.port == 0 {
                    return Ok(Message::nack("bind_addr must carry the actual port"));
                }
                let t = self.broker.timing();
                let deadline = Instant::now() + t.claim_wait;
                let pause = (t.claim_wait / 5).max(std::time::Duration::from_millis(1));
                loop {
                    let now = Instant::now();
                    match self
                        .broker
                        .with_registry(|r| r.claim(key, bind_addr.clone(), ping, now))
                    {
                        Ok((id, service)) => {
                            self.broker.watch(id);
                            return Ok(Message::Paired { id, service });
                        }
                        Err(Error::NoService(_)) if Instant::now() < deadline => {
                            // A client may start before its service has
                            // published; wait a little without holding the lock.
                            sleep(pause).await;
                        }
                        Err(Error::NoService(k)) => {
                            return Ok(Message::nack(format!("no service available for key {k}")));
                        }
                        Err(Error::Rejected(reason)) => return Ok(Message::nack(reason)),
                        Err(e) => return Err(e),
                    }
                }
            }

            Message::Ping { id } => {
                let now = Instant::now();
                let reply = self.broker.with_registry(|r| {
                    if r.mark_alive(id, now) {
                        r.heartbeat_for(id)
                    } else {
                        None
                    }
                });
                Ok(reply.unwrap_or_else(|| Message::nack(format!("unknown party {id}"))))
            }

            Message::Deliver { to, text } => {
                match self.broker.with_registry(|r| r.deliver(to, text)) {
                    Ok(()) => Ok(Message::Delivered),
                    Err(Error::Rejected(reason)) => Ok(Message::nack(reason)),
                    Err(e) => Err(e),
                }
            }

            other => Ok(Message::nack(format!(
                "unexpected {} at the broker",
                other.kind()
            ))),
        }
    }
}

impl Handler for BrokerHandler {
    fn handle(&self, msg: Message, peer: PeerInfo) -> impl Future<Output = Result<Message>> + Send {
        self.dispatch(msg, peer)
    }
}
