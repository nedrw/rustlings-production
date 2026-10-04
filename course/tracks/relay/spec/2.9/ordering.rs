//! Spec for `relay/2.9-ordering`: at most one delivery in flight per endpoint, and every other
//! endpoint in parallel.
//!
//! `kiln run` copies this file into the project as `tests/spec_ordering.rs`; it is the course's
//! file, not the learner's (plan §3 D2). The judgements here are about *overlap*, and overlap is not
//! something a test can read off a clock: `tests/support/lanes.rs` is a sender that counts, per
//! endpoint, how many attempts are in flight, the highest that count ever was, and the order the
//! attempts were let through. An endpoint can be **held**, so a test can say "this subscriber is
//! slow" and then wait for the events it needs instead of sleeping.
//!
//! Four properties, one sentence each:
//!
//! * **one endpoint is one lane.** Nothing is attempted to an endpoint while an attempt to it is
//!   in flight. The spec walks a queue one attempt at a time and checks the arithmetic at every
//!   step; the invariant it is really checking is "started minus finished is never more than one";
//! * **an endpoint's lane is nobody else's.** A held endpoint does not delay another endpoint,
//!   which is the difference between a keyed mutex and one big lock;
//! * **the order the queue chose is the order the subscriber sees.** The claim orders by
//!   `(next_attempt_at, id)`, and the attempts to one endpoint leave in that order;
//! * **an endpoint gets a turn before it gets a second one.** A batch of deliveries that all became
//!   due at the same instant is fired one attempt per endpoint, so a burst of retries at a
//!   subscriber that just came back arrives as one request per endpoint at a time.
//!
//! These tests run on a plain `#[tokio::test]`, i.e. on one thread: the lane is fair (whoever asked
//! first is served first) and the tasks are spawned in the order the claim answered, so the order
//! the last test reads is a property of the code rather than of a scheduler. Under a multi-threaded
//! runtime the *counts* still hold; the order would then be the runtime's, which is why only one
//! assertion here is about order and the rest are about counts.

mod support;

#[path = "support/lanes.rs"]
mod lanes;

use std::sync::Arc;

use relay::backoff::Budget;
use relay::db::Db;
use relay::delivery::Sender;
use relay::ordering::{Lanes, spread};
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;
use uuid::Uuid;

use lanes::Gated;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");

/// A subscriber URL the fixture never dials: the sender is the harness, and the URL only has to be
/// one this service accepts and a name a test can talk about.
fn hook(name: &str) -> String {
    format!("http://127.0.0.1:9/{name}")
}

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service with one tenant, over a database of its own.
    fn start() -> Self {
        let postgres = Postgres::start();
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
            // The subscribers of this file are a scripted target on `127.0.0.1`, and the service
            // refuses an address on its own networks unless the deployment says otherwise
            // (`relay/2.12-ssrf-defense`): this fixture is a deployment whose subscribers are
            // internal, so it says so.
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
        ]));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting; this file reads the tables.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks its database anything: {}",
            serving.body
        );
        Self { app, postgres }
    }

    /// Register a subscriber for the fixture's tenant.
    fn register(&self, url: &str) {
        let answer = self.app.client().send(
            Request::post("/endpoints", Vec::new())
                .bearer(ACME.1)
                .json(&json!({ "url": url })),
        );
        assert_eq!(
            answer.status, 201,
            "the fixture registers `{url}` for the tenant: {}",
            answer.body
        );
    }

    /// Accept `count` events, each one delivery per registered endpoint.
    fn accept(&self, prefix: &str, count: usize) {
        for index in 0..count {
            let answer = self.accept_one(&format!("{prefix}-{index}"));
            assert_eq!(
                answer.status, 201,
                "accepting event {index}: {}",
                answer.body
            );
        }
    }

    /// Accept one event under `key`.
    fn accept_one(&self, key: &str) -> Response {
        self.app.client().send(
            Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", key)
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
        )
    }

    /// The pool the worker runs on: the service's own policy.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }

    /// One query, as text.
    fn query(&self, sql: &str) -> String {
        self.postgres.query(sql)
    }

    /// The deliveries to one endpoint, in the order the claim hands them out.
    fn queue(&self, url: &str) -> Vec<Uuid> {
        self.query(&format!(
            "select id from deliveries where endpoint_id = \
             (select id from endpoints where url = '{url}') order by next_attempt_at, id"
        ))
        .lines()
        .map(|line| line.parse().expect("a delivery id"))
        .collect()
    }
}

