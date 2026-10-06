//! Logging, set up once at startup.
//!
//! Two rules, both learned from a line that could not be read back:
//!
//! * the *format* is a setting (a terminal wants text, a collector wants JSON), and the level filter
//!   is a setting too — so both are decided from the configuration instead of from a hardcoded
//!   `env_logger` call;
//! * colour is for a **terminal**, never for a pipe. `tracing` styles field names with escape
//!   sequences, which turns `accepted=3` into `accepted\u{1b}[0m\u{1b}[2m=\u{1b}[0m3` — a line that a
//!   person reads as the same thing and a program (a test, `grep`, a log shipper) cannot. So the
//!   subscriber asks whether stdout is a terminal and answers honestly: the same bytes for a human
//!   and for a machine.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

use crate::service::config::{Config, LogFormat};

/// Install the global subscriber described by `config`.
///
/// # Errors
///
/// [`crate::Error::Invalid`] when the level filter cannot be parsed (it comes from a file or the
/// environment, so it can be anything).
pub fn init(config: &Config) -> Result<(), crate::Error> {
    let filter = EnvFilter::try_new(&config.log_level).map_err(|error| crate::Error::Invalid {
        detail: format!("`{}` is not a log filter: {error}", config.log_level),
    })?;
    let ansi = std::io::stdout().is_terminal();
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi);
    match config.log_format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder.json().init(),
    }
    Ok(())
}
