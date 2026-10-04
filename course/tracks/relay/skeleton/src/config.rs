//! What the service reads at startup, and where each setting may come from.
//!
//! Step `relay/2.1-project-skeleton` implements the layering `mailer/1.4-config` established:
//! **defaults < file < environment < command line**, and the report [`Config::report`] prints is what
//! `--check-config` shows, so a deployment can ask what the service *would* run with without
//! starting it.
//!
//! Two rules this service keeps from T1, because both were learned the hard way there: a secret is
//! never printed (`database.url = set`, never the URL), and an environment variable wins over the
//! file, which wins over the default — in that order, tested together rather than one at a time.

use std::path::PathBuf;

use crate::error::Error;

/// Which format the log lines are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable, for a terminal.
    Pretty,
    /// One JSON object per line, for a log collector.
    Json,
}

impl LogFormat {
    /// Parse `pretty` or `json`.
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the text is neither `pretty` nor `json`.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let _ = text;
        todo!("step relay/2.1-project-skeleton")
    }

    /// The text form, as the configuration file writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LogFormat::Pretty => "pretty",
            LogFormat::Json => "json",
        }
    }
}

/// Where the service listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    /// Address to bind.
    pub host: String,
    /// Port to bind; 0 asks the kernel for one.
    pub port: u16,
}

/// What the service writes, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    /// `tracing` filter directive, e.g. `info` or `relay=debug`.
    pub level: String,
    /// Wire format.
    pub format: LogFormat,
}

/// Where the data lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Database {
    /// Postgres connection string, when one is configured.
    pub url: Option<String>,
}

/// Tenants seeded from configuration at startup (`name=key`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootstrap {
    /// One entry per tenant: the name and the API key it authenticates with.
    pub tenants: Vec<(String, String)>,
}

/// The effective configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Listen address and port.
    pub listen: Listen,
    /// Log level and format.
    pub log: Log,
    /// Database settings.
    pub database: Database,
    /// Tenants to seed on startup.
    pub bootstrap: Bootstrap,
}

/// Command line flags, the last layer of the stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    /// `--config <FILE>`: a TOML file of settings.
    pub config: Option<PathBuf>,
    /// `--listen-host <HOST>`.
    pub listen_host: Option<String>,
    /// `--listen-port <PORT>`.
    pub listen_port: Option<u16>,
    /// `--check-config`: print [`Config::report`] and exit.
    pub check_config: bool,
    /// `--help`.
    pub help: bool,
    /// `--version`.
    pub version: bool,
}

impl Flags {
    /// Parse everything after the program name.
    ///
    /// An unknown flag is an error, and so is a flag with no value: a service that ignores an
    /// argument it does not understand is a service whose configuration is a guess.
    /// # Errors
    ///
    /// Returns [`Error::Usage`] for an unknown flag, a flag missing its value, or a port that is
    /// not a number.
    pub fn from_args<I, S>(args: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let _ = args;
        todo!("step relay/2.1-project-skeleton")
    }
}

impl Config {
    /// Load the effective configuration: defaults, then the file, then the environment, then flags.
    ///
    /// `env` is the whole environment the caller wants considered, not the process's own — a test
    /// that had to mutate the process environment could not run in parallel with another one.
    /// # Errors
    ///
    /// Returns [`Error::Config`] when a layer contradicts another: an unreadable file, a value
    /// that is not the shape its setting expects, or a tenant entry that is not `name=api-key`.
    pub fn load(flags: &Flags, env: &[(String, String)]) -> Result<Self, Error> {
        let _ = (flags, env);
        todo!("step relay/2.1-project-skeleton")
    }

    /// The lines `--check-config` prints, one setting per line.
    ///
    /// The shape is part of the contract: `listen.host = …`, then `listen.port`, `log.level`,
    /// `log.format`, `database.url` (`set` or `not set` — never the value), and one line for the
    /// bootstrap tenants, which names them but not their keys.
    #[must_use]
    pub fn report(&self) -> Vec<String> {
        todo!("step relay/2.1-project-skeleton")
    }
}
