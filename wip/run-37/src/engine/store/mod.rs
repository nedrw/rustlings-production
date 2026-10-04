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
//! `jobq/3.6-transaction-isolation` widens the port by two methods, and the reason is the same
//! reason the port exists: **a settle leaves a trail**, the trail is where an operator and the next
//! tooling learn what happened to a job, and a trail that only one backend keeps is a promise the
//! engine cannot make. So [`Store::record`] (append a line, idempotently by the line's own identity)
//! and [`Store::events`] (read a job's lines, oldest first) are on the port, and the two backends
//! keep them the way each of them can: a unique index under a durable table, a keyed map in memory.
//! The *settles* append through that same promise inside their own transaction, so the row and the
//! line that describes it commit together — which is the subject of that step.
//!
//! The port is **sealed** (step `jobq/3.2-public-api-baseline`): `Sealed` is a supertrait, so the
//! two promises above are made by the crate and by the backends it ships, not by anyone who happens
//! to implement these methods. A third-party backend could not keep the second promise — this crate
//! cannot run its suite for code it does not have — so it is refused at the point where the promise
//! would have to be kept, which is the `impl`.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::engine::domain::{Job, JobId, JobState, Queue, SubmitRequest, WorkerId};
use crate::engine::error::Error;
use crate::engine::events::JobEvent;
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

/// How long a claim lasts when nobody says otherwise: a minute.
///
/// Long enough that a job which is merely slow is not handed to a second worker, short enough that a
/// `kill -9` is a delay rather than a lost afternoon. Both backends start from it, and the daemon
/// replaces it with `worker.lease_seconds` (`jobq/3.7-leases-and-recovery`).
pub(crate) const DEFAULT_LEASE: Duration = Duration::from_secs(60);

/// What a row's `last_error` says when a worker handed it back on its way out.
///
/// One string for both backends, because it is what an operator reads in the row (`jobq/3.7`): a
/// job that went back to the queue because somebody stopped is a different story from one that
/// failed, and the trail line next to it is the other half of that story.
pub(crate) const HANDED_BACK: &str = "the worker holding this job stopped and handed it back";

