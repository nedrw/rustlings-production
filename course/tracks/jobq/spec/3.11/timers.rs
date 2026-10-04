//! Spec for `jobq/3.11-delayed-timers`: the ready set, and the worker that sleeps on it.
//!
//! `kiln run` copies this file into the project as `tests/spec_timers.rs`. The store comes from the
//! fixture `jobq/3.5-store-port-conformance` built (`KILN_BACKEND` picks it), because the deadlines
//! the ready set holds are the store's own answer to a question the port now asks: what a worker
//! waits for and what the store will hand out have to be the same statement, and they are two
//! implementations of it.
//!
//! Four decisions about what this file is allowed to assert:
//!
//! * **the structure is judged on its own, deterministically.** Inserting, moving and forgetting a
//!   deadline, the earliest of them, and the arithmetic of the wait are properties of a value: no
//!   clock, no store, no container, and a failure that says which of them was wrong.
//! * **the size is part of the promise.** A hundred thousand recorded deadlines leave one entry per
//!   *queue*, and asking for the earliest a hundred thousand times has to be something a unit test can
//!   do — which is the whole difference between a worker that waits and a worker that polls.
//! * **the deadline is judged by what it saves.** The end-to-end case schedules a job *inside* the
//!   worker's poll interval and with a poll interval far outside it: a worker that only polled cannot
//!   pass it, and the case says so in its assertion message rather than in a comment.
//! * **the floor is judged twice**, because it is the half that is easy to lose: once in the table
//!   (a deadline an hour out waits the poll interval, not an hour), and once end to end on a clock the
//!   case moves, where a deadline two hours out must not become a two-hour sleep. The clock is
//!   `jobq/3.8-clock-injection`'s, handed to the store and to the worker as the same value — which is
//!   the only way "the deadline is two hours away" can be a case rather than a two-hour test.

// A test may read the wall clock; the *library* may not, and `jobq/3.8-clock-injection` is where the
// project's `clippy.toml` started refusing it — for every target, which is why the relaxation has to
// be written down here rather than left to a reviewer to remember (authoring.md §7.4). The end-to-end
// case needs a real instant to schedule a job a fraction of a second ahead, which is exactly the shape
// a worker's deadline has to handle in production.
#![allow(clippy::disallowed_methods)]

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use jobq::engine::{
    BoxFuture, Clock, Error, Job, JobId, JobState, MemoryStore, PostgresStore, Queue, ReadySet,
    Store, SubmitRequest, WorkerId,
};
use jobq::service::worker::{Handler, Shutdown, Worker, WorkerSettings};
use support::store::{Backend, backend};
use tokio::time::sleep;

/// The queue every case in this file uses.
const QUEUE: &str = "emails";
/// How often a case looks again while it waits for a condition.
const POLL: Duration = Duration::from_millis(20);
/// How long a case waits for a worker to settle something.
const DEADLINE: Duration = Duration::from_secs(5);
/// How long the end-to-end case's worker waits between turns when it has nothing to wake for: far
/// longer than the deadline under test, so a worker that only polled cannot pass the case.
const SLOW_POLL: Duration = Duration::from_secs(10);

/// That queue, parsed, with the failure a learner can act on.
fn queue() -> Queue {
    Queue::parse(QUEUE).expect("`emails` is the name this file uses everywhere")
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> serde_json::Value {
    serde_json::json!({ "to": body })
}

/// An instant, written the way this file reads them.
fn at(text: &str) -> DateTime<Utc> {
    text.parse()
        .unwrap_or_else(|error| panic!("`{text}` is an instant: {error}"))
}

/// A clock the case moves by hand, starting where no wall clock will reach during the run.
#[derive(Debug)]
struct FakeClock {
    /// The instant everything measured from it is measured from.
    now: Mutex<DateTime<Utc>>,
}

impl FakeClock {
    /// A clock stopped at `instant` (RFC 3339).
    fn at(instant: &str) -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(at(instant)),
        })
    }

    /// Move it forward.
    fn advance(&self, by: TimeDelta) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The handler these cases wire: reaching it is settling the job, and what the cases watch is the
/// store's row rather than the handler's memory.
struct Settles;

