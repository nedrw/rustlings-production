//! The `lakelet` telemetry store: the engine, the wire protocol, and the daemon that runs them.
//!
//! Step `lakelet/4.1-ingest-protocol` is what makes this crate work; the modules below are declared
//! with their signatures and their contracts, and the bodies are the step's work.
//!
//! The split is the one `jobq/3.1-engine-boundary` argued for, and it has the same reason here: the
//! **engine** is everything that can be judged without a socket — the frame codec, the record
//! format, the write path, the query engine — and the **service** is what the daemon adds around it:
//! configuration, logging, the accept loop, signals. A dependency from [`engine`] to [`service`] is a
//! bug the course checks for (one spec reads the source), because it is the one boundary a store can
//! never un-cross: the engine is what runs in a load generator, a test, and a repair tool.

pub mod engine;
pub mod error;
pub mod protocol;
pub mod service;

/// The crate's error type, at the root so a caller writes `lakelet::Error`.
pub use error::Error;
