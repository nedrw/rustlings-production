//! The ingest protocol, as bytes.
//!
//! Two layers, and step `lakelet/4.1-ingest-protocol` implements both:
//!
//! * [`frame`] — how a message is *delimited* on a stream: a length, a version, the payload and a
//!   checksum. A stream gives you bytes and nothing else: no message boundaries, no guarantee that
//!   one `read` returns one message, and no guarantee that the other end meant what it sent. The
//!   decoder therefore answers three ways, and [`frame::Decoded`] is that answer.
//! * [`batch`] — what the payload *means*: a count and that many records, each a series name, a
//!   timestamp in milliseconds and a value.
//!
//! The split is not decoration. The frame layer is the part that has to survive a hostile peer (a
//! length of four billion bytes, a checksum that does not match, a version from the future), and the
//! batch layer is the part a later step replaces with a columnar layout (`lakelet/4.7-columnar-segments`)
//! without touching a single line of the framing.

pub mod batch;
pub mod frame;
