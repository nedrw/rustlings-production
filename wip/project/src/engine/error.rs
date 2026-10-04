//! What can go wrong, in the vocabulary of the caller.
//!
//! The engine is a library, so this enum is part of its public surface: a caller branches on the
//! variant, reads the source chain when it wants the detail, and never has to know which store,
//! which driver or which process produced it. Step `jobq/3.3-library-errors` is where that promise
//! stops being a shape and becomes four things a test can hold this file to:
//!
//! * **a variant is the caller's decision.** The names here are what a caller matches on, so the
//!   list only grows (`#[non_exhaustive]`, step `jobq/3.2-public-api-baseline`) and a variant that
//!   exists keeps its shape: adding a failure means adding a variant, not redefining one.
//! * **a cause is a `#[source]`, not a sentence.** The two variants that carry somebody else's
//!   failure — the store's, and the operating system's — keep that failure whole, so `source()`
//!   walks to the thing that actually went wrong. A caller that knows the concrete type gets it back
//!   with `downcast_ref`; a caller that does not gets its `Display`.
//! * **the `Display` text is a promise.** It is what an operator reads in a log line, and
//!   `mailer/1.5-errors` already pinned the same kind of promise for one service. Rewriting a
//!   message is a change to the interface, so a test in this step pins every one of them.
//! * **the categories are the interface.** `is_retryable()` is the engine's answer to "is another
//!   attempt worth anything?", and `exit_code()` is the answer a shell reads; both are decisions a
//!   caller makes on this type alone rather than by reading a message.
//!
//! `jobq/3.6-transaction-isolation` adds the fifth class, and it is the smallest honest one: a store
//! that refused a *transaction* because somebody else got there first is not a store that is broken
//! — it is a store that answered "not this time", and the engine has a word for that
//! ([`Error::Conflict`]) plus a loop that acts on it (`crate::engine::retry`). It is a new variant
//! rather than a field on `Store` because 3.3's promise is the shape of a variant, and it is not a
//! string because a caller that wants the database's own code can walk `source()` to the driver's
//! error and read it there.

use thiserror::Error as ThisError;

use crate::engine::domain::JobId;

/// Everything the engine and its service can fail at.
///
/// `#[non_exhaustive]` (step `jobq/3.2-public-api-baseline`): whether it is the store's failure or
/// the environment's, a caller has to plan for a variant it has never seen. Growing the list is the
/// *compatible* way to add a failure, and it is the reason the store's cause is boxed rather than
/// named: the day `jobq/3.5-store-port-conformance` hands the engine a `sqlx::Error`, this enum
/// must not have to move for it.
#[non_exhaustive]
#[derive(Debug, ThisError)]
pub enum Error {
    /// The caller's input was rejected before anything was written.
    ///
    /// Not retryable: the same call gets the same answer, and the caller is the one who can fix it.
    #[error("invalid input: {detail}")]
    Invalid {
        /// What was wrong, phrased so the caller can fix the call.
        detail: String,
    },

    /// No job with that identity.
    ///
    /// Not retryable: looking again does not create the row. A caller that has *lost* the identity
    /// has a different problem, and `get` answering `Ok(None)` is how the port says so.
    #[error("no job {id}")]
    NotFound {
        /// Identity that was asked for.
        id: JobId,
    },

