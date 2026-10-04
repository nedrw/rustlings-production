//! What can go wrong, in the vocabulary of the caller.
//!
//! The engine is a library, so this enum is part of its public surface: a caller branches on the
//! variant, reads the source chain when it wants the detail, and never has to know which store,
//! which driver or which process produced it. Step `jobq/3.3-library-errors` is where that promise
//! gets written down and tested; here it is only the shape.

use thiserror::Error as ThisError;

use crate::engine::domain::JobId;

/// Everything the engine and its service can fail at.
#[derive(Debug, ThisError)]
pub enum Error {
    /// The caller's input was rejected before anything was written.
    #[error("invalid input: {detail}")]
    Invalid {
        /// What was wrong, phrased so the caller can fix the call.
        detail: String,
    },

    /// No job with that identity.
    #[error("no job {id}")]
    NotFound {
        /// Identity that was asked for.
        id: JobId,
    },

    /// The store refused the operation. Step `jobq/3.3-library-errors` is where this variant grows
    /// a `#[source]` chain instead of a string.
    #[error("store: {detail}")]
    Store {
        /// What the store said, in one line.
        detail: String,
    },

    /// The service could not read its configuration.
    #[error("configuration: {detail}")]
    Config {
        /// Which setting, and why it was refused.
        detail: String,
    },

    /// Something outside the program failed (a file, a socket, a child process).
    #[error("io: {source}")]
    Io {
        /// The underlying failure.
        #[from]
        source: std::io::Error,
    },
}

impl Error {
    /// Exit code for a binary that ends because of this error.
    ///
    /// One code per failure class, because a script's `$?` is an interface too (step
    /// `jobq/3.15-cli-contract` is where the classes are written down): a bad call is 2, a missing
    /// thing is 3, a refused store is 4, and a broken environment is 1.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Invalid { .. } => 2,
            Error::NotFound { .. } => 3,
            Error::Store { .. } => 4,
            Error::Config { .. } | Error::Io { .. } => 1,
        }
    }
}
