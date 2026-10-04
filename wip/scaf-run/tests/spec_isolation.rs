//! Spec for `jobq/3.6-transaction-isolation`: the trail a settle leaves, the constraint that makes
//! appending to it idempotent, and what a caller does when the database refuses a transaction.
//!
//! `kiln run` copies this file into the project as `tests/spec_isolation.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The fixture is the same one `jobq/3.5-store-port-conformance`
//! built: `support::store::backend` builds the store `KILN_BACKEND` names, so the fast run judges
//! the in-memory backend and the `conformance@2` gate re-runs this file with
//! `KILN_BACKEND=postgres`.
//!
//! Five decisions about what this file is allowed to assert:
//!
//! * **a settle is two writes with one commit, and this file can see the commit.** Every case below
//!   checks the *pair*: the row moved and the line exists, or neither did. The one that matters most
//!   is the refusal — a settle the port refused leaves no line behind, because a trail that records
//!   something that did not happen is worse than no trail at all.
//! * **the trail is about settles, and a claim is not one.** The case that says so is deliberately
//!   boring: write a job, claim it, look at the trail, and find it empty. An attempt that has been
//!   *started* is on the row (`attempts`); only the ways an attempt *ends* are lines.
//! * **a line's identity is `(job_id, kind, attempt)`, and appending is idempotent.** The assertion
//!   is the port's `record` answering `true` and then `false` — not a `select` followed by an
//!   `insert`, which is the shape the constraint exists to replace. What the durable backend uses
//!   for it (a unique index the statement defers to) and what the in-memory one uses (the key of a
//!   map) are its own business: this file only knows that the *second* append was a no-op.
//! * **the conflict class is the point, not the retry.** The last two cases inject the failure the
//!   step is about and then look at both halves of the answer: the raw path reports
//!   [`Error::Conflict`], retryable, with the driver's own `40001` in its source chain, and the
//!   wrapper converges where the raw path was refused. A wrapper that retried *everything* would
//!   pass the second half and fail the first.
//! * **the conflict cases need a database, and say so.** `40001` is a database refusing a
//!   transaction that lost a row to a concurrent commit; a store that serialises every caller behind
//!   one mutex has no concurrent commit to lose to, so those two cases stop on the in-memory backend
//!   with the reason written at [`container_url`]. Everything the in-memory backend *can* answer —
//!   the trail, its idempotence, the retry loop's own mechanics — runs on both backends, which is
//!   what the `conformance@2` gate re-runs.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use jobq::engine::{
    DEFAULT_ATTEMPTS, Error, EventKind, Job, JobEvent, JobId, JobState, PostgresStore, Queue,
    Retried, Store, SubmitRequest, WorkerId, retry,
};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use support::store::{Backend, backend};

/// The queue every case in this file uses.
const QUEUE: &str = "emails";
/// How often a case looks again while it waits for a condition.
const RETRY: Duration = Duration::from_millis(20);
/// How long a case waits for the writer it staged to be refused.
const RACE_DEADLINE: Duration = Duration::from_secs(20);
/// The reason the staged holder writes onto the row, so a case can tell its commit from the loser's.
const HELD_BY: &str = "held by the other writer";

/// That queue, parsed, with the failure a learner can act on.
fn queue() -> Queue {
    Queue::parse(QUEUE).expect("`emails` is the name this file uses everywhere")
}

/// A payload of the shape a producer would send.
fn payload(body: &str) -> Value {
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

/// One append, with a failure that names the backend that refused it.
async fn record(backend: &Backend, event: &JobEvent) -> bool {
    backend.store().record(event).await.unwrap_or_else(|error| {
        panic!(
            "the {} backend refused a line of the trail: {error}",
            backend.name()
        )
    })
}

/// One job's trail, with a failure that names the backend that refused it.
async fn trail(backend: &Backend, id: &JobId) -> Vec<JobEvent> {
    backend
        .store()
        .events(id)
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a read: {error}", backend.name()))
}

/// The connection string of the container this run is against, or `None` on the backend that has no
/// second connection to race with.
///
/// The two cases at the bottom of this file are the only ones that cannot run against the in-memory
/// store, and the reason is structural rather than an omission: `40001` is a database refusing a
/// transaction that lost a row to a concurrent commit, and a store that puts every caller behind one
/// mutex never has one. Everything the in-memory backend can answer runs on both.
fn container_url(backend: &Backend) -> Option<String> {
    match backend {
        Backend::Postgres { container, .. } => Some(container.url()),
        Backend::Memory(_) => None,
    }
}

/// Hold one job's row in an uncommitted write of its own, and answer the transaction doing it.
///
/// The holder is a plain second connection at the default isolation level: what makes the *store's*
/// settle lose is that it runs at `repeatable read`, so its snapshot is taken before this commit and
/// its write then meets a row version it never saw. Keeping the holder at the default level is what
/// says the level is the store's decision rather than a property of the race.
async fn hold_the_row(url: &str, id: &JobId) -> Transaction<'static, Postgres> {
    let pool: PgPool = PgPoolOptions::new()
        .connect(url)
        .await
        .expect("the container the fixture started is reachable");
    let mut held = pool
        .begin()
        .await
        .expect("a second connection can open a transaction");
    sqlx::query("update jobs set last_error = $2 where id = $1")
        .bind(id.as_uuid())
        .bind(HELD_BY)
        .execute(&mut *held)
        .await
        .expect("the holder can write the row it is holding");
    held
}

