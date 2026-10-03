//! Typed wire protocol shared by every transport.
//!
//! - [`message`]: the tagged [`Message`] enum, one variant per request and
//!   reply, with the request/reply table and the JSON shape;
//! - [`types`]: the records it carries ([`PartyId`], [`Key`],
//!   [`ServiceHandle`], [`ServiceRecord`], [`ClientRecord`]), a party's
//!   [`Role`], the shared store's [`StoreKey`], [`StoreOp`],
//!   [`StoreEntry`] and [`Stored`], and [`MeshData`], the value of the
//!   reserved entry `nsm_mesh_data`;
//! - [`codec`]: JSON [`encode`]/[`decode`] used by every transport, and the
//!   length-prefixed [`MessageCodec`] framing for TCP and TLS streams.
//!
//! The current wire format is [`PROTOCOL_VERSION`].

pub mod codec;
pub mod message;
pub mod types;

pub use codec::{Framed, MessageCodec, decode, encode, framed};
pub use message::{Message, PROTOCOL_VERSION};
pub use types::{
    ClientRecord, Key, MAX_STORE_KEY_BYTES, MESH_DATA_KEY, MeshData, PartyId,
    RESERVED_STORE_KEY_PREFIX, RegToken, Role, ServiceHandle, ServiceRecord, StoreEntry, StoreKey,
    StoreKeyError, StoreOp, Stored,
};
