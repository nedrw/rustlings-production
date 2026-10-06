//! Spec for `jobq/3.23-online-schema`: a schema change in three phases, and the middle one runs
//! in the queue it is migrating.
//!
//! `kiln run` copies this file into the project as `tests/spec_online_schema.rs`; it is the
//! course's file, not the learner's (plan §3 D2). Eight cases, and the decisions they are about
//! are the ones a review of a migration argues over:
//!
//! * **the expand rewrites nothing.** The column arrives nullable and with no default and the
//!   constraint arrives `not valid`, which is what makes the release that is already running able
//!   to keep writing rows that do not name the column. There is no `update` in the phase at all —
//!   `relay/2.23-online-schema` argued it for a service and it is the same argument here.
//! * **one expression in two places.** The read path's fallback is `computed()` inside a
//!   `coalesce`, and the batch statement writes `computed()`, so the stored number and the served
//!   one cannot disagree; a fallback written out a second time is how they start to.
//! * **the middle phase is a plan of jobs, and the plan is the rate.** `burst()` is the most
//!   batches one interval holds, so a plan that submits every batch due now is a *number* the
//!   suite reads, and the plan is a function of what the catalog says is left rather than of the
//!   size of the table.
//! * **every phase is safe to run twice, and says so before it runs.** The second expand, the
//!   second backfill of a filled table and the second contract answer `Action::Nothing`; the
//!   contract answers `Action::Blocked` while rows are still empty, which is the database's own
//!   `set not null` refusal moved to where it costs a deployment nothing.
//!
//! One case runs against the port and one against the database. `KILN_BACKEND=postgres` is what
//! runs the first on the durable backend, and the second is the database's alone: the in-memory
//! store has no schema to change, and `migrate()` answering `Ok(())` is the honest answer for a
//! store that keeps its rows in a map rather than a hole in this suite.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use jobq::engine::{
    Action, Applied, BURST_LIMIT, Change, DEFAULT_BATCH, DEFAULT_INTERVAL_SECONDS, JobFilter,
    PLAN_METRICS, Plan, Queue, RESUME_LIMIT, SubmitRequest, WorkerId, cost,
};

mod support;

use support::store::{Backend, backend};

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/migration.md";
/// The table the queue's own rows live in.
const TABLE: &str = "jobs";
/// The column this step's change adds.
const COLUMN: &str = "payload_bytes";
/// The expression that produces the column's value: the database's, in bytes.
const COMPUTED: &str = "octet_length(payload::text)";
/// The queue the backfill's jobs run on.
const QUEUE: &str = "migrations";
/// Rows one batch of the backfill fills.
const BATCH: u32 = 256;
/// The rate: one batch per this many seconds.
const INTERVAL_SECONDS: i64 = 1;
/// The size of the table the budget's counts are taken on.
const ROWS: u32 = 4_096;
/// The rows that are still empty when the run is resumed: half the table.
const DONE: u32 = 2_048;
/// The instant every case asks about, and a fixed one: a plan measured against the wall clock is
/// a plan a machine with a different clock answers differently.
const NOW_SECONDS: i64 = 1_700_000_000;

/// The instant every case is asked about.
fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(NOW_SECONDS, 0).expect("a fixed instant these cases state themselves")
}

/// The queue the backfill's jobs run on.
fn queue() -> Queue {
    Queue::parse(QUEUE).expect("`migrations` is a queue name")
}

/// This step's change, at the rate the course's baseline is written against.
fn change() -> Change {
    Change::new(TABLE, COLUMN, COMPUTED, queue())
        .with_batch(BATCH)
        .with_interval(Duration::seconds(INTERVAL_SECONDS))
}

/// The jobs a phase answered with, or a failure that names what it answered instead.
fn jobs(action: Action) -> Plan {
    match action {
        Action::Jobs(plan) => plan,
        other => panic!("this phase is the one that runs as jobs, and it answered {other:?}"),
    }
}

/// The statements a phase answered with, or a failure that names what it answered instead.
fn statements(action: Action) -> Vec<String> {
    match action {
        Action::Statements(lines) => lines,
        other => panic!("this phase has statements to run, and it answered {other:?}"),
    }
}

