//! Spec for `relay/2.22-rolling-upgrade`: two probes, one schema, two instances.
//!
//! `kiln run` copies this file into the project as `tests/spec_rolling.rs`.
//!
//! Every test here drives the real binary (`tests/support/service.rs`) and one Postgres
//! (`tests/support/postgres.rs`), because what is under test is a *process*: which lines it prints
//! and in which order, what its probes answer while its database changes underneath it, and what
//! happens when a second process joins it on the same database. A fixture that overlapped two
//! instances would be testing the fixture.
//!
//! Five properties, and the shape of the evidence for each:
//!
//! * **liveness is not readiness.** `/healthz` answers while the database is stopped, and `/readyz`
//!   does not — a process that is restarted because its database blipped loses the attempts it was
//!   holding (`relay/2.15-shutdown-leases`);
//! * **readiness is about the schema, and it is asked every time.** The spec moves the database
//!   through the states a rollout produces — current, half-applied, current again, and a schema
//!   from a *newer* release — and watches the same running process answer differently without being
//!   restarted. A probe that cached its startup answer would report a schema that no longer exists;
//! * **the schema is announced before the port is.** The order of two lines in the process's log is
//!   the whole startup contract: migrate, say which schema, then say where you listen;
//! * **a process that cannot reach its schema never says where it listens.** It exits non-zero
//!   instead, with the database in the message — the one failure mode a rollout must never turn
//!   into "listening, and answering 500s";
//! * **the window is served by both instances.** A second instance starts and becomes ready while
//!   the first one is serving, traffic through either answers without a single 5xx, the first one
//!   stops, and the second one keeps serving. And the *old* shape of a row still writes: the columns
//!   of `relay/2.5-atomic-claim` and `relay/2.19-trace-propagation` are not all of them, and an
//!   `insert` that names only the old ones is what the previous release is doing during the window.
//!
//! One honest limit: both instances are built from *this* binary, so the spec cannot show two
//! different releases talking to one schema. What it can show — and does — is the half that a
//! rolling upgrade turns on: the schema changes under a running process, that process keeps
//! serving, and a row written the way the previous release wrote it still lands.

mod support;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use relay::readiness;
use serde_json::json;
use support::client::Client;
use support::postgres::Postgres;
use support::service::Service;
use support::var;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// How long a test gives a process to do something.
const DEADLINE: Duration = Duration::from_secs(30);
/// How often a waiting test looks again.
const POLL: Duration = Duration::from_millis(50);
/// A database that cannot be reached at all: port 1 on the loopback interface is reserved and
/// nothing listens on it.
const NOWHERE: &str = "postgres://postgres:postgres@127.0.0.1:1/relay";

/// The settings a process-level test starts the service with.
fn settings(postgres: &Postgres) -> Vec<support::Var> {
    vec![
        var("RELAY__DATABASE__URL", postgres.url()),
        var(
            "RELAY__BOOTSTRAP__TENANTS",
            format!("{}={}", ACME.0, ACME.1),
        ),
        // `relay/2.12-ssrf-defense`: the subscribers of this file are on this machine.
        var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
        var("RELAY__RATE_LIMIT__BURST", "100000"),
        var("RELAY__RATE_LIMIT__PER_SECOND", "100000"),
    ]
}

/// Register a subscriber for `key` through `client`.
fn register(client: &Client, target: &str) -> String {
    let answer = client.post_json("/endpoints", ACME.1, &json!({ "url": target }));
    assert_eq!(
        answer.status, 201,
        "the fixture registers {target}: {}",
        answer.body
    );
    target.to_owned()
}

/// Accept one event through `client`, under an idempotency key of its own.
fn accept(client: &Client, idempotency: &str) -> u16 {
    client
        .send(
            support::client::Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", idempotency)
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
        )
        .status
}

/// Wait until the queue answers a yes/no question, or fail with what it did answer.
fn wait_until(postgres: &Postgres, question: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let answer = postgres.query(question);
        if answer.trim() == "t" {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "this test waited {DEADLINE:?} for `{question}`, and the database says `{answer}`"
        );
        std::thread::sleep(POLL);
    }
}

/// What one run of the binary printed, and how it ended.
struct Ran {
    /// The exit code, `None` when the deadline killed it.
    code: Option<i32>,
    /// Everything it printed on both streams.
    output: String,
}

/// Run the binary with `env` as its environment, and wait for it to end on its own.
fn run_binary(env: &[support::Var], deadline: Duration) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_relay"));
    command
        .args(["--listen-port", "0"])
        // The environment is the test's, whole: a process that depended on a setting the machine
        // happens to have set would pass here and fail there.
        .env_clear()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for variable in env {
        command.env(&variable.name, &variable.value);
    }
    let mut child = command.spawn().expect("the relay binary starts");
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .expect("a finished child's output can be read");
                return Ran {
                    code: status.code(),
                    output: format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                };
            }
            Ok(None) if started.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran {
                    code: None,
                    output: String::from("still running"),
                };
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => panic!("the binary could not be waited for: {error}"),
        }
    }
}

