//! JSON encoding and length-delimited framing.
//!
//! [`encode`] and [`decode`] turn a [`Message`] into its JSON bytes and back.
//! `decode` is the one place in the crate where bytes from a peer become a
//! `Message`; it never panics, and anything malformed is [`Error::Json`].
//! HTTP transports use these two directly, with the JSON as the request or
//! response body.
//!
//! On a TCP or TLS connection, messages are framed by [`MessageCodec`]:
//!
//! ```text
//! +-------------------+---------------------------+
//! | len: u32, big-end | JSON body, exactly len B  |
//! +-------------------+---------------------------+
//! ```
//!
//! `len` counts the body only. A prefix above the codec's `max_frame` is
//! rejected as [`Error::FrameTooLarge`] the moment the four prefix bytes are
//! in, without waiting for (or buffering) the body; the connection should be
//! dropped after that. EOF in the middle of a frame is [`Error::Closed`];
//! EOF exactly at a frame boundary ends the stream cleanly.

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use super::Message;
use crate::config::Limits;
use crate::{Error, Result};

/// Size of the length prefix in front of every framed message.
pub const LENGTH_PREFIX_BYTES: usize = 4;

/// Serialise a message to its JSON bytes (no length prefix).
pub fn encode(msg: &Message) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(msg)?)
}

/// Parse a message from JSON bytes (no length prefix).
///
/// Rejects anything that is not exactly one message: an unknown `type`,
/// missing or mistyped fields, and trailing non-whitespace bytes. Never
/// panics on malformed input.
pub fn decode(bytes: &[u8]) -> Result<Message> {
    Ok(serde_json::from_slice(bytes)?)
}

/// Length-prefixed [`Message`] framing for byte streams.
///
/// Implements [`Decoder`] and [`Encoder`] for use with
/// [`tokio_util::codec::Framed`]; see the [module docs](self) for the
/// format. The one tunable is the largest body accepted or produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageCodec {
    max_frame: usize,
}

impl MessageCodec {
    /// A codec that rejects bodies longer than `max_frame` bytes, in both
    /// directions.
    pub fn new(max_frame: usize) -> Self {
        MessageCodec { max_frame }
    }

    /// The largest body this codec accepts or produces, in bytes.
    pub fn max_frame(&self) -> usize {
        self.max_frame
    }

    fn check_size(&self, size: usize) -> Result<()> {
        if size > self.max_frame {
            return Err(Error::FrameTooLarge {
                size,
                limit: self.max_frame,
            });
        }
        Ok(())
    }
}

impl Default for MessageCodec {
    /// Uses [`Limits::default`]`().max_frame_bytes`.
    fn default() -> Self {
        MessageCodec::new(Limits::default().max_frame_bytes)
    }
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Message>> {
        if src.len() < LENGTH_PREFIX_BYTES {
            return Ok(None);
        }
        let len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
        self.check_size(len)?;
        let total = LENGTH_PREFIX_BYTES + len;
        if src.len() < total {
            // Grow once to fit the whole frame; bounded by `max_frame`.
            src.reserve(total - src.len());
            return Ok(None);
        }
        src.advance(LENGTH_PREFIX_BYTES);
        let body = src.split_to(len);
        decode(&body).map(Some)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Message>> {
        match self.decode(src)? {
            Some(msg) => Ok(Some(msg)),
            None if src.is_empty() => Ok(None),
            None => Err(Error::Closed),
        }
    }
}

impl Encoder<&Message> for MessageCodec {
    type Error = Error;