/// The reason a phase answered with, or a failure that names what it answered instead.
fn reason(action: Action) -> &'static str {
    match action {
        Action::Nothing(reason) | Action::Blocked(reason) => reason,
        other => panic!("this phase had nothing to do, and it answered {other:?}"),
    }
}

/// The plan for a table with `remaining` rows left to fill.
fn plan_for(remaining: u32) -> Plan {
    jobs(change().backfill(&Applied::expanded(remaining), now()))
}

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{PAGE} is the page this step delivers: {error}"))
}

#[test]
fn the_expand_phase_adds_a_column_and_rewrites_nothing() {
    let change = change();
    let expand = statements(change.expand(&Applied::before()));
    assert_eq!(
        expand.len(),
        2,
        "the expand is two statements and nothing else: a column that may not be empty, or a \
         constraint that says so, would refuse the inserts the release that is already running is \
         making"
    );
    let whole = expand.join(";\n").to_ascii_lowercase();
    assert!(
        !whole.contains("update"),
        "the expand rewrites no row: one `update` over the table the release that is already \
         running is writing to holds a write lock for as long as it takes, which is the outage \
         this phase exists to avoid — {whole}"
    );

    let added = expand
        .iter()
        .find(|statement| statement.contains("add column"))
        .expect("the expand adds the column")
        .to_ascii_lowercase();
    assert!(
        added.contains(&format!("add column {COLUMN} integer")),
        "the column arrives as the type the expression answers: {added}"
    );
    for forbidden in ["not null", "default"] {
        assert!(
            !added.contains(forbidden),
            "the column arrives nullable and with no default, so an insert naming only the columns \
             the previous release knows keeps landing (`{forbidden}` is the sentence that refuses \
             it): {added}"
        );
    }

    let index = expand
        .iter()
        .find(|statement| statement.contains("create index"))
        .expect("the expand creates the index")
        .to_ascii_lowercase();
    assert!(
        index.contains(&format!("where {COLUMN} is null")),
        "the index is partial on the predicate a batch reads, so finding the rows that are left \
         is a lookup rather than a scan of the table it is slowly converting: {index}"
    );
    assert!(
        index.contains("jobs_unfilled_payload_bytes"),
        "and it is named after the column it is about, because it is dropped in the contract: \
         {index}"
    );

    // What the phase does *not* contain, and the reason is the whole of the window: a check
    // constraint holds for every new row the moment it exists, and the old release's inserts are
    // new rows that do not name the column. `not valid` skips the rows that are already there and
    // nothing else, so the constraint belongs to the phase in which the old release is gone.
    let names = format!("{}|{}", change.constraint(), change.index());
    assert!(
        !statements(change.expand(&Applied::before()))
            .join(";\n")
            .contains(&change.constraint()),
        "the expand adds no constraint that the column may not be empty: it would be refused for \
         the old release's inserts while the new one is being deployed ({names})"
    );
    assert_eq!(
        change.constraint(),
        "jobs_payload_bytes_known",
        "the names an operator finds in the catalog are derived from the column they are about"
    );
    assert_eq!(change.index(), "jobs_unfilled_payload_bytes");
}

#[test]
fn the_read_path_answers_from_the_expression_the_backfill_writes() {
    let change = change();
    assert_eq!(
        change.computed(),
        COMPUTED,
        "the value is computed where the value is stored: in the database, from the payload the \
         row already carries"
    );
    assert_eq!(
        change.fallback(),
        format!("coalesce({COLUMN}, {COMPUTED})"),
        "the read path answers the column, and the expression the backfill writes when the column \
         is empty"
    );
    assert!(
        change.fallback().contains(change.computed()),
        "the fallback is built from `computed()` rather than written out: the stored number and \
         the computed one are one expression in two places, which is the whole of what makes the \
         expand phase invisible"
    );

    let statement = plan_for(ROWS)
        .statement(0)
        .expect("a plan of sixteen batches has a first one");
    assert!(
        statement.contains(COMPUTED),
        "the batch writes the same expression the read path falls back to: {statement}"
    );
    assert!(
        statement.contains(&format!("{COLUMN} is null")),
        "the batch selects the rows with no value rather than a bookmark, which is what makes a \
         second run fill nothing and an interrupted run finishable by anybody: {statement}"
    );
    assert!(
        statement.contains(&format!("limit {BATCH}")),
        "one statement, limited to the batch: that is what makes a batch atomic and small: \
         {statement}"
    );
    assert!(
        statement.starts_with("update "),
        "one statement per batch, and it is the one that writes: {statement}"
    );
}

