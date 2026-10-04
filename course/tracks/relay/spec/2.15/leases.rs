//! Spec for `relay/2.15-shutdown-leases`: a claim is a lease, and stopping is a sequence.
//!
//! `kiln run` copies this file into the project as `tests/spec_leases.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The step promises three things, and each is judged where it can be
//! watched rather than argued:
//!
//! * **a crash is not a lost delivery.** `kill -9` cannot be handled, so the *only* thing that
//!   recovers the rows it held is the instant on them: the first test leaves a row claimed by a
//!   process that is gone, ages its lease, and asks the next worker to find it — and it asserts the
//!   delivery arrives, which is the whole point of a lease;
//! * **a clean stop is not a crash.** A worker that is stopping knows it is stopping and knows which
//!   rows it holds, so the rows come back *now* instead of at the end of a lease — and the test that
//!   watches it does so with a lease of ten minutes, which is what makes "now" the only explanation;
//! * **the drain deadline is real.** A subscriber that never answers must not keep the process alive:
//!   the attempt is given a budget, and what the budget cuts is handed back to the queue rather than
//!   left invisible.
//!
//! Two of the tests drive the real binary (`tests/support/service.rs`): `SIGTERM` and `kill -9` are
//! things a *process* does, and a fixture that overlapped them would be testing the fixture. The rest
//! drive [`lease::serve`] directly with a subscriber the test holds
//! (`tests/support/load.rs`), which is how the deadline is measured without waiting for one.

mod support;

#[path = "support/load.rs"]
mod load;

#[path = "support/target.rs"]
mod target;

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use relay::backoff::Budget;
use relay::concurrency::{Gate, Limits, Worker};
use relay::config::{Config, Flags};
use relay::db::Db;
use relay::delivery::Sender;
use relay::lease;
use relay::ordering::Lanes;
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::Request;
use support::postgres::Postgres;
use support::service::Service;
use support::var;
use target::{Reply, Target};
use tokio::sync::oneshot;

use load::Counted;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// How long a test waits for a condition in the database, and for a process to do something.
const DEADLINE: Duration = Duration::from_secs(30);
/// How often a waiting test looks again.
const POLL: Duration = Duration::from_millis(50);
/// A lease long enough that a test never waits for one to expire, and short enough to be absurd as an
/// explanation for anything: nine minutes and change.
const A_LONG_LEASE: &str = "600";

