//! Spec for `jobq/3.16-dlq-tooling`: what is stuck, why, and the one way back.
//!
//! `kiln run` copies this file into the project as `tests/spec_dlq.rs`. Six cases, and every
//! one of them is about the *port* rather than about a backend: a dead-letter tool that read or
//! wrote a backend directly is the thing this step exists to prevent, so nothing below names
//! `MemoryStore` or `PostgresStore` except to build the store `KILN_BACKEND` asked for.
//!
//! * **the view is the rows in `failed`, and reading it is the same store.** A job that is
//!   queued, running or succeeded is not a dead letter, and the one that is keeps its payload,
//!   its queue and the sentence its last attempt died with. The claim is what makes the step a
//!   *tool* rather than a feature: an operator sees what is stuck without a second collection to
//!   keep in sync;
//! * **the decision is a value, and the model does not have it.** `transition` refuses every
//!   event on a finished job — that is what `jobq/3.14-job-state-machine` *means* — so a replay
//!   is a second life rather than a move, and `revived(job, now)` is the whole of what the second
//!   life changes. It is judged here pair by pair, the way the state machine's table is: the count
//!   is kept, the ceiling moves, the reason the last life died stays, and anything that is not a
//!   dead letter is refused by the name of the state it is in;
//! * **the port writes what the decision says.** The row a replay answers with is compared field
//!   by field with `revived`'s answer for the row as it was, which is what makes "one arithmetic,
//!   two backends" a fact this file can hold: a backend that decided by itself would have to
//!   agree with the engine by coincidence. Then the job is claimed, which is where "one more go"
//!   has to be visible — a replay that left the ceiling alone hands out nothing at all;
//! * **a second replay changes nothing, and leaves no line.** Re-issuing a command that timed out
//!   must not start a second life: the job is `queued` now, so the refusal names that state, and
//!   the row, the trail and the queue's numbers are exactly where the first replay left them;
//! * **every replay leaves one line, and two replays leave two.** The line names the attempt the
//!   job had spent, so a job that died twice and was replayed twice has two of them — which is
//!   the fact the count-keeping decision rests on: resetting `attempts` would make the second
//!   line identical to the first and the database would drop it. The stored spelling is asserted
//!   too, because it is what a migration, a query and a person's `grep` agree on;
//! * **the handle is the identity.** A replay goes through the same door as every other call:
//!   the queue's owner may put its own dead letters back, a tenant that may submit but not claim
//!   is refused with the action's name, and a row the handle cannot see is answered like a row
//!   nobody wrote.
//!
//! `support::store::backend` builds the store `KILN_BACKEND` names (`memory` when nothing says
//! otherwise), so the fast run judges the in-memory backend and the `conformance@2` gate re-runs
//! this file with `KILN_BACKEND=postgres`: the arithmetic is the engine's, the two promises that
//! make the method a port method — the row read under a lock, the row and its line in one
//! transaction — are the backends'. The clock is `jobq/3.8-clock-injection`'s, stopped at an
//! instant no wall clock will reach during the run, so "it is due now" is a statement this file
//! makes rather than one it waits for.

mod support;

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, TimeDelta, Utc};
use jobq::engine::{
    Clock, DEFAULT_LIST_LIMIT, Error, Event, EventKind, Job, JobId, JobState, MemoryStore,
    PostgresStore, Queue, QueueAction, QueueDeclaration, Reason, Store, SubmitRequest, Tenant,
    WorkerId, dead_letters, revived, transition,
};
use support::store::{Backend, backend};

/// The queue every dead letter in this file dies in.
const QUEUE: &str = "signup";

/// The instant the fake clocks start at: far enough away that a backend reading a database clock
/// instead of the injected one cannot be mistaken for one that does not.
const START: &str = "2030-01-01T00:00:00Z";

/// The sentence a dead letter's last attempt died with.
const REASON: &str = "the smtp server said 550 and meant it";

/// A clock the case moves by hand, so that "the instant it became due" is a value.
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

/// An instant written the way this file reads them.
fn at(text: &str) -> DateTime<Utc> {
    text.parse()
        .unwrap_or_else(|error| panic!("`{text}` is an instant: {error}"))
}

/// A queue, by name.
fn queue(name: &str) -> Queue {
    Queue::parse(name).unwrap_or_else(|error| panic!("`{name}` is a queue name: {error}"))
}

/// A tenant, by name.
fn tenant(name: &str) -> Tenant {
    Tenant::parse(name).unwrap_or_else(|error| panic!("`{name}` is a tenant name: {error}"))
}