#[test]
fn the_backfill_is_batched_paced_and_adds_up() {
    let plan = plan_for(ROWS);
    assert_eq!(
        plan.len(),
        16,
        "four thousand and ninety-six rows at two hundred and fifty-six a batch is sixteen jobs"
    );
    assert_eq!(plan.rows(), ROWS, "and every row is in exactly one batch");
    assert!(!plan.is_empty());
    assert_eq!(
        plan.change().rows_per_batch(),
        BATCH,
        "the plan carries the change it runs"
    );
    assert_eq!(plan.change().queue(), &queue());

    let batches = plan.batches();
    for (index, batch) in batches.iter().enumerate() {
        assert!(
            batch.rows() <= BATCH,
            "batch {index} fills {} rows and a batch may not be larger than the batch size: the \
             size is the whole of what keeps a batch's lock short",
            batch.rows()
        );
        let expected = now() + Duration::seconds(INTERVAL_SECONDS * index as i64);
        assert_eq!(
            batch.due_at(),
            expected,
            "batch {index} is due one interval after the one before it: the pace is the rate"
        );
    }
    assert_eq!(batches.iter().map(|batch| batch.rows()).sum::<u32>(), ROWS);

    // A table that is not a multiple of the batch size: the last batch is the remainder, and one
    // row still gets a batch of its own.
    let ragged = plan_for(ROWS + 1);
    assert_eq!(ragged.len(), 17);
    assert_eq!(
        ragged.batches().last().map(|batch| batch.rows()),
        Some(1),
        "a remainder is a batch like any other: `remaining / batch` would leave that row behind"
    );
    assert_eq!(plan_for(1).len(), 1, "one row left is one job, not zero");

    assert!(
        plan_for(ROWS).statement(15).is_some(),
        "the last batch of a plan has a statement"
    );
    assert!(
        plan_for(ROWS).statement(16).is_none(),
        "and there is no batch past the end: a plan is not a generator that keeps going"
    );
}

#[test]
fn the_guard_is_the_rate_and_the_plan_without_it() {
    let paced = plan_for(ROWS);
    assert_eq!(
        paced.burst(),
        1,
        "one batch per interval: the most batches due at once is the rate the plan keeps"
    );
    assert!(
        paced.burst() <= BURST_LIMIT,
        "and it stays inside the bound the course's baseline is written against ({BURST_LIMIT})"
    );

    // And the plan this guard exists for: the same batches, all due the instant the plan was made.
    // The counts are identical and the shape is an outage — the migration's jobs are the queue's
    // own capacity, so a table handed over at once is a queue that stops.
    let unpaced = jobs(
        change()
            .with_interval(Duration::zero())
            .backfill(&Applied::expanded(ROWS), now()),
    );
    assert_eq!(
        unpaced.len(),
        paced.len(),
        "the same work, split the same way"
    );
    assert_eq!(
        unpaced.rows(),
        paced.rows(),
        "and the same rows: what the guard is over is the shape, not the amount"
    );
    assert_eq!(
        unpaced.burst(),
        16,
        "with no interval every batch is due in the same one, which is the number `burst_max` \
         reports against a limit of {BURST_LIMIT}"
    );

    assert_eq!(
        Change::new(TABLE, COLUMN, COMPUTED, queue()).rows_per_batch(),
        DEFAULT_BATCH,
        "a change built without deciding the rate uses the project's default batch size"
    );
    assert_eq!(
        Change::new(TABLE, COLUMN, COMPUTED, queue()).interval(),
        Duration::seconds(DEFAULT_INTERVAL_SECONDS),
        "and the default pace: one batch a second, which is a rate rather than a burst"
    );
}

