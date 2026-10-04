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
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep, timeout_at};

use crate::engine::domain::{Job, Queue, WorkerId};
use crate::engine::error::Error;
use crate::engine::retry::{DEFAULT_ATTEMPTS, retry};
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
        let mut report = Report::default();
        // The renewals run *beside* the work rather than inside it, because the thing that has to be
        // renewed knows nothing about the thing that is running: a handler that takes longer than a
        // lease has no way to say "still here", and the row would quietly become claimable by
        // somebody else in the middle of the attempt it is describing.
        let renewing = tokio::spawn(renewing(
            Arc::clone(&self.store),
            self.settings.name.clone(),
            self.settings.lease,
            shutdown.clone(),
        ));

        while !shutdown.triggered() {
            // Every turn starts here: a row whose lease ran out belongs to nobody, and the worker
            // that finds it is the one that recovers it. One statement that writes nothing when
            // there is nothing to write (see `Store::reclaim_expired`), so a worker that has just
            // started — the replica that is supposed to pick up after a crash — does not have to
            // wait for its first turn to find out.
            report.reclaimed = report
                .reclaimed
                .saturating_add(self.store.reclaim_expired().await?);

            let turn = self.claim_a_turn(&mut report).await?;
            if turn.is_empty() {
                self.wait_for_work(&shutdown).await;
                continue;
            }

            // One task per job is what the batch buys, and the reason every row carries a lease of
            // its own: several attempts really are in flight at the same instant.
            let mut jobs: JoinSet<Result<Settled, Error>> = JoinSet::new();
            for job in turn {
                self.spawn_job(&mut jobs, job);
            }
            self.finish_turn(&mut jobs, &shutdown, &mut report).await?;
        }

        // The signal has ended the claiming. Whatever is still in flight gets the drain deadline,
        // and what the deadline cuts is handed back rather than waited for: a rollout that gives the
        // row back costs seconds, one that waits out a lease costs the lease.
        renewing.abort();
        tracing::info!(
            worker = %self.settings.name,
            claimed = report.claimed,
            succeeded = report.succeeded,
            failed = report.failed,
            reclaimed = report.reclaimed,
            handed_back = report.handed_back,
            "drained"
        );
        Ok(report)
    }

    /// Ask every queue for the next batch, and count what came back.
    async fn claim_a_turn(&self, report: &mut Report) -> Result<Vec<Job>, Error> {
        let mut turn = Vec::new();
        for queue in &self.settings.queues {
            let claimed = self
                .store
                .claim(&self.settings.name, queue, self.settings.batch)
                .await?;
            report.claimed = report
                .claimed
                .saturating_add(u32::try_from(claimed.len()).unwrap_or(u32::MAX));
            turn.extend(claimed);
        }
        Ok(turn)
    }

    /// Wait out an empty turn, without making a stop wait for the pause.
    async fn wait_for_work(&self, shutdown: &Shutdown) {
        tokio::select! {
            () = shutdown.wait() => {}
            () = sleep(self.settings.poll_interval) => {}
        }
    }

    /// Hand one job to the handler in a task of its own.
    ///
    /// The task owns clones rather than borrows (`Arc<dyn Store>`, `Arc<dyn Handler>`) because a
    /// spawned task has to outlive the call that made it: that is what lets several jobs be handled
    /// at once, and it is why the store the worker holds is shared rather than borrowed.
    fn spawn_job(&self, jobs: &mut JoinSet<Result<Settled, Error>>, job: Job) {
        let store = Arc::clone(&self.store);
        let handler = Arc::clone(&self.handler);
        let worker = self.settings.name.clone();
        jobs.spawn(async move { handle_one(store, handler, worker, job).await });
    }

    /// Collect what a turn's tasks came to, and drain when the signal says so.
    ///
    /// The shape is the promise: while there is no signal, this waits for the work; once there is
    /// one, the wait gets [`WorkerSettings::shutdown`] and then stops. What it stops *on* is handed
    /// back — the tasks are aborted (which drops each handler's child, see `service::handler`) and
    /// the rows this worker still holds go back to the queue with their leases released.
    async fn finish_turn(
        &self,
        jobs: &mut JoinSet<Result<Settled, Error>>,
        shutdown: &Shutdown,
        report: &mut Report,
    ) -> Result<(), Error> {
        let mut deadline: Option<Instant> = None;
        while !jobs.is_empty() {
            let joined = match deadline {
                Some(deadline) => match timeout_at(deadline, jobs.join_next()).await {
                    Ok(joined) => joined,
                    // The deadline bound the drain: stop waiting for what is left.
                    Err(_) => break,
                },
                None => tokio::select! {
                    joined = jobs.join_next() => joined,
                    () = shutdown.wait() => {
                        deadline = Some(Instant::now() + self.settings.shutdown);
                        continue;
                    }
                },
            };
            match joined {
                Some(Ok(Ok(Settled::Succeeded))) => report.succeeded += 1,
                Some(Ok(Ok(Settled::Failed))) => report.failed += 1,
                // A handler that panicked is a handler's bug, and the row is deliberately left
                // where it is: nothing was settled, so the lease will bring it back — to a worker
                // whose handler does not panic, which is the point of a lease.
                Some(Err(join)) => {
                    tracing::error!(worker = %self.settings.name, error = %join, "a job's task did not finish")
                }
                Some(Ok(Err(error))) => return Err(error),
                None => break,
            }
        }

        if jobs.is_empty() {
            return Ok(());
        }
        jobs.abort_all();
        while jobs.join_next().await.is_some() {}
        report.handed_back = report
            .handed_back
            .saturating_add(self.store.release(&self.settings.name).await?);
        tracing::warn!(
            worker = %self.settings.name,
            handed_back = report.handed_back,
            "the drain deadline passed with work still in flight"
        );
        Ok(())
    }
}

