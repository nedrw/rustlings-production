//! What the daemon adds around the engine: configuration, logging, the accept loop, signals.
//!
//! The layer rule is one-directional and a spec checks it: this module may use [`crate::engine`],
//! and [`crate::engine`] may not use this one.

pub mod config;
pub mod ingest;
pub mod logging;
