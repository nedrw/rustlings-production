//! What the daemon was asked to do, and where the answer came from.
//!
//! The layering is T1's `mailer/1.4-config` again, in a service that has no HTTP surface:
//! defaults in code, a TOML file next, the environment last, and the command line above all of
//! them — each layer saying which key it changed, so `--check-config` can tell an operator what the
//! process will actually do. One rule survives from that step unchanged: **the effective settings
//! can be printed, the connection string cannot**, because a report ends up in an issue tracker.

use std::path::PathBuf;

// The names a caller has to work with are the engine's re-exports (step
// `jobq/3.2-public-api-baseline`), so the service spells them the way its own callers do.
use crate::engine::{Error, Queue};

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
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self, Error> {
        let mut flags = Self::default();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--config" => flags.config = Some(PathBuf::from(value(&mut args, "--config")?)),
                "--worker-name" => flags.worker_name = Some(value(&mut args, "--worker-name")?),
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

impl LogFormat {
    /// The name a configuration file and the report use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LogFormat::Text => "text",
            LogFormat::Json => "json",
        }
    }

    /// Read the name.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for anything but `text` and `json`, so a typo in a configuration file is
    /// answered with a sentence instead of a log nobody can read.
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

/// One table of the configuration file. Every key is optional, because a file that has to repeat
/// the defaults is a file that goes stale.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    /// `[store]`.
    #[serde(default)]
    store: FileStore,
    /// `[worker]`.
    #[serde(default)]
    worker: FileWorker,
    /// `[log]`.
    #[serde(default)]
    log: FileLog,
}

/// `[store]` in a file.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStore {
    /// Connection string.
    #[serde(default)]
    url: Option<String>,
    /// Migrate on the way in.
    #[serde(default)]
    migrate: Option<bool>,
}

/// `[worker]` in a file.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileWorker {
    /// What this process calls itself.
    #[serde(default)]
    name: Option<String>,
    /// Queues to drain.
    #[serde(default)]
    queues: Option<Vec<String>>,
    /// Jobs per turn.
    #[serde(default)]
    batch: Option<usize>,
    /// Wait after an empty turn.
    #[serde(default)]
    poll_interval_ms: Option<u64>,
    /// How long the drain may take after a stop signal.
    #[serde(default)]
    shutdown_seconds: Option<u64>,
}

/// `[log]` in a file.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLog {
    /// Filter.
    #[serde(default)]
    level: Option<String>,
    /// `text` or `json`.
    #[serde(default)]
    format: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            store: StoreSettings {
                url: None,
                migrate: true,
            },
            worker: WorkerSettings {
                name: "jobqd".to_owned(),
                queues: Vec::new(),
                batch: 8,
                poll_interval_ms: 250,
                shutdown_seconds: 10,
            },
            log: LogSettings {
                level: "info".to_owned(),
                format: LogFormat::default(),
            },
        }
    }
}

impl Config {
    /// The values every layer starts from.
    fn defaults() -> Self {
        let mut config = Self::default();
        config.worker.queues = vec![Queue::default()];
        config
    }

    /// Merge defaults, the `--config` file, the environment and the flags.
    ///
    /// `env` is passed in rather than read here, so a test can state the environment it means
    /// (T1's `mailer/1.4-config` did this the same way, and for the same reason: a test that
    /// mutates the process environment cannot run beside another one).
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for a value that cannot be read (a bad number, an unknown log format) and
    /// for an environment variable whose name this build does not know; [`Error::Config`] when the
    /// file cannot be read or parsed.
    pub fn load(flags: &Flags, env: &[(String, String)]) -> Result<Self, Error> {
        let mut config = Self::defaults();
        if let Some(path) = &flags.config {
            config.apply_file(path)?;
        }
        config.apply_env(env)?;
        // The command line is the last word: an operator overriding one setting for one run should
        // not have to edit the file the deployment ships.
        if let Some(name) = &flags.worker_name {
            config.worker.name = name.clone();
        }
        config.validate()?;
        Ok(config)
    }

