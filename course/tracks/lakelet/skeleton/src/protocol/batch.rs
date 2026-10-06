//! What a payload means: a batch of samples.
//!
//! ```text
//!  0..4     u32, little endian: how many records follow
//!  then, per record:
//!     u16 little endian: length of the series name in bytes
//!     those bytes:       the series name (UTF-8: a metric name and its labels, as the sender wrote it)
//!     i64 little endian: the timestamp, milliseconds since the Unix epoch
//!     f64 little endian: the value
//! ```
//!
//! The layout is deliberately dull. `lakelet/4.7-columnar-segments` replaces it *inside the store*
//! with something that compresses, and this stays what a client sends: a wire format that needs a
//! decoder to be interesting is a wire format nobody debugs at 3am.

use crate::error::Error;

/// One measurement, as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// Series the value belongs to (a metric name and its labels, as text).
    pub series: String,
    /// Timestamp, milliseconds since the Unix epoch.
    pub ts_millis: i64,
    /// The value.
    pub value: f64,
}

/// Encode records into a payload.
#[must_use]
pub fn encode(records: &[Record]) -> Vec<u8> {
    let _ = records;
    todo!("step lakelet/4.1-ingest-protocol")
}

/// Read a payload into the records it carries.
///
/// # Errors
///
/// [`Error::Protocol`] when the count does not match the bytes, when a series name is not UTF-8, or
/// when the payload ends in the middle of a record — a batch is all of it or none of it, because a
/// half-written batch is exactly what a store must never accept as a whole.
pub fn decode(payload: &[u8]) -> Result<Vec<Record>, Error> {
    let _ = payload;
    todo!("step lakelet/4.1-ingest-protocol")
}
