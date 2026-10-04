//! Spec for `jobq/3.5-store-port-conformance`: the store port, judged from outside **every** backend
//! that claims to implement it.
//!
//! `kiln run` copies this file into the project as `tests/spec_port_conformance.rs`; it is the
//! course's file, not the learner's (plan §3 D2). It is one suite, and it is run more than once:
//! `support::store::backend` builds the store `KILN_BACKEND` names (`memory` when nothing says
//! otherwise), so the fast run judges the in-memory backend and the `conformance@2` gate re-runs
//! this same file with `KILN_BACKEND=postgres`. One file, N runs — not N files that drift apart.
//!
//! Four decisions about what this file is allowed to assert:
//!
//! * **the port's promises, and nothing else.** Not one assertion below names a table, a column, a
//!   query or a map: every one of them is a sentence from `Store`'s own documentation — the row an
//!   `enqueue` answers with, the order a `claim` hands jobs out in, the attempt a claim starts, the
//!   refusal to settle what nobody is running, `Ok(None)` for an identity nobody has. That is what
//!   makes the same file meaningful against backends that share no code: a suite that asserted how
//!   a backend *does* it would certify one implementation twice.
//! * **the order is a promise, not a habit.** The port's `claim` documentation fixes the order
//!   (priority, then the job that has been due longest, then identity) precisely so that a caller
//!   can have a policy at all; a backend that hands rows back in its own order is a backend that
//!   changes what the queue does. What this file deliberately does *not* assert is anything the port
//!   leaves open: how a backend stores its rows, what it does under the hood, what an index makes
//!   fast.
//! * **concurrency is one of the promises.** Two claims that run at the same instant may not
//!   overlap, and no single-threaded assertion can see that: the case at the bottom of this file
//!   starts several claims at once, which is the only way the read-then-write backend differs from
//!   the one that takes what it stamps.
//! * **every failure names the backend.** The same assertion now runs against two implementations,
//!   so a red line has to say which one disagreed — otherwise the report sends the reader to the
//!   suite when the bug is in a store.

// A test may read the wall clock; the *library* may not, and `jobq/3.8-clock-injection` is where the
// project's `clippy.toml` started refusing it — for every target, which is why the relaxation has to
// be written down here rather than left to a reviewer to remember (authoring.md §7.4). This file
// needs a real instant to say what a store that was *not* handed a clock does, and to compare a
// lease against the process's own clock rather than against an injected one.
#![allow(clippy::disallowed_methods)]

mod support;

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use jobq::engine::{
    DEFAULT_MAX_ATTEMPTS, Error, Job, JobId, JobState, Queue, SubmitRequest, WorkerId,
};
use support::store::{Backend, backend};

/// The queue every case in this file uses.
const QUEUE: &str = "emails";

/// That queue, parsed, with the failure a learner can act on.
fn queue() -> Queue {
    Queue::parse(QUEUE).expect("`emails` is the name this file uses everywhere")
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> serde_json::Value {
    serde_json::json!({ "to": body })
}

/// One write, with a failure that names the backend that refused it.
async fn enqueue(backend: &Backend, request: SubmitRequest) -> Job {
    backend
        .store()
        .enqueue(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "the {} backend refused a request the port accepts: {error}",
                backend.name()
            )
        })
}

/// One claim, with a failure that names the backend that refused it.
async fn claim(backend: &Backend, worker: &WorkerId, limit: usize) -> Vec<Job> {
    backend
        .store()
        .claim(worker, &queue(), limit)
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a claim: {error}", backend.name()))
}

/// One read, with a failure that names the backend that refused it.
async fn get(backend: &Backend, id: &JobId) -> Option<Job> {
    backend
        .store()
        .get(id)
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a read: {error}", backend.name()))
}

