//! Spec for `jobq/3.7-leases-and-recovery`: the worker loop, the lease a claim is, and what a
//! process that stops — politely or not — leaves behind.
//!
//! `kiln run` copies this file into the project as `tests/spec_leases.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The store comes from the same fixture
//! `jobq/3.5-store-port-conformance` built (`KILN_BACKEND` picks it), and so does the daemon.
//!
//! Five decisions about what this file is allowed to assert:
//!
//! * **the loop is judged in-process and the process is judged from outside.** A `Worker` over the
//!   fixture's store with this file's own handler is how the claim → handle → settle cycle, the
//!   renewal and the hand-back are observed: no signals, no second process, and a failure that says
//!   which of the three went wrong. The binary is then judged for what only a process can show —
//!   `kill -9` in the middle of a job, `SIGTERM` arriving while one is in flight, and the exit code.
//! * **a claim is a lease, and the row is where that is read.** Every case that cares about the
//!   lease reads it off the row (`Job::lease_owner`, `Job::lease_expires_at`) rather than out of the
//!   worker's report: what an operator can see is the standard the machinery is held to.
//! * **the attempt nobody watched is the point.** A crashed worker's job has to come back *and* the
//!   attempt it spent has to stay spent, or a payload that kills every worker is retried for ever;
//!   the same rule has to end it when the budget is gone, which is what
//!   `a_job_whose_attempts_are_spent_is_not_handed_out_again` checks by watching a job die without
//!   anybody failing it.
//! * **the double-worker window is asserted, not described.** A worker whose lease was reclaimed
//!   cannot settle the row any more: the row belongs to somebody else, and the settle is refused.
//!   That case is the whole reason every settle takes a worker name.
//! * **a lease that is renewed is not stolen.** Two workers, one short lease, one job that takes
//!   longer than the lease: the second worker must find nothing to reclaim and nothing to claim, and
//!   the first must finish. It is a case about *time*, so it gives the machine a margin: the lease is
//!   seconds and the work is twice as long.
//!
//! The conflict and trail promises of `jobq/3.6-transaction-isolation` are not re-asserted here
//! except where this step adds a line to the trail (`reclaimed`); the suite of that step still runs
//! (`kiln check jobq/3.6-transaction-isolation`), and a step that re-tested its predecessor would be
//! a second copy of it.

// A test may read the wall clock; the *library* may not, and `jobq/3.8-clock-injection` is where the
// project's `clippy.toml` started refusing it — for every target, which is why the relaxation has to
// be written down here rather than left to a reviewer to remember (authoring.md §7.4). This file
// needs a real instant to say what a store that was *not* handed a clock does, and to compare a
// lease against the process's own clock rather than against an injected one.
#![allow(clippy::disallowed_methods)]

mod support;

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use jobq::engine::{
    Error, EventKind, Job, JobId, JobState, MemoryStore, PostgresStore, Queue, Store,
    SubmitRequest, WorkerId,
};
use jobq::service::worker::{Handler, Report, Shutdown, Worker, WorkerSettings};
use serde_json::Value;
use support::service::Service;
use support::store::{Backend, backend};
use support::var;
use tokio::sync::Notify;
use tokio::time::sleep;

/// The queue every case in this file uses.
const QUEUE: &str = "emails";
/// How often a case looks again while it waits for a condition.
const POLL: Duration = Duration::from_millis(20);
/// How long a case waits for something it asked another process to do.
const DEADLINE: Duration = Duration::from_secs(30);

/// That queue, parsed, with the failure a learner can act on.
fn queue() -> Queue {
    Queue::parse(QUEUE).expect("`emails` is the name this file uses everywhere")
}

/// A payload that tells the handler of this file what to do with the job.
fn payload(what: &str) -> Value {
    serde_json::json!({ "do": what })
}

/// One write, with a failure that names the backend that refused it.
async fn enqueue(store: &dyn Store, request: SubmitRequest) -> Job {
    store
        .enqueue(request)
        .await
        .unwrap_or_else(|error| panic!("the store refused a request the port accepts: {error}"))
}

/// One claim, with a failure that names the store that refused it.
async fn claim(store: &dyn Store, worker: &WorkerId, limit: usize) -> Vec<Job> {
    store
        .claim(worker, &queue(), limit)
        .await
        .unwrap_or_else(|error| panic!("the store refused a claim: {error}"))
}

