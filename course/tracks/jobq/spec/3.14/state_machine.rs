//! Spec for `jobq/3.14-job-state-machine`: a job's life as one function, and the schema's half of it.
//!
//! `kiln run` copies this file into the project as `tests/spec_state_machine.rs`. Four claims, and
//! the reason each of them is written the way it is:
//!
//! * **the table is total, and it is judged pair by pair.** [`expected`] below is the table the
//!   step's body writes down, and the case walks **every** state against **every** event: a legal
//!   move answers the state the table names, and a move that is not allowed comes back as an
//!   `Illegal` whose reason is the right one of the three. Totality is the assertion itself — a
//!   `todo!()`, a `panic!()` or an `unreachable!()` on a pair nobody thought about fails here, and
//!   that is exactly what a total function is for;
//! * **a refusal names both sides.** Every refused move asserts the message against the *strings an
//!   operator has*: the state the job was in and the event that arrived. A refusal that said only
//!   "not allowed" would pass every assertion about the class and fail the one that matters at
//!   03:00;
//! * **the terminal states absorb.** `succeeded` and `failed` have no move out, so "this job is
//!   finished" and "somebody else is running it" stay two different answers to the same call — and
//!   `JobState::is_terminal` is checked against the model rather than against itself, because two
//!   statements of one fact are two things that can disagree;
//! * **the row is an encoding, and the schema closes it.** The round trip (`as_str` then `parse`) is
//!   asserted over `STATES`, the four names are pinned as an interface, an unknown name is refused
//!   *by the text that was stored* — and then the same four names are pushed through a real database
//!   by hand: each one is accepted and reads back as the state it spells, and a name the model does
//!   not have is refused by the constraint the step's migration adds. That last case starts a
//!   container of its own — which is also why this step declares an image — because "an `insert`
//!   somebody types into `psql` is refused" is not a claim the in-memory backend can be asked.
//!
//! The stores are judged too, and that is not an afterthought: **the model is load-bearing, not
//! decorative.** Every walk through the port — a claim, a completion, a failure with and without
//! budget, a claim that lapses — compares the state the *row* ended in with `transition`'s answer,
//! so a store that kept the rule to itself (the `attempts < max_attempts` of
//! `jobq/3.7-leases-and-recovery`) fails here even though every earlier suite still passes.
//! `support::store::backend` builds the store `KILN_BACKEND` names (`memory` when nothing says
//! otherwise), so the same walk runs against both backends.

mod support;

use jobq::engine::{
    EVENTS, Error, Event, JobId, JobState, PostgresStore, Queue, Reason, STATES, Store,
    SubmitRequest, UnknownState, WorkerId, transition,
};
use support::postgres::Postgres;
use support::store::backend;

/// The queue the schema case writes its own row into.
const QUEUE: &str = "state-machine";

/// The four names the column, the CLI and a dashboard group by.
const NAMES: [&str; 4] = ["queued", "running", "succeeded", "failed"];

/// The table this step is judged against, one answer per pair.
///
/// Written here rather than derived from anything in the project, because this *is* the contract:
/// the body prints the same table, so a machine that disagrees with it is told which pair and which
/// answer. The match has a catch-all arm because [`Event`] is `#[non_exhaustive]` — an event a
/// later step adds is that step's row, and this file is where the news lands first.
fn expected(state: JobState, event: Event) -> Result<JobState, Reason> {
    match (state, event) {
        (JobState::Queued, Event::Claim) => Ok(JobState::Running),
        (JobState::Running, Event::Complete) => Ok(JobState::Succeeded),
        (
            JobState::Running,
            Event::Fail { retryable: true } | Event::ClaimLapsed { retryable: true },
        ) => Ok(JobState::Queued),
        (
            JobState::Running,
            Event::Fail { retryable: false } | Event::ClaimLapsed { retryable: false },
        ) => Ok(JobState::Failed),
        (JobState::Succeeded | JobState::Failed, _) => Err(Reason::Terminal),
        (JobState::Queued, _) => Err(Reason::NotRunning),
        (JobState::Running, Event::Claim) => Err(Reason::NotQueued),
        (_, _) => panic!(
            "the step's table has no row for a job that is `{state}` and an event this suite does \
             not know (`{event}`): a new event arrives with its row, and this file is where the row \
             is written"
        ),
    }
}