/// What one task's settle came to, for the report.
enum Settled {
    /// The attempt finished the job.
    Succeeded,
    /// The attempt failed: the job went back to the queue, or out of it.
    Failed,
}

/// One job, from the handler to the settle: the whole of what a worker does with a claim.
///
/// The settle is retried with the engine's own bound, because a settle is a transaction the database
/// can refuse for concurrency (`jobq/3.6-transaction-isolation`): the first attempt losing a race is
/// a second attempt, not a failed job, and the store is the layer that reports the difference.
async fn handle_one(
    store: Arc<dyn Store>,
    handler: Arc<dyn Handler>,
    worker: WorkerId,
    job: Job,
) -> Result<Settled, Error> {
    let id = job.id();
    let started = Instant::now();
    match handler.handle(&job).await {
        Ok(()) => {
            retry(DEFAULT_ATTEMPTS, || store.complete(&worker, &id)).await?;
            tracing::info!(job = %id, worker = %worker, took = ?started.elapsed(), "succeeded");
            Ok(Settled::Succeeded)
        }
        Err(error) => {
            // The handler's failure is the *job's* failure: it is written on the row as the reason,
            // and the budget decides whether another attempt is allowed.
            let reason = error.to_string();
            let state = retry(DEFAULT_ATTEMPTS, || store.fail(&worker, &id, &reason)).await?;
            tracing::warn!(job = %id, worker = %worker, reason = %reason, "attempt failed");
            let _ = state.value();
            Ok(Settled::Failed)
        }
    }
}

/// Keep telling the store that the claims this worker holds are still alive.
///
/// A renewal is not a settle: it moves an instant, not a state, and it is not retried. It must not
/// be silent either — a renewal that keeps failing is a job that is about to be handed to somebody
/// else while this worker is still running it, and the log line is the only warning an operator
/// gets before the duplicate work starts.
async fn renewing(store: Arc<dyn Store>, worker: WorkerId, lease: Duration, shutdown: Shutdown) {
    let interval = renewal_interval(lease);
    while !shutdown.triggered() {
        tokio::select! {
            () = shutdown.wait() => break,
            () = sleep(interval) => {}
        }
        match store.renew(&worker).await {
            Ok(0) => {}
            Ok(renewed) => tracing::debug!(worker = %worker, renewed, "leases renewed"),
            Err(error) => tracing::warn!(
                worker = %worker,
                error = %error,
                "the lease could not be renewed: what this worker holds is one missed renewal away \
                 from being handed to another worker"
            ),
        }
    }
}

/// When to renew: halfway through the lease, so that two renewals may be lost before the claim
/// lapses — and never at a zero interval, which would be a renewal loop with no pause in it.
fn renewal_interval(lease: Duration) -> Duration {
    (lease / 2).max(Duration::from_millis(1))
}
