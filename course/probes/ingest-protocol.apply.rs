//! probe = "ingest/protocol@apply"
//! title = "Refuse a length before you read it"
//! hint = "Check the four bytes of length against the limit, the fifth byte against the version this build speaks, and the payload against its checksum — each of them before anything is buffered."
//!
//! Step `lakelet/4.1-ingest-protocol` is about the framing a message needs when it arrives on a
//! stream, and each of the four fields exists because of a fact about a stream: bytes arrive in
//! pieces the reader does not choose. Every field is one decision with a cost:
//!
//! * **the length counts the payload, and it is read first.** Four bytes tell a reader how much more
//!   will be a whole frame, before it holds one byte of the payload — that is what makes a stream
//!   readable without waiting for the peer to close it;
//! * **the limit is checked in those four bytes.** A peer that declares four gigabytes is refused
//!   *before* a buffer is reserved for it, or the peer decides how much memory this process holds.
//!   The refusal is an answer, never a wait for bytes that will not come;
//! * **the version sits in front of the payload.** A frame from a build this one does not speak is
//!   refused from its fifth byte, so the version byte buys a decision instead of a buffered payload
//!   nobody can parse;
//! * **the checksum covers the payload and not the header.** A damaged payload is then a checksum
//!   failure and a damaged length is the length's own business, which are two different sentences
//!   at 3am. A checksum over the header would let a corrupted length pass by arithmetic, and the
//!   sentence a reader got would point at the wrong field;
//! * **a strict prefix is not an error.** [`Decoded::Incomplete`] carries the number of bytes a
//!   reader still needs. A reader that asked for one byte at a time would be correct and unusable.
//!
//! ## What you write
//!
//! * [`Frame::encode`] — the bytes to put on the wire: length, version, payload, checksum;
//! * [`decode`] — the frame at the start of a buffer, or how many more bytes it needs, or why these
//!   bytes are not a frame of this protocol.
//!
//! [`Frame`], [`Decoded`], [`Refused`], [`crc32`], [`u32_at`] and the constants are given. The layout
//! and the refusal vocabulary are the interface two builds talk through, and a probe that let you
//! invent either one would be checking a different protocol; the checksum is given for the reason in
//! the last section.
//!
//! ## What the tests ask
//!
//! A frame this file encoded is a frame it reads back, nine bytes of framing and all, and an empty
//! payload is a frame too. Every strict prefix of a frame comes back as `Incomplete`, asking for
//! more than nothing and for no more than the frame is missing. A length above the limit is refused
//! from its four bytes, naming the length it saw and the limit, and a length exactly *at* the limit
//! is not refused — those bytes are simply not here yet. A version this build does not speak is
//! refused from the five bytes that carry it. One flipped payload byte is a checksum refusal naming
//! both numbers; one flipped *length* byte is not a checksum refusal at all, because the checksum
//! covers the payload. A buffer holding two frames answers the first one and the bytes it used. And
//! `encode` refuses a payload the protocol cannot describe, which is the same limit from the other
//! side.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no socket, no daemon and no store**. The step's real mechanism is the
//! accept loop — the buffer that keeps what a `read` did not finish, the connection that ends on a
//! refusal while the process keeps serving, the spool the accepted payload is written to — and none
//! of that is in this file. What is here is the codec's decisions, which is the part of the step one
//! file can hold.
//!
//! Three narrowings, stated rather than hidden:
//!
//! * **no `read_all`**, the walk that answers the frames a buffer holds and how many bytes they used.
//!   It is a loop over [`decode`], and the habit it re-checks — keep the tail, because `read` is not
//!   a message boundary — is a property of a connection rather than of a buffer: with no socket, the
//!   loop could only be asserted against itself;
//! * **the payload is opaque** and there is no `batch` codec and no log here. That is what the
//!   framing layer claims its payload is, so nothing is missing from *this* subject;
//! * **the checksum is [`crc32`], written out bitwise.** The real step takes `crc32fast`, the one
//!   dependency of the ingest path, and a probe has no dependencies at all. Same polynomial, same
//!   numbers, no table: what the exercise is about is *which bytes* the checksum covers and whether
//!   the refusal happens before the read, not how fast it runs. A hand-rolled SHA-256 would be worse
//!   than no checksum at all (the reasoning of `sec/hmac-signing@apply`); the CRC-32 here is the same
//!   function the real decoder computes, so the frames in this file are frames that decoder reads.
//!
//! One deviation of the API, for the same reason: the step's `Frame::encode` is infallible, because
//! the payload it is handed comes from the batch encoder and is already inside the limit. Here it
//! answers a `Result`, so that the limit is one rule with two directions rather than a rule only the
//! reader keeps — a sender that could frame what every reader refuses has not sent a message.