impl Handler for Settles {
    fn handle<'a>(&'a self, _job: &'a Job) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}

/// Settings for a worker of this file: one queue, one job per turn, and the poll interval under test.
fn settings(poll: Duration) -> WorkerSettings {
    WorkerSettings::new(WorkerId::new("timer-suite"), vec![queue()])
        .with_batch(4)
        .with_poll_interval(poll)
        .with_shutdown(Duration::from_secs(2))
        .with_lease(Duration::from_secs(60))
}

/// A store of this run's backend, as the worker needs it: shared, because the worker owns what it
/// drains and the case keeps a handle to ask the store questions.
///
/// The fixture hands out a store by reference, and this is a second one over the same backend — the
/// shape `jobq/3.7-leases-and-recovery` established, and the reason it is a copy rather than a
/// conversion is that the two callers want different things: the case wants `&dyn Store`, the worker
/// wants an `Arc` it can move into its tasks.
async fn store_for(backend: &Backend) -> Arc<dyn Store> {
    match backend {
        Backend::Memory(_) => Arc::new(MemoryStore::new()),
        Backend::Postgres { container, .. } => Arc::new(
            PostgresStore::connect(&container.url())
                .await
                .expect("the container the fixture started is reachable"),
        ),
    }
}

/// Wait until this job answers `holds`.
///
/// A condition with a deadline rather than a `sleep` (authoring.md §6.4): the failure message carries
/// what the row said instead, which is what a case that timed out is trying to find out.
async fn wait_until<F>(store: &dyn Store, id: &JobId, what: &str, holds: F)
where
    F: Fn(&Job) -> bool,
{
    let until = Instant::now() + DEADLINE;
    loop {
        let current = store
            .get(id)
            .await
            .unwrap_or_else(|error| panic!("the store refused a read: {error}"))
            .expect("the job this case is about is still there");
        if holds(&current) {
            return;
        }
        assert!(
            Instant::now() < until,
            "the job did not {what} within {DEADLINE:?}: it is {:?} with {} attempt(s)",
            current.state(),
            current.attempts()
        );
        sleep(POLL).await;
    }
}

/// The structure's own behaviour: record, move, forget, and the earliest of what is left.
#[test]
fn the_ready_set_answers_when_the_next_queue_is_due() {
    let mut deadlines = ReadySet::new();
    assert!(deadlines.is_empty(), "nothing has been recorded yet");
    assert_eq!(deadlines.len(), 0, "and it holds nothing to wait for");
    assert_eq!(
        deadlines.next_deadline(),
        None,
        "a worker holding this waits its poll interval: it has been told nothing, which is not the \
         same as being told nothing is due — that is what `remove` is for"
    );

    let noon = at("2030-01-01T12:00:00Z");
    let evening = at("2030-01-01T20:00:00Z");
    // Recorded in the wrong order on purpose: the answer is the earliest instant, not the first call.
    deadlines.insert(queue(), evening);
    deadlines.insert(Queue::parse("exports").expect("a queue name"), noon);
    assert_eq!(
        deadlines.next_deadline(),
        Some(noon),
        "the earliest deadline any queue owes, whatever order they were recorded in"
    );
    assert_eq!(
        deadlines.len(),
        2,
        "two queues owe work, and the size counts queues"
    );

    deadlines.remove(&Queue::parse("exports").expect("a queue name"));
    assert_eq!(
        deadlines.next_deadline(),
        Some(evening),
        "forgetting one queue leaves the other one's deadline, which is now the earliest"
    );
    deadlines.remove(&Queue::parse("exports").expect("a queue name"));
    assert_eq!(
        deadlines.len(),
        1,
        "forgetting a queue that owes nothing changes nothing: a worker asks this for every queue it \
         drains, and it cannot know which of them were known before it asked"
    );

    deadlines.remove(&queue());
    assert!(deadlines.is_empty(), "and then nothing is left to wait for");
    assert_eq!(
        deadlines.next_deadline(),
        None,
        "so there is no deadline to sleep on"
    );
}

/// A deadline that moves replaces the one it had, in both directions.
#[test]
fn a_deadline_that_moves_leaves_nothing_behind() {
    let mut deadlines = ReadySet::new();
    deadlines.insert(queue(), at("2030-01-01T09:00:00Z"));

    // A claim took the work, so the queue's next job is due later: the deadline moves forward.
    deadlines.insert(queue(), at("2030-01-01T11:00:00Z"));
    assert_eq!(
        deadlines.len(),
        1,
        "a deadline that moves is the same queue's, not a second entry: the promise is one entry per \
         queue with work, and a structure that grew here would grow with every claim"
    );
    assert_eq!(
        deadlines.next_deadline(),
        Some(at("2030-01-01T11:00:00Z")),
        "and the answer is the new deadline: the superseded instant is not this queue's any more, \
         and a worker that woke for it would wake for work nobody is waiting to give it"
    );

    // A job submitted (or reclaimed, or retried) closer than that moves it back: a moved deadline has
    // to be a *replacement*, not a floor on how early the answer may be.
    deadlines.insert(queue(), at("2030-01-01T09:30:00Z"));
    assert_eq!(
        deadlines.next_deadline(),
        Some(at("2030-01-01T09:30:00Z")),
        "a deadline that moves earlier is the answer too: this is the half a structure that only ever \
         pushed new entries would get right by accident until the day one moved the other way"
    );

    // And the retraction has to be real: what survives a queue's removal is another queue's deadline.
    deadlines.insert(
        Queue::parse("exports").expect("a queue name"),
        at("2030-01-01T10:00:00Z"),
    );
    deadlines.remove(&queue());
    assert_eq!(
        deadlines.next_deadline(),
        Some(at("2030-01-01T10:00:00Z")),
        "one of the two instants this queue no longer owes would answer here if the moves had been \
         recorded rather than applied, and the failure would look like a worker waking for work that \
         is not there"
    );
    assert_eq!(deadlines.len(), 1, "one queue is left");
}

/// A hundred thousand recorded deadlines are still one entry per queue, and still cheap to ask about.
#[test]
fn a_hundred_thousand_deadlines_are_still_one_entry_per_queue() {
    // Sixty-four queues, each with a year's worth of recorded deadlines: what a worker that has been
    // running for a while has said to this structure, and what a million jobs scheduled a year out
    // would look like from in here.
    const QUEUES: usize = 64;
    const PUSHES: usize = 100_000;
    let start = at("2030-01-01T00:00:00Z");
    let mut deadlines = ReadySet::new();
    for push in 0..PUSHES {
        let queue = Queue::parse(&format!("q{}", push % QUEUES)).expect("a queue name");
        let at = start + TimeDelta::hours(push as i64);
        deadlines.insert(queue, at);
    }
    assert_eq!(
        deadlines.len(),
        QUEUES,
        "the size is the number of queues, not the number of deadlines recorded: {PUSHES} pushes over \
         {QUEUES} queues leave {QUEUES} entries, and a structure that kept the superseded ones would \
         be holding a million jobs' worth of memory to answer a question about queues"
    );
    assert_eq!(
        deadlines.next_deadline(),
        Some(start + TimeDelta::hours(PUSHES as i64 - QUEUES as i64)),
        "the latest deadline recorded for each queue is the one it owes: queue 0 was last pushed \
         {QUEUES} pushes ago, and that is the earliest of the sixty-four answers"
    );

    // The hot path, and the reason the structure is what it is: asking for the earliest is what an
    // idle worker does on every turn, so it may not be a scan of everything recorded so far.
    let started = Instant::now();
    for _ in 0..PUSHES {
        assert!(
            deadlines.next_deadline().is_some(),
            "there is always an answer here"
        );
    }
    let spent = started.elapsed();
    assert!(
        spent < Duration::from_secs(5),
        "{PUSHES} questions took {spent:?}: the earliest deadline is asked for on every idle turn, and \
         an answer that scans what has been recorded turns a worker that waits into one that works"
    );
}

/// The wait is the deadline or the floor, whichever comes first.
#[test]
fn the_wait_is_the_deadline_or_the_floor_whichever_comes_first() {
    let floor = Duration::from_secs(30);
    let now = at("2030-01-01T00:00:00Z");
    let mut deadlines = ReadySet::new();
    assert_eq!(
        deadlines.wait(now, floor),
        floor,
        "nothing scheduled: the floor is what a worker waits, because a worker that waited for ever \
         would never see the job another process submits a second later"
    );

    deadlines.insert(queue(), now + TimeDelta::milliseconds(200));
    assert_eq!(
        deadlines.wait(now, floor),
        Duration::from_millis(200),
        "a job due in 200ms is a wake in 200ms: this is the whole point of holding a deadline, and a \
         worker that waited the floor instead is the poll loop this step replaces"
    );

    deadlines.insert(queue(), now + TimeDelta::hours(1));
    assert_eq!(
        deadlines.wait(now, floor),
        floor,
        "a deadline an hour out waits the *floor*, not the hour: the deadline may only shorten the \
         wait, because the store was asked a moment ago and a snapshot is not a promise about the \
         next hour"
    );

    deadlines.insert(queue(), now);
    assert_eq!(
        deadlines.wait(now, floor),
        floor,
        "a deadline that has already been reached is not a reason to look again *immediately*: the \
         work is due and the claim that just came back empty did not take it, which means something \
         other than the clock is holding it (a per-queue cap, or another worker) — waiting zero for \
         that is how an idle worker becomes a spin against the store"
    );
    deadlines.insert(queue(), now - TimeDelta::minutes(5));
    assert_eq!(
        deadlines.wait(now, floor),
        floor,
        "and a deadline in the past is the same case, not a negative duration"
    );
}

/// A job that becomes due inside the poll interval is claimed anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_due_before_the_next_poll_is_claimed_anyway() {
    let backend = backend().await;
    let store = store_for(&backend).await;
    let queue = queue();

