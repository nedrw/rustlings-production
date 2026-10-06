//! What the daemon adds around the engine: configuration, logging, the accept loop, signals.
//!
//! The layer rule is one-directional and a spec checks it: this module may use [`crate::engine`],
//! and [`crate::engine`] may not use this one.

pub mod config;
pub mod ingest;
pub mod logging;

// The modules a later step adds are declared inside this block: a scaffold may only write inside a
// `kiln:scaffold:begin`/`kiln:scaffold:end` region, which is what keeps your own edits safe
// (docs/authoring.md §5).
// kiln:scaffold:begin
// kiln:scaffold:end