/// The row, with a failure that says the store refused rather than that the job is gone.
async fn row(store: &dyn Store, id: &JobId) -> Job {
    store
        .get(id)
        .await
        .unwrap_or_else(|error| panic!("the store refused a read: {error}"))
        .expect("the job this case is about is still there")
}

/// One job's trail, oldest first.
async fn trail(store: &dyn Store, id: &JobId) -> Vec<EventKind> {
    store
        .events(id)
        .await
        .unwrap_or_else(|error| panic!("the store refused a read: {error}"))
        .into_iter()
        .map(|event| event.kind())
        .collect()
}

/// Wait until this job answers `holds`, and answer the row it answered with.
///
/// A condition with a deadline rather than a `sleep` (authoring.md §6.4), and the failure message
/// carries everything the answer needed to be read: a case that timed out is a case whose reader
/// wants to know what the row said instead.
async fn wait_until<F>(store: &dyn Store, id: &JobId, what: &str, holds: F) -> Job
where
    F: Fn(&Job) -> bool,
{
    let until = Instant::now() + DEADLINE;
    loop {
        let current = row(store, id).await;
        if holds(&current) {
            return current;
        }
        assert!(
            Instant::now() < until,
            "the job did not {what} within {DEADLINE:?}: it is {:?} with {} attempt(s), held by \
             {:?} until {:?}",
            current.state(),
            current.attempts(),
            current.lease_owner().map(WorkerId::as_str),
            current.lease_expires_at()
        );
        sleep(POLL).await;
    }
}

/// The handler this file's in-process cases run: its verdict comes from the payload, and it remembers
/// every job it was given.
///
/// The payload drives it so that one turn can hold a job that succeeds and a job that refuses (the
/// cycle has two ends, and a handler that only succeeded would let half of `Store::fail` be wrong).
/// `wait` is what makes a job *in flight* for as long as a case needs: the handler parks on a
/// [`Notify`] until the case releases it, with a patience bound so that a case that forgot to
/// release it fails instead of hanging.
struct Suite {
    /// Every job this handler was given, in order: a job that appears twice was handled twice.
    seen: Arc<Mutex<Vec<JobId>>>,
    /// Where the parking jobs wait.
    released: Arc<Notify>,
    /// How long a parking job waits for the release before giving up on the case.
    patience: Duration,
}

impl Suite {
    /// A handler, and the two handles a case needs to watch it.
    fn new(patience: Duration) -> (Self, Arc<Mutex<Vec<JobId>>>, Arc<Notify>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let released = Arc::new(Notify::new());
        (
            Self {
                seen: Arc::clone(&seen),
                released: Arc::clone(&released),
                patience,
            },
            seen,
            released,
        )
    }

    /// The jobs it was given, in order.
    fn runs(seen: &Arc<Mutex<Vec<JobId>>>) -> Vec<JobId> {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Handler for Suite {
    fn handle<'a>(&'a self, job: &'a Job) -> jobq::engine::BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(job.id());
            match job.payload().get("do").and_then(Value::as_str) {
                Some("refuse") => Err(Error::Invalid {
                    detail: "the smtp server said 421".to_owned(),
                }),
                Some("wait") => {
                    tokio::time::timeout(self.patience, self.released.notified())
                        .await
                        .unwrap_or_else(|_| {
                            panic!("nothing released this handler within {:?}", self.patience)
                        });
                    Ok(())
                }
                _ => Ok(()),
            }
        })
    }
}

/// Settings for a worker of this file: a name, the one queue, and the lease under test.
fn settings(name: &str, lease: Duration) -> WorkerSettings {
    WorkerSettings::new(WorkerId::new(name), vec![queue()])
        .with_batch(4)
        .with_poll_interval(Duration::from_millis(50))
        .with_shutdown(Duration::from_secs(2))
        .with_lease(lease)
}

