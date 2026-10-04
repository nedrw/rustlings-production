//! The engine: the part of this project another program could depend on.
//!
//! Six modules and one direction. `domain` is the vocabulary, `error` is what can go wrong,
//! `events` is the trail of what happened to a job, `store` is where jobs live, `retry` is the loop
//! for the failures another attempt can change, and `scheduler` is the front door. Nothing here
//! knows about configuration files, log formats or command lines — that is the service's business,
//! and the arrow only points this way.
//!
//! The modules are `pub(crate)` on purpose (step `jobq/3.2-public-api-baseline`): **what a caller
//! can name is what this module re-exports, and nothing else.** Moving `Job` between modules, or
//! splitting `scheduler` in two, is then a refactor inside the crate rather than a breaking change
//! for whoever depends on it — and the whole promise fits in one file a witness can compile
//! (`course/tracks/jobq/api/3.2-public-api-baseline.rs`, the `semver@baseline` gate).
//!
//! The two modules of `jobq/3.6-transaction-isolation` are re-exported from here like the rest, and
//! that is what keeps "the engine's surface grew by two names" a thing a witness can see: the paths
//! `engine::events::…` and `engine::retry::…` do not exist for a caller (`events` is a
//! `pub(crate)` module and `retry` — the module — is too, while `retry`, the *function*, is
//! re-exported below), so the next release can move them without touching anybody.

pub(crate) mod domain;
pub(crate) mod error;
pub(crate) mod events;
pub(crate) mod retry;
pub(crate) mod scheduler;
pub(crate) mod store;

mod sealed;

pub use domain::{
    DEFAULT_MAX_ATTEMPTS, Job, JobId, JobState, QUEUE_NAME_MAX, Queue, SubmitRequest, WorkerId,
};
pub use error::Error;
pub use events::{EventKind, JobEvent};
pub use retry::{DEFAULT_ATTEMPTS, Retried, retry};
pub use scheduler::Engine;
#[cfg(feature = "postgres")]
pub use store::PostgresStore;
pub use store::{BoxFuture, MemoryStore, Store};
