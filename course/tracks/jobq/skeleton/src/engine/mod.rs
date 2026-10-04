//! The engine: the part of this project another program could depend on.
//!
//! Four modules and one direction. [`domain`] is the vocabulary, [`error`] is what can go wrong,
//! [`store`] is where jobs live, and [`scheduler`] is the front door. Nothing here knows about
//! configuration files, log formats or command lines — that is the service's business, and the
//! arrow only points this way.

pub mod domain;
pub mod error;
pub mod scheduler;
pub mod store;

pub use domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
pub use error::Error;
pub use scheduler::Engine;
pub use store::{MemoryStore, Store};