    /// The store refused the operation.
    ///
    /// Retryable, as a class: a backend that could not answer is the one failure another attempt can
    /// change. The granularity is the backend's to improve — telling a serialisation conflict from a
    /// constraint violation needs a durable backend and an isolation level, which is the subject of
    /// `jobq/3.6-transaction-isolation` — and what a caller can hold the engine to today is the
    /// class.
    #[error("store: {source}")]
    Store {
        /// What the backend said, exactly as it said it.
        ///
        /// Boxed, so that the driver's type stays on the backend's side of the port: an engine whose
        /// public enum named `sqlx::Error` would have put a database in a caller's vocabulary, and
        /// the caller would have to be rebuilt the day the backend changes.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// The store refused the transaction because another writer got there first.
    ///
    /// Retryable, and more precisely than `Store` is: the four-oh-oh-oh-ones. PostgreSQL raises
    /// `40001` when a transaction at an isolation level that holds a snapshot loses a row to a
    /// concurrent commit, and `40P01` when it detects a deadlock; both mean "the same statements
    /// have not been tried yet", which is the one thing a retry can change. That is why the durable
    /// backend tells them apart from every other refusal it can answer with, and why the class
    /// exists at all rather than being folded into `Store` with a `detail` string nobody can branch
    /// on.
    ///
    /// What a caller does with it is a policy and not this enum's business: the bound, and whether
    /// there is a wait between attempts, belong to whoever is running the loop
    /// (`crate::engine::retry` is the engine's own answer, and `jobq/3.8-clock-injection` is where a
    /// backoff would get a clock to measure with).
    #[error("conflict: {source}")]
    Conflict {
        /// The driver's refusal, whole. The database's own code (`40001`) and message are in there:
        /// a `downcast_ref` reaches them, and a caller that only wants the sentence gets it from
        /// `Display`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// The service could not read its configuration.
    ///
    /// Not retryable: a file that says something this build cannot use does not say something else a
    /// moment later. The detail names the setting *and* what was wrong with it, because the person
    /// who reads it is looking at a deployment rather than at a call stack.
    #[error("configuration: {detail}")]
    Config {
        /// Which setting, and why it was refused.
        detail: String,
    },

    /// Something outside the program failed (a file, a socket, a child process).
    ///
    /// Not retryable: a full disk does not become empty by asking again, and a caller that wants to
    /// retry an interrupted read knows its own syscall better than this enum does. The whole
    /// `std::io::Error` is kept, including its kind, which is what a caller branches on — so `?` on
    /// an `io::Result` can stay the shortest path to this variant.
    #[error("io: {source}")]
    Io {
        /// The underlying failure.
        #[from]
        source: std::io::Error,
    },
}

impl Error {
    /// Whether trying the same operation again is worth an attempt.
    ///
    /// The engine classifies and the caller decides: how many attempts, with what backoff and what
    /// deadline, is a policy this type has no business holding. What it does owe a caller is the
    /// distinction between a failure the environment might resolve and one it will not — and the
    /// answer here is a class, deliberately: retrying a rejected payload does the same work twice
    /// for the same answer, while giving up on a store that could not answer drops work a second
    /// attempt would have finished.
    ///
    /// The default for anything this list does not name is `false`, which is the safe half of the
    /// decision: a variant nobody has classified yet is a variant nobody has argued is retryable,
    /// and a queue that retried the unknown would turn one bug into a loop.
    ///
    /// `Conflict` (`jobq/3.6-transaction-isolation`) is the sharper half of `Store`: a transaction a
    /// concurrent writer made the database refuse is the case where the *same* call is worth running
    /// again, and the class naming it is what lets a caller retry that without retrying a store that
    /// is simply unreachable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::Store { .. } | Error::Conflict { .. })
    }

    /// Exit code for a binary that ends because of this error.
    ///
    /// One code per failure class, because a script's `$?` is an interface too (step
    /// `jobq/3.15-cli-contract` is where the classes are written down): a bad call is 2, a missing
    /// thing is 3, a refused store is 4, and a broken environment is 1. The two binaries already
    /// exit with these codes, so the table moves only with them — `Conflict` shares the store's 4
    /// for the same reason: what a shell can act on is "the queue would not take it", and the
    /// caller that could act on the difference retried before it ever got here.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Invalid { .. } => 2,
            Error::NotFound { .. } => 3,
            Error::Store { .. } | Error::Conflict { .. } => 4,
            Error::Config { .. } | Error::Io { .. } => 1,
        }
    }
}
