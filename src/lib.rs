//! # NSM: NERSC Service Mesh
//!
//! A connection broker for environments where compute nodes cannot accept
//! inbound connections. Services *publish* themselves to a broker with a fixed
//! address; clients *claim* a service by a shared key and receive its address;
//! the broker keeps both sides alive with heartbeats and re-pairs clients when
//! a service disappears.
//!
//! ## Layout
//!
//! | Module | Role |
//! |---|---|
//! | [`cli`] | command-line definition (clap derive); shared with the REST control plane |
//! | [`config`] | every interval, timeout, limit and TLS path in one place |
//! | [`error`] | the crate error type; nothing below `main` panics on peer input |
//! | [`net`] | address grammar and local interface enumeration |
//! | [`protocol`] | typed wire messages and framing shared by all transports |
//! | [`tls`] | rustls configuration from PEM files |
//! | [`logging`] | `tracing` initialisation honouring `NSM_LOG_LEVEL` |
//! | [`legacy`] | the pre-cleanup implementation, kept compiling until the new backend replaces it |
//!
//! The `legacy` module is on its way out: it is excluded from formatting and
//! lints and will be deleted by the `cleanup/03-common-backend` branch. New
//! code must not depend on it except through [`legacy::run`].

#![deny(clippy::let_underscore_future, unused_must_use)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

pub mod cli;
pub mod config;
pub mod error;
pub mod logging;
pub mod net;
pub mod protocol;
pub mod tls;

#[allow(warnings, clippy::all, missing_docs)]
#[rustfmt::skip]
pub mod legacy;

pub use error::{Error, Result};
