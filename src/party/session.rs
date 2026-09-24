//! A registered party and its liveness loop.
//!
//! [`Session::publish`] and [`Session::claim`] bind the party's own server
//! *first* (so the broker can dial it the moment registration succeeds; the
//! old code registered first and lost the race), then register with retries,
//! then hand back a [`Session`]. [`Session::run`] keeps the party alive until
//! shutdown or until the broker is lost, which is reported as
//! [`Error::BrokerLost`] rather than by exiting the process.

use std::net::IpAddr;
use std::sync::Arc;

use tokio::time::{sleep, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{PartyHandler, PartyState, Role};
use crate::config::{Limits, Timing, TlsPaths};
use crate::net::Addr;
use crate::protocol::{Key, Message, PartyId, RegToken, ServiceHandle};
use crate::transport::{self, Client, Server};
use crate::{Error, Result};

/// Options common to publishing and claiming.
#[derive(Debug, Clone)]
pub struct PartyOpts {
    /// The broker to register with. Its transport family (TCP or HTTP)
    /// decides the family of this party's own listener.
    pub broker: Addr,
    /// Rendezvous key.
    pub key: Key,
    /// Local address to bind and to advertise to the broker.
    pub local_ip: IpAddr,
    /// Port to bind for heartbeats (0 picks a free port).
    pub bind_port: u16,
    /// Serve TLS on the party's own listener (needs `tls.cert`/`tls.key`).
    pub serve_tls: bool,
    /// One-sided liveness: ping the broker instead of being dialled.
    pub ping: bool,
    /// Certificate material for serving and for verifying the broker.
    pub tls: TlsPaths,
    /// Intervals and timeouts.
    pub timing: Timing,
    /// Size and count limits.
    pub limits: Limits,
}

impl PartyOpts {
    /// The address this party listens on: same family as the broker, TLS
    /// according to `serve_tls`.
    pub fn bind_addr(&self) -> Addr {
        Addr::new(
            self.broker.transport.with_tls(self.serve_tls),
            self.local_ip.to_string(),
            self.bind_port,
        )
    }
}

/// Options for publishing a service.
#[derive(Debug, Clone)]
pub struct PublishOpts {
    /// Registration and liveness settings.
    pub party: PartyOpts,
    /// Port the service itself listens on (at `party.local_ip`).
    pub service_port: u16,
}

/// Options for claiming a service.
#[derive(Debug, Clone)]
pub struct ClaimOpts {
    /// Registration and liveness settings.
    pub party: PartyOpts,
}

/// A registered party: its server, shared state and shutdown token.
#[derive(Debug)]
pub struct Session {
    id: PartyId,
    token: RegToken,
    state: Arc<PartyState>,
    server: Server,
    opts: PartyOpts,
    shutdown: CancellationToken,
}

impl Session {
    /// Bind, then register as a service.
    pub async fn publish(opts: PublishOpts) -> Result<Session> {
        let service_port = opts.service_port;
        Session::start(Role::Publisher, opts.party, move |bind_addr, key, ping| {
            Message::Publish {
                key,
                service_port,
                bind_addr,
                ping,
            }
        })
        .await
    }

    /// Bind, then register as a client; [`Session::service`] is the pairing.
    pub async fn claim(opts: ClaimOpts) -> Result<Session> {
        Session::start(Role::Claimer, opts.party, |bind_addr, key, ping| {
            Message::Claim {
                key,
                bind_addr,
                ping,
            }
        })
        .await
    }

    async fn start(
        role: Role,
        opts: PartyOpts,
        request: impl Fn(Addr, Key, bool) -> Message,
    ) -> Result<Session> {
        let shutdown = CancellationToken::new();
        let client = Arc::new(Client::new(
            opts.tls.clone(),
            opts.timing.clone(),
            opts.limits.clone(),
        ));
        let state = PartyState::new(role, opts.broker.clone(), opts.key, Arc::clone(&client));

        let server = transport::serve(
            &opts.bind_addr(),
            Arc::new(PartyHandler::new(Arc::clone(&state))),
            &opts.tls,
            &opts.limits,
            &opts.timing,
            shutdown.child_token(),
        )
        .await?;
        let bound = server.bound();
        info!(%role, %bound, broker = %opts.broker, "listening; registering with the broker");

        let (id, token) =
            match register(&client, &opts, &state, request(bound, opts.key, opts.ping)).await {
                Ok(pair) => pair,
                Err(e) => {
                    server.shutdown().await;
                    return Err(e);
                }
            };
        state.touch();
        info!(%role, %id, "registered");

        Ok(Session {
            id,
            token,
            state,
            server,
            opts,
            shutdown,
        })
    }

    /// The broker-assigned id.
    pub fn id(&self) -> PartyId {
        self.id
    }

    /// The registration token the broker issued (needed to act on this
    /// party's behalf; treat it like the rendezvous key).
    pub fn token(&self) -> RegToken {
        self.token
    }

    /// Service or client.
    pub fn role(&self) -> Role {
        self.state.role()
    }

    /// Where this party listens (transport, actual address and port).
    pub fn bound(&self) -> Addr {
        self.server.bound()
    }

    /// For a client: the service it is paired with (updated on re-pairing).
    pub fn service(&self) -> Option<ServiceHandle> {
        self.state.service()
    }

    /// Shared state, for inspection.
    pub fn state(&self) -> &Arc<PartyState> {
        &self.state
    }

    /// Cancelling this token ends [`Session::run`] cleanly.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Keep the party alive until the token is cancelled (`Ok`) or the broker
    /// is lost (`Err(BrokerLost)`), then stop the party's server.
    ///
    /// Two-sided mode watches for the broker's heartbeats to stop arriving
    /// for longer than [`Timing::broker_watchdog`]. Ping mode sends
    /// [`Message::Ping`] every [`Timing::heartbeat_interval`], applies the
    /// pending items the broker returns, and gives up after
    /// [`Timing::fail_threshold`] consecutive failures or when the broker no
    /// longer knows the party.
    pub async fn run(self) -> Result<()> {
        let outcome = if self.opts.ping {
            self.run_ping().await
        } else {
            self.run_watchdog().await
        };
        self.shutdown.cancel();
        self.server.shutdown().await;
        outcome
    }

    async fn run_watchdog(&self) -> Result<()> {
        let t = &self.opts.timing;
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return Ok(()),
                _ = sleep(t.heartbeat_interval) => {}
            }
            let silence = Instant::now().saturating_duration_since(self.state.last_contact());
            if silence > t.broker_watchdog {
                warn!(id = %self.id, ?silence, "no heartbeat from the broker; giving up");
                return Err(Error::BrokerLost(self.opts.broker.to_string()));
            }
        }
    }

    async fn run_ping(&self) -> Result<()> {
        let t = &self.opts.timing;
        let mut failures: u32 = 0;
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => return Ok(()),
                _ = sleep(t.heartbeat_interval) => {}
            }
            match self
                .state
                .client()
                .call(
                    &self.opts.broker,
                    Message::Ping {
                        id: self.id,
                        token: self.token,
                    },
                )
                .await
            {
                Ok(Message::Heartbeat {
                    token,
                    inbox,
                    service,
                }) if self.state.accepts_token(&token) => {
                    failures = 0;
                    let service = match self.state.role() {
                        Role::Publisher => None,
                        Role::Claimer => service,
                    };
                    self.state.apply_heartbeat(inbox, service);
                }
                Ok(Message::Nack { reason }) => {
                    warn!(id = %self.id, %reason, "broker rejected our ping");
                    return Err(Error::Rejected(reason));
                }
                Ok(other) => {
                    failures += 1;
                    debug!(id = %self.id, kind = other.kind(), failures, "unexpected ping reply");
                }
                Err(e) => {
                    failures += 1;
                    debug!(id = %self.id, error = %e, failures, "ping failed");
                }
            }
            if failures >= t.fail_threshold {
                warn!(id = %self.id, failures, "broker unreachable; giving up");
                return Err(Error::BrokerLost(self.opts.broker.to_string()));
            }
        }
    }
}

