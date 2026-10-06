//! Framing: one message on a stream of bytes.
//!
//! The layout step `lakelet/4.1-ingest-protocol` pins:
//!
//! ```text
//!  0..4        u32, little endian: length of the payload in bytes
//!  4           u8: protocol version (`VERSION`)
//!  5..5+n      the payload
//!  5+n..9+n    u32, little endian: crc32 of the payload
//! ```
//!
//! The header is fixed at nine bytes of overhead, the length counts the **payload only**, and the
//! checksum covers the payload and not the header — three choices with reasons a spec pins:
//! the reader can decide how much more to read from the length *before* it has any bytes, a version
//! byte can be read without knowing the length's semantics, and a corrupted length cannot make the
//! checksum pass by accident.

/// The version this build speaks.
pub const VERSION: u8 = 1;

/// The largest payload this protocol accepts, in bytes (1 MiB).
pub const MAX_PAYLOAD: usize = 1_048_576;

/// Bytes of framing around a payload: four of length, one of version, four of checksum.
pub const OVERHEAD: usize = 9;

/// One message, as it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Protocol version the sender used.
    pub version: u8,
    /// The bytes the sender meant to send.
    pub payload: Vec<u8>,
}

/// What a buffer turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// A whole frame, and how many bytes of the buffer it used. A buffer may hold more.
    Frame(Frame, usize),
    /// Not enough bytes yet. `needed` is how many more will be enough to decide — the number a
    /// reader uses to ask for a sensible chunk instead of one byte at a time.
    Incomplete {
        /// How many more bytes the decoder needs.
        needed: usize,
    },
    /// These bytes are not a frame of this protocol, with the sentence that says why.
    Invalid {
        /// What is wrong, phrased for a log line and for the client that sent it.
        reason: String,
    },
}

impl Frame {
    /// A frame of this version carrying `payload`.
    #[must_use]
    pub fn new(payload: impl Into<Vec<u8>>) -> Self {
        Self {
            version: VERSION,
            payload: payload.into(),
        }
    }

    /// The bytes to write on the wire: [`OVERHEAD`] plus the payload.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let _ = self;
        todo!("step lakelet/4.1-ingest-protocol")
    }
}

/// Read one frame out of the start of `buffer`.
///
/// Never panics and never allocates the length a peer claims: a length above [`MAX_PAYLOAD`] is
/// refused as [`Decoded::Invalid`] before a single byte of payload is reserved, which is the
/// difference between a protocol and a remote memory exhaustion.
#[must_use]
pub fn decode(buffer: &[u8]) -> Decoded {
    let _ = buffer;
    todo!("step lakelet/4.1-ingest-protocol")
}