/// Settings, as the environment variable list `Config::load` takes.
fn env(settings: &[(&str, &str)]) -> Vec<(String, String)> {
    settings
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// The settings a process-level test starts the service with: one seat, one tenant, and the
/// subscribers of this file are on `127.0.0.1` (`relay/2.12-ssrf-defense`).
fn settings(postgres: &Postgres, lease: &str, drain: &str) -> Vec<support::Var> {
    vec![
        var("RELAY__DATABASE__URL", postgres.url()),
        var(
            "RELAY__BOOTSTRAP__TENANTS",
            format!("{}={}", ACME.0, ACME.1),
        ),
        var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
        var("RELAY__DELIVERY__MAX_IN_FLIGHT", "1"),
        var("RELAY__DELIVERY__LEASE_SECONDS", lease),
        var("RELAY__DELIVERY__DRAIN_SECONDS", drain),
    ]
}

/// Wait until the queue answers `t` to a yes/no question, or fail with what it does answer.
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

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service with one tenant, over a database of its own.
    fn start(settings: &[(&str, &str)]) -> Self {
        let postgres = Postgres::start();
        let mut vars = vec![
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
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

    /// Register a subscriber, and accept one event: one delivery, owed.
    fn one_delivery(&self, target: &str) {
        let registered =
            self.app
                .client()
                .post_json("/endpoints", ACME.1, &json!({ "url": target }));
        assert_eq!(
            registered.status, 201,
            "the fixture registers `{target}`: {}",
            registered.body
        );
        let accepted = self.app.client().send(
            Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", "one")
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
        );
        assert_eq!(accepted.status, 201, "{}", accepted.body);
    }

    /// Accept `count` events, each under an idempotency key of its own.
    fn accept(&self, prefix: &str, count: usize) {
        for index in 0..count {
            let accepted = self.app.client().send(
                Request::post("/events", Vec::new())
                    .bearer(ACME.1)
                    .header("idempotency-key", &format!("{prefix}-{index}"))
                    .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
            );
            assert_eq!(accepted.status, 201, "{}", accepted.body);
        }
    }

    /// Claim the tenant's deliveries by hand, as a worker that stopped answering would have left them.
    fn claimed_by(&self, worker: &str, lease: &str) {
        self.postgres.execute(&format!(
            "update deliveries set state = 'claimed', locked_by = '{worker}', locked_at = now(), \
             lease_expires_at = now() + interval '{lease}'"
        ));
    }

    /// One number, as `psql` answers it.
    fn count(&self, sql: &str) -> i64 {
        self.postgres
            .query(sql)
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("`{sql}` answers a number: {error}"))
    }

    /// The pool the worker runs on: the service's own policy.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }

    /// A worker over this fixture's pool, handing deliveries to `sender`.
    fn worker(&self, pool: &PgPool, sender: Arc<dyn Sender>, seats: u32, name: &str) -> Worker {
        let limits = Limits::new(seats, 1000).expect("seats, and room for the queue");
        Worker::new(
            pool.clone(),
            sender,
            Arc::new(Gate::new(limits)),
            Arc::new(Lanes::new()),
            Budget::default(),
            limits,
            name,
        )
    }
}

/// A lease that has run out is work nobody holds, and it comes back counted.
#[tokio::test]
async fn an_expired_lease_is_given_back_to_the_queue() {
    let fixture = Fixture::start(&[]);
    fixture.one_delivery("http://127.0.0.1:9/hook");
    let pool = fixture.pool().await;

    fixture.claimed_by("worker-that-stopped", "600 seconds");
    assert_eq!(
        lease::reclaim_expired(&pool)
            .await
            .expect("the reclaim runs"),
        0,
        "a lease that is still running is a claim somebody holds: `kill -9` is not distinguishable \
         from a slow worker until the instant on the row says so"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'claimed'"),
        1,
        "and the row is exactly as it was"
    );

    // The instant on the row, moved by the fixture: the same thing waiting for the lease would do,
    // and seconds rather than minutes of it.
    fixture.postgres.execute(
        "update deliveries set lease_expires_at = now() - interval '1 second' \
          where state = 'claimed'",
    );
    assert_eq!(
        lease::reclaim_expired(&pool)
            .await
            .expect("the reclaim runs"),
        1,
        "now nobody holds it, and the work goes back to the queue"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'pending'"),
        1,
        "as `pending`, due now: the next worker that asks may take it"
    );
    assert_eq!(
        fixture.count(
            "select count(*) from deliveries where locked_by is null and lease_expires_at is null"
        ),
        1,
        "with no holder and no lease left on it"
    );
    assert_eq!(
        fixture.count("select attempts from deliveries"),
        1,
        "and with the attempt counted. Nobody saw what happened to it — the subscriber may have been \
         told, and may not — and the count is the only thing that ends a delivery whose worker keeps \
         dying: without it, that delivery is attempted for ever (`relay/2.10-dlq` is where it \
         belongs, and it cannot get there)"
    );
}

/// A stopping worker gives its own leases back, and nobody else's.
#[tokio::test]
async fn a_worker_gives_back_its_own_leases_and_nobody_elses() {
    let fixture = Fixture::start(&[]);
    fixture.one_delivery("http://127.0.0.1:9/hook");
    fixture.accept("second", 1);
    let pool = fixture.pool().await;
    assert_eq!(fixture.count("select count(*) from deliveries"), 2);

    // Two rows, claimed by two workers.
    fixture.claimed_by("worker-a", "600 seconds");
    fixture.postgres.execute(
        "update deliveries set locked_by = 'worker-b' \
          where id = (select id from deliveries order by id limit 1)",
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'claimed'"),
        2
    );

    assert_eq!(
        lease::release_held(&pool, "worker-a")
            .await
            .expect("the release runs"),
        1,
        "one of the two rows was worker-a's"
    );
    assert_eq!(
        fixture
            .count("select count(*) from deliveries where state = 'pending' and locked_by is null"),
        1,
        "and it is back in the queue"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where locked_by = 'worker-b'"),
        1,
        "while the other worker's row is untouched: a worker that could release anybody's lease could \
         hand another worker's in-flight attempt to a third one, which is two attempts at one \
         delivery with nobody to blame"
    );
}

/// A drain that runs out of time gives the work back instead of hiding it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_drain_that_runs_out_gives_the_work_back() {
    let fixture = Fixture::start(&[]);
    fixture.one_delivery("http://127.0.0.1:9/hook");
    let pool = fixture.pool().await;

    let counted = Arc::new(Counted::new());
    counted.hold();
    let sender: Arc<dyn Sender> = counted.clone();
    let worker = fixture.worker(&pool, sender, 1, "worker-stopping");
    let (stopping, shutdown) = oneshot::channel();
    let mut serving = Box::pin(lease::serve(
        &worker,
        Duration::from_millis(300),
        async move {
            let _ = shutdown.await;
        },
    ));

    tokio::select! {
        () = counted.wait_for_started(1) => {}
        finished = &mut serving => panic!(
            "the worker stopped before anything was in flight, so this test measured nothing: \
             {finished:?}"
        ),
    }
    let started = Instant::now();
    stopping.send(()).expect("the worker is still running");
    let report = serving.await.expect("the worker stops cleanly");

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the drain deadline is 300ms and the subscriber in this test never answers: a worker that \
         waits for its own HTTP deadline instead of the drain budget is a process an orchestrator \
         kills, and the rows it holds come back only when their leases do. It took {:?} ({report:?})",
        started.elapsed()
    );
    assert_eq!(
        fixture
            .count("select count(*) from deliveries where state = 'pending' and locked_by is null"),
        1,
        "and the row the deadline cut is back in the queue now, not in `claimed` until a lease runs \
         out"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'claimed'"),
        0,
        "nothing is left invisible: {report:?}"
    );
}