/// The worker the cases claim with.
fn worker() -> WorkerId {
    WorkerId::new("dlq-suite")
}

/// A store of this run's backend, reading every instant from `clock`.
///
/// The fixture hands out a store by reference and this is a second one over the same backend, the
/// shape `jobq/3.12-dedup-window` established: the cases want `&dyn Store`, the durable backend's
/// container is the same database either way, and the clock is the case's so that the instant a
/// replay is due at is something it can name.
async fn store_for(backend: &Backend, clock: &Arc<FakeClock>) -> Arc<dyn Store> {
    let clock = Arc::clone(clock) as Arc<dyn Clock>;
    match backend {
        Backend::Memory(_) => Arc::new(MemoryStore::new().with_clock(clock)),
        Backend::Postgres { container, .. } => Arc::new(
            PostgresStore::connect(&container.url())
                .await
                .expect("the container the fixture started is reachable")
                .with_clock(clock),
        ),
    }
}

/// One job queued in `queue`, with `attempts` allowed in total.
async fn submit(store: &dyn Store, queue: &Queue, attempts: u32) -> JobId {
    store
        .enqueue(
            SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "to": "ada@example.com" }),
            )
            .with_max_attempts(attempts),
        )
        .await
        .unwrap_or_else(|error| panic!("the port accepts this request: {error}"))
        .id()
}