/// The newest migration file in the project, as text.
///
/// A test that reads a migration *file* is measuring a structure rather than a behaviour, and the
/// reason is worth writing down (authoring.md §6.6): what makes `create index concurrently` work is
/// that sqlx runs a migration outside a transaction when, and only when, the file says so — the
/// runner and the file are two halves of one rule, and the other half is asserted below by asking
/// the database to run the statement inside a transaction.
fn newest_migration() -> String {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", directory.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect();
    files.sort();
    let newest = files
        .pop()
        .unwrap_or_else(|| panic!("this project has migrations"));
    std::fs::read_to_string(&newest)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", newest.display()))
}

/// Liveness answers without the database; readiness is about the schema.
#[test]
fn readiness_is_about_the_schema_not_about_the_database_answering() {
    let postgres = Postgres::start();
    let service = Service::start(&settings(&postgres));
    let client = service.client();
    assert_eq!(
        client.get("/readyz").status,
        200,
        "a process that has just migrated is ready: {}",
        client.get("/readyz").body
    );

    // The newest migration is recorded as *not* applied: what a half-finished migration leaves, and
    // what a database somebody migrated by hand looks like from inside this process.
    postgres.execute(
        "update _sqlx_migrations set success = false \
          where version = (select max(version) from _sqlx_migrations)",
    );
    let behind = client.get("/readyz");
    assert_eq!(
        behind.status, 503,
        "the schema this process was built for is not in the database, and readiness is a question \
         about the schema: {}",
        behind.body
    );
    let body = behind.json();
    assert_eq!(
        body.get("error"),
        Some(&json!("unavailable")),
        "the refusal is the ordinary error shape, with the token that means 'come back': {body}"
    );
    assert_eq!(
        body.get("retryable"),
        Some(&json!(true)),
        "and the same request will be answerable once the schema is applied: {body}"
    );
    let message = body
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert!(
        message.contains(&readiness::needed().to_string()),
        "the body names the version this process needs ({}), because 'not ready' without the \
         numbers is a page nobody can act on: {message}",
        readiness::needed()
    );
    assert_eq!(
        client.get("/healthz").status,
        200,
        "and this process is still *alive*: restarting it because its schema is behind would throw \
         away the attempts it is holding without applying a single migration"
    );

    // The migration is applied after all, by whoever was applying it: the same process, without a
    // restart, is ready again. A probe that cached its startup answer would still be reporting 503.
    postgres.execute(
        "update _sqlx_migrations set success = true \
          where version = (select max(version) from _sqlx_migrations)",
    );
    assert_eq!(
        client.get("/readyz").status,
        200,
        "the probe asks the database every time it is asked, which is the only way it can report a \
         schema that changed while the process was running: {}",
        client.get("/readyz").body
    );

    // A schema from the *future*: the new instance of a rollout has already migrated, and every old
    // instance is still serving. It is ready on purpose (`relay/2.23-online-schema`: migrations grow
    // the schema and never reinterpret it), and a second process must be able to start against it
    // rather than refuse.
    postgres.execute(
        "insert into _sqlx_migrations (version, description, installed_on, success, checksum, \
                                       execution_time) \
         values (99999, 'a migration from a newer release', now(), true, '\\x00'::bytea, 0)",
    );
    assert_eq!(
        client.get("/readyz").status,
        200,
        "an old instance keeps taking traffic against a schema that grew: {}",
        client.get("/readyz").body
    );
    let second = Service::start(&settings(&postgres));
    assert_eq!(
        second.client().get("/readyz").status,
        200,
        "and a process that starts against that schema must not refuse to run: {}",
        second.client().get("/readyz").body
    );

    // The database goes away for good. Liveness does not care; readiness does.
    postgres.stop();
    assert_eq!(
        client.get("/healthz").status,
        200,
        "liveness is this process being able to answer at all, and it never touches the database"
    );
    assert_eq!(
        client.get("/readyz").status,
        503,
        "a process whose database is gone cannot take traffic, and saying otherwise is how an \
         orchestrator routes requests into a 500: {}",
        client.get("/readyz").body
    );
}