    // 200ms from now, against a poll interval of ten seconds: the deadline is what makes the worker
    // wake, or nothing does.
    let due = Utc::now() + TimeDelta::milliseconds(200);
    let job = store
        .enqueue(SubmitRequest::new(queue.clone(), payload("succeed")).with_run_at(due))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "the {} backend refused a request the port accepts: {error}",
                backend.name()
            )
        });

    // The store's own half, asked the way the worker asks it: the earliest instant a job of this
    // queue may be claimed. A store that answered anything else would have the worker waiting for
    // work that is already due, or waking for a job it cannot have.
    let answered = store
        .next_due(&queue)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "the {} backend refused the question: {error}",
                backend.name()
            )
        })
        .expect("the queue has one job, and it is queued");
    assert!(
        (answered - due).num_milliseconds().abs() <= 1,
        "the store answers with the instant the job becomes claimable ({due}), and it answered \
         {answered}: an instant that survived a round trip through the backend, which is the only \
         thing a worker can compare a clock against"
    );
    assert_eq!(
        store
            .next_due(&Queue::parse("nothing-here").expect("a queue name"))
            .await
            .unwrap_or_else(|error| panic!(
                "the {} backend refused the question: {error}",
                backend.name()
            )),
        None,
        "and a queue with nothing queued answers `None` rather than an instant in the past: a worker \
         that was told `now` for an empty queue would never sleep"
    );

    let shutdown = Shutdown::new();
    let worker = Worker::new(Arc::clone(&store), Settles, settings(SLOW_POLL));
    let running = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { worker.run(shutdown).await }
    });

    // The deadline is the case's own and the poll interval is ten seconds: a worker that only polled
    // would not have looked again yet when this gives up, so the instant the *store* named — the
    // deadline the ready set holds — is the only thing that can settle the job in time.
    wait_until(&*store, &job.id(), "be settled by the worker", |job| {
        job.state() == JobState::Succeeded
    })
    .await;

    shutdown.trigger();
    let report = running
        .await
        .expect("the worker finishes")
        .expect("the worker drains without an error");
    assert_eq!(
        report.succeeded(),
        1,
        "and it settled exactly one job: waking early is a latency win, not a second attempt"
    );
}

