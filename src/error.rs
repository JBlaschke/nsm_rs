//! Crate-wide error type.
//!
//! Every fallible function below `main` returns [`Result`]. The binary maps
//! the error to an exit code and a message; nothing in the library panics on
//! input it did not produce itself, and nothing calls `std::process::exit`.

use std::net::SocketAddr;

use crate::net::addr::ParseAddrError;

/// Errors produced by the NSM library.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A socket, file or other OS-level operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A message could not be encoded or decoded as JSON.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A TLS configuration or handshake error.
    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    /// PEM material could not be parsed.
    #[error("PEM error: {0}")]
    Pem(#[from] rustls::pki_types::pem::Error),

    /// An address string did not follow `host:port`, `http://host:port` or
    /// `https://host:port`.
    #[error(transparent)]
    Addr(#[from] ParseAddrError),

    /// An address could not be resolved to a socket address.
    #[error("cannot resolve {0} to a socket address")]
    Resolve(String),

    /// The peer sent something the protocol does not allow at this point.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A frame exceeded the configured maximum size.
    #[error("frame of {size} bytes exceeds the limit of {limit} bytes")]
    FrameTooLarge {
        /// Size announced by the sender.
        size: usize,
        /// Configured limit.
        limit: usize,
    },

    /// The peer closed the connection before a complete message arrived.
    #[error("connection closed by peer")]
    Closed,

    /// An operation did not complete within its deadline.
    #[error("timed out after {0:?}")]
    Timeout(std::time::Duration),

    /// No service is currently available under the given key.
    #[error("no service available for key {0}")]
    NoService(u64),

    /// The broker rejected a request.
    #[error("broker rejected the request: {0}")]
    Rejected(String),

    /// The broker stopped answering heartbeats.
    #[error("lost contact with the broker at {0}")]
    BrokerLost(String),

    /// A party (service or client) stopped answering heartbeats.
    #[error("lost contact with peer {0}")]
    PeerLost(String),

    /// Configuration supplied by the operator is invalid or incomplete.
    #[error("configuration error: {0}")]
    Config(String),

    /// Exactly one local address was required but the filters matched a
    /// different number.
    #[error("expected exactly one local address, found {found}: {candidates:?}")]
    AmbiguousAddress {
        /// How many addresses matched.
        found: usize,
        /// The matching addresses, for the error message.
        candidates: Vec<String>,
    },

    /// A listener could not be bound.
    #[error("cannot bind {addr}: {source}")]
    Bind {
        /// Address that was requested.
        addr: SocketAddr,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Build a [`Error::Protocol`] from anything displayable.
    pub fn protocol(msg: impl std::fmt::Display) -> Self {
        Error::Protocol(msg.to_string())
    }

    /// Build a [`Error::Config`] from anything displayable.
    pub fn config(msg: impl std::fmt::Display) -> Self {
        Error::Config(msg.to_string())
    }

    /// True for errors that describe a peer or broker having gone away, as
    /// opposed to local misconfiguration or malformed input.
    pub fn is_disconnect(&self) -> bool {
        matches!(
            self,
            Error::Closed | Error::Timeout(_) | Error::BrokerLost(_) | Error::PeerLost(_)
        ) || matches!(self, Error::Io(e) if matches!(
            e.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::TimedOut
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_convert() {
        let e: Error = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset").into();
        assert!(e.is_disconnect());
        assert!(e.to_string().contains("reset"));
    }

    #[test]
    fn config_errors_are_not_disconnects() {
        assert!(!Error::config("missing cert").is_disconnect());
        assert_eq!(Error::config("x").to_string(), "configuration error: x");
    }

    #[test]
    fn frame_too_large_message() {
        let e = Error::FrameTooLarge { size: 10, limit: 5 };
        assert_eq!(
            e.to_string(),
            "frame of 10 bytes exceeds the limit of 5 bytes"
        );
    }
}
