//! Logging, set up once at startup.
//!
//! The rule from T1 that matters here: the *format* is a setting (a terminal wants text, a collector
//! wants JSON), and the level filter is a setting too — so both are decided from the configuration
//! instead of from a hardcoded `env_logger` call.

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
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match config.log_format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder.json().init(),
    }
    Ok(())
}
