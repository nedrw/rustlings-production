//! One run, one id, one line per request — the shape `mailer/1.6-logging` taught.
//!
//! Step `relay/2.1-project-skeleton` implements it. What matters here is that the service says
//! *where it is listening* in a line a program can read (`address=127.0.0.1:49152` in the pretty
//! format, `"address":"127.0.0.1:49152"` in JSON): the test harness waits for exactly that line
//! instead of guessing a port, and an operator greps for it.

use std::fmt;
use std::net::SocketAddr;

use crate::config::LogFormat;

/// Identifies one run of the process; every line of that run carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunId(String);

impl RunId {
    /// An id for this process, random enough to be unique in a log collector.
    #[must_use]
    pub fn of_this_process() -> Self {
        todo!("step relay/2.1-project-skeleton")
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Install the subscriber that writes the log, once per process.
///
/// Called by `main.rs` only: a test that installed a subscriber would fight the runner for stdout.
pub fn init(format: LogFormat, level: &str, run: &RunId) {
    let _ = (format, level, run);
    todo!("step relay/2.1-project-skeleton")
}

/// Announce that the service is listening, in the line the harness (and an operator) looks for.
pub fn announce(run: &RunId, address: SocketAddr) {
    let _ = (run, address);
    todo!("step relay/2.1-project-skeleton")
}
