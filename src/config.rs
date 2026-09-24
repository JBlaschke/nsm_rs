//! Tunables that were previously scattered as literals through the code.
//!
//! Three groups: [`Timing`] (every interval, timeout and threshold that
//! governs liveness), [`Limits`] (sizes and counts that bound resource use)
//! and [`TlsPaths`] (where certificate material comes from). All of them have
//! documented defaults; the CLI can override them and tests use
//! [`Timing::fast`].

use std::path::PathBuf;
use std::time::Duration;

/// Intervals, timeouts and thresholds for heartbeats and requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    /// How often the broker sends a heartbeat to each party (two-sided mode),
    /// and how often a party pings the broker (one-sided mode).
    pub heartbeat_interval: Duration,
    /// How long one heartbeat request may take before it counts as failed.
    pub heartbeat_timeout: Duration,
    /// Consecutive failed heartbeats after which a party is removed.
    pub fail_threshold: u32,
    /// In one-sided (ping) mode: silence after which a party is removed.
    pub ping_staleness: Duration,
    /// A party that has not heard from the broker for this long gives up.
    pub broker_watchdog: Duration,
    /// Deadline for a single request/response exchange (register, send, collect).
    pub request_timeout: Duration,
    /// Deadline for connecting to a peer.
    pub connect_timeout: Duration,
    /// How many times a party retries registering with the broker.
    pub register_attempts: u32,
    /// Pause between registration attempts.
    pub register_backoff: Duration,
    /// How long the broker waits for a matching service to appear when a
    /// client claims before any service has published.
    pub claim_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            heartbeat_interval: Duration::from_secs(2),
            heartbeat_timeout: Duration::from_secs(3),
            fail_threshold: 5,
            ping_staleness: Duration::from_secs(20),
            broker_watchdog: Duration::from_secs(30),
            request_timeout: Duration::from_secs(6),
            connect_timeout: Duration::from_secs(5),
            register_attempts: 6,
            register_backoff: Duration::from_secs(1),
            claim_wait: Duration::from_millis(1500),
        }
    }
}

impl Timing {
    /// Values scaled down for in-process tests: sub-second heartbeats and a
    /// low failure threshold so a failure scenario completes in well under a
    /// second of wall-clock time.
    pub fn fast() -> Self {
        Timing {
            heartbeat_interval: Duration::from_millis(50),
            heartbeat_timeout: Duration::from_millis(200),
            fail_threshold: 3,
            ping_staleness: Duration::from_millis(400),
            broker_watchdog: Duration::from_millis(600),
            request_timeout: Duration::from_millis(500),
            connect_timeout: Duration::from_millis(500),
            register_attempts: 3,
            register_backoff: Duration::from_millis(20),
            claim_wait: Duration::from_millis(100),
        }
    }

    /// Time after which a silent two-sided party is considered gone:
    /// `fail_threshold` heartbeats, each allowed `heartbeat_timeout`, spaced
    /// `heartbeat_interval` apart.
    pub fn detection_window(&self) -> Duration {
        (self.heartbeat_interval + self.heartbeat_timeout) * self.fail_threshold
    }
}

/// Bounds on resource use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Largest message accepted on any transport, in bytes.
    pub max_frame_bytes: usize,
    /// Connections a listener services concurrently; further connections
    /// wait in the accept queue.
    pub max_connections: usize,
    /// Registrations (services plus clients) a broker holds at once.
    pub max_registrations: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_frame_bytes: 64 * 1024,
            max_connections: 1024,
            max_registrations: 10_000,
        }
    }
}

/// Where TLS material is read from. Paths are resolved by the CLI (flags with
/// environment fallback); the library only ever sees this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsPaths {
    /// PEM certificate chain presented by a server (`--tls-cert`, `CERT_PATH`).
    pub cert: Option<PathBuf>,
    /// PEM private key matching `cert` (`--tls-key`, `KEY_PATH`).
    pub key: Option<PathBuf>,
    /// PEM bundle of root certificates used to verify peers
    /// (`--root-ca`, `ROOT_PATH`). `None` means the platform trust store.
    pub root_ca: Option<PathBuf>,
}

impl TlsPaths {
    /// True when both a certificate and a key are configured, i.e. this
    /// process can act as a TLS server.
    pub fn has_server_identity(&self) -> bool {
        self.cert.is_some() && self.key.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_detection_window_is_tens_of_seconds() {
        let t = Timing::default();
        let w = t.detection_window();
        assert!(
            w >= Duration::from_secs(10) && w <= Duration::from_secs(60),
            "{w:?}"
        );
    }

    #[test]
    fn fast_timing_is_sub_second_per_failure_cycle() {
        let t = Timing::fast();
        assert!(t.detection_window() < Duration::from_secs(2));
        assert!(t.heartbeat_interval < Duration::from_millis(100));
    }

    #[test]
    fn tls_server_identity_requires_both_files() {
        let mut p = TlsPaths::default();
        assert!(!p.has_server_identity());
        p.cert = Some("c.pem".into());
        assert!(!p.has_server_identity());
        p.key = Some("k.pem".into());
        assert!(p.has_server_identity());
    }

    #[test]
    fn limits_defaults_are_sane() {
        let l = Limits::default();
        assert!(l.max_frame_bytes >= 16 * 1024);
        assert!(l.max_connections > 0 && l.max_registrations > 0);
    }
}