/// A queue, by name.
fn queue(name: &str) -> Queue {
    Queue::parse(name).unwrap_or_else(|error| panic!("`{name}` is a queue name: {error}"))
}

/// The worker the store cases claim with.
fn worker() -> WorkerId {
    WorkerId::new("state-machine-suite")
}

/// One pair, phrased for a failure message: the two sides of a move are what a reader needs.
fn pair(state: JobState, event: Event) -> String {
    format!("`{state}` + `{event}`")
}

/// The state the model says a move the *store* cases take leaves a job in.
///
/// The walk below only takes moves the model allows, so a refusal here is the model disagreeing
/// with the case rather than with the store — which is why it panics instead of asserting.
fn model(from: JobState, event: Event) -> JobState {
    transition(from, event)
        .unwrap_or_else(|refused| panic!("the walk only takes moves the model allows: {refused}"))
}

/// One job with `attempts` in its budget, submitted to `queue` through the port.
async fn enqueue(store: &dyn Store, queue: &Queue, attempts: u32) -> JobId {
    store
        .enqueue(
            SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "to": "someone@example.com" }),
            )
            .with_max_attempts(attempts),
        )
        .await
        .unwrap_or_else(|error| panic!("the port accepts this request: {error}"))
        .id()
}

/// Claim one job from `queue`, asserting it is the one the case submitted.
async fn claim(store: &dyn Store, queue: &Queue, worker: &WorkerId, id: JobId) {
    let mut claimed = store
        .claim(worker, queue, 1)
        .await
        .unwrap_or_else(|error| panic!("claiming from a due queue cannot fail here: {error}"));
    let job = claimed
        .pop()
        .unwrap_or_else(|| panic!("the queue has one due job, so a claim of one answers one job"));
    assert_eq!(job.id(), id, "and it is the job this case submitted");
}

/// The state of one row, as the port holds it.
async fn state_of(store: &dyn Store, id: JobId) -> JobState {
    store
        .get(&id)
        .await
        .unwrap_or_else(|error| panic!("reading one row cannot fail here: {error}"))
        .unwrap_or_else(|| panic!("the row this case wrote is still there"))
        .state()
}

/// Every pair, and the one answer the table gives it.
#[test]
fn the_machine_is_total_and_every_pair_answers_the_table() {
    assert_eq!(
        EVENTS.len(),
        6,
        "both spellings of the two events that carry a flag belong in the list, or the walk below \
         never asks about the budget"
    );

    for state in STATES {
        for event in EVENTS {
            let answer = transition(state, event);
            let where_it_went = match answer {
                Ok(next) => format!("must move the job to `{next}`"),
                Err(refused) => format!("must be refused, and it answered: {refused}"),
            };
            match expected(state, event) {
                Ok(next) => assert_eq!(answer, Ok(next), "{} {where_it_went}", pair(state, event)),
                Err(reason) => {
                    let refused = answer.expect_err(
                        "the table below says this move is refused, and the machine moved the \
                         job instead",
                    );
                    assert_eq!(
                        refused.reason(),
                        reason,
                        "{} was refused for the wrong reason: a refusal is a class rather than a \
                         sentence, and \"already finished\" is not \"somebody else has it\"",
                        pair(state, event)
                    );
                    assert_eq!(
                        refused.from(),
                        state,
                        "the refusal names the state the job was in"
                    );
                    assert_eq!(
                        refused.event(),
                        event,
                        "and the event that arrived: two facts, because a refusal that carries only \
                         one of them cannot be acted on"
                    );
                    let message = refused.to_string();
                    assert!(
                        message.contains(state.as_str()) && message.contains(event.as_str()),
                        "{} is refused with a message that must name both sides — it is what an \
                         operator greps for — and this one says `{message}`",
                        pair(state, event)
                    );
                }
            }
        }
    }
}