/// A drain with time to spare lets the attempt finish, and does not give the row back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_drain_with_time_lets_the_attempt_finish() {
    let fixture = Fixture::start(&[]);
    fixture.one_delivery("http://127.0.0.1:9/hook");
    let pool = fixture.pool().await;

    let counted = Arc::new(Counted::new());
    counted.hold();
    let sender: Arc<dyn Sender> = counted.clone();
    let worker = fixture.worker(&pool, sender, 1, "worker-stopping");
    let (stopping, shutdown) = oneshot::channel();
    let mut serving = Box::pin(lease::serve(&worker, Duration::from_secs(10), async move {
        let _ = shutdown.await;
    }));

    tokio::select! {
        () = counted.wait_for_started(1) => {}
        finished = &mut serving => panic!(
            "the worker stopped before anything was in flight, so this test measured nothing: \
             {finished:?}"
        ),
    }
    stopping.send(()).expect("the worker is still running");
    // The subscriber answers, one moment after the signal: the drain is what gives it the chance.
    counted.release();
    serving.await.expect("the worker stops cleanly");

    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'sent'"),
        1,
        "the delivery was recorded as delivered, before the process left: a stop that gives up on an \
         attempt which was about to answer turns a delivery into a duplicate, and `SIGTERM` is a \
         routine event rather than a failure"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'pending'"),
        0,
        "and nothing was handed back: the row was finished, not abandoned"
    );
}

/// A process that is killed leaves its rows to the lease, and the next worker recovers them.
#[test]
fn a_killed_worker_leaves_a_row_the_next_one_recovers() {
    let postgres = Postgres::start();
    let target = Target::start(vec![Reply::silent(), Reply::ok("taken")]);
    let subscriber = target.url("/hook");

    let service = Service::start(&settings(&postgres, "2", "1"));
    let registered =
        service
            .client()
            .post_json("/endpoints", ACME.1, &json!({ "url": subscriber }));
    assert_eq!(registered.status, 201, "{}", registered.body);
    let accepted = service.client().send(
        Request::post("/events", Vec::new())
            .bearer(ACME.1)
            .header("idempotency-key", "crash")
            .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
    );
    assert_eq!(accepted.status, 201, "{}", accepted.body);

    // The worker takes the row, dials the subscriber, and parks there: the attempt is in flight.
    wait_until(
        &postgres,
        "select (select count(*) from deliveries where state = 'claimed') = 1",
    );
    let mut service = service;
    service.kill();

    assert_eq!(
        postgres.query("select state from deliveries"),
        "claimed",
        "the crash left the row held by a process that no longer exists — that is the cost of a \
         lease, and the reason the lease exists at all"
    );
    assert_eq!(
        postgres.query("select locked_by is not null from deliveries"),
        "t",
        "with a holder nobody can ask"
    );

    // The lease runs out: the instant on the row is the only record of the dead worker that survives.
    wait_until(
        &postgres,
        "select lease_expires_at <= now() from deliveries",
    );

    // A second worker, over the same database: it recovers the row and delivers it.
    let recovered = Service::start(&settings(&postgres, "2", "1"));
    wait_until(&postgres, "select state = 'sent' from deliveries");
    assert_eq!(
        target.requests().len(),
        2,
        "the subscriber was asked twice: once by the worker that died, and once by the worker that \
         recovered the row. A duplicate is what this service promises instead of a loss \
         (`relay/2.6-at-least-once`)"
    );
    assert_eq!(
        postgres.query("select attempts from deliveries"),
        "1",
        "and the recovery counted the attempt nobody could account for"
    );
    drop(recovered);
}

