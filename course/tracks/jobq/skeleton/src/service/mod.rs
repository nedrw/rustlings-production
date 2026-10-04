//! The daemon: the shell around the engine that makes it a service.
//!
//! Two modules, one direction: [`config`] decides what the process was asked to do, and [`logging`]
//! decides where the answers go. Neither of them implements queueing — that is the engine's job, and
//! the arrow only points this way (step `jobq/3.1-engine-boundary`).

/// Usage text, printed by `--help` and after a bad argument.
pub const USAGE: &str = "\
usage: jobqd [OPTIONS]

OPTIONS:
    --config <FILE>          read settings from a TOML file
    --worker-name <NAME>     name this worker in the log and in the rows it holds
    --check-config           print the effective settings and exit
    --help                   print this help and exit
    --version                print the version and exit

Settings are read from the file given by --config and overridden by the environment, where every
setting is spelled `JOBQ__SECTION__KEY` (for example `JOBQ__STORE__URL`).
";

pub mod config;
pub mod logging;

pub use config::{Config, Flags};
pub use logging::RunId;

/// Build the store the settings name, announce that this process is draining, and wait for a
/// signal.
///
/// The order is the point, and it is the same one `mailer/1.9-postgres` established: everything
/// the process needs is in place *before* it says it is running, because a process that announces
/// first and fails later is one an orchestrator routes work to.
///
/// # Errors
///
/// [`crate::engine::error::Error::Config`] when the settings do not name a store this build knows
/// how to open, and whatever the store itself reports when it cannot be opened.
pub async fn serve(config: Config, run: RunId) -> Result<(), crate::engine::error::Error> {
    todo!("jobq/3.1-engine-boundary: open the store, announce the drain, wait for the signal")
}