/// The end of a job is the end: nothing moves it again, and the model and `is_terminal` agree.
#[test]
fn a_terminal_state_absorbs_every_event() {
    assert_eq!(
        STATES
            .iter()
            .filter(|state| state.is_terminal())
            .map(|state| state.as_str())
            .collect::<Vec<&str>>(),
        ["succeeded", "failed"],
        "the two terminal states are the two a queue never hands out again, and that list is \
         `JobState::is_terminal` answering rather than this file's opinion"
    );

    for state in STATES {
        for event in EVENTS {
            match transition(state, event) {
                Ok(next) => assert!(
                    !state.is_terminal(),
                    "`{state}` is terminal and {} moved it to `{next}`: a terminal state has no \
                     move out, which is the promise every settle path in both backends rests on",
                    pair(state, event)
                ),
                Err(refused) => assert_eq!(
                    refused.reason() == Reason::Terminal,
                    state.is_terminal(),
                    "{} was refused as `{:?}`, and `is_terminal` says `{state}` is {}. The two \
                     statements of one fact have to agree, or the engine has two answers to \"may \
                     I run this again\"",
                    pair(state, event),
                    refused.reason(),
                    if state.is_terminal() {
                        "finished"
                    } else {
                        "not finished"
                    }
                ),
            }
        }
    }

    // The life of a job in the model's own words: claimed, finished, and the *finished* refusal
    // rather than the *busy* one.
    let running = transition(JobState::Queued, Event::Claim).expect("a queued job can be claimed");
    assert_eq!(running, JobState::Running);
    let done = transition(running, Event::Complete).expect("a running job can be completed");
    assert_eq!(done, JobState::Succeeded);
    let again = transition(done, Event::Complete).expect_err("completing it twice is not a move");
    assert_eq!(
        again.reason(),
        Reason::Terminal,
        "completing a completed job is not idempotence, it is a caller that did not look — and the \
         class is what tells it apart from a job somebody else is running"
    );
}

/// The row is an encoding of the model: the column holds a name, and a name nobody knows is refused
/// by its own text rather than rounded down.
#[test]
fn the_encoding_round_trips_and_an_unknown_name_is_refused_by_its_text() {
    assert_eq!(
        STATES
            .iter()
            .map(|state| state.as_str())
            .collect::<Vec<&str>>(),
        NAMES,
        "the names are an interface — the row stores them, the CLI prints them and a dashboard \
         groups by them — so a rename is a migration rather than a refactor, and this list is where \
         that is written down"
    );

    for state in STATES {
        assert_eq!(
            JobState::parse(state.as_str()),
            Ok(state),
            "every state the model has, the column can hold: `{}` is written as itself and read \
             back as `{state}`",
            state.as_str()
        );
    }

    for wrong in ["dead", "Queued", "running ", "", "SUCCEEDED"] {
        let refused = JobState::parse(wrong).expect_err("a name the model does not have");
        assert_eq!(
            refused,
            UnknownState::new(wrong),
            "`{wrong}` is not a state this build knows, and the refusal carries the text that was \
             stored: rounding it down to `queued` is how a job that finished is run a second time"
        );
        assert_eq!(
            refused.text(),
            wrong,
            "the text travels whole, because it is what an operator has in their hand"
        );
        assert!(
            refused.to_string().contains(wrong),
            "and the message names it: `{refused}`"
        );
    }
}

