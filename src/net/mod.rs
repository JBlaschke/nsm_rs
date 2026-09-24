//! Addresses and local network interfaces.

pub mod addr;
pub mod interfaces;

pub use addr::{Addr, ParseAddrError, Transport};
pub use interfaces::{IpVersion, LocalAddr, Selector};
