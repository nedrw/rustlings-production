//! The worker: the loop that turns a queue into work that happens.
//!
//! Everything before this step built an engine that can *hold* jobs, and none of it ran one. This
//! is where the process reads a queue and does something about it: claim, hand the job to a
//! [`Handler`], settle it, and — while that is happening — keep telling the store that the claim is
//! still alive.
//!
//! Four decisions, and the step argues them:
//!
//! * **a claim is a lease.** Every row the worker takes carries its owner and an instant after which
//!   the claim is presumed abandoned, and the worker renews it for as long as it is handling the
//!   job. That is the whole difference between a queue that survives a `kill -9` and one that holds a
//!   job until somebody notices: a process that dies without running a line of cleanup leaves a row
//!   that goes back to the queue when its lease runs out, and the attempt it spent stays spent
//!   (`jobq/3.7-leases-and-recovery` is where `reclaim_expired` does that).
//! * **the handler is a port, and the daemon is a shell.** The engine cannot know what a job *does*
//!   — that is the application's business, and this crate is the engine. So the loop takes a
//!   [`Handler`], the daemon wires one, and a test can wire its own: the two things a worker can be
//!   wrong about (does it settle what it claimed, does it stop when it is told) are then things a
//!   test can watch without a second process.
//! * **the drain is a deadline, not a hope.** `SIGTERM` ends the *claiming* immediately, then the
//!   work in flight gets [`WorkerSettings::shutdown`] to finish, and whatever is left goes back to
//!   the queue with its lease released rather than being waited out: a rollout that hands the row
//!   back costs seconds, one that waits for a lease costs the lease.
//! * **the settle is the caller's retry.** A settle runs at an isolation level that can be refused
//!   (`jobq/3.6-transaction-isolation`), so this loop settles through `engine::retry` with
//!   [`DEFAULT_ATTEMPTS`](crate::engine::DEFAULT_ATTEMPTS) — the store reports the class, the
//!   caller decides what to do about it, and the worker is the caller.
//!
//! What the loop needs from the port and the store does not have yet is written down in the step
//! body: `renew` (extend the leases this worker holds), `release` (hand back what it still holds)
//! and `reclaim_expired` (put back what a dead worker left). Those are the store's, not this
//! module's, because they are writes to rows rather than decisions about them.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::engine::domain::{Job, Queue, WorkerId};
use crate::engine::error::Error;
use crate::engine::store::{BoxFuture, Store};

/// What a worker does with a job it has claimed.
///
/// The error is *the attempt failed*, not *the worker is broken*: the daemon settles the job with
/// the failure's own words as the reason (`Store::fail`), and the job's budget decides whether that
/// means another attempt or the end of the line. A handler that cannot tell the two apart would
/// either hide a job's real problem behind a worker's or lose the queue because one payload is bad.
pub trait Handler: Send + Sync {
    /// Do the job, or say why this attempt failed.
    fn handle<'a>(&'a self, job: &'a Job) -> BoxFuture<'a, Result<(), Error>>;
}

/// A boxed handler is a handler.
///
/// The daemon decides which handler to install from its configuration, and that decision is made at
/// run time — so the thing it hands the worker is a `Box<dyn Handler>`, and the worker has to be able
/// to take one. Nothing else in this module needs the blanket implementation; this is the one shape
/// it exists for.
impl<T: Handler + ?Sized> Handler for Box<T> {
    fn handle<'a>(&'a self, job: &'a Job) -> BoxFuture<'a, Result<(), Error>> {
        (**self).handle(job)
    }
}

/// The one signal that stops a worker.
///
/// The daemon turns `SIGTERM` into [`Shutdown::trigger`] and a test calls it directly, which is the
/// point: "stop claiming, finish what is in flight" is a property of the loop, and a property that
/// can only be observed by sending a signal to a process is a property that is only tested once the
/// process exists.
#[derive(Debug, Clone)]
pub struct Shutdown {
    /// `false` while the worker may claim, `true` once it must stop.
    signal: watch::Sender<bool>,
}

impl Shutdown {
    /// A signal nobody has sent yet.
    #[must_use]
    pub fn new() -> Self {
        let (signal, _) = watch::channel(false);
        Self { signal }
    }

    /// Ask the worker to stop claiming and drain.
    ///
    /// Sending is idempotent, and it is allowed to have no listeners: a signal that arrives before
    /// the worker starts is a signal the worker finds already sent when it looks.
    pub fn trigger(&self) {
        let _ = self.signal.send(true);
    }

    /// `true` once the signal has been sent.
    #[must_use]
    pub fn triggered(&self) -> bool {
        *self.signal.borrow()
    }