/// Take the one job `queue` owes, asserting it is the one the case submitted.
async fn claim_one(store: &dyn Store, queue: &Queue, id: JobId) -> Job {
    let mut claimed = store
        .claim(&worker(), queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming from a due queue cannot fail here: {error}"));
    let job = claimed
        .pop()
        .unwrap_or_else(|| panic!("the queue has one due job, so a claim of one answers one job"));
    assert_eq!(job.id(), id, "and it is the job this case submitted");
    job
}

/// One row, as the port holds it.
async fn row(store: &dyn Store, id: JobId) -> Job {
    store
        .get(&id)
        .await
        .unwrap_or_else(|error| panic!("reading one row cannot fail here: {error}"))
        .unwrap_or_else(|| panic!("the row this case wrote is still there"))
}

/// One job's trail, as the pair an operator reads it as.
async fn trail(store: &dyn Store, id: JobId) -> Vec<(EventKind, u32)> {
    store
        .events(&id)
        .await
        .unwrap_or_else(|error| panic!("reading one job's trail cannot fail here: {error}"))
        .into_iter()
        .map(|line| (line.kind(), line.attempt()))
        .collect()
}

/// A dead letter: one job with an attempt to spend, claimed and failed with `REASON`.
async fn dead_letter(store: &dyn Store, queue: &Queue) -> JobId {
    let id = submit(store, queue, 1).await;
    claim_one(store, queue, id).await;
    store
        .fail(&worker(), &id, REASON)
        .await
        .unwrap_or_else(|error| panic!("recording a failure cannot fail here: {error}"));
    id
}

/// One case of "this job is not a dead letter", built through the port.
async fn not_a_dead_letter(store: &dyn Store, queue: &Queue, state: JobState) -> JobId {
    match state {
        JobState::Queued => submit(store, queue, 3).await,
        JobState::Running => {
            let id = submit(store, queue, 3).await;
            claim_one(store, queue, id).await;
            id
        }
        JobState::Succeeded => {
            let id = submit(store, queue, 3).await;
            claim_one(store, queue, id).await;
            store
                .complete(&worker(), &id)
                .await
                .unwrap_or_else(|error| panic!("completing a claimed job cannot fail: {error}"));
            id
        }
        other => panic!("this case only builds the three states a dead letter is not: `{other}`"),
    }
}

/// The view is the rows in `failed` — and nothing else about a job changed to get there.
#[tokio::test]
async fn the_dead_letter_queue_is_the_rows_in_failed_and_nothing_else() {
    // The value first, because it is the whole of the view: one rule, written once, so that a
    // command, a dashboard and a test ask the same question.
    assert_eq!(
        dead_letters().state(),
        Some(JobState::Failed),
        "a dead letter is a job the engine gave up on, and `failed` is the state that says so"
    );
    assert!(
        dead_letters().queue().is_none(),
        "the view names no queue: whose dead letters these are is the handle's question \
         (`jobq/3.13-queue-authz-attribution`), not the filter's"
    );
    assert_eq!(
        dead_letters().limit(),
        DEFAULT_LIST_LIMIT,
        "a queue has no natural end, so looking at it is a page: the same page a listing answers \
         with, because the view *is* a listing"
    );

    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;

    let dead = dead_letter(store.as_ref(), &queue(QUEUE)).await;
    let waiting = not_a_dead_letter(store.as_ref(), &queue("queued"), JobState::Queued).await;
    let running = not_a_dead_letter(store.as_ref(), &queue("running"), JobState::Running).await;
    let finished =
        not_a_dead_letter(store.as_ref(), &queue("succeeded"), JobState::Succeeded).await;

    let listed = store
        .list(&dead_letters())
        .await
        .unwrap_or_else(|error| panic!("listing the view cannot fail here: {error}"));
    assert_eq!(
        listed.iter().map(Job::id).collect::<Vec<JobId>>(),
        vec![dead],
        "one dead letter and three jobs that are not: a queued job is owed an attempt, a running \
         one is being served, and a succeeded one finished ({} backend)",
        backend.name()
    );
    for (id, what) in [
        (waiting, "queued"),
        (running, "running"),
        (finished, "succeeded"),
    ] {
        assert!(
            !listed.iter().any(|job| job.id() == id),
            "a {what} job is not stuck, so it is not in the dead-letter queue"
        );
    }

    // Why it is there, and what it cost: the two questions an operator asks before deciding.
    let job = row(store.as_ref(), dead).await;
    assert_eq!(job.state(), JobState::Failed, "the row itself says so");
    assert_eq!(
        job.queue(),
        &queue(QUEUE),
        "and it is still in the queue it died in: the view is the same store asked a narrower \
         question, not a copy that has to be kept in step"
    );
    assert!(
        job.payload().get("to").is_some(),
        "the payload is still the payload: {}",
        job.payload()
    );
    assert_eq!(
        job.last_error(),
        Some(REASON),
        "the sentence the last attempt died with is the answer to \"why did this stop\""
    );
    assert_eq!(
        (job.attempts(), job.max_attempts(), job.may_retry()),
        (1, 1, false),
        "and the counters say the budget is spent: that is what put it here"
    );
    assert_eq!(
        trail(store.as_ref(), dead).await,
        vec![(EventKind::AttemptFailed, 1)],
        "the trail is the third answer, and it is the one that says the job did not simply \
         vanish"
    );
}

/// A replay is a second life rather than a move, and only a dead letter has one.
#[tokio::test]
async fn a_replay_is_a_second_life_and_not_a_move_the_model_has() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let dead = dead_letter(store.as_ref(), &queue(QUEUE)).await;
    let before = row(store.as_ref(), dead).await;

    // The model's half: there is no event that does this, and the refusal is what says so. A
    // table with a `replay` row would make "my job is finished" and "somebody gave it another
    // go" the same answer.
    let refused = transition(before.state(), Event::Claim)
        .expect_err("a finished job has no move out, and that is what a second life is for");
    assert_eq!(
        refused.reason(),
        Reason::Terminal,
        "claimed after it died, the model's answer is \"finished\" rather than \"busy\": {refused}"
    );

    clock.advance(TimeDelta::minutes(5));
    let now = clock.now();
    let after = revived(&before, now).unwrap_or_else(|error| {
        panic!(
            "a dead letter has a second life, so this cannot be refused on the {} backend: \
             {error}",
            backend.name()
        )
    });

    assert_eq!(
        after.state(),
        JobState::Queued,
        "a second life starts where the first one did: `queued` is the state `enqueue` writes"
    );
    assert_eq!(
        after.attempts(),
        before.attempts(),
        "the count is the row's history and a replay does not un-spend an attempt that happened"
    );
    assert_eq!(
        after.max_attempts(),
        before.attempts() + 1,
        "the ceiling moves to one past the count: give it one more go, and exactly one — the \
         count is the fact and the ceiling is the policy"
    );
    assert!(
        after.may_retry(),
        "which is what makes the answer claimable again"
    );
    assert_eq!(
        after.run_at(),
        now,
        "it is due at the instant the decision was made at, and that instant is the store's \
         clock (`jobq/3.8-clock-injection`), not the database's"
    );
    assert_eq!(
        after.last_error(),
        before.last_error(),
        "and why the last life died is still there: an operator reading the row after a replay \
         is reading the incident, and the attempt that succeeds is what clears it"
    );
    assert_eq!(
        (after.lease_owner(), after.lease_expires_at()),
        (None, None),
        "no second life inherits a claim: a replayed row that looked running would be a job \
         nobody can recover"
    );
    assert_eq!(
        (after.id(), after.queue(), after.payload(), after.priority()),
        (
            before.id(),
            before.queue(),
            before.payload(),
            before.priority()
        ),
        "identity, queue, payload and priority are what they were: a replay that reset a \
         priority would be a scheduling decision nobody made"
    );
    assert_eq!(
        row(store.as_ref(), dead).await,
        before,
        "and asking the engine what a replay would write writes nothing: the row is untouched \
         until somebody replays it"
    );

    // Everything that is not a dead letter, judged pair by pair the way the step's table is: the
    // refusal has to name the state the job is in, because that is the fact the operator lacks.
    for state in [JobState::Queued, JobState::Running, JobState::Succeeded] {
        // A queue of its own per state, so that a claim below is about the job this case built
        // rather than about whichever row the port happens to hand out first.
        let queue = queue(&format!("not-dead-{}", state.as_str()));
        let id = not_a_dead_letter(store.as_ref(), &queue, state).await;
        let untouched = row(store.as_ref(), id).await;
        let refused = revived(&untouched, now).expect_err(
            "only a dead letter has a second life: a job that is already owed an attempt, or has \
             finished with it, is not one",
        );
        match &refused {
            Error::Invalid { detail } => {
                assert!(
                    detail.contains(state.as_str()),
                    "the refusal names the state the job is in — the operator has the identity already \
                     and needs to know whether it is `{}`: {detail}",
                    state.as_str()
                );
                assert!(
                    detail.contains(&id.to_string()),
                    "and the job, because a tool replays a list of ids and reports per id: {detail}"
                );
            }
            other => panic!(
                "a refused replay is the caller's own mistake, not a store failure: {other:?}"
            ),
        }
        assert_eq!(
            row(store.as_ref(), id).await,
            untouched,
            "and nothing moved: a refused replay is a refusal before it is an action"
        );
    }
}