/// Wait until somebody is waiting on a lock, which is what a store transaction that is about to be
/// refused looks like from outside.
///
/// A condition with a deadline rather than a `sleep` (authoring.md §6.4): the case needs the loser
/// to be *in flight* before the holder commits — commit earlier and the loser's snapshot would
/// include it, commit later and the case races the loser — and waiting for the database to say so is
/// the only way to be right about that without a clock.
async fn wait_until_a_writer_is_waiting(url: &str) {
    let probe: PgPool = PgPoolOptions::new()
        .connect(url)
        .await
        .expect("the container the fixture started is reachable");
    let deadline = Instant::now() + RACE_DEADLINE;
    loop {
        let waiting: (i64,) = sqlx::query_as("select count(*) from pg_locks where not granted")
            .fetch_one(&probe)
            .await
            .expect("the container answers a question about its own locks");
        if waiting.0 > 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "nothing ever waited for the held row within {RACE_DEADLINE:?}: the settle this case set up \
             to lose the race did not even reach the row. Check that a settle opens its transaction at \
             `repeatable read` — at the default level the same write waits, re-reads, and succeeds"
        );
        tokio::time::sleep(RETRY).await;
    }
}

/// One settle of `id`, in a task of its own, on a store a caller would have built for itself.
///
/// The task rather than a call in the case body, because the case needs the settle to be in flight
/// while the staged holder commits: a call that is blocked is only observable from outside it.
fn settle_in_the_background(
    url: String,
    id: JobId,
    reason: &'static str,
) -> tokio::task::JoinHandle<Result<JobState, Error>> {
    tokio::spawn(async move {
        let store = PostgresStore::connect(&url)
            .await
            .expect("the container the fixture started is reachable");
        let worker = WorkerId::new("suite-worker");
        store.fail(&worker, &id, reason).await
    })
}

/// The same settle, wrapped in the engine's own retry loop — the caller's half of the step.
fn wrapped_settle_in_the_background(
    url: String,
    id: JobId,
    reason: &'static str,
) -> tokio::task::JoinHandle<Result<Retried<JobState>, Error>> {
    tokio::spawn(async move {
        let store = PostgresStore::connect(&url)
            .await
            .expect("the container the fixture started is reachable");
        let worker = WorkerId::new("suite-worker");
        retry(DEFAULT_ATTEMPTS, || store.fail(&worker, &id, reason)).await
    })
}

/// A settle writes one line, and it names the attempt it settled.
#[tokio::test]
async fn a_settle_leaves_one_line_and_nothing_else_does() {
    let backend = backend().await;
    let queue = queue();
    let worker = WorkerId::new("suite-worker");

    let id = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("alice@example.com")),
    )
    .await
    .id();
    assert!(
        trail(&backend, &id).await.is_empty(),
        "a job that was written and never settled has an empty trail: the trail is about settles, \
         and writing a job down is not one — it has not been attempted, so there is nothing to say \
         about how an attempt ended"
    );

    assert_eq!(claim(&backend, &worker, 1).await.len(), 1, "the job is due");
    assert!(
        trail(&backend, &id).await.is_empty(),
        "taking a job is not settling it either: an attempt that has been *started* is on the row \
         (`attempts`), and a trail that recorded starts would double every claim's writes to say \
         something an operator reads off the row anyway ({} backend)",
        backend.name()
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

    let lines = trail(&backend, &id).await;
    assert_eq!(
        lines.len(),
        1,
        "one settle, one line: the completion is the only thing that happened to this job, and the \
         {} backend answered {} line(s)",
        backend.name(),
        lines.len()
    );
    assert_eq!(
        lines[0].kind(),
        EventKind::Completed,
        "the line says what happened"
    );
    assert_eq!(
        lines[0].attempt(),
        1,
        "and which attempt it happened to: the claim started the first one, so the completion that \
         ended it is the first one. A line whose attempt comes from a second read is a line that can \
         name an attempt the row had already left behind"
    );
    assert_eq!(lines[0].job_id(), id, "the line names the job it is about");
    assert_eq!(
        lines[0].detail(),
        None,
        "a success has no reason to record: `detail` is why something went wrong, and nothing did"
    );
}

