//! The broker: registry of services and clients, the store each claim
//! shares with its service, heartbeat monitor, the request handler
//! mounted on the broker's listener, what the broker counts, and the admin
//! listener that reports it.

pub mod admin;
pub mod handler;
pub mod listen;
pub mod metrics;
pub mod monitor;
pub mod registry;
pub mod store;

pub use admin::{AdminOpts, AdminServer};
pub use handler::BrokerHandler;
pub use listen::{BrokerHandle, ListenOpts, listen};
pub use metrics::{Metrics, RemovalReason, Status};
pub use monitor::{Broker, PartySummary};
pub use registry::{ClientEntry, Party, Registry, Removed, ServiceEntry};
pub use store::Store;