/// A store of this run's backend whose claims last `lease`.
///
/// The fixture hands out a store, and this is a *second* one over the same backend, because the cases
/// that watch a lease run out cannot wait a minute for it. Everything else about it is the fixture's:
/// the durable arm connects to the container the fixture started and migrated.
async fn store_leasing(backend: &Backend, lease: Duration) -> Arc<dyn Store> {
    match backend {
        Backend::Memory(_) => Arc::new(MemoryStore::new().with_lease(lease)),
        Backend::Postgres { container, .. } => Arc::new(
            PostgresStore::connect(&container.url())
                .await
                .expect("the container the fixture started is reachable")
                .with_lease(lease),
        ),
    }
}

/// The connection string of the container this run is against, or `None` on the in-memory backend.
///
/// The daemon cases are the ones that need it: `jobqd` in another process can only be sharing work
/// with this one through a database, because the in-memory store lives inside whichever process built
/// it. What the in-memory backend *can* show about a process — that the daemon starts, announces
/// itself and stops on a signal — is `a_daemon_on_the_memory_store_starts_and_stops`.
fn container_url(backend: &Backend) -> Option<String> {
    match backend {
        Backend::Postgres { container, .. } => Some(container.url()),
        Backend::Memory(_) => None,
    }
}

/// A program for `worker.handler`, written into this machine's temporary directory.
///
/// The daemon runs a program per attempt, and that is what makes the two process-level promises of
/// this step observable at all: a job that is still in flight when the process is killed, and one
/// that is still in flight when the signal arrives. Three lines of `sh`, named after the test that
/// wrote them and removed when the value drops.
struct Script {
    /// Where the program is.
    path: PathBuf,
}

impl Script {
    /// Write a script whose body is `body`, and make it executable.
    fn write(label: &str, body: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jobq-handler-{label}-{}-{}.sh",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))
            .unwrap_or_else(|error| panic!("{} could not be written: {error}", path.display()));
        std::fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap_or_else(|error| {
            panic!("{} could not be made executable: {error}", path.display())
        });
        Self { path }
    }

    /// The setting the daemon is started with.
    fn command(&self) -> String {
        self.path.display().to_string()
    }
}

impl Drop for Script {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One turn of a worker: claim, handle, settle — and a report that says which of the three happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_worker_claims_handles_and_settles() {
    let backend = backend().await;
    // The store is this case's own (`Arc`, because the worker owns what it drains and this case keeps
    // the handle to ask it questions); everything else about it is the fixture's.
    let store = store_leasing(&backend, Duration::from_secs(60)).await;
    let (handler, seen, _released) = Suite::new(Duration::from_secs(5));

    let settles = enqueue(&*store, SubmitRequest::new(queue(), payload("succeed"))).await;
    let refuses = enqueue(
        &*store,
        SubmitRequest::new(queue(), payload("refuse")).with_max_attempts(2),
    )
    .await;

    let shutdown = Shutdown::new();
    let worker = Worker::new(
        Arc::clone(&store),
        handler,
        settings("suite-worker", Duration::from_secs(60)),
    );
    let running = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { worker.run(shutdown).await }
    });

    wait_until(&*store, &settles.id(), "be settled by the worker", |job| {
        job.state() == JobState::Succeeded
    })
    .await;
    wait_until(&*store, &refuses.id(), "be failed by the worker", |job| {
        job.last_error().is_some()
    })
    .await;
    shutdown.trigger();
    let report: Report = running
        .await
        .expect("the worker finishes")
        .expect("the worker drains without an error");

    // Two attempts for the job that refuses (its budget is two, and the worker keeps turning), and
    // one for the job that succeeds: three claims, two failures, one success.
    assert_eq!(
        report.claimed(),
        3,
        "the worker claimed the job that refuses twice — the first attempt sent it back to the \
         queue, and the turn after that took it again — and the job that succeeds once"
    );
    assert_eq!(
        report.succeeded(),
        1,
        "and settled one of them the way its handler said"
    );
    assert_eq!(
        report.failed(),
        2,
        "and the other one the other way, twice: the budget it was given is what ended it"
    );
    let runs = Suite::runs(&seen);
    assert_eq!(runs.len(), 3, "the handler saw every attempt: {runs:?}");
    assert_eq!(
        runs.iter().filter(|id| **id == refuses.id()).count(),
        2,
        "the job that refuses was handled twice — one call per attempt, and no attempt was handed \
         to two handlers at once"
    );

    let refused = row(&*store, &refuses.id()).await;
    assert_eq!(
        refused.state(),
        JobState::Failed,
        "its budget of two attempts is spent, so the job is out of the queue: a handler's failure \
         is the job's, and the worker does not decide otherwise"
    );
    assert_eq!(
        refused.attempts(),
        2,
        "and both attempts are counted: an attempt starts when a worker takes the job"
    );
    assert!(
        refused
            .last_error()
            .is_some_and(|reason| reason.contains("421")),
        "the handler's own words are what the row keeps, so an operator reads why: {:?}",
        refused.last_error()
    );
    assert_eq!(
        trail(&*store, &refuses.id()).await,
        vec![EventKind::AttemptFailed, EventKind::AttemptFailed],
        "and the trail has one line per settle the worker wrote \
         (`jobq/3.6-transaction-isolation`), so the story of two attempts is readable afterwards"
    );
    assert_eq!(
        refused.lease_owner(),
        None,
        "a job nobody is running is held by nobody: the lease went away with the attempt"
    );
}

