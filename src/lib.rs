#![doc = include_str!("../README.md")]
//!
//! ## Crate layout
//!
//! | Module | Role |
//! |---|---|
//! | [`cli`] | command-line definition (clap derive); shared with the REST control plane |
//! | [`config`] | every interval, timeout, limit and TLS path in one place |
//! | [`error`] | the crate error type; nothing below `main` panics on peer input |
//! | [`net`] | address grammar and local interface enumeration |
//! | [`protocol`] | typed wire messages and framing shared by all transports |
//! | [`tls`] | rustls configuration from PEM files |
//! | [`transport`] | one request, one reply over TCP, TLS, HTTP or HTTPS; the `Handler` trait |
//! | [`broker`] | registry, heartbeat monitor and request handler of the broker |
//! | [`party`] | the service and client sides: bind, register, stay alive |
//! | [`ops`] | the operations, typed and print-free, shared by the CLI and the control plane |
//! | [`rest`] | the REST control plane behind `nsm serve` |
//! | [`logging`] | `tracing` initialisation honouring `NSM_LOG_LEVEL` |
//!
//! Import direction is strictly downward: `main` → [`cli`] → [`rest`]/[`ops`]
//! → [`broker`]/[`party`] → [`transport`] → [`protocol`]/[`net`]/[`tls`] →
//! [`config`]/[`error`]. Nothing below `main` prints to stdout, panics on peer
//! input or exits the process. The guides in `docs/` (architecture, protocol,
//! REST API) and `CONTRIBUTING.md` live in the repository next to this README.

#![deny(clippy::let_underscore_future, unused_must_use, missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]
#![warn(rustdoc::broken_intra_doc_links)]

pub mod broker;
pub mod cli;
pub mod config;
pub mod error;
pub mod logging;
pub mod net;
pub mod ops;
pub mod party;
pub mod protocol;
pub mod rest;
pub mod tls;
pub mod transport;

#[cfg(test)]
pub(crate) mod testing;

pub use error::{Error, Result};