/// Every failed attempt gets its own line, and the trail reads oldest first.
#[tokio::test]
async fn each_failed_attempt_gets_its_own_line() {
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

    for attempt in 1..=2 {
        assert_eq!(
            claim(&backend, &worker, 1).await.len(),
            1,
            "attempt {attempt} of a budget of two is due"
        );
        let state = backend
            .store()
            .fail(&worker, &id, reason)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "the {} backend refused to record attempt {attempt}: {error}",
                    backend.name()
                )
            });
        let expected = if attempt == 1 {
            JobState::Queued
        } else {
            JobState::Failed
        };
        assert_eq!(
            state, expected,
            "the budget decides where attempt {attempt} leaves the job"
        );

        let lines = trail(&backend, &id).await;
        assert_eq!(
            lines.len(),
            attempt as usize,
            "attempt {attempt} failed, so the trail holds {attempt} line(s): every attempt that \
             ended is a line nobody may lose, because the count of lines is what an operator reads \
             when they ask how often this job has gone wrong"
        );
        let last = &lines[attempt as usize - 1];
        assert_eq!(
            last.kind(),
            EventKind::AttemptFailed,
            "attempt {attempt} failed"
        );
        assert_eq!(
            last.attempt(),
            attempt,
            "and its line names it: two failures with different attempts are two lines, which is \
             why the identity of a line is `(job, kind, attempt)` and not the job"
        );
        assert_eq!(
            last.detail(),
            Some(reason),
            "the reason travels with the line: the row keeps the last one, and this is the one that \
             belonged to this attempt"
        );
        assert!(
            lines[0].at() <= last.at(),
            "oldest first: the trail is a story, and a story told out of order is a report an operator \
             has to sort before reading ({:?} came back before {:?} on the {} backend)",
            lines[0].at(),
            last.at(),
            backend.name()
        );
    }
}

/// A settle the port refused writes nothing at all.
#[tokio::test]
async fn a_refused_settle_leaves_nothing_behind() {
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
        "expected `Error::Invalid` for a settle of a job that is not running, got {refused:?}"
    );
    assert_eq!(
        get(&backend, &id)
            .await
            .expect("the job is still there")
            .state(),
        JobState::Queued,
        "the refused write changed nothing on the row"
    );
    assert!(
        trail(&backend, &id).await.is_empty(),
        "and nothing in the trail: a settle that was refused is not something that happened to the \
         job, and a line for it would be the trail disagreeing with the row on the very first case \
         anybody would check ({} backend)",
        backend.name()
    );

    // The same promise one step further in: a settle that *did* happen is refused a second time, and
    // the refusal does not append a second line.
    assert_eq!(claim(&backend, &worker, 1).await.len(), 1, "the job is due");
    backend
        .store()
        .complete(&worker, &id)
        .await
        .expect("the first completion is the one that settles it");
    let twice = backend
        .store()
        .complete(&worker, &id)
        .await
        .expect_err("a job is completed once");
    assert!(
        matches!(twice, Error::Invalid { .. }),
        "expected `Error::Invalid` for the second completion, got {twice:?}"
    );
    assert_eq!(
        trail(&backend, &id).await.len(),
        1,
        "the trail still holds exactly the one line the successful completion wrote: an operator \
         counting these lines is counting settles, not attempts to settle"
    );
}