/// A worker that is told to stop stops claiming, and hands back what it is still holding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopping_worker_hands_back_what_it_holds() {
    let backend = backend().await;
    let store = store_leasing(&backend, Duration::from_secs(60)).await;
    let (handler, _, released) = Suite::new(Duration::from_secs(30));

    let held = enqueue(&*store, SubmitRequest::new(queue(), payload("wait"))).await;
    let shutdown = Shutdown::new();
    let worker = Worker::new(
        Arc::clone(&store),
        handler,
        settings("suite-worker", Duration::from_secs(60)).with_shutdown(Duration::from_millis(300)),
    );
    let running = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { worker.run(shutdown).await }
    });
    wait_until(&*store, &held.id(), "be claimed", |job| {
        job.state() == JobState::Running
    })
    .await;

    let started = Instant::now();
    shutdown.trigger();
    let report = running
        .await
        .expect("the worker finishes")
        .expect("a deadline that passes is not an error: it is what the deadline is for");
    let drained = started.elapsed();

    assert!(
        drained < Duration::from_secs(3),
        "the drain gave the job its 300ms and then stopped waiting: it took {drained:?}. A stop that \
         waits for the handler is a stop that takes as long as the slowest job in the queue"
    );
    assert_eq!(report.handed_back(), 1, "the job went back to the queue");
    let row = row(&*store, &held.id()).await;
    assert_eq!(
        row.state(),
        JobState::Queued,
        "handed back, not failed: nothing about the job went wrong, this worker did"
    );
    assert_eq!(
        row.attempts(),
        1,
        "the attempt that started is still counted"
    );
    assert_eq!(
        row.lease_owner(),
        None,
        "and the lease is released, which is the difference between a rollout that costs seconds and \
         one that costs a lease"
    );
    assert_eq!(
        trail(&*store, &held.id()).await,
        vec![EventKind::Reclaimed],
        "the trail says the attempt was abandoned rather than finished"
    );
    released.notify_waiters();
}

/// An expired lease comes back with its attempt counted, and the next worker takes it from there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_lease_comes_back_to_the_queue() {
    let backend = backend().await;
    let lease = Duration::from_millis(300);
    let store: Arc<dyn Store> = store_leasing(&backend, lease).await;
    let (handler, _, _released) = Suite::new(Duration::from_secs(5));

    let job = enqueue(&*store, SubmitRequest::new(queue(), payload("succeed"))).await;
    assert_eq!(
        claim(&*store, &WorkerId::new("ghost"), 1).await.len(),
        1,
        "a worker takes the job and then stops existing: that is the whole of this case, and it is \
         why nothing here settles or releases it"
    );
    wait_until(&*store, &job.id(), "have its lease run out", |job| {
        job.lease_expires_at().is_some_and(|at| at <= Utc::now())
    })
    .await;

    let shutdown = Shutdown::new();
    let worker = Worker::new(
        Arc::clone(&store),
        handler,
        settings("suite-worker", lease).with_poll_interval(Duration::from_millis(20)),
    );
    let running = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { worker.run(shutdown).await }
    });
    wait_until(
        &*store,
        &job.id(),
        "be finished by the next worker",
        |job| job.state() == JobState::Succeeded,
    )
    .await;
    shutdown.trigger();
    let report = running
        .await
        .expect("the worker finishes")
        .expect("recovering a dead worker's job is not an error");

    assert!(
        report.reclaimed() >= 1,
        "the worker put the abandoned row back before claiming it: report says {} reclaimed",
        report.reclaimed()
    );
    let finished = row(&*store, &job.id()).await;
    assert_eq!(
        finished.attempts(),
        2,
        "the attempt nobody watched is counted: one from the worker that died, one from the worker \
         that finished it. Forgiving it is how a payload that kills every worker is retried for ever"
    );
    assert_eq!(
        trail(&*store, &job.id()).await,
        vec![EventKind::Reclaimed, EventKind::Completed],
        "and the trail is the story an operator reconstructs the outage from: the attempt was \
         abandoned, then the next one finished the job"
    );
}