/// The stores move jobs the way the model says, which is what makes the model load-bearing rather
/// than a second description of the same thing.
///
/// Three walks, each in a queue of its own, so that "the job this case submitted" is a fact about
/// the claim rather than about which of a queue's other jobs a store happened to hand out: the
/// port's order is priority and age, and the walks would otherwise compete for one queue.
#[tokio::test]
async fn the_store_moves_jobs_the_way_the_model_says() {
    let backend = backend().await;
    let store = backend.store();
    let worker = worker();

    // queued --claim--> running, and a second claim takes nothing: the model refuses `claim` from
    // `running`, and the store's own selection predicate is the same fact written as SQL.
    let claiming = queue("state-machine-claim");
    let first = enqueue(store, &claiming, 3).await;
    claim(store, &claiming, &worker, first).await;
    assert_eq!(
        state_of(store, first).await,
        model(JobState::Queued, Event::Claim),
        "a claim leaves the job where the model says it does ({} backend)",
        backend.name()
    );
    let nobody = WorkerId::new("nobody");
    assert!(
        store
            .claim(&nobody, &claiming, 10)
            .await
            .unwrap_or_else(|error| panic!("claiming cannot fail here: {error}"))
            .is_empty(),
        "a running job is not handed out again: the model refuses a second claim, and the store's \
         selection is that same rule"
    );

    // running --complete--> succeeded, and then the terminal state absorbs: the second settle is
    // refused, nothing moves, and no claim finds it again.
    store
        .complete(&worker, &first)
        .await
        .unwrap_or_else(|error| panic!("completing a claimed job cannot fail: {error}"));
    assert_eq!(
        state_of(store, first).await,
        model(JobState::Running, Event::Complete)
    );
    let refused = store
        .complete(&worker, &first)
        .await
        .expect_err("completing a job that is already finished is not a move");
    assert!(
        matches!(refused, Error::Invalid { .. }),
        "a settle the model refuses is the class `jobq/3.7-leases-and-recovery` pinned for it: got \
         {refused:?}"
    );
    let mut claimed = store
        .claim(&worker, &claiming, 10)
        .await
        .unwrap_or_else(|error| panic!("claiming cannot fail here: {error}"));
    claimed.retain(|job| job.id() == first);
    assert!(
        claimed.is_empty(),
        "and nothing hands a finished job out again: the terminal state is the reason a queue does \
         not run work twice"
    );

    // running --fail--> queued while the budget lasts, failed when it does not: the destination is
    // the model's answer and the budget is the store's own question.
    let failing = queue("state-machine-fail");
    let retried = enqueue(store, &failing, 2).await;
    claim(store, &failing, &worker, retried).await;
    store
        .fail(&worker, &retried, "the smtp server said 421")
        .await
        .unwrap_or_else(|error| panic!("recording a failure cannot fail here: {error}"));
    assert_eq!(
        state_of(store, retried).await,
        model(JobState::Running, Event::Fail { retryable: true }),
        "one attempt of two is spent, so the job is owed another: it waits to be claimed again"
    );
    claim(store, &failing, &worker, retried).await;
    store
        .fail(&worker, &retried, "the smtp server said 421")
        .await
        .unwrap_or_else(|error| panic!("recording a failure cannot fail here: {error}"));
    assert_eq!(
        state_of(store, retried).await,
        model(JobState::Running, Event::Fail { retryable: false }),
        "the second attempt spends the budget, so the job is finished rather than queued: that is \
         how one payload that fails every time stops occupying a worker"
    );

    // running --claim lapse--> the same two destinations, reached by a worker that hands its claim
    // back instead of settling it. The model's event is one; the story on the row — a `reclaimed`
    // line, the reason in `last_error`, the counter that moves — is the store's, which is why it is
    // a second event at all.
    let lapsing = queue("state-machine-lapse");
    let handed_back = enqueue(store, &lapsing, 2).await;
    claim(store, &lapsing, &worker, handed_back).await;
    store
        .release(&worker)
        .await
        .unwrap_or_else(|error| panic!("handing back cannot fail here: {error}"));
    assert_eq!(
        state_of(store, handed_back).await,
        model(JobState::Running, Event::ClaimLapsed { retryable: true }),
        "a handed-back job goes back to the queue while its budget has an attempt left"
    );

    let spent = enqueue(store, &queue("state-machine-spent"), 1).await;
    claim(store, &queue("state-machine-spent"), &worker, spent).await;
    store
        .release(&worker)
        .await
        .unwrap_or_else(|error| panic!("handing back cannot fail here: {error}"));
    assert_eq!(
        state_of(store, spent).await,
        model(JobState::Running, Event::ClaimLapsed { retryable: false }),
        "and out of the queue when it does not: a job nobody may run again is terminal rather than \
         waiting for ever ({} backend)",
        backend.name()
    );
}

