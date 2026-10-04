//! The store port: everything the engine needs from a database, and nothing else.
//!
//! A port is a promise in two directions. The engine promises to use nothing but these methods, so
//! a backend only has to implement them; a backend promises the behaviour the conformance suite
//! asserts, so the engine does not have to care which one it was handed.
//!
//! `jobq/3.5-store-port-conformance` is the step that makes the second half of that real: it adds
//! the durable backend below and the suite that runs the same assertions against both of them. A
//! promise the suite asserts has to be a promise written *here* — a suite that reads a backend
//! instead of the port is a suite that certifies one implementation, and the disagreement it
//! cannot see is the one this whole arrangement exists to catch.
//!
//! The methods return boxed futures rather than being `async fn`: a trait with an `async fn` is not
//! dyn-compatible (rustc E0038, docs/handoff-m0.md §2), and the worker holds its store behind a
//! reference. One box a call is the price of that, and it is paid once per claim, not per job.
//!
//! The port is **sealed** (step `jobq/3.2-public-api-baseline`): `Sealed` is a supertrait, so the
//! two promises above are made by the crate and by the backends it ships, not by anyone who happens
//! to implement five methods. A third-party backend could not keep the second promise — this crate
//! cannot run its suite for code it does not have — so it is refused at the point where the promise
//! would have to be kept, which is the `impl`.

use std::future::Future;
use std::pin::Pin;

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;
use crate::engine::sealed::Sealed;

pub(crate) mod memory;
/// The durable backend, which is a feature rather than a fixture: a build that does not ask for it
/// has no driver in it at all (step `jobq/3.4-feature-assembly`).
#[cfg(feature = "postgres")]
pub(crate) mod postgres;

pub use memory::MemoryStore;
#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;

/// A future that can cross a trait object.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where jobs live.
///
/// Implemented by this crate's own backends, and only by those: see the module documentation and
/// `crate::engine::sealed`.
pub trait Store: Sealed + Send + Sync + 'static {
    /// Write a job down and hand back the row, with its identity and its resolved `run_at`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the request could never run (an empty queue, an attempt budget of
    /// zero); [`Error::Store`] when the backend refused it.
    fn enqueue<'a>(&'a self, request: SubmitRequest) -> BoxFuture<'a, Result<Job, Error>>;

    /// Take up to `limit` due jobs for `worker`, and mark them running.
    ///
    /// Claim order is part of the port rather than a detail of one backend: the jobs that are due
    /// come back highest priority first, then the one that has been due longest, then by ascending
    /// identity — never in whatever order a store happens to hand its rows out, because a caller
    /// that receives work in a different sequence from every backend has no policy at all
    /// (`jobq/3.9-priority-fairness` is where the policy behind the order is argued, and it can
    /// only be argued because the port fixes the order first). Two workers asking at the same time
    /// must come back with disjoint sets: that is the whole reason the port exists rather than
    /// every caller writing its own query.
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