/// A worker whose lease was reclaimed cannot settle the attempt somebody else is running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_worker_cannot_settle_a_job_it_no_longer_holds() {
    let backend = backend().await;
    let lease = Duration::from_millis(300);
    let store: Arc<dyn Store> = store_leasing(&backend, lease).await;

    // The worker that is about to be replaced takes the job and keeps it past its lease, which is
    // exactly what a process that is still alive but not renewing looks like from the outside.
    let slow = WorkerId::new("slow-worker");
    let job = enqueue(&*store, SubmitRequest::new(queue(), payload("wait"))).await;
    assert_eq!(claim(&*store, &slow, 1).await.len(), 1);
    wait_until(&*store, &job.id(), "have its lease run out", |job| {
        job.lease_expires_at().is_some_and(|at| at <= Utc::now())
    })
    .await;

    // The next worker recovers it and is handling it when the slow one comes back to settle.
    let (handler, _, released) = Suite::new(Duration::from_secs(30));
    let shutdown = Shutdown::new();
    let worker = Worker::new(
        Arc::clone(&store),
        handler,
        settings("next-worker", lease).with_poll_interval(Duration::from_millis(20)),
    );
    let running = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { worker.run(shutdown).await }
    });
    wait_until(&*store, &job.id(), "be claimed by the next worker", |job| {
        job.state() == JobState::Running
            && job.lease_owner().map(WorkerId::as_str) == Some("next-worker")
    })
    .await;

    let finished = store.complete(&slow, &job.id()).await;
    assert!(
        matches!(finished, Err(Error::Invalid { .. })),
        "a worker whose claim was reclaimed may not complete the row any more: expected \
         `Error::Invalid`, got {finished:?}. Without the lease in the guard, the settle would match \
         *running* and overwrite the attempt somebody else is in the middle of"
    );
    let failed = store
        .fail(&slow, &job.id(), "the slow worker gave up")
        .await;
    assert!(
        matches!(failed, Err(Error::Invalid { .. })),
        "and it may not fail it either: expected `Error::Invalid`, got {failed:?}"
    );
    let untouched = row(&*store, &job.id()).await;
    assert_eq!(
        untouched.state(),
        JobState::Running,
        "nothing the late worker sent changed the row"
    );
    assert_eq!(
        untouched.attempts(),
        2,
        "and the attempt count did not move: the retry that was refused wrote nothing"
    );
    assert_eq!(
        trail(&*store, &job.id()).await,
        vec![EventKind::Reclaimed],
        "and the trail has only the reclaim: a refused settle leaves nothing behind"
    );

    // The worker that *does* hold it finishes it, and the attempt the slow one started stays counted.
    released.notify_waiters();
    wait_until(&*store, &job.id(), "be finished by its owner", |job| {
        job.state() == JobState::Succeeded
    })
    .await;
    shutdown.trigger();
    running
        .await
        .expect("the worker finishes")
        .expect("draining after a normal turn is not an error");
}

