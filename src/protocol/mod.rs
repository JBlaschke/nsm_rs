//! Typed wire protocol shared by every transport.
//!
//! - [`message`]: the tagged [`Message`] enum, one variant per request and
//!   reply, with the request/reply table and the JSON shape;
//! - [`types`]: the records it carries ([`PartyId`], [`Key`],
//!   [`ServiceHandle`], [`ServiceRecord`], [`ClientRecord`]);
//! - [`codec`]: JSON [`encode`]/[`decode`] used by every transport, and the
//!   length-prefixed [`MessageCodec`] framing for TCP and TLS streams.
//!
//! The current wire format is [`PROTOCOL_VERSION`].

pub mod codec;
pub mod message;
pub mod types;

pub use codec::{decode, encode, framed, Framed, MessageCodec};
pub use message::{Message, PROTOCOL_VERSION};
pub use types::{ClientRecord, Key, PartyId, ServiceHandle, ServiceRecord};
