//! Spec for `relay/2.14-backpressure`: how much work this process holds, and whose.
//!
//! `kiln run` copies this file into the project as `tests/spec_backpressure.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Three properties, and each is judged where it can be
//! observed rather than argued:
//!
//! * **the bound holds.** `tests/support/load.rs` is a subscriber that records how many attempts are
//!   in flight, and can be *held*: with every attempt parked, the number in flight is the number of
//!   seats the worker took, so "never more than the limit" is a count inside the adapter instead of a
//!   hopeful reading of a clock. Two workers share one gate in the first test, because the bound is
//!   the *process's* — two workers with a gate each have twice the bound and no idea that they do;
//! * **the queue grows where it belongs.** Under a held subscriber the number that grows is the row
//!   count in `deliveries`, and the process's own count stays at the limit: a worker that claims a
//!   thousand rows and attempts them one at a time grows a thousand `Claimed` values and a thousand
//!   tasks, which is the failure this step exists to prevent;
//! * **fairness is per tenant.** A flood from one tenant must not stop the next one's only event
//!   from being attempted: `claim::claim` takes the oldest rows, and the oldest rows of a flood are
//!   *all* the flood's. The third test gives one tenant 24 events and the other one, and asks that
//!   the quiet tenant's delivery is attempted within three turns of two seats.
//!
//! The last three tests are the boundaries: the limits come from configuration and zero is refused,
//! the claim is capped by the seats, and the ingest path answers `503` when the queue is at its
//! bound — the transfer of `mailer/1.14-worker`'s "capacity before the work" to a queue that lives
//! in the database.

mod support;

#[path = "support/load.rs"]
mod load;

use std::process::Command;
use std::sync::Arc;

use relay::backoff::Budget;
use relay::claim::LEASE;
use relay::concurrency::{self, Gate, Limits, Worker};
use relay::config::{Config, Flags};
use relay::db::Db;
use relay::delivery::Sender;
use relay::ordering::Lanes;
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;
use uuid::Uuid;

use load::Counted;

/// The tenants these tests seed, and the keys they authenticate with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
const GLOBEX: (&str, &str) = ("globex", "key-globex-91b7");

/// A subscriber URL the fixture never dials: the sender is the harness, and the URL only has to be
/// one this service accepts (`relay/2.12-ssrf-defense`) and a name a test can talk about.
fn hook(name: &str) -> String {
    format!("http://127.0.0.1:9/{name}")
}

/// Settings, as the environment variable list `Config::load` takes.
fn env(settings: &[(&str, &str)]) -> Vec<(String, String)> {
    settings
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service with two tenants, over a database of its own.
    fn start(settings: &[(&str, &str)]) -> Self {
        let postgres = Postgres::start();
        let mut vars = vec![
            var(app::DATABASE_URL, postgres.url()),
            var(
                app::BOOTSTRAP_TENANTS,
                format!("{}={},{}={}", ACME.0, ACME.1, GLOBEX.0, GLOBEX.1),
            ),
            // The subscribers in this file are on `127.0.0.1`, and a service refuses an address on
            // its own networks unless the deployment says otherwise (`relay/2.12-ssrf-defense`).
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
            // This file ingests events in loops, and the *ingest* quota of
            // `relay/2.13-rate-limit` is not what it is about: a fixture that spent it would be
            // measuring the wrong bound.
            var("RELAY__RATE_LIMIT__BURST", "100000"),
            var("RELAY__RATE_LIMIT__PER_SECOND", "100000"),
        ];
        for (name, value) in settings {
            vars.push(var(name, *value));
        }
        let app = App::start(app::config(&vars));
        let ready = app.client().get("/readyz");
        assert_eq!(
            ready.status, 200,
            "the service has to be serving before this file asks its database anything: {}",
            ready.body
        );
        Self { app, postgres }
    }

    /// Register a subscriber for `key`.
    fn register_for(&self, key: &str, target: &str) {
        let answer = self
            .app
            .client()
            .post_json("/endpoints", key, &json!({ "url": target }));
        assert_eq!(
            answer.status, 201,
            "the fixture registers `{target}`: {}",
            answer.body
        );
    }

    /// Accept `count` events for `key`, each under an idempotency key of its own.
    fn accept(&self, key: &str, prefix: &str, count: usize) -> Response {
        let mut last = None;
        for index in 0..count {
            let answer = self.app.client().send(
                Request::post("/events", Vec::new())
                    .bearer(key)
                    .header("idempotency-key", &format!("{prefix}-{index}"))
                    .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
            );
            last = Some(answer);
        }
        last.unwrap_or_else(|| panic!("`accept` writes at least one event"))
    }

    /// One event, answering what the service said.
    fn ingest(&self, key: &str, idempotency: &str) -> Response {
        self.app.client().send(
            Request::post("/events", Vec::new())
                .bearer(key)
                .header("idempotency-key", idempotency)
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
        )
    }

    /// The pool the workers run on: the service's own policy.
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

    /// The delivery owed to the one endpoint at `target`.
    fn delivery_at(&self, target: &str) -> Uuid {
        self.postgres
            .query(&format!(
                "select d.id from deliveries as d \
                   join endpoints as e on e.id = d.endpoint_id \
                  where e.url = '{target}'"
            ))
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("`{target}` has a delivery: {error}"))
    }
}

