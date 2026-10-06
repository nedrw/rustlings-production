//! The error type the whole crate answers with.
//!
//! Four variants, one per layer that can refuse something: the command line, the configuration, the
//! wire, and the store. A caller decides what to do from the variant, which is the point of a typed
//! error (`jobq/3.3-library-errors` argued it for a library; this is the same rule in a second
//! project).

use std::path::PathBuf;

/// Everything that can go wrong, named by where it went wrong.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A command line, a file or an environment variable said something impossible.
    #[error("invalid configuration: {detail}")]
    Invalid {
        /// What was wrong, phrased for the person who typed it.
        detail: String,
    },

    /// A configuration file could not be read or understood.
    #[error("cannot read the configuration file {path}: {source}")]
    Config {
        /// File that was being read.
        path: PathBuf,
        /// What the reader or the parser said.
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The bytes on the wire are not frames this build can read.
    #[error("protocol error: {detail}")]
    Protocol {
        /// Which part of the wire the reader was looking at.
        detail: String,
    },

    /// The store could not do what it was asked.
    #[error("store error: {source}")]
    Store {
        /// The cause, kept as a boxed error so the store's own type stays out of this crate's
        /// surface — a caller who needs it asks with `downcast_ref`.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl Error {
    /// The short name a log line and a spec use.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Invalid { .. } => "invalid",
            Error::Config { .. } => "config",
            Error::Protocol { .. } => "protocol",
            Error::Store { .. } => "store",
        }
    }
}