/// A write answers with the row it stored, and with the instant that row became due.
#[tokio::test]
async fn a_write_answers_with_the_row_and_the_instant_it_is_due() {
    let backend = backend().await;
    let queue = queue();

    let before = Utc::now();
    let job = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("alice@example.com")),
    )
    .await;
    let after = Utc::now();

    assert_eq!(
        job.state(),
        JobState::Queued,
        "a job that was just stored is waiting for a worker, on the {} backend",
        backend.name()
    );
    assert_eq!(job.attempts(), 0, "nothing has tried to run it yet");
    assert_eq!(
        job.max_attempts(),
        DEFAULT_MAX_ATTEMPTS,
        "a request that asked for no budget gets the default one: the row carries the budget the \
         caller asked for, so a worker holding it can decide whether another attempt is worth \
         anything"
    );
    assert_eq!(
        job.payload(),
        &payload("alice@example.com"),
        "the caller's payload travels unchanged"
    );
    assert!(
        job.run_at() >= before && job.run_at() <= after,
        "a request that named no instant became due the moment it was written: the {} backend \
         answered {:?}, and this case started between {before:?} and {after:?}. A store that left \
         `run_at` unset (or resolved it against some other clock) hands out a job that is not due \
         yet, or parks one that is",
        backend.name(),
        job.run_at()
    );

    // An instant the caller *did* name comes back exactly, not rounded to whatever the store's
    // clock happens to be able to hold: a schedule is the caller's decision.
    let at: DateTime<Utc> = before + Duration::milliseconds(1_500);
    let scheduled = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("bob@example.com")).with_run_at(at),
    )
    .await;
    assert_eq!(
        scheduled.run_at(),
        at,
        "a `run_at` the caller named is the instant the row carries: `None` is the only value the \
         store gets to resolve"
    );

    // And the row is readable through the port, not only in the answer to the write.
    let read = get(&backend, &job.id())
        .await
        .expect("the row that was just written is there");
    assert_eq!(read.id(), job.id());
    assert_eq!(read.state(), job.state());
    assert_eq!(read.payload(), job.payload());
    assert_eq!(
        read.run_at(),
        job.run_at(),
        "the instant a write resolved is the instant a read answers with"
    );

    // A request that could never run is refused rather than stored, and the port's own
    // documentation of `enqueue` is where that promise is written down.
    let refused = backend
        .store()
        .enqueue(
            SubmitRequest::new(queue.clone(), payload("nobody@example.com")).with_max_attempts(0),
        )
        .await
        .expect_err("a job with a budget of zero attempts can never run, so nothing may store it");
    assert!(
        matches!(refused, Error::Invalid { .. }),
        "an impossible request is the caller's mistake, not a store failure: expected \
         `Error::Invalid`, got {refused:?}"
    );
}

/// A claim hands out the jobs that are due, in the order the port fixes, stamped as running.
#[tokio::test]
async fn a_claim_hands_out_due_jobs_in_the_order_the_port_promises() {
    let backend = backend().await;
    let queue = queue();
    let worker = WorkerId::new("suite-worker");
    let now = Utc::now();

    // Four jobs that are all due, written so that each of the port's three ordering rules decides
    // one position: the highest priority first, then the job that has been due the longest, and
    // last the pair that ties on both — which identity settles, so two stores agree.
    let longest = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("longest@example.com"))
            .with_run_at(now - Duration::hours(3)),
    )
    .await;
    let urgent = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("urgent@example.com"))
            .with_priority(7)
            .with_run_at(now - Duration::minutes(1)),
    )
    .await;
    let tie_a = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("tie-a@example.com"))
            .with_run_at(now - Duration::hours(2)),
    )
    .await;
    let tie_b = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("tie-b@example.com"))
            .with_run_at(now - Duration::hours(2)),
    )
    .await;

    let claimed = claim(&backend, &worker, 10).await;
    assert_eq!(
        claimed.len(),
        4,
        "four jobs are due and the claim asked for ten, so all four come back: a worker left idle \
         next to a full queue is a queue that has stopped"
    );
    assert_eq!(
        claimed[0].id(),
        urgent.id(),
        "priority comes first: the port's `claim` fixes the order so that a caller can have a \
         policy at all, and this job was the only one written with a priority above zero"
    );
    assert_eq!(
        claimed[1].id(),
        longest.id(),
        "then the job that has been due the longest: among jobs of one priority the queue is \
         first-in-first-out, so the one written with the oldest `run_at` is the one that has been \
         waiting"
    );
    let mut tied = [claimed[2].id(), claimed[3].id()];
    let mut expected = [tie_a.id(), tie_b.id()];
    tied.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        tied, expected,
        "the last two places belong to the two jobs that tied on priority and instant"
    );
    assert_eq!(
        claimed[2].id(),
        tied[0],
        "two jobs that agree on priority and on when they became due come back in identity order, \
         because the alternative is an order that changes between two runs of the same queue"
    );

    // The claim is an attempt: it stopped being queued and it is being worked on.
    for job in &claimed {
        assert_eq!(
            job.state(),
            JobState::Running,
            "a claimed job is a running job on the {} backend",
            backend.name()
        );
        assert_eq!(
            job.attempts(),
            1,
            "an attempt starts when a worker takes the job, not when it finishes: a worker that \
             dies holding one has still spent it, and a count that stayed at zero would let one \
             broken payload occupy every worker forever"
        );
    }

    // The stamp is on the row, not only in the answer the claim returned.
    let read = get(&backend, &urgent.id())
        .await
        .expect("the claimed job is still there");
    assert_eq!(read.state(), JobState::Running);
    assert_eq!(
        read.attempts(),
        1,
        "a store that marked the job running only in what it answered with would hand the same \
         job out again on the next claim"
    );

    assert!(
        claim(&backend, &worker, 10).await.is_empty(),
        "a claim marks what it hands out, so the next claim finds nothing: the queue the four jobs \
         came from is empty"
    );
}