/// A deadline two hours out does not become a two-hour sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deadline_far_away_does_not_become_a_long_sleep() {
    // One clock, two holders: the store stamps `run_at` from it and the worker measures the wait
    // against it, because a worker with a clock of its own would be the second authority on when the
    // deadline arrives (`jobq/3.8-clock-injection`).
    let clock = FakeClock::at("2030-01-01T00:00:00Z");
    let shared: Arc<dyn Clock> = Arc::clone(&clock) as Arc<dyn Clock>;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new().with_clock(Arc::clone(&shared)));
    let queue = queue();

    // Two hours out, and the case moves the clock past it: this is the shape of a schedule far enough
    // ahead that "sleep until it" would be correct and still the wrong answer.
    let due = at("2030-01-01T02:00:00Z");
    let job = store
        .enqueue(SubmitRequest::new(queue.clone(), payload("succeed")).with_run_at(due))
        .await
        .expect("the in-memory store accepts this request");

    let shutdown = Shutdown::new();
    let worker = Worker::new(
        Arc::clone(&store),
        Settles,
        settings(Duration::from_millis(200)),
    )
    .with_clock(Arc::clone(&shared));
    let running = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { worker.run(shutdown).await }
    });

    // The two hours pass here rather than in the worker, in steps. The first step is a fraction of
    // the deadline, so whichever of the two happens first — the worker's first turn, or this loop —
    // the wait the worker computes is a wait for a deadline that has not arrived yet, which is the
    // wait this case is about; the steps after it are large enough to arrive well inside the case's
    // own deadline, so the condition below is what a *sleeping* worker fails rather than a race. A
    // worker that slept until the deadline would look again two hours from now: this loop would run
    // out of patience with the job still queued, which is the failure message below.
    let started = Instant::now();
    let until = started + DEADLINE;
    let mut step = TimeDelta::milliseconds(100);
    loop {
        let current = store
            .get(&job.id())
            .await
            .unwrap_or_else(|error| panic!("the store refused a read: {error}"))
            .expect("the job this case is about is still there");
        if current.state() == JobState::Succeeded {
            break;
        }
        assert!(
            Instant::now() < until,
            "the clock moved past {due} and the job is still {:?} with {} attempt(s) {DEADLINE:?} \
             after this case started, with a poll interval of 200ms: the interval is the floor, so a \
             deadline two hours out is a shorter wait rather than a two-hour sleep — a worker that \
             waited for the deadline would look exactly like this",
            current.state(),
            current.attempts()
        );
        clock.advance(step);
        step = TimeDelta::minutes(30);
        sleep(POLL).await;
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the job was settled {:?} after the clock started moving, with a poll interval of 200ms: the \
         deadline moved the worker's wake *earlier* than the interval, never later",
        started.elapsed()
    );

    shutdown.trigger();
    running
        .await
        .expect("the worker finishes")
        .expect("the worker drains without an error");
}