/// The same line appended twice is one line, whatever the backend has to do to keep that promise.
#[tokio::test]
async fn the_same_line_appended_twice_is_one_line() {
    let backend = backend().await;
    let queue = queue();

    let id = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("carol@example.com")),
    )
    .await
    .id();

    let line = JobEvent::new(
        id,
        EventKind::AttemptFailed,
        1,
        Some("the smtp server said 421".to_owned()),
        Utc::now(),
    );
    assert!(
        record(&backend, &line).await,
        "the first append of a line is new — a caller that gets `false` here is looking at a line \
         somebody already wrote"
    );
    assert!(
        !record(&backend, &line).await,
        "the second append of the *same* line is a no-op rather than a second row ({} backend). This \
         is the whole argument for putting the rule in the database instead of in the caller: a \
         caller that reads first and writes second has a window two writers both pass through, and a \
         retried transaction is exactly a caller arriving twice",
        backend.name()
    );

    let lines = trail(&backend, &id).await;
    assert_eq!(
        lines.len(),
        1,
        "two appends of `(job, kind, attempt)`, one line: the {} backend answered {} line(s). Two \
         lines here would make the trail double-count every retry it was supposed to absorb",
        backend.name(),
        lines.len()
    );
    assert_eq!(
        lines[0].id(),
        line.id(),
        "and it is the line that was appended, identity and all: a store that minted a fresh identity \
         while reading would answer a different line every time it was asked"
    );

    // A *different* attempt is a different line, because the identity of a line is the triple and not
    // the job: appending is idempotent, not lossy.
    let next = JobEvent::new(
        id,
        EventKind::AttemptFailed,
        2,
        Some("the smtp server said 421".to_owned()),
        Utc::now(),
    );
    assert!(
        record(&backend, &next).await,
        "the second attempt's failure is not a duplicate of the first one's"
    );
    assert_eq!(
        trail(&backend, &id).await.len(),
        2,
        "one line per attempt, and the second one is new"
    );
}

/// The retry loop repeats what another attempt can change, and stops at its bound.
#[tokio::test]
async fn the_wrapper_retries_what_is_worth_retrying_and_stops_at_its_bound() {
    // A failure the *caller* can fix is not retried, however many attempts it was given: this is the
    // half of the policy that keeps a rejected payload from becoming a loop.
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let error = retry(DEFAULT_ATTEMPTS, move || {
        let counted = Arc::clone(&counted);
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            Err::<u8, Error>(Error::Invalid {
                detail: "the payload is not an object".to_owned(),
            })
        }
    })
    .await
    .expect_err("a rejected payload gets the same answer the second time");
    assert!(
        matches!(error, Error::Invalid { .. }),
        "the wrapper hands the caller the failure it was given, unchanged: expected \
         `Error::Invalid`, got {error:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "and it ran the operation once: `Error::is_retryable` is the engine's answer to whether \
         another attempt is worth anything, and the wrapper is not allowed to have a second opinion"
    );

    // A retryable failure once, and then the answer: the loop converges and says how long it took.
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let outcome = retry(DEFAULT_ATTEMPTS, move || {
        let counted = Arc::clone(&counted);
        async move {
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(Error::Conflict {
                    source: Box::new(std::io::Error::other(
                        "could not serialize access due to concurrent update",
                    )),
                })
            } else {
                Ok("second time lucky")
            }
        }
    })
    .await
    .expect("a conflict is exactly the failure another attempt can change");
    assert_eq!(
        outcome.attempts(),
        2,
        "and the count is how many attempts it took, not how many were allowed: a settle that \
         converged on the second try is a different story from one that never lost the race, and this \
         is the number a log line or a counter needs"
    );
    assert_eq!(
        outcome.value(),
        "second time lucky",
        "the answer is the one that worked"
    );

    // A failure that keeps coming back stops at the bound, and the caller gets the failure itself.
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let error = retry(2, move || {
        let counted = Arc::clone(&counted);
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            Err::<u8, Error>(Error::Store {
                source: Box::new(std::io::Error::other(
                    "the connection went away mid-statement",
                )),
            })
        }
    })
    .await
    .expect_err("a store that never answers is not fixed by trying again");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the bound is the whole of the policy: two attempts were asked for and two were made. \
         Retrying without a limit turns one contended row into a process that never finishes, and \
         that is the failure mode this number exists to prevent"
    );
    assert!(
        matches!(error, Error::Store { .. }),
        "and the caller is handed the *last* failure rather than a sentence about having tried: what \
         went wrong is the only part of this a caller can act on. Expected `Error::Store`, got \
         {error:?}"
    );

    // `attempts` is read as "at least one": an operation nobody runs has no answer to give.
    let outcome = retry(0, || async { Ok(7u8) })
        .await
        .expect("zero attempts means one, not none");
    assert_eq!(
        outcome.attempts(),
        1,
        "an operation that was never run cannot have succeeded, so `0` is read as one attempt"
    );
    assert_eq!(outcome.value(), 7);
}