/// A state the model does not have cannot be written by anybody, including somebody with `psql`.
#[tokio::test]
async fn the_schema_refuses_a_state_the_model_does_not_have() {
    // A container of its own, because this case is about the *schema*: the in-memory backend has no
    // column to write by hand, and what is being judged is a constraint in
    // `migrations/0006_job_states.sql`. The table's shape is the project's own
    // (`migrations/0001_jobs.sql` wrote it), which is why the statements below may name its
    // columns.
    let container = Postgres::start();
    let store = PostgresStore::connect(&container.url())
        .await
        .expect("the container the fixture started is reachable");
    store
        .migrate()
        .await
        .expect("this project's migration applies to the schema it describes");

    let id = "67e55044-10b1-426f-9247-bb680e5fe0c8";
    let job: JobId = id.parse().expect("a uuid this file wrote");
    let written = format!(
        "insert into jobs (id, queue, payload, state, priority, run_at, attempts, max_attempts) \
         values ('{id}', '{QUEUE}', '{{}}'::jsonb, 'queued', 0, now(), 0, 3)"
    );
    container.try_execute(&written).expect(
        "a row in a state the model has is a row the schema takes, and this case needs one to \
         update",
    );

    // The four names the enum spells, pushed through the database by hand and read back through the
    // port: the encoding round-trips *through the column*, which is what makes the constraint safe
    // to have. A migration that misspelled one of the four fails here rather than in production.
    for state in STATES {
        let name = state.as_str();
        container
            .try_execute(&format!(
                "update jobs set state = '{name}' where id = '{id}'"
            ))
            .unwrap_or_else(|refused| {
                panic!("`{name}` is a state the model has, so the column must take it: {refused}")
            });
        let row = store
            .get(&job)
            .await
            .expect("the container is reachable")
            .expect("the row this case wrote is still there");
        assert_eq!(
            row.state(),
            state,
            "a row holding `{name}` reads back as `{state}`: the column is the encoding of the \
             model, so the two directions of it have to meet"
        );
    }

    // And a name the model does not have is refused — by the constraint, whose name is the thing an
    // operator looks up in `migrations/0006_job_states.sql`.
    let refused = container
        .try_execute(&format!("update jobs set state = 'dead' where id = '{id}'"))
        .expect_err(
            "a state the model does not have must be refused by the database: without the \
             constraint, a hand-written row is a job that reads back as nothing at all — or, in a \
             query that compares the column, a job that stops matching and stops moving",
        );
    assert!(
        refused.contains("jobs_state_known"),
        "the constraint is named, so the refusal hands the operator the name to look up: {refused}"
    );

    let inserted = format!(
        "insert into jobs (id, queue, payload, state, priority, run_at, attempts, max_attempts) \
         values (gen_random_uuid(), '{QUEUE}', '{{}}'::jsonb, 'dead', 0, now(), 0, 3)"
    );
    let refused = container.try_execute(&inserted).expect_err(
        "the insert path is closed too: an update is not the only way a state reaches the table, \
         and this is the path a migration or a tool would take",
    );
    assert!(
        refused.contains("jobs_state_known"),
        "one constraint answers both writes: {refused}"
    );

    assert_eq!(
        store
            .get(&job)
            .await
            .expect("the container is reachable")
            .expect("the row is still there")
            .state(),
        JobState::Failed,
        "the refusals moved nothing: the row is where the last accepted update left it, which is \
         also the answer to \"did a refused write half-happen\""
    );

    // The same text the constraint refuses is what the model refuses, and both refusals are written
    // for the same reader — the one holding the value that was stored.
    let unknown = JobState::parse("dead").expect_err("`dead` is not a state this build knows");
    assert!(
        unknown.to_string().contains("dead"),
        "a refusal that dropped the text would leave the operator with nothing to grep: {unknown}"
    );
}