/// The order of the two lines a rollout is read from: migrate, then say where you listen.
#[test]
fn the_schema_is_announced_before_the_port_is() {
    let postgres = Postgres::start();
    let service = Service::start(&settings(&postgres));
    let lines = service.log_lines();
    let schema = lines
        .iter()
        .position(|line| line.contains("the schema is in place"))
        .unwrap_or_else(|| {
            panic!("the process has to say which schema it found, and it said: {lines:#?}")
        });
    let listening = lines
        .iter()
        .position(|line| line.contains("listening"))
        .unwrap_or_else(|| panic!("the process never said where it listens: {lines:#?}"));

    assert!(
        schema < listening,
        "the schema is announced *before* the port: a service that says where it listens first is a \
         service that took traffic before it knew whether it could answer\n{lines:#?}"
    );
    assert!(
        lines
            .get(schema)
            .is_some_and(|line| line.contains(&readiness::needed().to_string())),
        "the line names the version it is on ({}), so a rollout can be read out of a log without \
         asking the service anything: {:?}",
        readiness::needed(),
        lines.get(schema)
    );
}

/// A process that cannot reach its schema never says where it listens.
#[test]
fn a_process_that_cannot_reach_its_schema_never_says_where_it_listens() {
    let ran = run_binary(
        &[
            var("RELAY__DATABASE__URL", NOWHERE),
            var(
                "RELAY__BOOTSTRAP__TENANTS",
                format!("{}={}", ACME.0, ACME.1),
            ),
        ],
        DEADLINE,
    );
    assert_eq!(
        ran.code,
        Some(1),
        "a database that cannot be reached is the deployment's problem and exit code 1 is its \
         sentence (`mailer/1.5-errors`): {}",
        ran.output
    );
    assert!(
        !ran.output.contains("listening"),
        "the process never said where it listens: it never finished the step that comes before \
         that, and a port announced here would be a port that accepts requests it cannot answer\n{}",
        ran.output
    );
    assert!(
        ran.output.contains("database"),
        "and the failure names the database, because that is the thing an operator has to go and \
         look at:\n{}",
        ran.output
    );
}

/// The window: a second instance is ready while the first serves, and the first is the one that
/// stops.
#[test]
fn the_second_instance_is_ready_while_the_first_one_serves() {
    let postgres = Postgres::start();
    let mut first = Service::start(&settings(&postgres));
    let subscriber = register(&first.client(), "http://127.0.0.1:9/window");
    assert_eq!(
        first.client().get("/readyz").status,
        200,
        "the instance that migrated is ready: {}",
        first.client().get("/readyz").body
    );

    // The new instance comes up against a database that is already migrated: it has nothing to do,
    // and it becomes ready without stopping anything.
    let second = Service::start(&settings(&postgres));
    assert_eq!(
        second.client().get("/readyz").status,
        200,
        "the second instance is ready while the first is still serving: {}",
        second.client().get("/readyz").body
    );

    // Both versions serve during the window, and nothing is refused: the events alternate between
    // the two processes, and a single 5xx would be the outage a rolling upgrade is supposed to
    // avoid.
    for index in 0..12 {
        let client = if index % 2 == 0 {
            first.client()
        } else {
            second.client()
        };
        let status = accept(&client, &format!("window-{index}"));
        assert!(
            status == 201 || status == 200,
            "event {index} was answered with {status} while both instances were serving"
        );
    }
    assert_eq!(
        postgres.query("select count(*) from events where idempotency_key like 'window-%'"),
        "12",
        "and every one of them was accepted once, whichever process answered"
    );
    assert_eq!(
        postgres.query("select count(*) from deliveries"),
        "12",
        "the fan-out is one delivery per event per endpoint, and the endpoint was registered once"
    );
    assert_eq!(
        postgres.query("select count(*) from _sqlx_migrations"),
        readiness::needed().to_string(),
        "the second instance applied no migration of its own: one row per migration this binary \
         embeds, and the migration is what only one process does"
    );

    // The old instance goes away first, and the new one keeps serving. This is the order the whole
    // step exists for: the process that stops is the one nothing has to be ready for.
    first.stop();
    assert_eq!(
        first.wait_for_exit(),
        0,
        "a SIGTERM is a graceful stop, and it exits zero (`relay/2.15-shutdown-leases`)"
    );
    assert_eq!(
        second.client().get("/readyz").status,
        200,
        "the instance that is left is ready: {}",
        second.client().get("/readyz").body
    );
    let after = accept(&second.client(), "after-the-window");
    assert_eq!(
        after, 201,
        "and it takes traffic the moment the other one is gone — which is what makes stopping the \
         old instance first safe"
    );

    // The subscriber the fixture registered is on port 9, so the deliveries are owed rather than
    // delivered; what this test is about is the window, and `relay/2.15-shutdown-leases` is where
    // the rows an instance was holding are accounted for.
    let _ = subscriber;
}

