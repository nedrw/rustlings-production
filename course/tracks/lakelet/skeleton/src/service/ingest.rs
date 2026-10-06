//! The accept loop: what happens to a connection once the kernel hands it over.
//!
//! Step `lakelet/4.1-ingest-protocol` writes the loop, and the interesting part of it is not "accept
//! a connection" — it is what a *stream* does to a naive reader:
//!
//! * `read` returns what has arrived, not what was sent: a frame can arrive in two pieces, and two
//!   frames can arrive in one;
//! * a peer can send half a frame and stop, which must not leave the daemon holding a buffer for
//!   ever;
//! * a peer can send a length that would allocate a gigabyte, which must be refused *before* the
//!   allocation and not after;
//! * every accepted payload goes to the store exactly once, and an accepted payload is one the
//!   client is allowed to believe is safe: [`Spool::append`] answers only when the record can be read
//!   back.
//!
//! The connection's counters are logged (`accepted`, `rejected`, `bytes`) because that is how an
//! operator sees a client that is sending garbage, and the spec reads those lines.

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::engine::Spool;
use crate::error::Error;

/// How many bytes are read from a connection at a time.
///
/// Bigger than a frame's header and smaller than the largest payload: a reader that asks for exactly
/// what it still needs is a reader that makes one syscall per frame, and one that asks for a
/// megabyte is one that holds a megabyte per idle connection.
pub const READ_CHUNK: usize = 64 * 1024;

/// Counters one connection (and the daemon) reports.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    /// Frames accepted and stored.
    pub accepted: u64,
    /// Frames refused, with the reason in the log.
    pub rejected: u64,
    /// Payload bytes stored.
    pub bytes: u64,
}

/// Serve connections until `shutdown` resolves.
///
/// # Errors
///
/// [`Error::Store`] when the store cannot be written to (which is fatal: a store that cannot store
/// must not keep accepting), and [`Error::Protocol`] only for a failure of the listener itself — a
/// peer that sends nonsense closes its own connection and is counted instead.
///
/// # Panics
///
/// Never: the loop's job is to keep one bad client from ending the process.
pub async fn serve(
    listener: TcpListener,
    _store: Arc<Mutex<Spool>>,
    _shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), Error> {
    let _ = listener;
    todo!("step lakelet/4.1-ingest-protocol")
}