    /// Resolve once the signal has been sent, or immediately when it already was.
    pub async fn wait(&self) {
        let mut receiver = self.signal.subscribe();
        if *receiver.borrow() {
            return;
        }
        let _ = receiver.changed().await;
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// How a worker drains.
///
/// Every field is a decision an operator makes, and the defaults are the ones a small deployment
/// wants: one job at a time is not a default (a worker that is slow because it is polite is a
/// worker that needs more replicas), and neither is a lease long enough to hide a crash.
#[derive(Debug, Clone)]
pub struct WorkerSettings {
    /// What this worker calls itself on the rows it holds, in its log and in the trail.
    name: WorkerId,
    /// Queues to drain, in the order to ask for them.
    queues: Vec<Queue>,
    /// How many jobs one turn asks each queue for.
    batch: usize,
    /// How long to wait before asking again after a turn that found nothing.
    poll_interval: Duration,
    /// How long the drain may take once the signal arrives.
    shutdown: Duration,
    /// How long a claim is good for, on the rows this worker takes.
    ///
    /// The store stamps it and this loop renews against it, so the two have to agree:
    /// `worker.lease_seconds` is one setting in the daemon's configuration for exactly that reason,
    /// and a caller that builds a store and a worker by hand (a test, the CLI of
    /// `jobq/3.15-cli-contract`) sets both from one value.
    lease: Duration,
}

impl WorkerSettings {
    /// Settings for a worker that drains `queues` under `name`.
    #[must_use]
    pub fn new(name: WorkerId, queues: Vec<Queue>) -> Self {
        Self {
            name,
            queues,
            batch: 8,
            poll_interval: Duration::from_millis(250),
            shutdown: Duration::from_secs(10),
            lease: Duration::from_secs(60),
        }
    }

    /// How many jobs one turn asks each queue for.
    #[must_use]
    pub fn with_batch(mut self, batch: usize) -> Self {
        self.batch = batch;
        self
    }

    /// How long to wait before asking again after a turn that found nothing.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// How long the drain may take once the signal arrives.
    #[must_use]
    pub fn with_shutdown(mut self, deadline: Duration) -> Self {
        self.shutdown = deadline;
        self
    }

    /// How long a claim is good for, on the rows this worker takes.
    #[must_use]
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    /// What this worker calls itself.
    #[must_use]
    pub fn name(&self) -> &WorkerId {
        &self.name
    }

    /// Queues to drain.
    #[must_use]
    pub fn queues(&self) -> &[Queue] {
        &self.queues
    }

    /// Jobs per turn and queue.
    #[must_use]
    pub fn batch(&self) -> usize {
        self.batch
    }

    /// Wait after a turn that found nothing.
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// The drain deadline.
    #[must_use]
    pub fn shutdown(&self) -> Duration {
        self.shutdown
    }

    /// How long a claim is good for.
    #[must_use]
    pub fn lease(&self) -> Duration {
        self.lease
    }
}

/// What one run of a worker did.
///
/// Counts, not rows: the rows are the store's answer (`Store::events`), and this is what the worker
/// can say about its own turn — which is the line a stopping process logs, and the number an
/// operator compares against the queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Jobs this worker took out of a queue.
    claimed: u32,
    /// Attempts that finished their job.
    succeeded: u32,
    /// Attempts that failed (the job may or may not have another one left).
    failed: u32,
    /// Rows this worker put back because their lease had run out.
    reclaimed: u32,
    /// Rows this worker handed back when the drain deadline passed.
    handed_back: u32,
}

impl Report {
    /// Jobs this worker took out of a queue.
    #[must_use]
    pub fn claimed(&self) -> u32 {
        self.claimed
    }

    /// Attempts that finished their job.
    #[must_use]
    pub fn succeeded(&self) -> u32 {
        self.succeeded
    }

    /// Attempts that failed.
    #[must_use]
    pub fn failed(&self) -> u32 {
        self.failed
    }

    /// Rows this worker put back because their lease had run out.
    #[must_use]
    pub fn reclaimed(&self) -> u32 {
        self.reclaimed
    }

    /// Rows this worker handed back when the drain deadline passed.
    #[must_use]
    pub fn handed_back(&self) -> u32 {
        self.handed_back
    }
}

/// A worker over one store, with one handler and one set of settings.
///
/// The store is shared rather than borrowed (an `Arc<dyn Store>`) because the whole point of a turn
/// is that several jobs are handled *at once*: each one is a task, and a task that borrows the store
/// from the worker cannot outlive the call that gives it work. The port was written with boxed
/// futures and a seal for this (step `jobq/3.5-store-port-conformance`): one store behind a
/// reference, several callers.
pub struct Worker {
    /// Where the jobs are.
    store: Arc<dyn Store>,
    /// What the jobs are done by.
    handler: Arc<dyn Handler>,
    /// What this worker was told to do.
    settings: WorkerSettings,
}

impl Worker {
    /// A worker that drains `store` with `handler`.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        handler: impl Handler + 'static,
        settings: WorkerSettings,
    ) -> Self {
        Self {
            store,
            handler: Arc::new(handler),
            settings,
        }
    }

    /// Drain until `shutdown` arrives, then finish or hand back what is in flight.
    ///
    /// The turn, in order: ask every queue for up to [`WorkerSettings::batch`] due jobs, hand each
    /// one to the handler in its own task (this is what the lease has to cover), settle what comes
    /// back, renew the leases this worker holds while that is happening, and reclaim what a worker
    /// that is no longer here left behind. A turn that finds nothing waits
    /// [`WorkerSettings::poll_interval`] — interruptibly, so a stop does not have to wait for an
    /// idle pause.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the store refuses something this loop cannot make sense of: a claim it
    /// cannot make, a reclaim it cannot complete. A job that fails is not an error here — it is
    /// [`Store::fail`], and the report counts it.
    pub async fn run(&self, shutdown: Shutdown) -> Result<Report, Error> {
        let _ = (shutdown, &self.store, &self.handler, &self.settings);
        todo!("jobq/3.7-leases-and-recovery: claim, handle, settle, renew — and drain when told")
    }
}
