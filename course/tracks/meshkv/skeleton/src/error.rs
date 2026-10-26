//! The error type the whole crate answers with.
//!
//! Three variants, one per layer that can refuse something: the command line and the cluster
//! specification, the wire, and the sockets underneath both. A caller decides what to do from the
//! variant, which is the point of a typed error (`jobq/3.3-library-errors` argued it for a library;
//! this is the same rule in a project where "what to do" includes "retry against another node").

use std::io;

/// Everything that can go wrong, named by where it went wrong.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A command line, a cluster specification or a configuration said something impossible.
    #[error("invalid configuration: {detail}")]
    Invalid {
        /// What was wrong, phrased for the person who typed it.
        detail: String,
    },

    /// The bytes on the wire are not what this build's handshake says.
    #[error("protocol error: {detail}")]
    Protocol {
        /// Which part of the wire the reader was looking at.
        detail: String,
    },

    /// A socket refused, dropped or could not be reached.
    #[error("network error: {source}")]
    Network {
        /// The cause, kept as the platform's own error rather than a string.
        #[source]
        source: io::Error,
    },
}

/// The crate's result type: everything answers with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// A configuration refusal.
    #[must_use]
    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::Invalid {
            detail: detail.into(),
        }
    }

    /// A wire refusal.
    #[must_use]
    pub fn protocol(detail: impl Into<String>) -> Self {
        Self::Protocol {
            detail: detail.into(),
        }
    }
}