/// One endpoint: one attempt at a time, in the order the queue chose.
#[tokio::test]
async fn deliveries_to_one_endpoint_leave_in_the_order_they_were_queued() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let endpoint = hook("ordered");
    fixture.register(&endpoint);
    fixture.accept("ordered", 6);
    let queued = fixture.queue(&endpoint);
    assert_eq!(queued.len(), 6, "six deliveries are waiting: {queued:?}");

    let gated = Arc::new(Gated::new());
    let sender: Arc<dyn Sender> = gated.clone();
    let turn = spread(
        &pool,
        sender,
        "worker-a",
        Arc::new(Lanes::new()),
        &Budget::default(),
    )
    .await
    .expect("the worker runs");

    assert_eq!(
        (turn.claimed, turn.sent, turn.retried, turn.dead, turn.shed),
        (6, 6, 0, 0, 0),
        "every delivery was attempted once and taken: {turn:?}"
    );
    assert_eq!(
        gated.peak(&endpoint),
        1,
        "the subscriber saw at most one attempt to this endpoint at a time, over {} of them: two \
         events of one kind arriving at one subscriber in the wrong order is a subscriber that has \
         to sort them itself, and most of them cannot",
        gated.started(&endpoint)
    );
    assert_eq!(
        gated.order(),
        queued,
        "the six attempts left in the order the claim queued them, which is `(next_attempt_at, id)`: \
         a lane that serves whoever happens to ask first is a lane that reorders a subscriber's \
         events"
    );
}

/// A queue is walked one attempt at a time: nothing is attempted while the previous attempt is out.
#[tokio::test]
async fn one_endpoint_is_one_lane() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let endpoint = hook("walked");
    fixture.register(&endpoint);
    fixture.accept("walked", 4);

    let gated = Arc::new(Gated::new());
    gated.hold(&endpoint);
    let sender: Arc<dyn Sender> = gated.clone();
    let lanes = Arc::new(Lanes::new());
    let budget = Budget::default();
    let mut working = Box::pin(spread(&pool, sender, "worker-a", lanes, &budget));

    // The lane hands the endpoint to one attempt at a time, so the test releases one attempt per
    // step and checks what the fixture counted in between: at most one attempt outstanding.
    for step in 1..=4 {
        tokio::select! {
            _ = gated.wait_for_started(&endpoint, step) => {}
            finished = &mut working => panic!(
                "the turn ended with only {} of four attempts started, so this test measured \
                 nothing: {finished:?}",
                gated.started(&endpoint)
            ),
        }
        assert_eq!(
            gated.finished(&endpoint),
            step - 1,
            "before attempt {step} was let through, {} attempt(s) had finished: an endpoint that is \
             asked a new question before the previous one is answered is a subscriber being \
             hammered, and a queue whose order means nothing",
            gated.finished(&endpoint)
        );
        assert_eq!(
            gated.peak(&endpoint),
            1,
            "and the most this endpoint ever had in flight is the number that has to stay at one"
        );
        gated.release(&endpoint);
    }

    let turn = working.await.expect("the worker runs");
    assert_eq!(
        turn.sent, 4,
        "the lane delayed all four attempts, and dropped none: {turn:?}"
    );
    assert_eq!(
        gated.finished(&endpoint),
        4,
        "and every one of them was taken in the end: {}",
        gated.describe()
    );
}

/// A held endpoint delays its own queue and nothing else.
#[tokio::test]
async fn a_slow_endpoint_does_not_hold_up_a_fast_one() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let slow = hook("slow");
    let fast = hook("fast");
    fixture.register(&slow);
    fixture.register(&fast);
    fixture.accept("mixed", 2);

    let gated = Arc::new(Gated::new());
    gated.hold(&slow);
    let sender: Arc<dyn Sender> = gated.clone();
    let lanes = Arc::new(Lanes::new());
    let budget = Budget::default();
    let mut working = Box::pin(spread(&pool, sender, "worker-a", lanes, &budget));

    tokio::select! {
        _ = gated.wait_for_finished(&fast, 2) => {}
        finished = &mut working => panic!(
            "the turn ended while `{slow}` was still held, so this test measured nothing: \
             {finished:?}"
        ),
    }
    assert_eq!(
        (gated.started(&slow), gated.finished(&slow)),
        (1, 0),
        "the held endpoint had exactly one attempt out and none finished, so its second attempt was \
         waiting for the lane: {}",
        gated.describe()
    );
    assert_eq!(
        gated.finished(&fast),
        2,
        "and both attempts to the endpoint that answers were taken while the other one was held: \
         one subscriber that does not answer must not stop every other subscriber from being \
         delivered to, which is the difference between a keyed mutex and one big lock"
    );

    gated.release_all();
    let turn = working.await.expect("the worker runs");
    assert_eq!(
        (turn.claimed, turn.sent),
        (4, 4),
        "every delivery was attempted and taken once the held endpoint was released: {turn:?}"
    );
}