/// A job that keeps being renewed is not taken away, however long its handler takes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_that_keeps_being_renewed_is_not_stolen() {
    let backend = backend().await;
    // Two seconds of lease against four and a half seconds of work: the renewal has half the lease to
    // spare, twice over, so a machine that stalls for a moment does not turn this into a failure
    // about the machine (authoring.md §6.4's "a condition, not a clock" — the condition here *is*
    // time passing, and it is given a margin rather than a sleep in the middle).
    let lease = Duration::from_secs(2);
    let store: Arc<dyn Store> = store_leasing(&backend, lease).await;
    let job = enqueue(&*store, SubmitRequest::new(queue(), payload("wait"))).await;

    let (holder_handler, _, released) = Suite::new(Duration::from_secs(30));
    let holder_shutdown = Shutdown::new();
    let holder = Worker::new(
        Arc::clone(&store),
        holder_handler,
        settings("holder", lease).with_poll_interval(Duration::from_millis(50)),
    );
    let holding = tokio::spawn({
        let shutdown = holder_shutdown.clone();
        async move { holder.run(shutdown).await }
    });

    wait_until(&*store, &job.id(), "be claimed by its holder", |job| {
        job.state() == JobState::Running
            && job.lease_owner().map(WorkerId::as_str) == Some("holder")
    })
    .await;
    // The second worker starts *after* that, and that order is the case: two workers asking at the
    // same instant is case `claims_running_at_the_same_time_never_overlap` of
    // `jobq/3.5-store-port-conformance`, and what is under test here is what happens to a job that
    // one of them is already holding when the other looks.
    let (watcher_handler, watcher_seen, _watcher_release) = Suite::new(Duration::from_secs(30));
    let watcher_shutdown = Shutdown::new();
    let watcher = Worker::new(
        Arc::clone(&store),
        watcher_handler,
        settings("watcher", lease).with_poll_interval(Duration::from_millis(50)),
    );
    let watching = tokio::spawn({
        let shutdown = watcher_shutdown.clone();
        async move { watcher.run(shutdown).await }
    });

    // Watch the row for more than two leases, and check the thing that matters on every look: it is
    // still held, and its lease has never run out (a reclaim would have moved it back to `queued`
    // and the watcher would have claimed it).
    let until = Instant::now() + Duration::from_millis(4500);
    while Instant::now() < until {
        let current = row(&*store, &job.id()).await;
        assert_eq!(
            current.state(),
            JobState::Running,
            "the row went back to {:?} while its worker was still handling it",
            current.state()
        );
        assert_eq!(
            current.lease_owner().map(WorkerId::as_str),
            Some("holder"),
            "the row belongs to the worker that took it, for as long as it says so"
        );
        sleep(POLL).await;
    }

    released.notify_waiters();
    wait_until(&*store, &job.id(), "be finished by its holder", |job| {
        job.state() == JobState::Succeeded
    })
    .await;
    watcher_shutdown.trigger();
    holder_shutdown.trigger();
    let watched = watching
        .await
        .expect("the watcher finishes")
        .expect("draining is not an error");
    holding
        .await
        .expect("the holder finishes")
        .expect("draining is not an error");

    assert_eq!(
        row(&*store, &job.id()).await.attempts(),
        1,
        "the job was attempted once: a second attempt would mean the lease lapsed while the first \
         worker was still running it, and the queue handed the same work to somebody else"
    );
    assert_eq!(
        watched.reclaimed(),
        0,
        "the watcher reclaimed nothing: a lease that is being renewed is not an expired one"
    );
    assert_eq!(
        watched.claimed(),
        0,
        "and it claimed nothing: the row was never free"
    );
    assert!(
        Suite::runs(&watcher_seen).is_empty(),
        "the watcher's handler never saw the job: {:?}",
        Suite::runs(&watcher_seen)
    );
}

/// The daemon drains what is in its queue, and stops when it is asked to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_daemon_drains_what_it_claims() {
    let backend = backend().await;
    let Some(url) = container_url(&backend) else {
        return;
    };
    let store = backend.store();
    let handler = Script::write("drain", "exit 0");

    let mut daemon = Service::start(&[
        var("JOBQ__STORE__URL", url),
        var("JOBQ__WORKER__NAME", "daemon"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "5"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
        var("JOBQ__WORKER__HANDLER", handler.command()),
    ]);

    let first = enqueue(store, SubmitRequest::new(queue(), payload("succeed"))).await;
    let second = enqueue(store, SubmitRequest::new(queue(), payload("succeed"))).await;
    for job in [&first, &second] {
        wait_until(store, &job.id(), "be settled by the daemon", |job| {
            job.state() == JobState::Succeeded
        })
        .await;
        assert_eq!(row(store, &job.id()).await.attempts(), 1);
    }

    daemon.stop();
    assert_eq!(
        daemon.wait_for_exit(),
        0,
        "`SIGTERM` is a stop, not a crash: the daemon exits 0 (the contract \
         `jobq/3.1-engine-boundary` pinned)"
    );
    assert!(
        !daemon.is_running(),
        "and it is gone: {:?}",
        daemon.log_lines()
    );
}

