//! Spec for `relay/2.23-online-schema`: a column that arrives while the previous release writes.
//!
//! `kiln run` copies this file into the project as `tests/spec_schema.rs`.
//!
//! The claim this step makes is not "a column was added": it is that the three phases of a schema
//! change — expand, backfill, contract — are *separate*, and that each one is safe while the
//! previous release is still running. So every test here is one of those phases observed from
//! outside:
//!
//! * **the old code path still writes.** An `insert` that names only the columns of
//!   `migrations/0003_events.sql` is what the previous release is doing during the window, and it
//!   has to land in the new schema — which is a property of the *migration* (nullable, no default)
//!   rather than of the code that reads it. The spec writes those rows with raw SQL, because that
//!   is the only way to be the previous release from inside this one;
//! * **the backfill is batched and idempotent.** `backfill::bytes` fills at most the batch it was
//!   asked for — asserted by count, not by reading the SQL — and running it again on a finished
//!   table does nothing, which is what makes it resumable and safe to run twice;
//! * **the new code has a defined answer while the column is null.** The page the API serves asks
//!   for the size; a row whose column is empty because it predates the backfill still gets an
//!   answer, computed from the payload exactly the way the backfill would have stored it. A read
//!   path that answered `null` — or an error — for a row nobody has backfilled yet would make the
//!   expand phase a breaking change;
//! * **the contract phase has not happened, and it is checked.** The column is still nullable in
//!   the catalog: making it `not null` in the same deploy as the column itself would refuse every
//!   write the previous release makes, which is the outage the three phases exist to avoid;
//! * **the backfill is a step somebody runs.** `relay --backfill` connects, migrates, fills in
//!   batches and exits zero — a rollout's middle phase as a command rather than a paragraph.

mod support;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use relay::backfill;
use relay::db::Db;
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// How long a test gives a process to do something.
const DEADLINE: Duration = Duration::from_secs(30);
/// How often a waiting test looks again.
const POLL: Duration = Duration::from_millis(50);

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service with one tenant, over a database of its own, migrated.
    fn start() -> Self {
        let postgres = Postgres::start();
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
            var("RELAY__RATE_LIMIT__BURST", "100000"),
            var("RELAY__RATE_LIMIT__PER_SECOND", "100000"),
        ]));
        let ready = app.client().get("/readyz");
        assert_eq!(
            ready.status, 200,
            "the service has to be serving before this fixture asks its database anything: {}",
            ready.body
        );
        Self { app, postgres }
    }

    /// The pool the tests run the backfill on: the service's own policy.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }

    /// One number, as `psql` answers it.
    fn count(&self, sql: &str) -> i64 {
        self.postgres
            .query(sql)
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("`{sql}` answers a number: {error}"))
    }

    /// Write `count` events the way the release before this step wrote them: the columns of
    /// `0003_events.sql`, and nothing else.
    fn write_legacy_events(&self, prefix: &str, count: usize) -> usize {
        for index in 0..count {
            self.postgres.execute(&format!(
                "insert into events (id, tenant_id, idempotency_key, kind, payload, received_at) \
                 values (gen_random_uuid(), (select id from tenants where name = '{}'), \
                         '{prefix}-{index}', 'invoice.paid', \
                         jsonb_build_object('n', {index}), now())",
                ACME.0
            ));
        }
        count
    }

    /// Accept one event through the API, and answer what the service said.
    fn accept(&self, idempotency: &str) -> Response {
        self.app.client().send(
            Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", idempotency)
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 7 } })),
        )
    }

    /// One page of this tenant's events, as the API answers it.
    fn page(&self) -> serde_json::Value {
        let answer = self.app.client().get_with_key("/events", ACME.1);
        assert_eq!(answer.status, 200, "the page answers: {}", answer.body);
        answer.json()
    }

    /// The event with `idempotency` in the page, as the API answered it.
    fn listed(&self, idempotency: &str) -> serde_json::Value {
        // The page is ordered newest first and these fixtures write few rows, so one page holds
        // them all: a test that paged would be testing `relay/2.4-cursor-pagination`.
        let accepted = self.count(&format!(
            "select count(*) from events where idempotency_key = '{idempotency}'"
        ));
        assert_eq!(accepted, 1, "the fixture wrote `{idempotency}` once");
        let id = self.postgres.query(&format!(
            "select id from events where idempotency_key = '{idempotency}'"
        ));
        let page = self.page();
        let events = page
            .get("events")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("a page has an `events` array: {page}"));
        events
            .iter()
            .find(|event| event.get("id").and_then(serde_json::Value::as_str) == Some(id.trim()))
            .cloned()
            .unwrap_or_else(|| panic!("`{idempotency}` ({id}) is in the page: {page}"))
    }
}