/// A burst that became due together is fired one attempt per endpoint, and every endpoint's first
/// attempt is out before any endpoint's second one starts.
#[tokio::test]
async fn every_endpoint_gets_a_turn_before_any_endpoint_gets_a_second_one() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let endpoints: Vec<String> = (0..6).map(|index| hook(&format!("fan-{index}"))).collect();
    for endpoint in &endpoints {
        fixture.register(endpoint);
    }
    fixture.accept("burst", 2);

    let gated = Arc::new(Gated::new());
    for endpoint in &endpoints {
        gated.hold(endpoint);
    }
    let sender: Arc<dyn Sender> = gated.clone();
    let lanes = Arc::new(Lanes::new());
    let budget = Budget::default();
    let mut working = Box::pin(spread(&pool, sender, "worker-a", lanes, &budget));

    tokio::select! {
        _ = gated.wait_for_each_endpoint_started(&endpoints, 1) => {}
        finished = &mut working => panic!(
            "the turn ended with endpoints still held, so this test measured nothing: {finished:?}"
        ),
    }
    for endpoint in &endpoints {
        assert_eq!(
            gated.started(endpoint),
            1,
            "every endpoint had exactly one attempt out while all of them were held — an endpoint \
             that has not had a turn while another one is on its second is an endpoint that starves: \
             {}",
            gated.describe()
        );
        assert_eq!(
            gated.peak(endpoint),
            1,
            "and no endpoint was asked twice at once: a subscriber that just came back must not be \
             handed a burst of simultaneous requests by the worker that is retrying it"
        );
    }

    gated.release_all();
    let turn = working.await.expect("the worker runs");
    assert_eq!(
        (turn.claimed, turn.sent),
        (12, 12),
        "twelve deliveries — two events for six endpoints — and every one of them was attempted \
         once and taken: {turn:?}"
    );
    for endpoint in &endpoints {
        assert_eq!(
            (gated.started(endpoint), gated.peak(endpoint)),
            (2, 1),
            "every endpoint ended up with both of its deliveries, one at a time: {}",
            gated.describe()
        );
    }
}

/// The lanes are not a place to keep every URL this worker has ever seen.
#[tokio::test]
async fn the_lanes_are_forgotten_when_nobody_holds_them() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let endpoint = hook("forgotten");
    fixture.register(&endpoint);
    fixture.accept("forgotten", 1);

    let gated = Arc::new(Gated::new());
    gated.hold(&endpoint);
    let sender: Arc<dyn Sender> = gated.clone();
    let lanes = Arc::new(Lanes::new());
    let budget = Budget::default();
    let mut working = Box::pin(spread(
        &pool,
        sender,
        "worker-a",
        Arc::clone(&lanes),
        &budget,
    ));

    tokio::select! {
        _ = gated.wait_for_started(&endpoint, 1) => {}
        finished = &mut working => panic!(
            "the turn ended before the held attempt started, so this test measured nothing: \
             {finished:?}"
        ),
    }
    assert_eq!(
        lanes.known(),
        1,
        "while an attempt is out, its endpoint has a lane — that is what the entry in the map is"
    );

    gated.release_all();
    let turn = working.await.expect("the worker runs");
    assert_eq!(turn.sent, 1, "the attempt reached the subscriber: {turn:?}");
    assert_eq!(
        lanes.known(),
        0,
        "and once nobody holds it, the lane is gone: a worker that keeps a gate per endpoint it has \
         ever talked to grows with its tenants' URL history, and that is a memory leak shaped like a \
         hash map"
    );
}