/// The version this build speaks.
pub const VERSION: u8 = 1;

/// The bytes one length takes: four, little endian.
pub const LENGTH: usize = 4;

/// The largest payload this protocol accepts, in bytes (1 MiB).
pub const MAX_PAYLOAD: usize = 1_048_576;

/// Bytes of framing around a payload: four of length, one of version, four of checksum.
pub const OVERHEAD: usize = 9;

/// One message, as it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The protocol version the sender used.
    pub version: u8,
    /// The bytes the sender meant to send.
    pub payload: Vec<u8>,
}

impl Frame {
    /// A frame of this build's version carrying `payload`.
    #[must_use]
    pub fn new(payload: impl Into<Vec<u8>>) -> Self {
        Self {
            version: VERSION,
            payload: payload.into(),
        }
    }

    /// The bytes to write on the wire: [`OVERHEAD`] plus the payload.
    ///
    /// # Errors
    ///
    /// [`Refused::AboveLimit`] when the payload is longer than [`MAX_PAYLOAD`]: the limit is the
    /// protocol's, and it binds the sender for the same reason it binds the reader — a frame no
    /// reader will accept is not a message.
    pub fn encode(&self) -> Result<Vec<u8>, Refused> {
        todo!(
            "ingest/protocol@apply: the length, the version, the payload and the checksum over the payload"
        )
    }
}

/// What a buffer turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// A whole frame, and how many bytes of the buffer it used. A buffer may hold more than one.
    Frame(Frame, usize),
    /// Not enough bytes yet. `needed` is how many more will be enough to decide.
    Incomplete {
        /// How many more bytes the decoder needs.
        needed: usize,
    },
    /// These bytes are not a frame of this protocol, with the field that is wrong in it.
    Invalid(Refused),
}

/// Why these bytes are not a frame of this protocol, or why a payload cannot be framed.
///
/// Each variant carries the two numbers a log line needs — the value that was seen and the value
/// that was expected — because a refusal that only said "bad frame" would make the operator read hex
/// at 3am.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// A header states a payload length this protocol does not accept.
    AboveLimit {
        /// The length the header states, or the payload a sender asked to frame.
        length: usize,
        /// [`MAX_PAYLOAD`], the largest payload a frame of this protocol carries.
        limit: usize,
    },
    /// The version byte is not the version this build speaks.
    Version {
        /// The version the frame carries.
        seen: u8,
        /// [`VERSION`], the version this build speaks.
        spoken: u8,
    },
    /// The checksum the frame carries is not the checksum of its payload.
    Checksum {
        /// What the frame says its payload's checksum is.
        expected: u32,
        /// What the payload's bytes actually checksum to.
        found: u32,
    },
}

/// The CRC-32 (ISO-HDLC) of `bytes`: the checksum this format carries, as a `u32`.
///
/// Given rather than written, because the real step's checksum is a dependency (`crc32fast`) and a
/// probe has none: this is the same polynomial and the same numbers, computed bitwise instead of
/// through a table. See the last section for what that is worth and what it is not.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// The little-endian `u32` at `offset`, or `None` when the buffer does not hold four bytes there.
///
/// Given because the length field and the checksum are read through it: two readers of the same four
/// bytes are two chances for a reader to disagree with the writer about which field it is looking at.
#[must_use]
pub fn u32_at(buffer: &[u8], offset: usize) -> Option<u32> {
    let bytes = buffer.get(offset..offset + LENGTH)?;
    let mut array = [0u8; LENGTH];
    array.copy_from_slice(bytes);
    Some(u32::from_le_bytes(array))
}

