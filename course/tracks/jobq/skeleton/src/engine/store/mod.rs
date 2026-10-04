//! The store port: everything the engine needs from a database, and nothing else.
//!
//! A port is a promise in two directions. The engine promises to use nothing but these methods, so
//! a backend only has to implement them; a backend promises the behaviour the conformance suite
//! asserts (step `jobq/3.5-store-port-conformance` is where the durable backend and that suite
//! arrive), so the engine does not have to care which one it was handed.
//!
//! The methods return boxed futures rather than being `async fn`: a trait with an `async fn` is not
//! dyn-compatible (rustc E0038, docs/handoff-m0.md §2), and the worker holds its store behind a
//! reference. One box a call is the price of that, and it is paid once per claim, not per job.

use std::future::Future;
use std::pin::Pin;

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;

pub mod memory;

pub use memory::MemoryStore;

/// A future that can cross a trait object.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where jobs live.
pub trait Store: Send + Sync + 'static {
    /// Write a job down and hand back the row, with its identity and its resolved `run_at`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the request could never run (an empty queue, an attempt budget of
    /// zero); [`Error::Store`] when the backend refused it.
    fn enqueue<'a>(&'a self, request: SubmitRequest) -> BoxFuture<'a, Result<Job, Error>>;

    /// Take up to `limit` due jobs for `worker`, and mark them running.
    ///
    /// Two workers asking at the same time must come back with disjoint sets: that is the whole
    /// reason the port exists rather than every caller writing its own query.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the claim.
    fn claim<'a>(
        &'a self,
        worker: &'a WorkerId,
        queue: &'a Queue,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<Job>, Error>>;

    /// Finish a job this worker holds.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the job is gone, [`Error::Store`] when the backend refused the
    /// write.
    fn complete<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
    ) -> BoxFuture<'a, Result<(), Error>>;

    /// Record a failed attempt and decide where the job goes next: back to `queued` while the
    /// attempt budget lasts, to `failed` when it does not.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the job is gone, [`Error::Store`] when the backend refused the
    /// write.
    fn fail<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<JobState, Error>>;

    /// Read one job, whether or not a worker holds it.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the read (a missing row is `Ok(None)`, not an
    /// error: looking is allowed to come up empty).
    fn get<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Option<Job>, Error>>;
}
