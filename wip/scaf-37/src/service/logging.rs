//! Structured logging, configured once at startup.
//!
//! Same shape as `mailer/1.6-logging` and `relay`'s copy of it — one subscriber, JSON or
//! text, a run id on every line — with one difference that this project forces: the daemon has no
//! request to hang a span on, so the *worker and the job* are the fields. Step
//! `jobq/3.17-error-budget-alerts` is where those fields are judged as a contract.

use std::fmt;

use crate::engine::Error;

/// One process, one run: the value that ties every line of this process together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunId(String);

impl RunId {
    /// A run id for this process: its name and its process id, both of which an operator already
    /// has in `ps` and in a container label.
    #[must_use]
    pub fn of_this_process() -> Self {
        Self(format!(
            "{}-{}",
            std::env::args()
                .next()
                .as_deref()
                .map_or("jobq", |program| program
                    .rsplit('/')
                    .next()
                    .unwrap_or(program)),
            std::process::id()
        ))
    }

    /// A run id with a name a test picked, so two processes in one test can be told apart.
    #[must_use]
    pub fn named(name: &str) -> Self {
        Self(name.to_owned())
    }

    /// The value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Install the subscriber for the rest of the process's life.
///
/// Called once, before anything that might log: a daemon with no subscriber drops every line it
/// writes, and a daemon that installs two panics inside the second one.
///
/// # Errors
///
/// [`Error::Config`] when the level filter cannot be parsed — a typo in a
/// filter is worth a sentence, not a silently ignored log line.
pub fn init(
    format: crate::service::config::LogFormat,
    level: &str,
    run: &RunId,
) -> Result<(), Error> {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_new(level).map_err(|error| Error::Config {
        detail: format!("log level `{level}`: {error}"),
    })?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // stdout, because the harness (and an operator's `kubectl logs`) reads where the daemon was
        // told to write; stderr stays for the panic messages that never came through the log.
        .with_writer(std::io::stdout)
        .with_ansi(false);
    match format {
        crate::service::config::LogFormat::Text => builder.init(),
        crate::service::config::LogFormat::Json => builder.json().init(),
    }

    // The first line of every run, and the one an operator uses to tell two runs apart in a
    // collector: T1's `mailer/1.6-logging` asked for exactly this and it still holds.
    tracing::info!(run = %run, "logging installed");
    Ok(())
}