#[test]
fn the_plan_is_a_function_of_what_is_left() {
    let whole = plan_for(ROWS);
    let resume = plan_for(ROWS - DONE);
    assert_eq!(
        resume.len(),
        whole.len() / 2,
        "half the rows left is half the jobs: resuming a backfill is planning what the catalog \
         says is missing rather than planning the table again"
    );
    assert_eq!(
        resume.rows(),
        DONE,
        "and the rows add up to what is left, not to the size of the table"
    );
    assert_eq!(
        resume.batches().first().map(|batch| batch.due_at()),
        Some(now()),
        "the first batch of a resumed plan is due now: the work that is left starts when the plan \
         is made"
    );

    assert_eq!(
        cost(&change(), ROWS, ROWS - DONE, now()),
        [
            "jobq-budget backfill_jobs=16".to_owned(),
            "jobq-budget burst_max=1".to_owned(),
            "jobq-budget resume_ratio_max=0.500".to_owned(),
        ],
        "the three lines a harness prints, in the order a report reads them, and the ratio against \
         the bound the course ships"
    );
    assert_eq!(
        PLAN_METRICS,
        ["backfill_jobs", "burst_max", "resume_ratio_max"],
        "the names a page, a harness and the course's baseline are written in, in one list"
    );
    let ratio = resume.len() as f64 / whole.len() as f64;
    assert!(
        RESUME_LIMIT > ratio,
        "the reference plan passes the bound the course ships: {ratio} against {RESUME_LIMIT}"
    );

    // A plan with no denominator prints no guard line at all: a `0` would be a *pass* for a plan
    // that measured nothing, and the gate that reads the lines treats a missing metric as
    // `blocked` instead.
    let nothing = change().backfill(&Applied::expanded(0), now());
    assert!(
        matches!(&nothing, Action::Nothing(_)),
        "a table whose rows all have a value has nothing to backfill"
    );
    let text = reason(nothing);
    assert!(
        text.contains("fills nothing"),
        "and the reason it has nothing to do is a sentence a deployment can log: {text}"
    );

    assert_eq!(
        cost(&change(), 0, 0, now()),
        [
            "jobq-budget backfill_jobs=0".to_owned(),
            "jobq-budget burst_max=0".to_owned(),
        ],
        "a change of no rows prints two lines and no ratio: `0.0` would compare with the bound like \
         any other ratio and report a pass for a plan that measured nothing, and the gate that \
         reads these lines treats a missing metric as `blocked`"
    );
    assert_eq!(
        change().plan(0, now()).burst(),
        0,
        "a plan with no batches has no rate to report, and a burst of `1` would be a number about \
         a job that does not exist"
    );
}

#[test]
fn running_a_phase_twice_changes_nothing() {
    let change = change();
    assert!(
        matches!(change.expand(&Applied::before()), Action::Statements(_)),
        "a database that has never seen the change gets the expand's statements"
    );
    assert!(
        matches!(change.expand(&Applied::expanded(ROWS)), Action::Nothing(_)),
        "and a database that has the column gets nothing to run: `add column` is not idempotent, \
         so the phase asks the catalog first"
    );
    assert!(matches!(
        change.backfill(&Applied::expanded(0), now()),
        Action::Nothing(_)
    ));
    assert!(matches!(
        change.contract(&Applied::contracted()),
        Action::Nothing(_)
    ));
    assert!(matches!(
        change.contract(&Applied::expanded(0)),
        Action::Statements(_)
    ));

    // The half-applied contract: rows still have no value, and `set not null` would be refused by
    // the database with `column "payload_bytes" of relation "jobs" contains null values`. The
    // phase says `Blocked` rather than letting a deployment find that out halfway through.
    let early = reason(change.contract(&Applied::expanded(ROWS)));
    assert!(
        early.contains("set not null"),
        "the contract that runs while rows are empty is refused before it reaches a database, and \
         the reason names what would have been refused: {early}"
    );

    // And the phases before the expand: the middle and the last one have nothing to fill and
    // nothing to check until the column is there — a different sentence from `Nothing`, because a
    // caller can act on one of them.
    assert!(matches!(
        change.backfill(&Applied::before(), now()),
        Action::Blocked(_)
    ));
    assert!(matches!(
        change.contract(&Applied::before()),
        Action::Blocked(_)
    ));
    let too_early = reason(change.contract(&Applied::before()));
    assert!(
        too_early.contains("expand"),
        "the reason names the phase that has to run first: {too_early}"
    );

    // The contract's statements, in order, and what each of them is for: the constraint arrives
    // `not valid` (a catalog change, not a scan under a deploy's lock), the validation is the
    // separate statement that can be interrupted and run again, `set not null` is the last of the
    // fallback's reasons to exist, and the index a batch read goes last — with `if exists`, because
    // a contract whose first run got that far and no further has to be finishable by the next one.
    let contract = statements(change.contract(&Applied::expanded(0)));
    assert_eq!(contract.len(), 4);
    let joined = contract.join(";\n").to_ascii_lowercase();
    assert!(
        joined.contains(&format!(
            "add constraint {} check ({COLUMN} is not null) not valid",
            change.constraint()
        )),
        "the constraint arrives in the contract rather than the expand, and unvalidated: {joined}"
    );
    assert!(
        joined.contains(&format!("validate constraint {}", change.constraint())),
        "validating is its own statement, so it can be interrupted and run again: {joined}"
    );
    assert!(joined.contains(&format!("alter column {COLUMN} set not null")));
    assert!(
        joined.contains("drop index if exists"),
        "and the index a batch read goes last, with `if exists`: {joined}"
    );
}