/// The limit is the limit: a held subscriber sees at most `in_flight` attempts, from any worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_process_never_attempts_more_than_it_has_seats() {
    let fixture = Fixture::start(&[]);
    for name in ["a", "b", "c", "d", "e", "f"] {
        fixture.register_for(ACME.1, &hook(name));
    }
    fixture.accept(ACME.1, "load", 3);
    let pool = fixture.pool().await;

    let limits = Limits::new(2, 1000).expect("two seats, and room for the queue");
    let counted = Arc::new(Counted::new());
    counted.hold();
    let sender: Arc<dyn Sender> = counted.clone();

    // Two workers, one gate: the bound belongs to the process, not to a loop.
    let gate = Arc::new(Gate::new(limits));
    let lanes = Arc::new(Lanes::new());
    let worker = |name: &str| {
        Worker::new(
            pool.clone(),
            Arc::clone(&sender),
            Arc::clone(&gate),
            Arc::clone(&lanes),
            Budget::default(),
            limits,
            name,
        )
    };
    let first_worker = worker("worker-0");
    let second_worker = worker("worker-1");
    let mut first = Box::pin(first_worker.work(12));
    let mut second = Box::pin(second_worker.work(12));
    tokio::select! {
        () = counted.wait_for_started(2) => {}
        finished = &mut first => panic!(
            "the first worker ended with {} attempt(s) started, so this test measured nothing: \
             {finished:?}",
            counted.started()
        ),
        finished = &mut second => panic!(
            "the second worker ended with {} attempt(s) started, so this test measured nothing: \
             {finished:?}",
            counted.started()
        ),
    }

    assert_eq!(
        counted.started(),
        2,
        "two seats are two attempts in flight, and the third cannot start until one comes back: {}",
        counted.describe()
    );
    assert_eq!(
        counted.peak(),
        2,
        "the most the subscriber ever saw in flight is the bound: {}",
        counted.describe()
    );
    assert_eq!(
        gate.in_flight(),
        2,
        "and the process agrees with what the subscriber counted: {}",
        counted.describe()
    );

    // The subscriber starts answering: both workers drain, and the bound still holds all the way.
    counted.release();
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("the first worker runs");
    let second = second.expect("the second worker runs");
    assert_eq!(
        first.claimed + second.claimed,
        18,
        "three events and six endpoints are eighteen deliveries, claimed once each: {first:?} and \
         {second:?}"
    );
    assert_eq!(
        counted.finished(),
        18,
        "and every one of them was attempted: {}",
        counted.describe()
    );
    assert!(
        counted.peak() <= 2,
        "the peak over the whole run is the bound: {}",
        counted.describe()
    );
}