/// A job whose instant has not arrived is not handed to anybody.
#[tokio::test]
async fn a_job_scheduled_in_the_future_is_not_claimed() {
    let backend = backend().await;
    let queue = queue();
    let worker = WorkerId::new("suite-worker");
    let now = Utc::now();

    let due = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("due@example.com"))
            .with_run_at(now - Duration::seconds(1)),
    )
    .await;
    let later = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("later@example.com"))
            .with_run_at(now + Duration::hours(1)),
    )
    .await;

    let claimed = claim(&backend, &worker, 10).await;
    assert_eq!(
        claimed.len(),
        1,
        "exactly one of the two jobs is due: a claim that took the other one has ignored the \
         instant the caller scheduled it for, which is how a reminder goes out an hour early"
    );
    assert_eq!(claimed[0].id(), due.id());

    let waiting = get(&backend, &later.id())
        .await
        .expect("the scheduled job is still there");
    assert_eq!(
        waiting.state(),
        JobState::Queued,
        "a job that is not due yet stays exactly where it is"
    );
    assert_eq!(
        waiting.attempts(),
        0,
        "and nothing has tried to run it: a claim that took it would also have spent an attempt"
    );
}

/// Only the work somebody is running can be settled, and settling it twice is refused.
#[tokio::test]
async fn a_completion_settles_only_a_running_job() {
    let backend = backend().await;
    let queue = queue();
    let worker = WorkerId::new("suite-worker");

    let id = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("alice@example.com")),
    )
    .await
    .id();

    let refused = backend
        .store()
        .complete(&worker, &id)
        .await
        .expect_err("a job nobody claimed is not a job anybody can complete");
    assert!(
        matches!(refused, Error::Invalid { .. }),
        "a job that is not running is refused as input the caller got wrong, not as a broken store: \
         expected `Error::Invalid`, got {refused:?}"
    );
    assert_eq!(
        get(&backend, &id)
            .await
            .expect("the job is still there")
            .state(),
        JobState::Queued,
        "and the refused write changed nothing"
    );

    let missing = backend
        .store()
        .complete(&worker, &JobId::new())
        .await
        .expect_err("an identity that was never written is not a job to complete");
    assert!(
        matches!(missing, Error::NotFound { .. }),
        "a job that does not exist is a different answer from one that is not running — the caller \
         acts on the difference — so expected `Error::NotFound`, got {missing:?}"
    );

    assert_eq!(
        claim(&backend, &worker, 1).await.len(),
        1,
        "the job is due, so a claim takes it"
    );
    backend
        .store()
        .complete(&worker, &id)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "the {} backend refused to complete the job its own claim started: {error}",
                backend.name()
            )
        });
    assert_eq!(
        get(&backend, &id)
            .await
            .expect("the job is still there")
            .state(),
        JobState::Succeeded,
        "a completed job is done, and a store that answers anything else here cannot tell a retry \
         from a duplicate"
    );

    let twice = backend
        .store()
        .complete(&worker, &id)
        .await
        .expect_err("a job is completed once");
    assert!(
        matches!(twice, Error::Invalid { .. }),
        "completing a job that is already done is refused rather than accepted twice: expected \
         `Error::Invalid`, got {twice:?}"
    );
}

/// A failure spends an attempt and lands where the budget leaves it.
#[tokio::test]
async fn a_failure_counts_the_attempt_and_settles_only_a_running_job() {
    let backend = backend().await;
    let queue = queue();
    let worker = WorkerId::new("suite-worker");
    let reason = "the smtp server said 421";

    let id = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("bob@example.com")).with_max_attempts(2),
    )
    .await
    .id();

    let refused = backend
        .store()
        .fail(&worker, &id, reason)
        .await
        .expect_err("a failure is recorded against an attempt somebody started");
    assert!(
        matches!(refused, Error::Invalid { .. }),
        "a job nobody claimed cannot fail: expected `Error::Invalid`, got {refused:?}"
    );

    let missing = backend
        .store()
        .fail(&worker, &JobId::new(), reason)
        .await
        .expect_err("there is no attempt to record against an identity that does not exist");
    assert!(
        matches!(missing, Error::NotFound { .. }),
        "expected `Error::NotFound`, got {missing:?}"
    );

    assert_eq!(claim(&backend, &worker, 1).await.len(), 1, "the job is due");
    let state = backend
        .store()
        .fail(&worker, &id, reason)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "the {} backend refused to record a failure against its own claim: {error}",
                backend.name()
            )
        });
    assert_eq!(
        state,
        JobState::Queued,
        "one attempt of a budget of two is spent, so the job goes back to the queue: the answer \
         says where it went, and a store that answered `Failed` here would drop work that has an \
         attempt left"
    );
    let row = get(&backend, &id).await.expect("the job is still there");
    assert_eq!(row.attempts(), 1, "the attempt was counted");
    assert_eq!(
        row.last_error(),
        Some(reason),
        "the operator needs the reason on the row, not only in a log line that has rotated away"
    );

    assert_eq!(
        claim(&backend, &worker, 1).await.len(),
        1,
        "the job went back to the queue, so it is handed out again"
    );
    let state = backend
        .store()
        .fail(&worker, &id, reason)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "the {} backend refused to record the second failure: {error}",
                backend.name()
            )
        });
    assert_eq!(
        state,
        JobState::Failed,
        "the second attempt spends the budget, so the job is terminal"
    );
    assert!(
        claim(&backend, &worker, 10).await.is_empty(),
        "a job whose budget is spent is never handed out again"
    );
}