#[tokio::test]
async fn the_backfill_jobs_are_jobs_in_the_queue_it_migrates() {
    // The port rather than the engine, and `KILN_BACKEND` rather than a store: the middle phase is
    // jobs on a queue in the table being changed, so what it means to be a job is the port's
    // answer, and `KILN_BACKEND=postgres kiln run jobq/3.23-online-schema` is what proves the two
    // backends agree about it.
    let backend = backend().await;
    let port = backend.store();
    let worker = WorkerId::new("migration-worker");

    // Something the queue was already carrying: the change is supposed to be invisible to it.
    let serving = port
        .enqueue(SubmitRequest::new(
            queue(),
            serde_json::json!({"to": "old@example.com"}),
        ))
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a submit: {error}", backend.name()));

    let plan = plan_for(ROWS);
    let mut submitted = Vec::new();
    for request in plan.requests() {
        let job = port.enqueue(request).await.unwrap_or_else(|error| {
            panic!(
                "the {} backend refused a backfill job: {error}",
                backend.name()
            )
        });
        submitted.push(job.id());
    }
    assert_eq!(
        submitted.len(),
        plan.len(),
        "one job per batch, submitted to the queue the table belongs to"
    );
    assert_eq!(
        plan.requests().len(),
        plan.len(),
        "the port gets the same plan on every backend"
    );

    let claimed = port
        .claim(&worker, &queue(), plan.len() + 1)
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a claim: {error}", backend.name()));
    assert_eq!(
        claimed.len(),
        plan.len() + 1,
        "the migration's jobs are claimable like any other work, and so is the job that was \
         already in the queue: the middle phase runs on the same workers as the traffic it walks \
         past"
    );
    for id in &submitted {
        assert!(
            claimed.iter().any(|job| job.id() == *id),
            "every batch the plan submitted is handed to a worker"
        );
    }

    // What a worker reads out of one: which batch it is, because that is what it has to run.
    let first = plan
        .requests()
        .into_iter()
        .next()
        .expect("the plan has a first request");
    assert_eq!(first.queue(), &queue());
    assert_eq!(
        first.payload()["migration"]["rows"].as_u64(),
        Some(u64::from(BATCH)),
        "a batch job carries how many rows it fills: a worker that had to guess would be a worker \
         that fills the table one row at a time or all at once"
    );
    assert_eq!(
        first.payload()["migration"]["column"],
        serde_json::Value::from(COLUMN)
    );
    assert_eq!(
        first.payload()["migration"]["batch"],
        serde_json::Value::from(0)
    );

    let first_claimed = claimed
        .first()
        .expect("the claim answered with the jobs that were due");
    port.complete(&worker, &first_claimed.id())
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a settle: {error}", backend.name()));

    let leftover = port.list(&JobFilter::new()).await.unwrap_or_else(|error| {
        panic!("the {} backend refused a listing: {error}", backend.name())
    });
    assert!(
        leftover.iter().any(|job| job.id() == serving.id()),
        "the queue kept the work it was already carrying, through the whole thing"
    );
}