/// Under a held subscriber, what grows is the table: the process holds exactly its seats.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queue_grows_where_it_belongs() {
    let fixture = Fixture::start(&[]);
    for name in ["a", "b", "c", "d"] {
        fixture.register_for(ACME.1, &hook(name));
    }
    fixture.accept(ACME.1, "flood", 24);
    let pool = fixture.pool().await;

    let limits = Limits::new(3, 1000).expect("three seats, and room for the queue");
    let gate = Arc::new(Gate::new(limits));
    let counted = Arc::new(Counted::new());
    counted.hold();
    let sender: Arc<dyn Sender> = counted.clone();
    let one = Worker::new(
        pool.clone(),
        sender,
        Arc::clone(&gate),
        Arc::new(Lanes::new()),
        Budget::default(),
        limits,
        "worker-0",
    );
    let mut working = Box::pin(one.work(1));
    tokio::select! {
        () = counted.wait_for_started(3) => {}
        finished = &mut working => panic!(
            "the worker ended with {} attempt(s) started, so this test measured nothing: \
             {finished:?}",
            counted.started()
        ),
    }

    assert_eq!(
        (counted.started(), gate.in_flight()),
        (3, 3),
        "the work this process holds is its seats: {}",
        counted.describe()
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'pending'"),
        93,
        "and the other 93 deliveries are where they belong — in the table. A worker that claimed \
         them all and held them in memory would be a worker that dies of the queue's depth instead \
         of the work's slowness: {}",
        counted.describe()
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'claimed'"),
        3,
        "three claimed rows, one per seat, and no row claimed that nobody can attempt"
    );

    counted.release();
    let report = working.await.expect("the worker runs");
    assert_eq!(
        report.claimed, 3,
        "one turn takes what it has seats for and no more: {report:?}"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'pending'"),
        93,
        "and the queue is still the queue: {}",
        counted.describe()
    );
}

/// A tenant with one event is served while another tenant's flood is being worked through.
#[tokio::test]
async fn the_quiet_tenant_is_not_waiting_behind_the_flood() {
    let fixture = Fixture::start(&[]);
    let flood = hook("flood");
    let quiet = hook("quiet");
    fixture.register_for(ACME.1, &flood);
    fixture.register_for(GLOBEX.1, &quiet);
    // The flood first, so every one of its deliveries is older than the quiet tenant's only one:
    // the order a claim that reads the queue strictly would hand out.
    fixture.accept(ACME.1, "flood", 24);
    let quiet_delivery = fixture.accept(GLOBEX.1, "quiet", 1);
    assert_eq!(quiet_delivery.status, 201, "{}", quiet_delivery.body);
    let pool = fixture.pool().await;

    let limits = Limits::new(2, 1000).expect("two seats, and room for the queue");
    let counted = Arc::new(Counted::new());
    let sender: Arc<dyn Sender> = counted.clone();
    let worker = Worker::new(
        pool.clone(),
        sender,
        Arc::new(Gate::new(limits)),
        Arc::new(Lanes::new()),
        Budget::default(),
        limits,
        "worker-0",
    );
    let report = worker.work(3).await.expect("the worker runs");

    let quiet = fixture.delivery_at(&quiet);
    assert_eq!(
        report.claimed, 6,
        "three turns of two seats claim six deliveries: {report:?}"
    );
    assert!(
        counted.deliveries().contains(&quiet),
        "the tenant with one event had it attempted within three turns, while the other tenant had \
         24 waiting. A claim that takes the oldest rows takes the flood's rows — every one of them \
         is older — and the quiet tenant's delivery waits for the whole flood: attempted {:?}, and \
         the quiet one is {quiet}",
        counted.deliveries()
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'sent'"),
        6,
        "six deliveries were attempted and taken"
    );
}

/// A claim never takes more rows than there are seats, whatever it is asked for.
#[tokio::test]
async fn a_claim_is_capped_by_the_seats() {
    let fixture = Fixture::start(&[]);
    fixture.register_for(ACME.1, &hook("capped"));
    fixture.accept(ACME.1, "capped", 40);
    let pool = fixture.pool().await;

    let claimed = concurrency::claim_fair(&pool, "worker-a", 3, LEASE)
        .await
        .expect("the claim runs");
    assert_eq!(
        claimed.len(),
        3,
        "three seats, three rows out of the table: {} were taken",
        claimed.len()
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'claimed'"),
        3,
        "and the rows are stamped, so no other worker can take them"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'pending'"),
        37,
        "the rest of the 40 are exactly where they were"
    );
    assert!(
        concurrency::claim_fair(&pool, "worker-a", 0, LEASE)
            .await
            .expect("the claim runs")
            .is_empty(),
        "a claim for no seats takes nothing: it is what a worker with no room left asks for"
    );
}

