//! Starting a broker: registry, monitor and listener wired together.
//!
//! [`listen`] checks the limits before it binds anything: a full store's
//! `stored` reply, at most [`Limits::max_store_bytes`] plus
//! [`REPLY_OVERHEAD`], must fit the broker's own frame limit, or a party's
//! `list` of a full store could never be answered.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::info;

use super::handler::BrokerHandler;
use super::monitor::Broker;
use super::store::REPLY_OVERHEAD;
use crate::config::{BrokerPolicy, Limits, Timing, TlsPaths};
use crate::net::Addr;
use crate::transport::{self, Client, Server};
use crate::{Error, Result};

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
///
/// # Errors
///
/// [`Error::Config`] when a full store's reply would not fit
/// [`Limits::max_frame_bytes`] (checked before anything is bound), and
/// whatever binding the listener fails with.
pub async fn listen(opts: ListenOpts, shutdown: CancellationToken) -> Result<BrokerHandle> {
    check_limits(&opts.limits)?;
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

/// Refuse a store budget whose full reply would not fit the frame limit.
fn check_limits(limits: &Limits) -> Result<()> {
    if limits.max_store_bytes.saturating_add(REPLY_OVERHEAD) > limits.max_frame_bytes {
        return Err(Error::config(format!(
            "--max-store-bytes {} leaves no room for a full store's reply within --max-frame-bytes {}",
            limits.max_store_bytes, limits.max_frame_bytes
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::net::Transport;

    fn limits(max_frame_bytes: usize, max_store_bytes: usize) -> Limits {
        Limits {
            max_frame_bytes,
            max_store_bytes,
            ..Limits::default()
        }
    }

    #[test]
    fn a_full_stores_reply_must_fit_the_frame_limit() {
        assert!(check_limits(&Limits::default()).is_ok());
        assert!(check_limits(&limits(4096, 4096 - REPLY_OVERHEAD)).is_ok());
        assert!(check_limits(&limits(64 * 1024, 32 * 1024)).is_ok());
        // The default budget needs 17408 bytes of frame.
        assert!(check_limits(&limits(17 * 1024, 16 * 1024)).is_ok());
        // The smallest budget the command line allows (256) needs 1280, so a
        // frame limit of 1024 to 1279 can never start a broker from there.
        assert!(check_limits(&limits(1280, 256)).is_ok());
        for (frame, store) in [
            (4096, 4096 - REPLY_OVERHEAD + 1),
            (4096, 16 * 1024),
            (17 * 1024 - 1, 16 * 1024),
            (1279, 256),
            (1024, 256),
        ] {
            match check_limits(&limits(frame, store)) {
                Err(Error::Config(msg)) => assert_eq!(
                    msg,
                    format!(
                        "--max-store-bytes {store} leaves no room for a full store's reply within --max-frame-bytes {frame}"
                    )
                ),
                other => panic!("frame {frame}, store {store}: {other:?}"),
            }
        }
        assert!(
            check_limits(&limits(usize::MAX - 1, usize::MAX)).is_err(),
            "no overflow"
        );
    }

    #[tokio::test]
    async fn listen_refuses_a_budget_that_does_not_fit_before_binding() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let opts = ListenOpts {
                bind: Addr::new(Transport::Tcp, "127.0.0.1", 0),
                tls: TlsPaths::default(),
                timing: Timing::fast(),
                limits: limits(4096, 16 * 1024),
                policy: BrokerPolicy::default(),
            };
            let err = listen(opts, CancellationToken::new()).await.unwrap_err();
            assert!(matches!(err, Error::Config(_)), "{err:?}");
            let text = err.to_string();
            assert!(
                text.contains("--max-store-bytes 16384") && text.contains("--max-frame-bytes 4096"),
                "{text}"
            );
        })
        .await
        .unwrap();
    }
}