/// A clean stop gives its leases back before it exits, however long they had to run.
#[test]
fn a_clean_stop_gives_its_leases_back() {
    let postgres = Postgres::start();
    // The subscriber never answers, and the lease is ten minutes: nothing but a deliberate release
    // can make this row claimable again inside a test.
    let target = Target::start(vec![Reply::silent()]);

    let service = Service::start(&settings(&postgres, A_LONG_LEASE, "1"));
    let registered =
        service
            .client()
            .post_json("/endpoints", ACME.1, &json!({ "url": target.url("/hook") }));
    assert_eq!(registered.status, 201, "{}", registered.body);
    for index in 0..5 {
        let accepted = service.client().send(
            Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", &format!("stop-{index}"))
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
        );
        assert_eq!(accepted.status, 201, "{}", accepted.body);
    }

    // One seat, one silent subscriber: the worker takes one delivery and is stuck on it, and the
    // other four stay in the queue.
    wait_until(
        &postgres,
        "select (select count(*) from deliveries where state = 'claimed') = 1",
    );

    let mut service = service;
    service.stop();
    let code = service.wait_for_exit();
    assert_eq!(
        code, 0,
        "a process asked to stop stops: a worker that kept claiming would still be running, and the \
         orchestrator would have killed it instead"
    );

    assert_eq!(
        postgres.query("select count(*) from deliveries where state = 'claimed'"),
        "0",
        "nothing is left claimed: the leases were given back, and the lease was ten minutes, so \
         expiry cannot be the explanation"
    );
    assert_eq!(
        postgres.query("select count(*) from deliveries where state = 'pending'"),
        "5",
        "all five deliveries are in the queue, including the one the drain deadline cut: its row \
         keeps its failure count, because nothing about it failed"
    );
    assert_eq!(
        target.requests().len(),
        1,
        "and the worker took on no new work while it was stopping: the only request the subscriber \
         saw is the one the drain could not finish"
    );
}

/// The lease and the drain are configuration, and neither may be zero.
#[test]
fn the_lease_and_the_drain_come_from_the_configuration() {
    let configured = Config::load(
        &Flags::default(),
        &env(&[
            ("RELAY__DELIVERY__LEASE_SECONDS", "90"),
            ("RELAY__DELIVERY__DRAIN_SECONDS", "5"),
        ]),
    )
    .expect("the settings load");
    assert_eq!(
        (configured.delivery.lease, configured.delivery.drain),
        (Duration::from_secs(90), Duration::from_secs(5)),
        "the two numbers are the two decisions: how long a crash is invisible, and how long a stop \
         waits"
    );
    let defaults = Config::load(&Flags::default(), &env(&[])).expect("the defaults load");
    assert!(
        defaults.delivery.lease > Duration::ZERO && defaults.delivery.drain > Duration::ZERO,
        "and a service nobody configured has both: {:?}",
        defaults.delivery
    );
    for name in [
        "RELAY__DELIVERY__LEASE_SECONDS",
        "RELAY__DELIVERY__DRAIN_SECONDS",
    ] {
        assert!(
            Config::load(&Flags::default(), &env(&[(name, "0")])).is_err(),
            "`{name} = 0` is refused where it is read: a lease of nothing makes every claim expire \
             under the worker holding it, and a drain of nothing refuses to let an attempt answer"
        );
        assert!(
            Config::load(&Flags::default(), &env(&[(name, "soon")])).is_err(),
            "and so is a number that is not a number"
        );
    }

    let ran = Command::new(env!("CARGO_BIN_EXE_relay"))
        .arg("--check-config")
        .env_clear()
        .env("RELAY__DELIVERY__LEASE_SECONDS", "90")
        .output()
        .expect("the relay binary runs");
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        ran.status.success(),
        "`--check-config` succeeds without a database: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        stdout.contains("delivery.lease_seconds = 90"),
        "a deployment checks the number it is about to be measured by: {stdout}"
    );
}

/// Recovering expired leases is one statement over the rows an index holds.
#[test]
fn the_lease_has_an_index_to_walk() {
    let fixture = Fixture::start(&[]);

    let indexes = fixture
        .postgres
        .query("select indexdef from pg_indexes where tablename = 'deliveries'");
    assert!(
        indexes
            .lines()
            .any(|line| line.contains("lease_expires_at") && line.contains("claimed")),
        "reclaiming expired leases is one `update` over the rows that are claimed *and* expired, run \
         once per worker per turn: without an index in that shape it walks every delivery this \
         service has ever owed, and the cost of recovering from a crash grows with the tenant's \
         history. The indexes here are:\n{indexes}"
    );
}