#[tokio::test]
async fn the_old_release_writes_rows_this_one_serves() {
    // The database's half, and the reason this step is in a track with a Postgres backend at all:
    // `not valid`, `validate constraint`, an insert written the way the *previous* release writes
    // one, and a contract that is safe the second time. None of it is answerable by the in-memory
    // store, which has no schema to change and answers `migrate()` with `Ok(())`.
    let backend = backend().await;
    let Backend::Postgres { container, .. } = &backend else {
        return;
    };
    let change = change();

    for statement in statements(change.expand(&Applied::before())) {
        container.try_execute(&statement).unwrap_or_else(|refusal| {
            panic!("the expand has to apply to a database that is serving ({statement}): {refusal}")
        });
    }
    assert_eq!(
        container.query(
            "select is_nullable from information_schema.columns where table_name = 'jobs' \
             and column_name = 'payload_bytes'"
        ),
        "YES",
        "the column the expand added is nullable: a `not null` column refuses the insert the \
         release that is already running is about to make"
    );
    assert_eq!(
        container.query(
            "select count(*) from pg_indexes where indexname = 'jobs_unfilled_payload_bytes'"
        ),
        "1",
        "and the partial index a batch reads is there, because the backfill is about to run \
         against a table that is being written to"
    );

    // A row written the way the previous release writes one: it names the columns it knows.
    container.execute(
        "insert into jobs (id, queue, payload, state, priority, run_at, attempts, max_attempts) \
         values ('11111111-1111-4111-8111-111111111111', 'migrations', '{\"to\": \"old\"}'::jsonb, \
         'queued', 0, now(), 0, 3)",
    );
    assert_eq!(
        container.query(
            "select payload_bytes is null from jobs \
             where id = '11111111-1111-4111-8111-111111111111'"
        ),
        "t",
        "the row the old release wrote has no value in the new column"
    );
    assert_eq!(
        container.query(&format!(
            "select {} from jobs where id = '11111111-1111-4111-8111-111111111111'",
            change.fallback()
        )),
        "13",
        "and the read path answers it anyway: `{{\"to\": \"old\"}}` is thirteen bytes, computed by \
         the same expression the backfill will store"
    );

    // The contract's last word, run early, is refused by the database — and that is the refusal the
    // phase answers `Blocked` for rather than letting a deploy find it halfway through.
    let last_word = "alter table jobs alter column payload_bytes set not null";
    let refusal = container
        .try_execute(last_word)
        .expect_err("a column with a row that has no value cannot be made `not null`");
    assert!(
        refusal.contains("contains null values"),
        "the database names the row rather than the statement: {refusal}"
    );

    let batch = plan_for(1).statement(0).expect("one row left is one batch");
    assert_eq!(
        container.query(&format!(
            "with filled as ({batch} returning 1) select count(*) from filled"
        )),
        "1",
        "one batch fills the row the old release left behind"
    );
    assert_eq!(
        container.query(&format!(
            "select payload_bytes = {} from jobs \
             where id = '11111111-1111-4111-8111-111111111111'",
            change.computed()
        )),
        "t",
        "and what it stored is the number the fallback computes: one expression, two places, no \
         disagreement to find"
    );
    assert_eq!(
        container.query(&format!(
            "with filled as ({batch} returning 1) select count(*) from filled"
        )),
        "0",
        "a second run of the same batch fills nothing, which is the whole of what makes a backfill \
         resumable"
    );

    let contract = statements(change.contract(&Applied::expanded(0)));
    for statement in &contract {
        container.try_execute(statement).unwrap_or_else(|refusal| {
            panic!(
                "the contract has to apply to a database that is serving ({statement}): {refusal}"
            )
        });
    }
    assert_eq!(
        container.query(
            "select is_nullable from information_schema.columns where table_name = 'jobs' \
             and column_name = 'payload_bytes'"
        ),
        "NO",
        "after the contract the column may not be empty"
    );
    assert_eq!(
        container.query(
            "select convalidated from pg_constraint where conname = 'jobs_payload_bytes_known'"
        ),
        "t",
        "the constraint the contract added `not valid` has been validated"
    );
    assert_eq!(
        container.query(
            "select count(*) from pg_indexes where indexname = 'jobs_unfilled_payload_bytes'"
        ),
        "0",
        "and the index the backfill read is gone: after the contract there is nothing unfilled to \
         find"
    );

    // Safe to run twice, in the two halves that make it true. The statements a *half*-applied run
    // can be finished by are runnable again — a validation that was interrupted, a `set not null`
    // that already holds, a drop that has nothing left to drop — while the one statement that is
    // not idempotent by nature is refused, which is why the phase asks the catalog *before* it
    // runs rather than letting the database be the thing that notices.
    for statement in contract
        .iter()
        .filter(|statement| !statement.contains("add constraint"))
    {
        container.try_execute(statement).unwrap_or_else(|refusal| {
            panic!("a half-applied contract has to be finishable by the next run ({statement}): {refusal}")
        });
    }
    let refused = container
        .try_execute(&contract[0])
        .expect_err("adding the same constraint twice is refused by the database");
    assert!(
        refused.contains("already exists"),
        "the refusal names the constraint: {refused}"
    );
    assert!(
        matches!(change.contract(&Applied::contracted()), Action::Nothing(_)),
        "which is why the phase that ran is the phase that answers `Nothing` the second time: no \
         statement here is safe by accident"
    );
}