/// The limits are configuration, and a limit of nothing is refused where it is read.
#[test]
fn the_limits_come_from_the_configuration_and_zero_is_refused() {
    for (in_flight, backlog) in [(0, 1), (1, 0), (0, 0)] {
        assert!(
            Limits::new(in_flight, backlog).is_err(),
            "`in_flight = {in_flight}`, `backlog = {backlog}` is a service that delivers nothing or \
             accepts nothing, and nobody who wrote it meant that"
        );
    }

    let configured = Config::load(
        &Flags::default(),
        &env(&[
            ("RELAY__DELIVERY__MAX_IN_FLIGHT", "2"),
            ("RELAY__DELIVERY__MAX_BACKLOG", "5"),
        ]),
    )
    .expect("the settings load");
    assert_eq!(
        (
            configured.delivery.limits.in_flight(),
            configured.delivery.limits.backlog()
        ),
        (2, 5),
        "the bound is the one the configuration asked for"
    );
    let defaults = Config::load(&Flags::default(), &env(&[])).expect("the defaults load");
    assert!(
        defaults.delivery.limits.in_flight() > 0 && defaults.delivery.limits.backlog() > 0,
        "and a service nobody configured still has a bound: {:?}",
        defaults.delivery.limits
    );
    assert!(
        Config::load(
            &Flags::default(),
            &env(&[("RELAY__DELIVERY__MAX_IN_FLIGHT", "0")])
        )
        .is_err(),
        "a bound of zero is refused, not quietly turned into one"
    );
    assert!(
        Config::load(
            &Flags::default(),
            &env(&[("RELAY__DELIVERY__MAX_BACKLOG", "plenty")])
        )
        .is_err(),
        "and so is a number that is not a number"
    );

    let ran = Command::new(env!("CARGO_BIN_EXE_relay"))
        .arg("--check-config")
        .env_clear()
        .env("RELAY__DELIVERY__MAX_IN_FLIGHT", "2")
        .output()
        .expect("the relay binary runs");
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        ran.status.success(),
        "`--check-config` succeeds without a database: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        stdout.contains("delivery.max_in_flight = 2"),
        "the report names the bound, so a deployment can check it without starting anything: \
         {stdout}"
    );
}

/// The ingest path has a bottom: over the bound, the answer is "come back", not a row nobody drains.
#[test]
fn the_ingest_path_has_a_bottom() {
    let fixture = Fixture::start(&[("RELAY__DELIVERY__MAX_BACKLOG", "3")]);
    fixture.register_for(ACME.1, &hook("bottom"));

    for index in 0..3 {
        let answer = fixture.ingest(ACME.1, &format!("kept-{index}"));
        assert_eq!(
            answer.status, 201,
            "event {index} is accepted: the queue is at its bound and not past it: {}",
            answer.body
        );
    }

    let over = fixture.ingest(ACME.1, "over");
    assert_eq!(
        over.status, 503,
        "the fourth event is past the bound, and the answer is `come back` rather than a row this \
         service cannot promise to work through: {}",
        over.body
    );
    let body = over.json();
    assert_eq!(
        body.get("error"),
        Some(&json!("unavailable")),
        "the refusal is the ordinary error shape, and its token says the service is not ready for \
         this *now*: {body}"
    );
    assert_eq!(
        body.get("retryable"),
        Some(&json!(true)),
        "and that the same request will succeed later: {body}"
    );

    // The queue moves — in a deployment these rows are delivered — and the service accepts again.
    fixture.postgres.execute("delete from deliveries");
    let after = fixture.ingest(ACME.1, "after");
    assert_eq!(
        after.status, 201,
        "an empty queue is room again: {}",
        after.body
    );
}