/// The port writes what the engine says, on both backends, and the job runs again.
#[tokio::test]
async fn the_port_writes_what_the_engine_says_and_one_more_go_is_exactly_one() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let dead = dead_letter(store.as_ref(), &queue(QUEUE)).await;
    let before = row(store.as_ref(), dead).await;

    clock.advance(TimeDelta::hours(2));
    let now = clock.now();
    let answer = store
        .replay(&dead)
        .await
        .unwrap_or_else(|error| panic!("a dead letter can be replayed: {error}"));
    assert_eq!(
        answer,
        revived(&before, now)
            .unwrap_or_else(|error| panic!("the decision accepts this row: {error}")),
        "the row the port writes is the row the engine's decision says it writes ({} backend): \
         a backend that decided by itself would have to agree with the engine by coincidence",
        backend.name()
    );
    assert_eq!(
        row(store.as_ref(), dead).await,
        answer,
        "and the answer is the row: a replay answers with what it wrote, so an operator does not \
         have to read it back"
    );
    assert_eq!(
        answer.run_at(),
        now,
        "due at the instant the store's clock read, which no wall clock will reach during this \
         run: a statement about time rather than a wait for it"
    );

    // One more go means one more attempt, and it is the last of the budget: the claim counts the
    // attempt it starts, and the failure that follows spends the ceiling the replay moved.
    let claimed = claim_one(store.as_ref(), &queue(QUEUE), dead).await;
    assert_eq!(
        claimed.state(),
        JobState::Running,
        "a replayed job is work the queue hands out again"
    );
    assert_eq!(
        claimed.attempts(),
        before.attempts() + 1,
        "and the attempt it is running is one past the count that killed the first life — the \
         count was kept, so the second life's attempt is the next number rather than the first"
    );
    assert_eq!(
        claimed.max_attempts(),
        claimed.attempts(),
        "with nothing left after it: one more go, not a fresh budget"
    );
    let state = store
        .fail(&worker(), &dead, "the smtp server said 550 again")
        .await
        .unwrap_or_else(|error| panic!("recording a failure cannot fail here: {error}"));
    assert_eq!(
        state,
        JobState::Failed,
        "so the second failure buries it again, and that is the budget rather than a bug"
    );

    // One more go is a rule rather than an allowance: a second replay moves the ceiling again, by
    // exactly one, from wherever the count has got to.
    let spent = row(store.as_ref(), dead).await;
    let again = store
        .replay(&dead)
        .await
        .unwrap_or_else(|error| panic!("a dead letter can be replayed again: {error}"));
    assert_eq!(
        (again.attempts(), again.max_attempts()),
        (spent.attempts(), spent.attempts() + 1),
        "a job that died twice is owed one attempt, not the two it just spent: {} vs {}",
        again.attempts(),
        again.max_attempts()
    );
}