    /// The file layer.
    fn apply_file(&mut self, path: &std::path::Path) -> Result<(), Error> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Config {
            detail: format!("{} could not be read: {source}", path.display()),
        })?;
        let file: FileConfig = toml::from_str(&text).map_err(|error| Error::Config {
            detail: format!("{} could not be parsed: {error}", path.display()),
        })?;

        if let Some(url) = file.store.url {
            self.store.url = Some(url);
        }
        if let Some(migrate) = file.store.migrate {
            self.store.migrate = migrate;
        }
        if let Some(name) = file.worker.name {
            self.worker.name = name;
        }
        if let Some(queues) = file.worker.queues {
            self.worker.queues = parse_queues("worker.queues", &queues)?;
        }
        if let Some(batch) = file.worker.batch {
            self.worker.batch = batch;
        }
        if let Some(interval) = file.worker.poll_interval_ms {
            self.worker.poll_interval_ms = interval;
        }
        if let Some(seconds) = file.worker.shutdown_seconds {
            self.worker.shutdown_seconds = seconds;
        }
        if let Some(level) = file.log.level {
            self.log.level = level;
        }
        if let Some(format) = file.log.format {
            self.log.format = LogFormat::parse(&format)?;
        }
        Ok(())
    }

    /// The environment layer: `JOBQ__SECTION__KEY`, the convention T1 established.
    fn apply_env(&mut self, env: &[(String, String)]) -> Result<(), Error> {
        for (name, value) in env {
            let Some(key) = name.strip_prefix(ENV_PREFIX) else {
                continue;
            };
            let mut parts = key.split("__");
            match (parts.next(), parts.next(), parts.next()) {
                (Some("STORE"), Some("URL"), None) => self.store.url = Some(value.clone()),
                (Some("STORE"), Some("MIGRATE"), None) => {
                    self.store.migrate = parse_bool(name, value)?;
                }
                (Some("WORKER"), Some("NAME"), None) => self.worker.name = value.clone(),
                (Some("WORKER"), Some("QUEUES"), None) => {
                    let names: Vec<String> = value
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect();
                    self.worker.queues = parse_queues(name, &names)?;
                }
                (Some("WORKER"), Some("BATCH"), None) => {
                    self.worker.batch = parse_number(name, value)?;
                }
                (Some("WORKER"), Some("POLL_INTERVAL_MS"), None) => {
                    self.worker.poll_interval_ms = parse_number(name, value)?;
                }
                (Some("WORKER"), Some("SHUTDOWN_SECONDS"), None) => {
                    self.worker.shutdown_seconds = parse_number(name, value)?;
                }
                (Some("LOG"), Some("LEVEL"), None) => self.log.level = value.clone(),
                (Some("LOG"), Some("FORMAT"), None) => {
                    self.log.format = LogFormat::parse(value)?;
                }
                // A misspelled variable that is quietly ignored is how a process runs with a
                // setting nobody chose (the lesson of `mailer/1.4-config`).
                _ => {
                    return Err(Error::Invalid {
                        detail: format!("`{name}` is not a setting this daemon knows"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Settings that are individually valid but together are not.
    fn validate(&self) -> Result<(), Error> {
        if self.worker.queues.is_empty() {
            return Err(Error::Invalid {
                detail:
                    "at least one queue is required: a worker that drains nothing is a process \
                         that looks healthy and does nothing"
                        .to_owned(),
            });
        }
        if self.worker.batch == 0 {
            return Err(Error::Invalid {
                detail: "worker.batch must be at least 1".to_owned(),
            });
        }
        if self.worker.poll_interval_ms == 0 {
            return Err(Error::Invalid {
                detail: "worker.poll_interval_ms must be at least 1: a zero would busy-wait"
                    .to_owned(),
            });
        }
        if self.worker.shutdown_seconds == 0 {
            return Err(Error::Invalid {
                detail: "worker.shutdown_seconds must be at least 1".to_owned(),
            });
        }
        if self.log.level.trim().is_empty() {
            return Err(Error::Invalid {
                detail: "log.level cannot be empty".to_owned(),
            });
        }
        Ok(())
    }

    /// The effective settings, one `key = value` per line, with the connection string redacted.
    #[must_use]
    pub fn report(&self) -> Vec<String> {
        let queues: Vec<&str> = self.worker.queues.iter().map(Queue::as_str).collect();
        vec![
            format!(
                "store.url = {}",
                self.store
                    .url
                    .as_deref()
                    .map_or_else(|| "memory".to_owned(), redact)
            ),
            format!("store.migrate = {}", self.store.migrate),
            format!("worker.name = {}", self.worker.name),
            format!("worker.queues = {}", queues.join(",")),
            format!("worker.batch = {}", self.worker.batch),
            format!("worker.poll_interval_ms = {}", self.worker.poll_interval_ms),
            format!("worker.shutdown_seconds = {}", self.worker.shutdown_seconds),
            format!("log.level = {}", self.log.level),
            format!("log.format = {}", self.log.format.as_str()),
        ]
    }
}

/// A connection string with the credentials taken out of it.
///
/// The user is kept and the password is not: an operator matching a report against a
/// `pg_stat_activity` row needs the first, and nobody needs the second in a ticket.
fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    match rest.split_once('@') {
        Some((credentials, host)) => {
            let user = credentials
                .split_once(':')
                .map_or(credentials, |(user, _)| user);
            format!("{scheme}://{user}@{host}")
        }
        None => url.to_owned(),
    }
}

/// Parse a queue list, naming where it came from when one of the names is not valid.
fn parse_queues(source: &str, names: &[String]) -> Result<Vec<Queue>, Error> {
    if names.is_empty() {
        return Err(Error::Invalid {
            detail: format!("{source} must name at least one queue"),
        });
    }
    names
        .iter()
        .map(|name| {
            Queue::parse(name).map_err(|error| Error::Invalid {
                detail: format!("{source}: {error}"),
            })
        })
        .collect()
}

/// Parse a number that came from the environment, naming the variable when it is not one.
fn parse_number<T: std::str::FromStr>(name: &str, value: &str) -> Result<T, Error> {
    value.parse::<T>().map_err(|_| Error::Invalid {
        detail: format!("`{name}={value}` is not a number"),
    })
}

/// Parse a boolean that came from the environment.
fn parse_bool(name: &str, value: &str) -> Result<bool, Error> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        other => Err(Error::Invalid {
            detail: format!("`{name}={other}` is not a boolean: use `true` or `false`"),
        }),
    }
}