/// What a row's `last_error` says when its lease ran out.
pub(crate) const ABANDONED: &str = "the worker holding this job stopped without finishing it";

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

    /// Take up to `limit` due jobs for `worker`, mark them running, and lease them to that worker.
    ///
    /// The claim is a **lease** from step `jobq/3.7-leases-and-recovery` on: the row carries the
    /// name of the worker that took it and an instant after which the claim is presumed abandoned,
    /// and the claimant extends that instant with [`Store::renew`] for as long as it is handling
    /// the job. A claim that is only a state change is a row nobody can recover from a process that
    /// was killed before it could hand anything back.
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
    /// "This worker" is the whole of the phrase from `jobq/3.7-leases-and-recovery` on: a settle is
    /// refused unless the row still carries this worker's lease, so a worker whose claim was
    /// already declared abandoned cannot settle the attempt its successor is running.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the job is gone, [`Error::Invalid`] when the job is not running or
    /// its lease belongs to somebody else, [`Error::Conflict`] when the backend refused the
    /// transaction and a caller may try again, [`Error::Store`] when the backend refused the write.
    fn complete<'a>(
        &'a self,
        worker: &'a WorkerId,
        id: &'a JobId,
    ) -> BoxFuture<'a, Result<(), Error>>;

    /// Record a failed attempt and decide where the job goes next: back to `queued` while the
    /// attempt budget lasts, to `failed` when it does not.
    ///
    /// The lease guard is [`Store::complete`]'s, for the same reason: only the worker the row still
    /// names may say what happened to the attempt.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the job is gone, [`Error::Invalid`] when the job is not running or
    /// its lease belongs to somebody else, [`Error::Conflict`] when the backend refused the
    /// transaction and a caller may try again, [`Error::Store`] when the backend refused the write.
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

    /// Append one line to a job's trail, answering whether it was new.
    ///
    /// Appending is **idempotent by the line's own identity** (`jobq/3.6-transaction-isolation`): a
    /// line is identified by `(job_id, kind, attempt)`, so appending the same settle twice leaves
    /// one line, and the `false` a caller gets back is how it finds out its second attempt wrote
    /// nothing new. The two backends keep that promise differently — a unique index the statement
    /// defers to (`on conflict … do nothing`) under a table, the map the lines are keyed by in
    /// memory — and it is on the port rather than in either of them because a caller that has to
    /// know which backend it is talking to has no port.
    ///
    /// The settles a backend owns (`complete`, `fail`, and the reclaim of
    /// `jobq/3.7-leases-and-recovery`) append their own lines *inside* the transaction that changes
    /// the row, so this door is for callers whose work is not a settle: the tooling of
    /// `jobq/3.16-dlq-tooling`, and the suite that holds every backend to the same trail.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the write.
    fn record<'a>(&'a self, event: &'a JobEvent) -> BoxFuture<'a, Result<bool, Error>>;

    /// Bring this backend's schema up to date.
    ///
    /// Runs on every start of the daemon, against a store that is usually already current, so "the
    /// second run changes nothing" is part of what this owes rather than a detail of how it is
    /// written (`mailer/1.9-postgres` made the same promise for one service, and the store fixture
    /// of `jobq/3.5-store-port-conformance` has been calling this method's implementation since
    /// then). `jobq/3.7-leases-and-recovery` is where the *daemon* starts doing it too, and the
    /// reason it belongs on the port rather than on one backend is that a store a process has to
    /// prepare before it can use it is a store whose preparation every caller can ask for.
    ///
    /// A backend with no schema answers `Ok(())`: a no-op rather than a special case every caller
    /// has to know about.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when a migration failed.
    fn migrate<'a>(&'a self) -> BoxFuture<'a, Result<(), Error>>;

    /// Extend the leases this worker holds, and answer how many rows moved.
    ///
    /// One statement over the rows the *worker* holds — `lease_owner = this worker and state =
    /// running` — because the filter is the safety property: a worker that could renew anybody's
    /// lease could keep another worker's attempt alive for ever, or (worse, and much more likely by
    /// accident) hand its own expired rows back as if they had never lapsed.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the write.
    fn renew<'a>(&'a self, worker: &'a WorkerId) -> BoxFuture<'a, Result<u32, Error>>;

    /// Hand back every row this worker still holds, and answer how many rows moved.
    ///
    /// This is what a drain uses when its deadline passes, and what a stop that does not want to
    /// wait out a lease uses at all: the same work the reclaimer does when a lease runs out, done
    /// early because somebody knows it is not coming back. The job goes to `queued` (or to `failed`
    /// when its attempts are spent, the rule [`Store::fail`] uses), with its attempt kept — the
    /// attempt was started, whether or not anybody watched it end — its lease released, and a line
    /// of the trail that says it was handed back rather than finished.
    ///
    /// The filter is the same `lease_owner = this worker`, and it is the whole safety property
    /// again: handing back somebody else's claim puts an attempt that is running right now back in
    /// the queue, which is a second worker on one job.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the write.
    fn release<'a>(&'a self, worker: &'a WorkerId) -> BoxFuture<'a, Result<u32, Error>>;

    /// Put back every row whose lease has run out, and answer how many rows moved.
    ///
    /// A row whose lease has run out belongs to nobody: the worker that took it is gone (that is
    /// what a lease is for) or is so far behind that another worker finishing the job is the better
    /// trade. It goes back to `queued` — or to `failed` when its attempts are spent, which is the
    /// half that keeps a payload that kills its worker from being retried for ever — with its
    /// attempt kept, its lease released, and a `reclaimed` line in its trail.
    ///
    /// The predicate is `state = 'running' and lease_expires_at <= now`: the instant, not the owner,
    /// is what decides, because a clock that disagrees by a second must not be able to strand a row
    /// (see the step body on the two clocks a lease puts side by side).
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the write.
    fn reclaim_expired<'a>(&'a self) -> BoxFuture<'a, Result<u32, Error>>;

    /// One job's trail: what has happened to it, oldest first.
    ///
    /// The order is a promise, for the reason `claim`'s order is one: an operator reads a story, and
    /// two backends telling it in different orders is two answers to one question. Lines that share
    /// an instant come back in `(kind, attempt)` order so that the tie is decided the same way by
    /// both. A job nobody wrote has an empty trail rather than an error, exactly as `get` answers
    /// `Ok(None)` for it.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused the read.
    fn events<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Vec<JobEvent>, Error>>;
}