/// A second replay of one job changes nothing, and leaves nothing behind.
#[tokio::test]
async fn a_second_replay_of_a_job_that_is_owed_again_changes_nothing() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let dead = dead_letter(store.as_ref(), &queue(QUEUE)).await;

    clock.advance(TimeDelta::minutes(1));
    let first = store
        .replay(&dead)
        .await
        .unwrap_or_else(|error| panic!("a dead letter can be replayed: {error}"));
    let lines = store
        .events(&dead)
        .await
        .unwrap_or_else(|error| panic!("reading one job's trail cannot fail here: {error}"));
    let numbers = store
        .usage()
        .await
        .unwrap_or_else(|error| panic!("reading the report cannot fail here: {error}"));

    // The command again, as a script that timed out and retried it would.
    clock.advance(TimeDelta::minutes(1));
    let refused = store
        .replay(&dead)
        .await
        .expect_err("the job is owed an attempt now, so it is not a dead letter any more");
    match &refused {
        Error::Invalid { detail } => assert!(
            detail.contains(JobState::Queued.as_str()),
            "the refusal says what the job is now, which is what the operator needs to hear: {detail}"
        ),
        other => panic!("replaying what is not a dead letter is the caller's mistake: {other:?}"),
    }

    assert_eq!(
        row(store.as_ref(), dead).await,
        first,
        "so the row is exactly where the first replay put it ({} backend)",
        backend.name()
    );
    assert_eq!(
        trail(store.as_ref(), dead).await,
        vec![(EventKind::AttemptFailed, 1), (EventKind::Replayed, 1)],
        "and so is the trail: a command that changed nothing is not a line — an audit trail is the \
         list of things that happened, not of commands that were run"
    );
    assert_eq!(
        store
            .events(&dead)
            .await
            .unwrap_or_else(|error| panic!("reading one job's trail cannot fail here: {error}")),
        lines,
        "including the instants on it: the refused call stamped nothing"
    );
    assert_eq!(
        store
            .usage()
            .await
            .unwrap_or_else(|error| panic!("reading the report cannot fail here: {error}")),
        numbers,
        "and the queue's numbers are about work: a replay is not a claim, a success or a failure, \
         so the attempt that follows is what they will count"
    );
}

/// Every replay leaves one line, and the line says which attempt came back.
#[tokio::test]
async fn two_replays_at_two_attempts_are_two_lines_in_the_trail() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let queue = queue(QUEUE);
    let dead = dead_letter(store.as_ref(), &queue).await;

    assert_eq!(
        EventKind::Replayed.as_str(),
        "replayed",
        "the stored spelling is an interface — a migration, a query and a person's `grep` all \
         agree on it — so it is written once and asserted here"
    );

    // Two lives, two deaths, two replays. The clock moves between them because the port orders a
    // job's lines by the instant they happened, and a case that stamped them all with one instant
    // would be asserting the tie-breaking rule rather than the order.
    clock.advance(TimeDelta::minutes(1));
    let replayed_at = clock.now();
    let before = store
        .replay(&dead)
        .await
        .unwrap_or_else(|error| panic!("a dead letter can be replayed: {error}"));
    let lines = store
        .events(&dead)
        .await
        .unwrap_or_else(|error| panic!("reading one job's trail cannot fail here: {error}"));
    let last = lines.last().expect("a replay appends a line");
    assert_eq!(
        (last.kind(), last.attempt(), last.at()),
        (EventKind::Replayed, before.attempts(), replayed_at),
        "the line names the attempt the job had spent when it was put back, and the instant the \
         operator did it: {last}"
    );
    assert_eq!(
        last.detail(),
        None,
        "and it carries no reason: the line is the fact, and the reason the attempt died is on \
         the line before it"
    );
    assert_eq!(last.job_id(), dead, "it is this job's line");

    claim_one(store.as_ref(), &queue, dead).await;
    clock.advance(TimeDelta::minutes(1));
    store
        .fail(&worker(), &dead, "the smtp server said 550, still")
        .await
        .unwrap_or_else(|error| panic!("recording a failure cannot fail here: {error}"));
    clock.advance(TimeDelta::minutes(1));
    store
        .replay(&dead)
        .await
        .unwrap_or_else(|error| panic!("a dead letter can be replayed again: {error}"));

    assert_eq!(
        trail(store.as_ref(), dead).await,
        vec![
            (EventKind::AttemptFailed, 1),
            (EventKind::Replayed, 1),
            (EventKind::AttemptFailed, 2),
            (EventKind::Replayed, 2)
        ],
        "two lives, and each replay is its own line: a line's identity is `(job, kind, attempt)`, \
         so this is only true while the count is kept — a replay that reset it to zero would \
         append the line the first replay already wrote, and the history would say it was \
         replayed once ({} backend)",
        backend.name()
    );
}

