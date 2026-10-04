//! The engine: the part of this project another program could depend on.
//!
//! Four modules and one direction. `domain` is the vocabulary, `error` is what can go wrong,
//! `store` is where jobs live, and `scheduler` is the front door. Nothing here knows about
//! configuration files, log formats or command lines — that is the service's business, and the
//! arrow only points this way.
//!
//! The modules are `pub(crate)` on purpose (step `jobq/3.2-public-api-baseline`): **what a caller
//! can name is what this module re-exports, and nothing else.** Moving `Job` between modules, or
//! splitting `scheduler` in two, is then a refactor inside the crate rather than a breaking change
//! for whoever depends on it — and the whole promise fits in one file a witness can compile
//! (`course/tracks/jobq/api/3.2-public-api-baseline.rs`, the `semver@baseline` gate).
//!
//! Everything that was reachable at `engine::<module>::…` before this step is reachable here now:
//! a path may move, but a name may not disappear silently.

pub(crate) mod domain;
pub(crate) mod error;
pub(crate) mod scheduler;
pub(crate) mod store;

mod sealed;

pub use domain::{
    DEFAULT_MAX_ATTEMPTS, Job, JobId, JobState, QUEUE_NAME_MAX, Queue, SubmitRequest, WorkerId,
};
pub use error::Error;
pub use scheduler::Engine;
#[cfg(feature = "postgres")]
pub use store::PostgresStore;
pub use store::{BoxFuture, MemoryStore, Store};