    fn encode(&mut self, msg: &Message, dst: &mut BytesMut) -> Result<()> {
        let body = encode(msg)?;
        self.check_size(body.len())?;
        let len = u32::try_from(body.len()).map_err(|_| Error::FrameTooLarge {
            size: body.len(),
            limit: u32::MAX as usize,
        })?;
        dst.reserve(LENGTH_PREFIX_BYTES + body.len());
        dst.put_u32(len);
        dst.extend_from_slice(&body);
        Ok(())
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = Error;

    fn encode(&mut self, msg: Message, dst: &mut BytesMut) -> Result<()> {
        Encoder::<&Message>::encode(self, &msg, dst)
    }
}

/// A byte stream wrapped in [`MessageCodec`]: a `Stream` of decoded
/// [`Message`]s and a `Sink` for outgoing ones.
pub type Framed<T> = tokio_util::codec::Framed<T, MessageCodec>;

/// Wrap `io` in a [`MessageCodec`] that accepts bodies up to `max_frame`
/// bytes.
pub fn framed<T: AsyncRead + AsyncWrite>(io: T, max_frame: usize) -> Framed<T> {
    tokio_util::codec::Framed::new(io, MessageCodec::new(max_frame))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::protocol::message::all_variants;
    use crate::protocol::PartyId;

    fn frame(msg: &Message) -> BytesMut {
        let mut buf = BytesMut::new();
        MessageCodec::default().encode(msg, &mut buf).unwrap();
        buf
    }

    /// A `Nack` whose JSON body is exactly `body_len` bytes long.
    fn nack_with_body_len(body_len: usize) -> Message {
        let overhead = encode(&Message::nack("")).unwrap().len();
        Message::nack("x".repeat(body_len - overhead))
    }

    #[test]
    fn free_functions_round_trip_every_variant() {
        for msg in all_variants() {
            let bytes = encode(&msg).unwrap();
            assert_eq!(decode(&bytes).unwrap(), msg);
        }
    }

    #[test]
    fn codec_round_trips_every_variant() {
        let mut codec = MessageCodec::default();
        for msg in all_variants() {
            let mut buf = BytesMut::new();
            codec.encode(msg.clone(), &mut buf).unwrap();
            let body_len = buf.len() - LENGTH_PREFIX_BYTES;
            assert_eq!(&buf[..LENGTH_PREFIX_BYTES], (body_len as u32).to_be_bytes());
            assert_eq!(codec.decode(&mut buf).unwrap(), Some(msg));
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn owned_and_borrowed_encoders_agree() {
        let msg = Message::Ping { id: PartyId(9) };
        let mut by_ref = BytesMut::new();
        let mut by_val = BytesMut::new();
        MessageCodec::default().encode(&msg, &mut by_ref).unwrap();
        MessageCodec::default().encode(msg, &mut by_val).unwrap();
        assert_eq!(by_ref, by_val);
    }

    #[test]
    fn bodies_of_exactly_1024_and_2048_bytes() {
        let mut codec = MessageCodec::default();
        for len in [1024usize, 2048] {
            let msg = nack_with_body_len(len);
            assert_eq!(encode(&msg).unwrap().len(), len);
            let mut buf = frame(&msg);
            assert_eq!(buf.len(), LENGTH_PREFIX_BYTES + len);
            assert_eq!(&buf[..LENGTH_PREFIX_BYTES], (len as u32).to_be_bytes());
            assert_eq!(codec.decode(&mut buf).unwrap(), Some(msg));
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn two_frames_in_one_buffer_decode_in_order() {
        let first = Message::Ping { id: PartyId(1) };
        let second = nack_with_body_len(1024);
        let mut buf = frame(&first);
        buf.extend_from_slice(&frame(&second));
        let mut codec = MessageCodec::default();
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(first));
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(second));
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        assert!(buf.is_empty());
    }

    #[test]
    fn frame_fed_one_byte_at_a_time() {
        let msg = Message::Deliver {
            to: PartyId(3),
            text: "split me".into(),
        };
        let bytes = frame(&msg);
        let mut codec = MessageCodec::default();
        let mut buf = BytesMut::new();
        let (last, head) = bytes.split_last().unwrap();
        for byte in head {
            buf.put_u8(*byte);
            assert_eq!(
                codec.decode(&mut buf).unwrap(),
                None,
                "after {} bytes",
                buf.len()
            );
        }
        buf.put_u8(*last);
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(msg));
        assert!(buf.is_empty());
    }

    #[test]
    fn oversized_prefix_is_rejected_before_the_body_arrives() {
        let mut codec = MessageCodec::new(64);
        let size = codec.max_frame() + 1;
        // Only the prefix, no body at all: the check must not wait for it.
        let mut buf = BytesMut::from(&(size as u32).to_be_bytes()[..]);
        match codec.decode(&mut buf) {
            Err(Error::FrameTooLarge { size: s, limit }) => {
                assert_eq!((s, limit), (size, 64));
            }
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
        // Exactly the limit is still acceptable: the codec waits for the body.
        let mut buf = BytesMut::from(&(64u32).to_be_bytes()[..]);
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        // The extreme prefix is rejected too, without any allocation attempt.
        let mut buf = BytesMut::from(&u32::MAX.to_be_bytes()[..]);
        assert!(matches!(
            codec.decode(&mut buf),
            Err(Error::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn garbage_body_of_valid_length_is_a_json_error() {
        let body = b"not json at all";
        let mut buf = BytesMut::from(&(body.len() as u32).to_be_bytes()[..]);
        buf.extend_from_slice(body);
        let mut codec = MessageCodec::default();
        assert!(matches!(codec.decode(&mut buf), Err(Error::Json(_))));
        // The bad frame was consumed; the stream can be read on.
        assert!(buf.is_empty());

        let mut buf = BytesMut::from(&0u32.to_be_bytes()[..]);
        assert!(matches!(codec.decode(&mut buf), Err(Error::Json(_))));
    }

    #[test]
    fn decode_eof_with_partial_frame_is_closed() {
        let mut codec = MessageCodec::default();
        let bytes = frame(&Message::Collect);

        let mut half = BytesMut::from(&bytes[..bytes.len() / 2]);
        assert!(matches!(codec.decode_eof(&mut half), Err(Error::Closed)));

        let mut prefix_only = BytesMut::from(&bytes[..2]);
        assert!(matches!(
            codec.decode_eof(&mut prefix_only),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn decode_eof_at_frame_boundary_is_none() {
        let mut codec = MessageCodec::default();
        let mut empty = BytesMut::new();
        assert_eq!(codec.decode_eof(&mut empty).unwrap(), None);

        let mut buf = frame(&Message::Delivered);
        assert_eq!(
            codec.decode_eof(&mut buf).unwrap(),
            Some(Message::Delivered)
        );
        assert_eq!(codec.decode_eof(&mut buf).unwrap(), None);
    }

    #[test]
    fn encoder_rejects_messages_above_max_frame() {
        let mut codec = MessageCodec::new(8);
        let mut buf = BytesMut::new();
        match codec.encode(Message::Collect, &mut buf) {
            Err(Error::FrameTooLarge { size, limit }) => {
                assert_eq!(size, encode(&Message::Collect).unwrap().len());
                assert_eq!(limit, 8);
            }
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
        assert!(
            buf.is_empty(),
            "nothing may be written for a rejected frame"
        );
    }

    #[test]
    fn default_codec_uses_the_configured_limit() {
        assert_eq!(
            MessageCodec::default().max_frame(),
            Limits::default().max_frame_bytes
        );
    }

    #[tokio::test]
    async fn framed_wraps_io_with_the_given_limit() {
        let (a, _b) = tokio::io::duplex(16);
        let f = framed(a, 1234);
        assert_eq!(f.codec().max_frame(), 1234);
    }

    /// Drive the codec against a real async byte stream whose chunks are
    /// tiny (7 bytes), so every frame arrives fragmented and frames
    /// coalesce arbitrarily; then hit EOF exactly at a frame boundary.
    #[tokio::test]
    async fn decodes_a_fragmenting_duplex_stream_to_eof() {
        let expected = all_variants();
        let (mut writer, mut reader) = tokio::io::duplex(7);
        let to_send = expected.clone();
        let producer = tokio::spawn(async move {
            for msg in &to_send {
                writer.write_all(&frame(msg)).await.unwrap();
            }
            // Dropping the writer is EOF for the reader.
        });

        let mut codec = MessageCodec::default();
        let mut buf = BytesMut::new();
        let mut got = Vec::new();
        loop {
            let n = reader.read_buf(&mut buf).await.unwrap();
            if n == 0 {
                while let Some(msg) = codec.decode_eof(&mut buf).unwrap() {
                    got.push(msg);
                }
                break;
            }
            while let Some(msg) = codec.decode(&mut buf).unwrap() {
                got.push(msg);
            }
        }
        producer.await.unwrap();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn truncated_duplex_stream_ends_in_closed() {
        // The buffer must hold the whole truncated write: the reader only
        // starts after `write_all` returns, on this same task.
        let (mut writer, mut reader) = tokio::io::duplex(256);
        let bytes = frame(&nack_with_body_len(1024));
        writer.write_all(&bytes[..100]).await.unwrap();
        drop(writer);

        let mut codec = MessageCodec::default();
        let mut buf = BytesMut::new();
        loop {
            let n = reader.read_buf(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            assert_eq!(codec.decode(&mut buf).unwrap(), None);
        }
        assert_eq!(buf.len(), 100);
        assert!(matches!(codec.decode_eof(&mut buf), Err(Error::Closed)));
    }
}