/// A daemon killed in the middle of a job leaves a lease, and the next worker recovers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_killed_daemon_gives_its_job_back() {
    let backend = backend().await;
    let Some(url) = container_url(&backend) else {
        return;
    };
    let store = backend.store();
    let slowness = Script::write("killed", "exec sleep 30");
    let quickness = Script::write("recovery", "exit 0");

    let job = enqueue(
        store,
        SubmitRequest::new(queue(), payload("succeed")).with_max_attempts(3),
    )
    .await;
    let mut killed = Service::start(&[
        var("JOBQ__STORE__URL", url.clone()),
        var("JOBQ__WORKER__NAME", "doomed"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "2"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
        var("JOBQ__WORKER__HANDLER", slowness.command()),
    ]);

    let held = wait_until(store, &job.id(), "be claimed by the daemon", |job| {
        job.state() == JobState::Running
    })
    .await;
    assert_eq!(
        held.lease_owner().map(WorkerId::as_str),
        Some("doomed"),
        "the row says who is holding it: that is the name an operator greps for, and the whole of \
         what the recovery has to go on"
    );

    // `kill -9`: no handler, no cleanup, no chance to hand anything back. The only thing left is the
    // instant on the row.
    killed.kill();

    let mut recovery = Service::start(&[
        var("JOBQ__STORE__URL", url),
        var("JOBQ__WORKER__NAME", "recovery"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "2"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
        var("JOBQ__WORKER__HANDLER", quickness.command()),
    ]);

    wait_until(store, &job.id(), "be finished by the next daemon", |job| {
        job.state() == JobState::Succeeded
    })
    .await;
    assert_eq!(
        row(store, &job.id()).await.attempts(),
        2,
        "the attempt the killed daemon spent is counted: it was started, and a queue that forgot it \
         would hand a poison payload out for ever"
    );
    assert_eq!(
        trail(store, &job.id()).await,
        vec![EventKind::Reclaimed, EventKind::Completed],
        "and the trail says what happened: abandoned, then finished"
    );

    recovery.stop();
    assert_eq!(recovery.wait_for_exit(), 0);
}

/// A job whose attempts are spent is not handed out again, even though nobody failed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_whose_attempts_are_spent_is_not_handed_out_again() {
    let backend = backend().await;
    let Some(url) = container_url(&backend) else {
        return;
    };
    let store = backend.store();
    let slowness = Script::write("spent", "exec sleep 30");
    let quickness = Script::write("spent-recovery", "exit 0");

    let job = enqueue(
        store,
        SubmitRequest::new(queue(), payload("succeed")).with_max_attempts(1),
    )
    .await;
    let mut killed = Service::start(&[
        var("JOBQ__STORE__URL", url.clone()),
        var("JOBQ__WORKER__NAME", "doomed"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "2"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
        var("JOBQ__WORKER__HANDLER", slowness.command()),
    ]);
    wait_until(store, &job.id(), "be claimed", |job| {
        job.state() == JobState::Running
    })
    .await;
    killed.kill();

    let mut recovery = Service::start(&[
        var("JOBQ__STORE__URL", url),
        var("JOBQ__WORKER__NAME", "recovery"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "2"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
        var("JOBQ__WORKER__HANDLER", quickness.command()),
    ]);

    let spent = wait_until(store, &job.id(), "be given up on", |job| {
        job.state() == JobState::Failed
    })
    .await;
    assert_eq!(
        spent.attempts(),
        1,
        "the reclaim counted the attempt the dead worker spent, and its budget was one: the job is \
         out of the queue, which is the only thing that ends a payload that kills whoever runs it"
    );
    assert!(
        spent.last_error().is_some(),
        "and the row says why it is there, because the trail is not the first place anybody looks"
    );

    recovery.stop();
    assert_eq!(recovery.wait_for_exit(), 0);
}

/// `SIGTERM` stops the claiming and lets the job in flight finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signal_drains_without_claiming_again() {
    let backend = backend().await;
    let Some(url) = container_url(&backend) else {
        return;
    };
    let store = backend.store();
    let handler = Script::write("draining", "sleep 2");

    let in_flight = enqueue(store, SubmitRequest::new(queue(), payload("succeed"))).await;
    let mut daemon = Service::start(&[
        var("JOBQ__STORE__URL", url),
        var("JOBQ__WORKER__NAME", "daemon"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "10"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
        var("JOBQ__WORKER__SHUTDOWN_SECONDS", "10"),
        var("JOBQ__WORKER__HANDLER", handler.command()),
    ]);
    wait_until(store, &in_flight.id(), "be claimed", |job| {
        job.state() == JobState::Running
    })
    .await;

    // The signal arrives while that job is being handled, and a second job is written right after it:
    // the job in flight finishes, and the one nobody had claimed yet is still waiting when the
    // process is gone.
    daemon.stop();
    let afterwards = enqueue(store, SubmitRequest::new(queue(), payload("succeed"))).await;

    assert_eq!(
        daemon.wait_for_exit(),
        0,
        "the in-flight job finished inside the drain window, so the stop was clean"
    );
    assert_eq!(
        row(store, &in_flight.id()).await.state(),
        JobState::Succeeded,
        "what was in flight when the signal arrived was finished rather than abandoned: a drain that \
         dropped it would make every rollout look like a failure"
    );
    let waiting = row(store, &afterwards.id()).await;
    assert_eq!(
        waiting.state(),
        JobState::Queued,
        "and nothing was claimed after the signal: the job written right after it is still waiting"
    );
    assert_eq!(
        waiting.attempts(),
        0,
        "with no attempt spent on it, so the next process takes it as new work"
    );
}

/// When the drain deadline passes, the job in flight is handed back rather than waited for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_drain_deadline_hands_the_job_back() {
    let backend = backend().await;
    let Some(url) = container_url(&backend) else {
        return;
    };
    let store = backend.store();
    let handler = Script::write("deadline", "exec sleep 30");

    let job = enqueue(store, SubmitRequest::new(queue(), payload("succeed"))).await;
    let mut daemon = Service::start(&[
        var("JOBQ__STORE__URL", url),
        var("JOBQ__WORKER__NAME", "daemon"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "30"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
        var("JOBQ__WORKER__SHUTDOWN_SECONDS", "1"),
        var("JOBQ__WORKER__HANDLER", handler.command()),
    ]);
    wait_until(store, &job.id(), "be claimed", |job| {
        job.state() == JobState::Running
    })
    .await;

    let signalled = Instant::now();
    daemon.stop();
    assert_eq!(
        daemon.wait_for_exit(),
        0,
        "a deadline is a decision, not an error"
    );
    let stopped = signalled.elapsed();
    assert!(
        stopped < Duration::from_secs(10),
        "the daemon gave the job its one second and then stopped waiting: it took {stopped:?}"
    );

    let handed_back = row(store, &job.id()).await;
    assert_eq!(
        handed_back.state(),
        JobState::Queued,
        "the job went back to the queue rather than staying invisible for the rest of its lease"
    );
    assert_eq!(
        handed_back.attempts(),
        1,
        "with the attempt it spent still counted"
    );
    assert_eq!(
        handed_back.lease_owner(),
        None,
        "and no owner: the process that held it is gone, and the row says so"
    );
}

/// A daemon on the in-memory store announces itself and stops on a signal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_daemon_on_the_memory_store_starts_and_stops() {
    let backend = backend().await;
    if container_url(&backend).is_some() {
        return;
    }

    let mut daemon = Service::start(&[
        var("JOBQ__STORE__URL", "memory"),
        var("JOBQ__WORKER__NAME", "daemon"),
        var("JOBQ__WORKER__QUEUES", QUEUE),
        var("JOBQ__WORKER__LEASE_SECONDS", "2"),
        var("JOBQ__WORKER__POLL_INTERVAL_MS", "20"),
    ]);
    assert!(daemon.is_running(), "it is draining");
    daemon.stop();
    assert_eq!(
        daemon.wait_for_exit(),
        0,
        "and `SIGTERM` ends it the way `jobq/3.1-engine-boundary` pinned: exit 0"
    );
}