/// Send the registration request with retries on transport failures; a
/// [`Message::Nack`] is final.
async fn register(
    client: &Client,
    opts: &PartyOpts,
    state: &PartyState,
    request: Message,
) -> Result<(PartyId, RegToken)> {
    let t = &opts.timing;
    let mut last_err: Option<Error> = None;
    for attempt in 1..=t.register_attempts.max(1) {
        if attempt > 1 {
            sleep(t.register_backoff).await;
        }
        match client.call(&opts.broker, request.clone()).await {
            Ok(Message::Registered { id, token }) if state.role() == Role::Publisher => {
                state.set_token(token);
                state.set_id(id);
                return Ok((id, token));
            }
            Ok(Message::Paired { id, token, service }) if state.role() == Role::Claimer => {
                state.set_service(service);
                state.set_token(token);
                state.set_id(id);
                return Ok((id, token));
            }
            Ok(Message::Nack { reason }) => return Err(Error::Rejected(reason)),
            Ok(other) => {
                return Err(Error::protocol(format!(
                    "broker answered {} with {}",
                    request.kind(),
                    other.kind()
                )))
            }
            Err(e) if e.is_disconnect() || matches!(e, Error::Io(_)) => {
                debug!(attempt, error = %e, "registration attempt failed");
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| Error::BrokerLost(opts.broker.to_string())))
}
