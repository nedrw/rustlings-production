//! The engine: everything that can be judged without a socket.
//!
//! Nothing here may depend on [`crate::service`] — the daemon is a *user* of this module, and the
//! reason is the one `jobq/3.1-engine-boundary` established: the same engine runs in a test, in a
//! load generator and in a repair tool, and none of those has a configuration file or a signal
//! handler. The specs read the source for that rule, so it is a property and not a hope.
//!
//! Step `lakelet/4.1-ingest-protocol` brings the first inhabitant: [`spool`], the append-only file
//! every accepted frame is written to. `lakelet/4.3-write-path-adr` decides how hard it is flushed,
//! `lakelet/4.4-wal-recovery` turns it into a log that survives a torn write, and everything from
//! `4.7` on is written *by* a flush of it.

pub mod spool;

/// The write path, at the engine's root: a caller writes `lakelet::engine::Spool`.
pub use spool::Spool;
