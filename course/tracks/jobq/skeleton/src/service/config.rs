//! What the daemon was asked to do, and where the answer came from.
//!
//! The layering is T1's `mailer/1.4-config` again, in a service that has no HTTP surface:
//! defaults in code, a TOML file next, the environment last, and the command line above all of
//! them — each layer saying which key it changed, so `--check-config` can tell an operator what the
//! process will actually do. One rule survives from that step unchanged: **the effective settings
//! can be printed, the connection string cannot**, because a report ends up in an issue tracker.

use std::path::PathBuf;

use crate::engine::domain::Queue;
use crate::engine::error::Error;

/// Environment prefix every setting is spelled with (`JOBQ__STORE__URL`).
pub const ENV_PREFIX: &str = "JOBQ__";

/// The command line, parsed but not yet merged with anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    /// `--config <FILE>`.
    pub config: Option<PathBuf>,
    /// `--check-config`: print the effective settings and exit.
    pub check_config: bool,
    /// `--worker-name <NAME>`: what this process calls itself.
    pub worker_name: Option<String>,
    /// `--help`.
    pub help: bool,
    /// `--version`.
    pub version: bool,
}

impl Flags {
    /// Parse arguments (without the program name).
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for an unknown flag or a flag whose value is missing: a daemon that
    /// ignores an argument it does not understand is a daemon whose operator believes it did
    /// something it did not.
    pub fn from_args(args: impl Iterator<Item = String>) -> Result<Self, Error> {
        let _ = args;
        todo!("jobq/3.1-engine-boundary: parse the command line")
    }
}

/// Where the log goes and in what shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable lines, for a terminal.
    #[default]
    Text,
    /// One JSON object per line, for a collector.
    Json,
}

/// Everything the daemon needs to know about the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSettings {
    /// Filter, in `tracing`'s `EnvFilter` syntax.
    pub level: String,
    /// Line shape.
    pub format: LogFormat,
}

/// Where the jobs are and whether to migrate them on the way in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSettings {
    /// Connection string. `None` means the daemon was not pointed at a store yet.
    pub url: Option<String>,
    /// Apply the embedded migrations before draining anything.
    pub migrate: bool,
}

/// How the daemon drains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSettings {
    /// What this process calls itself in the log and in the rows it holds.
    pub name: String,
    /// Queues to drain, in the order they were configured.
    pub queues: Vec<Queue>,
    /// How many jobs one turn asks for.
    pub batch: usize,
    /// How long to wait before asking again after an empty turn.
    pub poll_interval_ms: u64,
    /// How long the drain may take after a stop signal before the process ends anyway.
    pub shutdown_seconds: u64,
}

/// The effective settings: every layer already applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Store connection.
    pub store: StoreSettings,
    /// Drain behaviour.
    pub worker: WorkerSettings,
    /// Log shape.
    pub log: LogSettings,
}

impl Config {
    /// Merge defaults, the `--config` file, the environment and the flags.
    ///
    /// `env` is passed in rather than read here, so a test can state the environment it means
    /// (T1's `mailer/1.4-config` did this the same way, and for the same reason: a test that
    /// mutates the process environment cannot run beside another one).
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for a value that cannot be read (a bad number, an unknown log format);
    /// [`Error::Config`] when the file cannot be read or parsed.
    pub fn load(flags: &Flags, env: &[(String, String)]) -> Result<Self, Error> {
        let _ = (flags, env);
        todo!("jobq/3.1-engine-boundary: merge defaults, file, environment and flags")
    }

    /// The effective settings, one `key = value` per line, with the connection string redacted.
    #[must_use]
    pub fn report(&self) -> Vec<String> {
        let _ = self;
        todo!("jobq/3.1-engine-boundary: print what this process will do, without the password")
    }
}
