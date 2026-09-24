//! Starting a broker: registry, monitor and listener wired together.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::info;

use super::handler::BrokerHandler;
use super::monitor::Broker;
use crate::config::{BrokerPolicy, Limits, Timing, TlsPaths};
use crate::net::Addr;
use crate::transport::{self, Client, Server};
use crate::Result;

/// Everything needed to run a broker.
#[derive(Debug, Clone)]
pub struct ListenOpts {
    /// Address to listen on; its transport decides the wire (`tls`/`https`
    /// need `tls.cert` and `tls.key`).
    pub bind: Addr,
    /// Certificate material for the listener and for dialling parties.
    pub tls: TlsPaths,
    /// Intervals and thresholds.
    pub timing: Timing,
    /// Size and count limits.
    pub limits: Limits,
    /// Admission policy.
    pub policy: BrokerPolicy,
}

/// A running broker.
#[derive(Debug)]
pub struct BrokerHandle {
    server: Server,
    broker: Arc<Broker>,
    shutdown: CancellationToken,
}

impl BrokerHandle {
    /// Where the broker listens (transport, actual address and port).
    pub fn bound(&self) -> Addr {
        self.server.bound()
    }

    /// The broker state, for status and tests.
    pub fn broker(&self) -> &Arc<Broker> {
        &self.broker
    }

    /// Cancelling this token stops the listener and every monitor task.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Run until the shutdown token is cancelled.
    pub async fn run(self) -> Result<()> {
        self.server.wait().await;
        Ok(())
    }

    /// Stop the broker and wait for its listener to close.
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        self.server.shutdown().await;
    }
}

/// Bind the listener and start the monitor tasks.
pub async fn listen(opts: ListenOpts, shutdown: CancellationToken) -> Result<BrokerHandle> {
    let client = Arc::new(Client::new(
        opts.tls.clone(),
        opts.timing.clone(),
        opts.limits.clone(),
    ));
    let broker = Broker::new(
        client,
        opts.timing.clone(),
        opts.limits.clone(),
        opts.policy.clone(),
        shutdown.child_token(),
    );
    broker.start_sweeper();
    let server = transport::serve(
        &opts.bind,
        Arc::new(BrokerHandler::new(Arc::clone(&broker))),
        &opts.tls,
        &opts.limits,
        &opts.timing,
        shutdown.child_token(),
    )
    .await?;
    info!(bound = %server.bound(), "broker listening");
    Ok(BrokerHandle {
        server,
        broker,
        shutdown,
    })
}