/// Read one frame out of the start of `buffer`.
///
/// Never panics and never allocates the length a peer claims: a length above [`MAX_PAYLOAD`] is
/// refused as [`Decoded::Invalid`] before one byte of payload is reserved, which is the difference
/// between a protocol and a remote memory exhaustion.
#[must_use]
pub fn decode(buffer: &[u8]) -> Decoded {
    let _ = buffer;
    todo!(
        "ingest/protocol@apply: length, version, then payload and checksum — or how much more is needed, or the refusal"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload that is not round-numbered, so a length and a payload mixed up cannot pass.
    fn payload(marker: u8) -> Vec<u8> {
        (0..37u8).map(|index| marker.wrapping_add(index)).collect()
    }

    #[test]
    fn a_frame_round_trips_and_the_framing_is_nine_bytes() {
        assert_eq!(
            OVERHEAD, 9,
            "the framing is fixed: four of length, one of version, four of checksum"
        );
        assert_eq!(
            MAX_PAYLOAD, 1_048_576,
            "a mebibyte is the largest payload a peer may ask this process to hold"
        );

        let frame = Frame::new(payload(1));
        let bytes = frame.encode().expect("37 bytes are well inside the limit");
        assert_eq!(
            bytes.len(),
            OVERHEAD + frame.payload.len(),
            "a frame is its payload plus the framing, and nothing else"
        );
        assert_eq!(
            u32_at(&bytes, 0),
            Some(37),
            "the length counts the payload only, not the frame"
        );
        assert_eq!(
            bytes.get(LENGTH),
            Some(&VERSION),
            "the version rides in front of the payload"
        );
        match decode(&bytes) {
            Decoded::Frame(decoded, used) => {
                assert_eq!(
                    decoded, frame,
                    "the version and the payload come back unchanged"
                );
                assert_eq!(
                    used,
                    bytes.len(),
                    "a whole buffer is consumed by the frame it holds"
                );
            }
            other => panic!("a frame this file just encoded was not read back: {other:?}"),
        }

        // An empty payload is a frame too, and it is the smallest one there is.
        let empty = Frame::new(Vec::new())
            .encode()
            .expect("nothing is inside the limit");
        assert_eq!(empty.len(), OVERHEAD, "framing around no payload at all");
        assert_eq!(
            decode(&empty),
            Decoded::Frame(Frame::new(Vec::new()), OVERHEAD),
            "an empty body is a message: a peer that sent one byte of nothing is not the same as a \
             peer that sent nothing"
        );
    }

    #[test]
    fn every_strict_prefix_asks_for_what_is_missing() {
        let bytes = Frame::new(payload(2)).encode().expect("inside the limit");
        assert_eq!(
            decode(&[]),
            Decoded::Incomplete { needed: 4 },
            "an empty buffer is four bytes away from knowing how long the frame is"
        );
        for length in 0..bytes.len() {
            let prefix = bytes.get(..length).expect("a prefix of the frame");
            match decode(prefix) {
                Decoded::Incomplete { needed } => {
                    let missing = bytes.len() - length;
                    assert!(
                        needed > 0,
                        "a strict prefix of {length} byte(s) of a {} byte frame is not a whole one",
                        bytes.len()
                    );
                    assert!(
                        needed <= missing,
                        "a prefix of {length} byte(s) asked for {needed} more and is missing \
                         {missing}"
                    );
                }
                other => panic!("a prefix of {length} byte(s) answered {other:?}"),
            }
        }
    }

    #[test]
    fn a_length_above_the_limit_is_refused_from_its_four_bytes() {
        for length in [MAX_PAYLOAD + 1, 4_000_000, u32::MAX as usize] {
            let header = (length as u32).to_le_bytes();
            assert_eq!(
                decode(&header),
                Decoded::Invalid(Refused::AboveLimit {
                    length,
                    limit: MAX_PAYLOAD
                }),
                "a declared payload of {length} byte(s) is refused in the four bytes that carry it, \
                 before a byte of it is buffered: this is the difference between a protocol and a \
                 remote memory exhaustion"
            );
        }

        // The other direction of the same boundary: at the limit is legal, and the answer to its four
        // bytes is that the rest has not arrived.
        let at_limit = (MAX_PAYLOAD as u32).to_le_bytes();
        match decode(&at_limit) {
            Decoded::Incomplete { needed } => assert!(
                needed > 0 && needed <= OVERHEAD + MAX_PAYLOAD - LENGTH,
                "an at-the-limit frame is not here yet, so the answer is how much more is: {needed}"
            ),
            other => panic!("a payload exactly at the limit is legal, and answered {other:?}"),
        }
    }

    #[test]
    fn a_version_this_build_does_not_speak_is_refused_from_the_fifth_byte() {
        let seen = VERSION.wrapping_add(9);
        let mut header = (MAX_PAYLOAD as u32).to_le_bytes().to_vec();
        header.push(seen);
        assert_eq!(
            decode(&header),
            Decoded::Invalid(Refused::Version {
                seen,
                spoken: VERSION
            }),
            "five bytes are enough to refuse a frame from another build: the point of the version \
             byte is that its payload is never buffered"
        );

        // And this build's own version is not a version refusal: with only the header here, the
        // answer is that the rest has not arrived.
        let mut ours = 4u32.to_le_bytes().to_vec();
        ours.push(VERSION);
        match decode(&ours) {
            Decoded::Incomplete { .. } => {}
            other => panic!("a frame of this build's version is not a version failure: {other:?}"),
        }
    }

    #[test]
    fn the_checksum_covers_the_payload_and_a_damaged_length_is_not_its_failure() {
        let frame = Frame::new(payload(3));
        let bytes = frame.encode().expect("inside the limit");

        let mut damaged = frame.payload.clone();
        damaged[0] ^= 0x01;
        assert_eq!(
            decode(&{
                let mut flipped = bytes.clone();
                flipped[LENGTH + 1] ^= 0x01;
                flipped
            }),
            Decoded::Invalid(Refused::Checksum {
                expected: crc32(&frame.payload),
                found: crc32(&damaged),
            }),
            "a payload byte that changed is a checksum failure naming both numbers, and the frame is \
             not handed on"
        );

        // A version byte that changed is the version's failure, not the checksum's: two fields, two
        // sentences, so the log line points at the field that is wrong.
        let mut version = bytes.clone();
        version[LENGTH] = VERSION.wrapping_add(1);
        assert_eq!(
            decode(&version),
            Decoded::Invalid(Refused::Version {
                seen: VERSION.wrapping_add(1),
                spoken: VERSION,
            }),
            "a frame whose version byte changed is refused as a version, even though nothing else \
             about it moved"
        );

        // A damaged length, on the other hand, cannot be a checksum failure: the checksum covers the
        // payload, so what changes is where this frame is judged to end.
        let mut longer = bytes.clone();
        longer[0] = (frame.payload.len() as u32 + 4).to_le_bytes()[0];
        match decode(&longer) {
            Decoded::Incomplete { .. } => {}
            other => panic!(
                "a length that moved by four bytes is not a checksum failure — the checksum cannot \
                 pass or fail by arithmetic on the header — and it answered {other:?}"
            ),
        }
    }

    #[test]
    fn the_limit_is_the_protocols_and_binds_the_sender_too() {
        let too_big = Frame::new(vec![0u8; MAX_PAYLOAD + 1]);
        assert_eq!(
            too_big.encode(),
            Err(Refused::AboveLimit {
                length: MAX_PAYLOAD + 1,
                limit: MAX_PAYLOAD
            }),
            "a sender may not frame what every reader refuses: the limit is a property of the \
             protocol, not of one side of it"
        );
        assert_eq!(
            Frame::new(vec![0u8; MAX_PAYLOAD])
                .encode()
                .map(|bytes| bytes.len()),
            Ok(OVERHEAD + MAX_PAYLOAD),
            "a payload exactly at the limit is a frame"
        );
    }

    #[test]
    fn one_buffer_can_hold_more_than_one_frame() {
        let first = Frame::new(payload(4)).encode().expect("inside the limit");
        let second = Frame::new(payload(5)).encode().expect("inside the limit");
        let mut both = first.clone();
        both.extend_from_slice(&second);

        match decode(&both) {
            Decoded::Frame(frame, used) => {
                assert_eq!(
                    frame.payload,
                    payload(4),
                    "the frame at the start of the buffer"
                );
                assert_eq!(
                    used,
                    first.len(),
                    "and how many bytes it used, which is the number a reader drains"
                );
                assert!(
                    used < both.len(),
                    "the second frame is still in the buffer: {used} of {} byte(s)",
                    both.len()
                );
            }
            other => panic!("a buffer holding two frames answered {other:?}"),
        }
    }
}