/// The previous release's `insert` still lands, and the page still answers for the row it wrote.
#[test]
fn the_previous_release_still_writes_rows_this_one_serves() {
    let fixture = Fixture::start();
    fixture.write_legacy_events("legacy", 3);

    assert_eq!(
        fixture.count("select count(*) from events where payload_bytes is null"),
        3,
        "an `insert` that names only the columns of `0003_events.sql` is legal against this schema, \
         and the column this step added is empty on those rows: that is what makes the expand phase \
         safe while the old release is still writing"
    );

    let listed = fixture.listed("legacy-0");
    let size = listed
        .get("bytes")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| panic!("a listed event carries the size of its payload: {listed}"));
    assert_eq!(
        size,
        fixture.count(
            "select octet_length(payload::text) from events where idempotency_key = 'legacy-0'"
        ),
        "and a row nobody has backfilled yet still answers a size, computed the way the backfill \
         would have stored it: {listed}"
    );
}

/// The backfill fills what is missing, and running it again does nothing.
#[tokio::test]
async fn the_backfill_fills_what_is_missing_and_can_be_run_again() {
    let fixture = Fixture::start();
    fixture.write_legacy_events("past", 5);
    let pool = fixture.pool().await;

    let filled = backfill::bytes(&pool, backfill::BATCH)
        .await
        .expect("the batch runs");
    assert_eq!(
        filled, 5,
        "every row that had no value was filled, and the answer says how many"
    );
    assert_eq!(
        fixture.count("select count(*) from events where payload_bytes is null"),
        0,
        "nothing is left empty"
    );
    assert_eq!(
        fixture.count(
            "select count(*) from events where payload_bytes <> octet_length(payload::text)"
        ),
        0,
        "and every value is the one the read path would compute: the stored number and the computed \
         one are the same number, or the cache is a second answer"
    );

    // The property the whole phase rests on: a backfill that is interrupted, re-run, or run by two
    // people at once must not write anything twice.
    let again = backfill::bytes(&pool, backfill::BATCH)
        .await
        .expect("the batch runs");
    assert_eq!(
        again, 0,
        "a second run over a finished table fills nothing — the predicate is 'no value', not a \
         bookmark, so re-running is free and safe"
    );
    assert_eq!(
        fixture.count("select count(*) from events"),
        5,
        "and it wrote no row, only values on the rows that were already there"
    );

    // The new code writes the column for what it accepts, so the backfill's business is the past
    // and only the past: a row accepted now is never one of the rows waiting for a batch.
    let accepted = fixture.accept("accepted-now");
    assert_eq!(accepted.status, 201, "{}", accepted.body);
    assert_eq!(
        fixture.count(
            "select count(*) from events \
              where idempotency_key = 'accepted-now' and payload_bytes is null"
        ),
        0,
        "an event this release accepts carries its size from the moment it is written"
    );
    assert_eq!(
        fixture.count(
            "select count(*) from events where idempotency_key = 'accepted-now' \
               and payload_bytes <> octet_length(payload::text)"
        ),
        0,
        "and the number is the one the database computed from the value it stored — the same          expression the backfill writes, so a stored size and a computed one can never disagree"
    );

    // The service keeps serving while this happened, and the rows it now holds answer the same
    // number from the stored column as from the payload.
    let listed = fixture.listed("past-0");
    assert_eq!(
        listed.get("bytes").and_then(serde_json::Value::as_i64),
        Some(fixture.count(
            "select octet_length(payload::text) from events where idempotency_key = 'past-0'"
        )),
        "the page answers the size for a backfilled row too: {listed}"
    );
}