/// An identity nobody has written is not an error, it is an empty answer.
#[tokio::test]
async fn reading_an_unknown_job_answers_nothing() {
    let backend = backend().await;

    let read = get(&backend, &JobId::new()).await;
    assert!(
        read.is_none(),
        "`get` answers `Ok(None)` for a row that is not there — looking is allowed to come up \
         empty, and the engine is the layer that turns that into `Error::NotFound` for the calls \
         that owe the caller an answer about one job (jobq/3.1-engine-boundary). The {} backend \
         answered {read:?}, and a store that refused instead would make `inspect` unable to tell \
         a missing job from a broken backend",
        backend.name()
    );
}

/// Claims that run at the same instant come back disjoint, and between them hand out everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claims_running_at_the_same_time_never_overlap() {
    let backend = Arc::new(backend().await);
    let queue = queue();
    let count = 12;

    let mut queued = Vec::new();
    for index in 0..count {
        queued.push(
            enqueue(
                &backend,
                SubmitRequest::new(queue.clone(), payload(&format!("user{index}@example.com"))),
            )
            .await
            .id(),
        );
    }
    queued.sort_unstable();

    // Eight workers, two jobs apiece, all asking at once: more claim slots than jobs, so every due
    // job has a claim that can take it and no worker has to wait for another one to finish.
    let mut workers = Vec::new();
    for index in 0..8 {
        let backend = Arc::clone(&backend);
        let queue = queue.clone();
        workers.push(tokio::spawn(async move {
            let store = backend.store();
            let worker = WorkerId::new(format!("worker-{index}"));
            let mut mine = Vec::new();
            loop {
                let batch = store
                    .claim(&worker, &queue, 2)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("the {} backend refused a claim: {error}", backend.name())
                    });
                if batch.is_empty() {
                    break;
                }
                mine.extend(batch.into_iter().map(|job| job.id()));
            }
            mine
        }));
    }

    let mut handed_out = Vec::new();
    for worker in workers {
        handed_out.extend(worker.await.expect("every worker finishes its claims"));
    }

    let total = handed_out.len();
    let mut distinct = handed_out.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        total,
        "two workers came back with the same job ({} of {} claims were repeats). A claim has to \
         take the rows it is about to stamp in the same step: selecting which rows are free and \
         writing the decision down afterwards lets every worker in that window pick the same ones, \
         and the engine then runs one job twice with no way to notice",
        total - distinct.len(),
        total
    );
    assert_eq!(
        distinct,
        queued,
        "between them the workers have to come back with every due job exactly once: {} jobs were \
         stored and {} came back, so a claim either lost work or invented it",
        queued.len(),
        total
    );
}

/// The step that prepares the schema is safe to run against a schema that is already current.
#[tokio::test]
async fn the_schema_step_can_be_run_again() {
    let backend = backend().await;
    let queue = queue();
    let worker = WorkerId::new("suite-worker");

    // The fixture already applied the schema once. Running the step again is what a daemon does on
    // every start, against a database that is usually already current.
    for attempt in 1..=2 {
        backend.migrate().await.unwrap_or_else(|error| {
            panic!(
                "the {} backend refused run {attempt} of its schema step over a database it had \
                 already prepared: {error}",
                backend.name()
            )
        });
    }

    // And the store still is a store afterwards: nothing the second run did took the rows' place.
    let id = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("alice@example.com")),
    )
    .await
    .id();
    let claimed = claim(&backend, &worker, 1).await;
    assert_eq!(
        claimed.len(),
        1,
        "a job written after the schema step ran twice is claimed like any other"
    );
    assert_eq!(claimed[0].id(), id);
}
