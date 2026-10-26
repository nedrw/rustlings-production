//! The `meshkv` distributed key-value store: nodes, a consensus log, and the state machine the log
//! applies to.
//!
//! Step `meshkv/5.1-multi-node-topology` is what makes this crate work; the modules below are
//! declared with their signatures and their contracts, and the bodies are the step's work.
//!
//! The split is the one `jobq/3.1-engine-boundary` argued for and `lakelet/4.1-ingest-protocol`
//! kept: what can be judged without a socket lives on its own, and the daemon is the thin part that
//! binds, serves and stops. Here the split is sharper than in either earlier project, because a
//! distributed system has to test *without* trusting the network: [`cluster`] and the node's
//! handshake are pure values and one line of text, so the steps after this one can drive whole
//! clusters through a simulated network instead of a real one.

pub mod cli;
pub mod cluster;
pub mod error;
pub mod node;

// The modules a later step adds are declared inside this block: a scaffold may only write inside a
// `kiln:scaffold:begin`/`kiln:scaffold:end` region, which is what keeps your own edits safe
// (docs/authoring.md §5).
// kiln:scaffold:begin
// kiln:scaffold:end

/// The crate's error type, at the root so a caller writes `meshkv::Error`.
pub use error::Error;