/// A transaction the database refused for concurrency is its own class, with the database's own
/// reason inside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transaction_the_database_refused_is_a_conflict_with_a_cause() {
    let backend = backend().await;
    let Some(url) = container_url(&backend) else {
        return;
    };
    let queue = queue();
    let worker = WorkerId::new("suite-worker");
    let reason = "the smtp server said 421";

    let id = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("dave@example.com")),
    )
    .await
    .id();
    assert_eq!(
        claim(&backend, &worker, 1).await.len(),
        1,
        "the job has to be running: a settle of a job nobody holds is refused by the port before it \
         ever reaches the database's isolation, which is a different case (`a_refused_settle_leaves_\
         nothing_behind`)"
    );

    let held = hold_the_row(&url, &id).await;
    let loser = settle_in_the_background(url.clone(), id, reason);
    wait_until_a_writer_is_waiting(&url).await;
    held.commit()
        .await
        .expect("the holder commits the row it was holding");

    let error = loser
        .await
        .expect("the settle finishes")
        .expect_err("the loser of the race is refused rather than quietly re-read");
    assert!(
        matches!(error, Error::Conflict { .. }),
        "a transaction the database refused because another writer got there first is its own \
         failure class: expected `Error::Conflict`, got {error:?}. Folded into `Error::Store`, a \
         caller could not tell `again` from `the connection is gone`, and the retry that makes this \
         case survivable would be a guess"
    );
    assert!(
        error.is_retryable(),
        "and the class is retryable: this is the failure that made `is_retryable` worth having"
    );
    let cause = std::error::Error::source(&error)
        .expect("the conflict keeps the driver's own failure, so a caller can read the code");
    let code = cause
        .downcast_ref::<sqlx::Error>()
        .and_then(|driver| driver.as_database_error())
        .and_then(|error| error.code().map(|code| code.to_string()));
    assert_eq!(
        code.as_deref(),
        Some("40001"),
        "the race this case injects is PostgreSQL's `40001` — a transaction at `repeatable read` \
         that lost a row to a commit it never saw — and the source chain is where a caller reads \
         that. A `40P01` (a deadlock the server broke) is the same class and takes the same path"
    );

    // And the refused transaction wrote nothing at all, which is the half of the promise a caller
    // cannot see from the error alone.
    let row = get(&backend, &id).await.expect("the job is still there");
    assert_eq!(
        row.state(),
        JobState::Running,
        "the loser's rollback left the row exactly where the holder's commit left it: still running, \
         still held by the worker that claimed it"
    );
    assert_eq!(
        row.last_error(),
        Some(HELD_BY),
        "the holder's own write is on the row, which is what proves the race happened: the settle was \
         refused *because* that commit landed after its snapshot. If this reads back as the settle's \
         own reason, the settle was not refused at all and this case proved nothing"
    );
    assert!(
        trail(&backend, &id).await.is_empty(),
        "and no line: the row change and the line are one transaction, so a refusal leaves neither. A \
         trail that recorded the attempt anyway would be the one thing this step exists to prevent"
    );
}

/// The wrapped settle converges where the raw one was refused, and the refused attempt leaves no
/// second line behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wrapper_converges_where_the_raw_path_was_refused() {
    let backend = backend().await;
    let Some(url) = container_url(&backend) else {
        return;
    };
    let queue = queue();
    let worker = WorkerId::new("suite-worker");
    let reason = "the smtp server said 421";

    let id = enqueue(
        &backend,
        SubmitRequest::new(queue.clone(), payload("erin@example.com")),
    )
    .await
    .id();
    assert_eq!(
        claim(&backend, &worker, 1).await.len(),
        1,
        "the job is running"
    );

    let held = hold_the_row(&url, &id).await;
    let loser = wrapped_settle_in_the_background(url.clone(), id, reason);
    wait_until_a_writer_is_waiting(&url).await;
    held.commit()
        .await
        .expect("the holder commits the row it was holding");

    let outcome = loser.await.expect("the wrapped settle finishes").expect(
        "a conflict is retryable, so the wrapper's second attempt has to finish the settle",
    );
    assert_eq!(
        outcome.attempts(),
        2,
        "the first attempt lost the race and the second one won it: an attempt count of one would \
         mean the injected conflict did not happen, and an attempt count of three would mean the \
         retry raced something else"
    );
    assert_eq!(
        outcome.value(),
        JobState::Queued,
        "and the settle did what it would have done without the race: attempt one of a budget of \
         three failed, so the job is back in the queue"
    );

    let lines = trail(&backend, &id).await;
    assert_eq!(
        lines.len(),
        1,
        "the refused attempt wrote no line and the retry wrote exactly one: {} line(s). This is the \
         pair the step is about — the transaction that was rolled back left nothing, and the one \
         that committed left both halves",
        lines.len()
    );
    assert_eq!(lines[0].kind(), EventKind::AttemptFailed);
    assert_eq!(
        lines[0].attempt(),
        1,
        "the attempt the loser was refused on"
    );
    assert_eq!(lines[0].detail(), Some(reason), "with the caller's reason");
}