/// The old shape of a row still writes, and the service still delivers what it wrote.
#[test]
fn the_previous_release_still_writes_rows_this_one_delivers() {
    let postgres = Postgres::start();
    let service = Service::start(&settings(&postgres));
    let client = service.client();
    register(&client, "http://127.0.0.1:9/first");
    let second = register(&client, "http://127.0.0.1:9/second");
    assert_eq!(accept(&client, "new-shape"), 201);

    // **An event, as the release before `relay/2.19-trace-propagation` wrote one**: the columns of
    // `0003_events.sql` and no others. This is the half of a dual-version window that a spec can
    // actually run on one machine: the *writing* code is the previous release, and everything the
    // schema gained since must be optional for it.
    postgres.execute(
        "insert into events (id, tenant_id, idempotency_key, kind, payload, received_at) \
         values (gen_random_uuid(), (select id from tenants where name = 'acme'), \
                 'legacy-event', 'invoice.paid', '{}'::jsonb, now())",
    );
    // **A delivery, as the release before this step wrote one**: the columns of `0005_deliveries.sql`
    // and nothing else — no trace, no lease. Its endpoint is one the event was *not* fanned out to
    // (it was registered after the event was accepted), so the row is a debt this service has to
    // notice rather than a duplicate of one it already has.
    postgres.execute(&format!(
        "insert into deliveries (id, event_id, endpoint_id, state, attempts, next_attempt_at) \
         values (gen_random_uuid(), \
                 (select id from events where idempotency_key = 'legacy-event'), \
                 (select id from endpoints where url = '{second}'), 'pending', 0, now())"
    ));

    assert_eq!(
        postgres.query(
            "select count(*) from deliveries as d \
               join events as v on v.id = d.event_id \
              where v.idempotency_key = 'legacy-event'"
        ),
        "1",
        "the row is there, written by the old column list into the new schema"
    );
    assert_eq!(
        postgres.query(
            "select d.trace_id is null from deliveries as d \
               join events as v on v.id = d.event_id \
              where v.idempotency_key = 'legacy-event'"
        ),
        "t",
        "and the columns this release added are null on it: that is what makes the write legal, and \
         it is the shape `relay/2.23-online-schema` is about"
    );

    // The new process has to keep working on rows it did not write: the worker picks the row up and
    // attempts it, which is the promise the old release relies on while it is still running.
    wait_until(
        &postgres,
        "select state <> 'pending' from deliveries as d \
           join events as v on v.id = d.event_id \
          where v.idempotency_key = 'legacy-event'",
    );
    assert!(
        postgres
            .query(
                "select attempts >= 1 from deliveries as d \
                   join events as v on v.id = d.event_id \
                  where v.idempotency_key = 'legacy-event'"
            )
            .trim()
            == "t",
        "the attempt was counted on the row, so a delivery the previous release created is delivered \
         by this one rather than sitting in the queue for ever"
    );
}

/// The migration that grew the schema was applied online: the index is there, and the pair of rules
/// that makes it online is the pair the file obeys.
#[test]
fn the_migration_that_grew_the_schema_was_applied_online() {
    let postgres = Postgres::start();
    let service = Service::start(&settings(&postgres));
    assert_eq!(
        service.client().get("/readyz").status,
        200,
        "the migration ran before the process became ready: {}",
        service.client().get("/readyz").body
    );

    assert_eq!(
        postgres.query(
            "select count(*) from pg_index as i \
              where i.indexrelid = 'deliveries_endpoint_claimable_idx'::regclass and i.indisvalid"
        ),
        "1",
        "the index the per-tenant claim reads is in the database and valid"
    );
    let definition = postgres.query(
        "select indexdef from pg_indexes where indexname = 'deliveries_endpoint_claimable_idx'",
    );
    assert!(
        definition.contains("endpoint_id"),
        "and it is the index the claim asks for — keyed on the endpoint: {definition}"
    );

    // The other half of the rule, asked of the database rather than asserted about the file: a
    // `CREATE INDEX CONCURRENTLY` cannot run inside a transaction. The statement below is in one,
    // and the server refuses it — which is *why* the migration has to say it is not transactional.
    // Together with the valid index above, the pair is the proof: an index built by a
    // non-transactional migration exists, and the same statement inside a transaction does not run
    // at all.
    let refused = postgres
        .try_execute("begin; create index concurrently spec_probe_idx on deliveries (id); commit");
    let why = refused.err().unwrap_or_else(|| {
        panic!(
            "a `create index concurrently` inside a transaction is refused by this server, and here \
             it was not: if that is no longer true, the migration's `-- no-transaction` line — and \
             this assertion — need re-reading"
        )
    });
    assert!(
        why.contains("transaction"),
        "and the refusal says which rule was broken: {why}"
    );

    let newest = newest_migration();
    assert!(
        newest.starts_with("-- no-transaction"),
        "the newest migration is the one this step rolled out, and it declares itself \
         non-transactional: sqlx runs a migration inside a transaction unless the file's first line \
         says otherwise, and a `create index concurrently` inside one fails"
    );
    assert!(
        newest.to_ascii_lowercase().contains("concurrently"),
        "and it builds the index online, which is the difference between a rollout and a pause: \
         {newest}"
    );
}
