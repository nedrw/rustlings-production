//! What the daemon reads at startup, and where each setting may come from.
//!
//! The layering is the one `mailer/1.4-layered-config` established and `jobq/3.4-feature-assembly`
//! took to design depth — **defaults < file < environment < command line** — and step
//! `lakelet/4.1-ingest-protocol` re-stands it in the fourth project rather than inventing a fifth way
//! to read a port number. Two properties the specs rely on:
//!
//! * [`Config::report`] prints **every** setting as `key = value`, which is what `--check-config`
//!   shows: a deployment can ask what the daemon *would* run with without starting one. Later steps
//!   add keys to this report (never change or remove one — a script may be parsing it);
//! * a key nobody recognises is a **refusal**, not a default: a configuration file with a typo in it
//!   must not silently serve on the wrong port.

use std::path::PathBuf;

use crate::error::Error;

/// Which format the log lines are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable, for a terminal.
    Text,
    /// One JSON object per line, for a log collector.
    Json,
}

impl LogFormat {
    /// The text form, as the configuration file writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LogFormat::Text => "text",
            LogFormat::Json => "json",
        }
    }

    /// Read the text form.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for anything but `text` and `json`, so a typo in a configuration file is
    /// answered with a sentence instead of a logger nobody can read.
    pub fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "text" => Ok(LogFormat::Text),
            "json" => Ok(LogFormat::Json),
            other => Err(Error::Invalid {
                detail: format!("`{other}` is not a log format: use `text` or `json`"),
            }),
        }
    }
}

/// Everything the daemon needs to start, once the four layers had their say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Address the ingest port binds (port `0` asks the kernel, which is what the tests use).
    pub listen_addr: String,
    /// Directory the store owns.
    pub store_dir: PathBuf,
    /// Verbosity filter (`info`, `debug`, `lakelet=debug`, …).
    pub log_level: String,
    /// Log format.
    pub log_format: LogFormat,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:7000".to_owned(),
            store_dir: PathBuf::from("data"),
            log_level: "info".to_owned(),
            log_format: LogFormat::Text,
        }
    }
}

impl Config {
    /// The settings as `key = value` lines, one per line, in a fixed order.
    ///
    /// Fixed order and no secrets: this text is what a deployment diffs between two machines.
    #[must_use]
    pub fn report(&self) -> String {
        format!(
            "listen.addr = {}\nstore.dir = {}\nlog.level = {}\nlog.format = {}",
            self.listen_addr,
            self.store_dir.display(),
            self.log_level,
            self.log_format.as_str()
        )
    }
}

/// What the command line asked for, before any of it is read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Flags {
    /// `--config <path>`.
    pub config: Option<PathBuf>,
    /// `--listen-addr <addr>`.
    pub listen_addr: Option<String>,
    /// `--store-dir <path>`.
    pub store_dir: Option<PathBuf>,
    /// `--log-level <filter>`.
    pub log_level: Option<String>,
    /// `--log-format <text|json>`.
    pub log_format: Option<String>,
    /// `--check-config`: print the report and exit without starting.
    pub check_config: bool,
    /// `--help` / `-h`.
    pub help: bool,
    /// `--version`.
    pub version: bool,
}

impl Flags {
    /// Read the command line.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for an unknown flag or a flag whose value is missing: a daemon that
    /// accepts what it does not understand starts with the wrong settings and never says so. This is
    /// also what the `exit-code` gate probes every binary with.
    pub fn from_args(args: impl Iterator<Item = String>) -> Result<Self, Error> {
        let mut flags = Self::default();
        let mut args = args;
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--config" => flags.config = Some(PathBuf::from(value(&mut args, "--config")?)),
                "--listen-addr" => flags.listen_addr = Some(value(&mut args, "--listen-addr")?),
                "--store-dir" => {
                    flags.store_dir = Some(PathBuf::from(value(&mut args, "--store-dir")?));
                }
                "--log-level" => flags.log_level = Some(value(&mut args, "--log-level")?),
                "--log-format" => flags.log_format = Some(value(&mut args, "--log-format")?),
                "--check-config" => flags.check_config = true,
                "--help" | "-h" => flags.help = true,
                "--version" => flags.version = true,
                other => {
                    return Err(Error::Invalid {
                        detail: format!("unknown argument `{other}`"),
                    });
                }
            }
        }
        Ok(flags)
    }
}

/// The value of a flag that needs one, or a sentence naming the flag that was left hanging.
fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, Error> {
    args.next().ok_or_else(|| Error::Invalid {
        detail: format!("`{flag}` needs a value"),
    })
}

/// One table of the configuration file. Every key is optional: a file that has to repeat the
/// defaults is a file that goes stale.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    /// `[listen]`.
    #[serde(default)]
    listen: FileListen,
    /// `[store]`.
    #[serde(default)]
    store: FileStore,
    /// `[log]`.
    #[serde(default)]
    log: FileLog,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileListen {
    addr: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStore {
    dir: Option<PathBuf>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLog {
    level: Option<String>,
    format: Option<String>,
}

/// Build the configuration from the four layers, in order.
///
/// # Errors
///
/// [`Error::Config`] when the file cannot be read or parsed, and [`Error::Invalid`] when an
/// environment variable or a flag carries something this daemon cannot use.
pub fn resolve(
    flags: &Flags,
    environment: impl Fn(&str) -> Option<String>,
) -> Result<Config, Error> {
    let mut config = Config::default();

    if let Some(path) = &flags.config {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Config {
            path: path.clone(),
            source: Box::new(source),
        })?;
        let file: FileConfig = toml::from_str(&text).map_err(|source| Error::Config {
            path: path.clone(),
            source: Box::new(source),
        })?;
        if let Some(addr) = file.listen.addr {
            config.listen_addr = addr;
        }
        if let Some(dir) = file.store.dir {
            config.store_dir = dir;
        }
        if let Some(level) = file.log.level {
            config.log_level = level;
        }
        if let Some(format) = file.log.format {
            config.log_format = LogFormat::parse(&format)?;
        }
    }

    if let Some(addr) = environment("LAKELET_LISTEN_ADDR") {
        config.listen_addr = addr;
    }
    if let Some(dir) = environment("LAKELET_STORE_DIR") {
        config.store_dir = PathBuf::from(dir);
    }
    if let Some(level) = environment("LAKELET_LOG_LEVEL") {
        config.log_level = level;
    }
    if let Some(format) = environment("LAKELET_LOG_FORMAT") {
        config.log_format = LogFormat::parse(&format)?;
    }

    if let Some(addr) = &flags.listen_addr {
        config.listen_addr = addr.clone();
    }
    if let Some(dir) = &flags.store_dir {
        config.store_dir = dir.clone();
    }
    if let Some(level) = &flags.log_level {
        config.log_level = level.clone();
    }
    if let Some(format) = &flags.log_format {
        config.log_format = LogFormat::parse(format)?;
    }

    Ok(config)
}