/// A replay goes through the same door as every other call.
#[tokio::test]
async fn the_handle_decides_whose_dead_letter_it_is() {
    let backend = backend().await;
    let clock = FakeClock::at(START);
    let store = store_for(&backend, &clock).await;
    let owned = queue("signup-owned");
    let shared = queue("signup-shared");
    let owner = tenant("acme");
    let guest = tenant("globex");

    // The owner's queue, and a queue a guest may put work into but not take it out of.
    store
        .declare_queue(&QueueDeclaration::new(owned.clone(), owner.clone()))
        .await
        .unwrap_or_else(|error| panic!("declaring a queue cannot fail here: {error}"));
    store
        .declare_queue(
            &QueueDeclaration::new(shared.clone(), owner.clone())
                .with_grant(guest.clone(), QueueAction::Submit),
        )
        .await
        .unwrap_or_else(|error| panic!("declaring a queue cannot fail here: {error}"));

    let mine = dead_letter(store.as_ref(), &owned).await;
    let theirs = dead_letter(store.as_ref(), &shared).await;

    // Whose handle it is decides what a call is about: a queue a tenant is not in the model for
    // does not exist for it, which is the rule `jobq/3.13-queue-authz-attribution` wrote for every
    // read — a dead letter in somebody else's queue is not a row this handle can even read.
    let as_guest = store.scoped(&guest);
    assert!(
        as_guest
            .get(&mine)
            .await
            .unwrap_or_else(|error| panic!("reading one row cannot fail here: {error}"))
            .is_none(),
        "a row outside this handle's queues is `Ok(None)`, the answer a row nobody wrote gets: the \
         two must not be distinguishable from inside the door"
    );

    // The action is the question a queue's model answers, and putting work back into a queue you
    // may not take work out of is somebody else's decision.
    let refused = as_guest
        .replay(&theirs)
        .await
        .expect_err("a guest may submit to this queue and not put its work back");
    assert!(
        matches!(&refused, Error::Forbidden { action } if action == QueueAction::Claim.as_str()),
        "and the refusal names the action rather than the row, because the queue in a message about \
         somebody else's row would be a queue the caller can already see: {refused:?}"
    );
    assert_eq!(
        row(store.as_ref(), theirs).await.state(),
        JobState::Failed,
        "nothing moved, so the dead letter is still dead"
    );

    // A row the handle cannot see is answered like a row nobody wrote, and one that does not exist
    // is the same answer — the two must not be distinguishable.
    let stranger = store.scoped(&tenant("nobody"));
    for id in [mine, theirs] {
        let refused = stranger
            .replay(&id)
            .await
            .expect_err("a queue this tenant is not in the model for does not exist for it");
        assert!(
            matches!(&refused, Error::NotFound { id: asked } if *asked == id),
            "a queue nobody declared is what the engine says about a queue this handle may not \
             see: {refused:?}"
        );
    }
    let unwritten: JobId = "00000000-0000-4000-8000-000000000000"
        .parse()
        .expect("a uuid this file wrote");
    let refused = store
        .replay(&unwritten)
        .await
        .expect_err("nobody wrote this job");
    assert!(
        matches!(&refused, Error::NotFound { id } if *id == unwritten),
        "and an id that was never written gets the same answer the operator's own door gives it: \
         {refused:?}"
    );

    // The owner's own door: the same call, on a handle that may claim from the queue, happens.
    let as_owner = store.scoped(&owner);
    let moved = as_owner
        .replay(&mine)
        .await
        .unwrap_or_else(|error| panic!("the queue's owner may put its own work back: {error}"));
    assert_eq!(
        moved.state(),
        JobState::Queued,
        "and the row is owed an attempt again: {} backend",
        backend.name()
    );
}