#[test]
fn the_page_states_the_change_the_plan_and_the_rate() {
    let page = page();
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a runbook with a blank in it is a migration nobody \
             has written down"
        );
    }
    for metric in PLAN_METRICS {
        assert!(
            page.contains(metric),
            "{PAGE} does not state `{metric}`, which is a number the gate compares — a metric a \
             page does not explain is a number a reviewer has to guess at"
        );
    }
    assert!(
        page.contains(&BURST_LIMIT.to_string()) && page.contains(&RESUME_LIMIT.to_string()),
        "{PAGE} does not state the two bounds ({BURST_LIMIT} and {RESUME_LIMIT}), so the page and \
         the baseline the gate reads are two different budgets"
    );
    for phase in ["expand", "backfill", "contract"] {
        assert!(
            low.contains(phase),
            "{PAGE} does not name the `{phase}` phase: the three phases are the whole procedure, \
             and a page that skips one is a page nobody can run"
        );
    }
    assert!(
        page.contains("count") || page.contains("batch") || page.contains("rows"),
        "{PAGE} does not say what the metrics count: a budget of wall-clock time is a budget a \
         loaded machine fails, and the page is where that decision is written down"
    );
    assert!(
        low.contains("resum") || low.contains("interrupt"),
        "{PAGE} does not say what to do about a run that died halfway, which is the half of the \
         procedure the middle phase exists for"
    );
}

/// A change whose batch size is zero still advances.
///
/// `with_batch(0)` is what somebody types when they mean "let the system decide", and the answer is
/// one row a batch: a plan that made batches of no rows would be a loop that never advances, and the
/// deadline below is what turns that into a failure a reader can act on rather than a judge that
/// waits for its own timeout.
#[test]
fn a_plan_for_a_zero_batch_still_advances() {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let plan = Change::new(TABLE, COLUMN, COMPUTED, queue())
            .with_batch(0)
            .with_interval(Duration::seconds(INTERVAL_SECONDS))
            .plan(ROWS, now());
        let _ = sender.send((plan.len(), plan.rows(), plan.burst()));
    });
    let (batches, rows, burst) = receiver.recv_timeout(std::time::Duration::from_secs(5)).expect(
        "a plan for a zero batch has to finish: a width of no rows is a loop that never advances, \
         and a migration that hangs is worse than one that refuses",
    );
    assert_eq!(
        batches, ROWS as usize,
        "one row a batch is the only width that both advances and stays inside the row the change \
         is meant to keep small"
    );
    assert_eq!(
        rows, ROWS,
        "and every row is in exactly one of those batches"
    );
    assert_eq!(
        burst, 1,
        "the rate is unchanged by the width: one batch due per interval, so the plan stays a \
         trickle rather than a burst"
    );
}