/// One batch is one batch: the sizes are asserted, not read out of the SQL.
#[tokio::test]
async fn the_backfill_takes_a_batch_at_a_time() {
    let fixture = Fixture::start();
    fixture.write_legacy_events("batched", 10);
    let pool = fixture.pool().await;

    assert_eq!(
        backfill::bytes(&pool, 3).await.expect("the batch runs"),
        3,
        "a batch of three fills three rows, however many are waiting"
    );
    assert_eq!(
        fixture.count("select count(*) from events where payload_bytes is not null"),
        3,
        "and exactly those three were written"
    );
    assert_eq!(
        backfill::bytes(&pool, 3).await.expect("the batch runs"),
        3,
        "the next batch carries on from the first"
    );
    assert_eq!(backfill::bytes(&pool, 3).await.expect("the batch runs"), 3);
    assert_eq!(
        backfill::bytes(&pool, 3).await.expect("the batch runs"),
        1,
        "the last batch fills what is left rather than failing on a short one"
    );
    assert_eq!(
        backfill::bytes(&pool, 3).await.expect("the batch runs"),
        0,
        "and the run after it has nothing to do"
    );

    // The loop a deployment runs: it stops when a batch fills nothing, and it says how many
    // batches that took.
    fixture.write_legacy_events("looped", 7);
    let run = backfill::run(&pool, 3).await.expect("the run finishes");
    assert_eq!(
        run.rows, 7,
        "the run filled the seven rows that were waiting: {run:?}"
    );
    assert_eq!(
        run.batches, 3,
        "in batches of three: 3, 3, 1 — and no fourth batch, because the run stops on the batch \
         that fills nothing: {run:?}"
    );
    assert_eq!(
        fixture.count("select count(*) from events where payload_bytes is null"),
        0
    );
}

/// The middle phase is a command a rollout can run, and it exits zero like any other step.
#[test]
fn the_backfill_is_a_step_a_rollout_runs() {
    let fixture = Fixture::start();
    fixture.write_legacy_events("command", 4);
    assert_eq!(
        fixture.count("select count(*) from events where payload_bytes is null"),
        4
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_relay"))
        .arg("--backfill")
        .env_clear()
        .env("RELAY__DATABASE__URL", fixture.postgres.url())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the relay binary starts");
    let started = Instant::now();
    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => break child.wait_with_output().expect("its output can be read"),
            Ok(None) if started.elapsed() >= DEADLINE => {
                let _ = child.kill();
                panic!("`relay --backfill` did not finish within {DEADLINE:?}");
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => panic!("the binary could not be waited for: {error}"),
        }
    };
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "a backfill that finished is a step that succeeded: {said}"
    );
    assert!(
        fixture.count("select count(*) from events where payload_bytes is null") == 0,
        "and the rows are filled by the command rather than by the test: {said}"
    );
    assert!(
        said.contains('4'),
        "the line it prints says how many rows it filled, because a step of a rollout that reports \
         nothing is a step nobody can tell apart from one that did nothing: {said}"
    );
}

/// The contract phase has not happened: the column is still the one the expand phase added.
#[test]
fn the_column_is_still_nullable_because_the_contract_phase_is_a_later_release() {
    let fixture = Fixture::start();
    assert_eq!(
        fixture.count(
            "select count(*) from pg_attribute \
              where attrelid = 'events'::regclass and attname = 'payload_bytes' and not attisdropped"
        ),
        1,
        "the column is in the table"
    );
    assert_eq!(
        fixture.count(
            "select count(*) from pg_attribute \
              where attrelid = 'events'::regclass and attname = 'payload_bytes' and attnotnull"
        ),
        0,
        "and it is still nullable: `not null` is the contract phase, and it belongs to a release \
         that ships *after* the backfill has finished everywhere. Doing both in one deploy refuses \
         every `insert` the previous release makes — which is the outage the three phases exist to \
         avoid"
    );
    assert_eq!(
        fixture
            .count("select count(*) from pg_indexes where indexname = 'events_missing_bytes_idx'"),
        1,
        "the partial index the backfill reads is in place: without it, each batch walks the whole \
         history to find the rows it still owes"
    );

    // And the read path still has its fallback, which is the other half of "the contract has not
    // happened": a column that is `not null` is a column the read path can trust, and this one
    // cannot be trusted yet.
    fixture.write_legacy_events("contract", 1);
    let listed = fixture.listed("contract-0");
    assert!(
        listed
            .get("bytes")
            .and_then(serde_json::Value::as_i64)
            .is_some(),
        "a row with no stored value still answers: {listed}"
    );
}
