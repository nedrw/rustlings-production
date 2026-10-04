//! Spec for `relay/2.17-chaos-injection`: the failures you are afraid of, injected on purpose.
//!
//! `kiln run` copies this file into the project as `tests/spec_chaos.rs`; it is the course's file,
//! not the learner's (plan §3 D2). What the step adds is `tests/support/faults.rs` — a proxy that
//! sits between the worker and its subscriber and does what the test tells it to — and this file is
//! the evidence that the service survives it.
//!
//! Four fault classes, and each one asks a different question of the code written before this step:
//!
//! * **a slow answer** (`Fault::Delay`) is the case a *budget* is for: an attempt that finishes
//!   inside its deadline succeeds and costs one attempt, and one that does not is given back to the
//!   queue with its count spent and a wait written on it. Both halves are judged, because a service
//!   that retried the slow-but-successful case would be a duplicate-delivery machine;
//! * **a reset connection** (`Fault::Reset`) is the ordinary failure — a crashed subscriber, a
//!   firewall — and the promise is that it is a *delay*: every accepted event ends as `sent` or
//!   `dead`, and nothing in between is invisible;
//! * **a half-open connection** (`Fault::HalfOpen`) is the one failure only a deadline catches: the
//!   subscriber took the request and went quiet, so no read ever returns "closed". What the test
//!   asserts is the shape of the failure in the row (`last_error` says nothing arrived for the read
//!   deadline) and that the turn *ended* — a service without a deadline does not fail this test,
//!   it hangs in it;
//! * **a dropped connection** (`Fault::Loss`) is loss that is *counted*: the fixture drops every
//!   n-th connection, so the same test run drops the same connections, and a test can assert that
//!   the fault happened instead of hoping it did.
//!
//! Two properties are asserted once for the whole matrix rather than per fault:
//!
//! * **nothing is lost.** After a fault has been healed, the number of deliveries that are
//!   delivered or buried equals the number of events that were accepted, and no row is left
//!   `claimed` — an invisible row is the failure mode a lease is supposed to bound, and "the
//!   delivery comes back eventually" is not the same promise as "it came back";
//! * **the counters say the fault happened.** A proxy that injected nothing would make every
//!   assertion above pass vacuously, so every test reads the fixture's own record: connections
//!   accepted, requests forwarded, connections the fault took.

mod support;

#[path = "support/faults.rs"]
mod faults;

#[path = "support/target.rs"]
mod target;

use std::sync::Arc;
use std::time::{Duration, Instant};

use relay::backoff::{Breaker, BreakerPolicy, Budget};
use relay::db::Db;
use relay::delivery::{self, Sender, Tick};
use relay::outbound::{HttpSender, Policy};
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::Request;
use support::postgres::Postgres;
use support::var;
use target::{Reply, Target};

use faults::{Fault, Proxy};

/// The tenant these tests act as.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// The worker every turn in this file claims as.
const WORKER: &str = "chaos";
/// The most turns [`Fixture::drain`] will run before it calls the queue stuck.
const DRAIN_TURNS: usize = 12;

/// One database, one in-process service, one scripted subscriber, and a proxy in front of it.
struct Fixture {
    app: App,
    postgres: Postgres,
    target: Target,
    proxy: Proxy,
}

impl Fixture {
    /// Start the whole chain: service → proxy → subscriber.
    ///
    /// The endpoint is registered at the *proxy's* URL, and the proxy forwards to the target: that
    /// one line is what makes every fault below a fault on the path the service actually uses.
    fn start() -> Self {
        let postgres = Postgres::start();
        let target = Target::start(vec![Reply::ok("{}")]);
        let proxy = Proxy::start(target.address());
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
            // The subscriber in this file is on `127.0.0.1`, and a service refuses an address on
            // its own networks unless the deployment says otherwise (`relay/2.12-ssrf-defense`).
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
            var("RELAY__RATE_LIMIT__BURST", "100000"),
            var("RELAY__RATE_LIMIT__PER_SECOND", "100000"),
        ]));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting; this file reads tables and registers an endpoint.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks anything of it: {}",
            serving.body
        );

        let registered =
            app.client()
                .post_json("/endpoints", ACME.1, &json!({ "url": proxy.url("/hook") }));
        assert_eq!(
            registered.status,
            201,
            "the fixture registers the proxy `{}`: {}",
            proxy.url("/hook"),
            registered.body
        );
        Self {
            app,
            postgres,
            target,
            proxy,
        }
    }

    /// Accept `count` events, each under an idempotency key of its own: one delivery each.
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

    /// The pool the worker runs on: the service's own connection string.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }

    /// A sender that speaks HTTP through the proxy, with the deadlines the test names.
    fn sender(&self, policy: Policy) -> Arc<dyn Sender> {
        Arc::new(HttpSender::new(policy))
    }

    /// One turn of the worker, over this fixture's queue.
    async fn tick(&self, pool: &PgPool, sender: &Arc<dyn Sender>) -> Tick {
        delivery::tick_with_budget(pool, Arc::clone(sender), WORKER, &budget())
            .await
            .expect("one turn of the worker runs")
    }

    /// Make every delivery that is waiting for its next turn due now, the way an injected clock
    /// would: the backoff of `relay/2.8-retry-budget` is not what this file measures.
    fn fast_forward(&self) {
        self.postgres.execute(
            "update deliveries set next_attempt_at = now() - interval '1 second' \
              where state in ('pending', 'retrying')",
        );
    }

    /// One question, answered as a number.
    fn count(&self, sql: &str) -> i64 {
        self.text(sql)
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("`{sql}` answers a number: {error}"))
    }

    /// One question, answered as text.
    fn text(&self, sql: &str) -> String {
        self.postgres.query(sql)
    }

    /// Every delivery's state, one word each: what a failure message can show.
    fn states(&self) -> String {
        self.text("select state from deliveries order by id")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Run turns until nothing is owed, and answer how many it took.
    ///
    /// Bounded on purpose: a queue that never drains is the failure this file is about, and a test
    /// that waited for ever would report the defect as a hang.
    async fn drain(&self, pool: &PgPool, sender: &Arc<dyn Sender>) -> usize {
        for round in 1..=DRAIN_TURNS {
            self.fast_forward();
            self.tick(pool, sender).await;
            if self.count("select count(*) from deliveries where state in ('pending', 'retrying')")
                == 0
            {
                return round;
            }
        }
        panic!(
            "the queue did not drain in {DRAIN_TURNS} turns: the deliveries are `{}`, and the \
             subscriber saw {} request(s)",
            self.states(),
            self.target.requests().len()
        );
    }
}

/// The policy these tests deliver with: the deadlines are short, because a test that waits for the
/// production ones is a test nobody runs.
///
/// The read deadline and the attempt deadline are separate arguments because which one fires *is*
/// what two of these tests are about: a slow subscriber trips the read deadline, and an attempt
/// that is still going after both of its hops trips the outer one.
fn policy(read: Duration, deadline: Duration) -> Policy {
    Policy {
        connect: Duration::from_secs(2),
        read,
        deadline,
        ..Policy::default()
    }
}

/// The budget every turn in this file runs with.
///
/// One field is changed, and the reason is the step: the breaker of `relay/2.8-retry-budget` *sheds*
/// attempts once an endpoint has failed a few times, and a shed attempt is not a fault this file
/// injected — it would make the counters meaningless and the deliveries look lost. What the breaker
/// does about a flapping endpoint is that step's subject, not this one's.
fn budget() -> Budget {
    Budget {
        breaker: Breaker::new(BreakerPolicy {
            failures: 1000,
            cooldown: Duration::from_secs(1),
        }),
        ..Budget::default()
    }
}

/// A delay the attempt outlives is absorbed: one attempt, one delivery, no retry.
#[tokio::test]
async fn a_delay_inside_the_deadline_is_absorbed() {
    let fixture = Fixture::start();
    fixture.accept("slow-but-here", 1);
    fixture
        .proxy
        .inject(Fault::Delay(Duration::from_millis(150)));
    let pool = fixture.pool().await;
    let sender = fixture.sender(policy(Duration::from_secs(5), Duration::from_secs(5)));

    let tick = fixture.tick(&pool, &sender).await;
    assert_eq!(
        tick.sent, 1,
        "the answer arrived inside the deadline, so this is an ordinary delivery: {tick:?}"
    );
    assert_eq!(
        fixture.states(),
        "sent",
        "and the delivery is done, not owed again: a service that retried every slow subscriber \
         would deliver twice as often as it was asked to"
    );
    assert_eq!(
        fixture.count("select attempts from deliveries"),
        0,
        "a success does not spend an attempt"
    );
    assert_eq!(
        fixture.target.requests().len(),
        1,
        "the subscriber was told once"
    );
    assert_eq!(
        (fixture.proxy.forwarded(), fixture.proxy.dropped()),
        (1, 0),
        "and the proxy forwarded the one attempt it saw: nothing here was injected"
    );
}

/// A delay the attempt does not outlive is a retry with a wait written on it.
#[tokio::test]
async fn a_delay_past_the_deadline_spends_an_attempt_and_the_delivery_comes_back() {
    let fixture = Fixture::start();
    fixture.accept("too-slow", 1);
    fixture
        .proxy
        .inject(Fault::Delay(Duration::from_millis(1500)));
    let pool = fixture.pool().await;
    // The read deadline is longer than the attempt's, so the deadline this test reads about is the
    // outer one: an attempt that has not finished after every hop it was allowed is the failure a
    // budget bounds (`relay/2.7-outbound-client`).
    let sender = fixture.sender(policy(Duration::from_secs(5), Duration::from_millis(300)));

    let tick = fixture.tick(&pool, &sender).await;
    assert_eq!(tick.retried, 1, "{tick:?}");
    assert_eq!(
        fixture.states(),
        "retrying",
        "the delivery is owed again rather than lost or buried"
    );
    assert_eq!(
        fixture.count("select attempts from deliveries"),
        1,
        "and the attempt that ran out of time is counted: the count is the only thing that ends a \
         delivery that keeps failing (`relay/2.10-dlq`)"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where next_attempt_at > now()"),
        1,
        "the next attempt is not now: the wait is the budget's answer (`relay/2.8-retry-budget`)"
    );
    let why = fixture.text("select last_error from deliveries");
    assert!(
        why.contains("did not finish within"),
        "and the row says which deadline it lost, so an operator can read it: `{why}`"
    );

    // Heal the fault, and the same delivery goes out on its second attempt and is taken.
    fixture.proxy.heal();
    let turns = fixture.drain(&pool, &sender).await;
    assert_eq!(fixture.states(), "sent", "after {turns} turn(s)");
    assert!(
        !fixture.target.requests().is_empty(),
        "the subscriber was reached once the answer was allowed to arrive"
    );
}

/// A connection that is reset is a delay, not a loss: the service heals when the fault stops.
#[tokio::test]
async fn a_reset_connection_is_retried_and_the_service_heals() {
    let fixture = Fixture::start();
    fixture.accept("reset", 3);
    fixture.proxy.inject(Fault::Reset);
    let pool = fixture.pool().await;
    let sender = fixture.sender(policy(Duration::from_secs(2), Duration::from_secs(2)));

    for _ in 0..2 {
        fixture.fast_forward();
        fixture.tick(&pool, &sender).await;
    }
    assert_eq!(
        fixture.states(),
        "retrying retrying retrying",
        "every delivery met the fault and is owed again"
    );
    assert_eq!(
        fixture.count("select sum(attempts) from deliveries"),
        6,
        "two attempts each, and each one counted"
    );
    assert_eq!(
        (fixture.proxy.dropped(), fixture.proxy.forwarded()),
        (6, 0),
        "the fault is what happened: a proxy that had forwarded these would make this test about \
         nothing"
    );
    assert_eq!(
        fixture.target.requests().len(),
        0,
        "the subscriber never heard about any of them"
    );
    let why = fixture.text("select last_error from deliveries limit 1");
    assert!(
        why.contains("connection closed")
            || why.contains("connection broke")
            || why.contains("writing the request"),
        "and the failure is written down in the words of what happened to the socket: `{why}`"
    );

    // The fault stops. Nothing is lost, and nothing has to be replayed by hand.
    fixture.proxy.heal();
    let turns = fixture.drain(&pool, &sender).await;
    assert_eq!(
        fixture.states(),
        "sent sent sent",
        "after {turns} turn(s): a reset is a delay, which is what at-least-once means"
    );
    assert_eq!(
        fixture.target.requests().len(),
        3,
        "the subscriber was reached once per delivery"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'claimed'"),
        0,
        "and no row is left invisible by a lease: the service healed, it did not wait one out"
    );
}

/// A subscriber that takes the request and goes quiet is cut by the deadline, not by luck.
#[tokio::test]
async fn a_half_open_connection_is_cut_by_the_deadline() {
    let fixture = Fixture::start();
    fixture.accept("half-open", 1);
    fixture.proxy.inject(Fault::HalfOpen);
    let pool = fixture.pool().await;
    // Short read deadline, long attempt deadline: a connection that is *silent* is ended by the
    // read deadline, and the test can tell one deadline from the other by what the row says.
    let sender = fixture.sender(policy(Duration::from_millis(300), Duration::from_secs(5)));

    let started = Instant::now();
    let tick = fixture.tick(&pool, &sender).await;
    let took = started.elapsed();
    assert_eq!(tick.retried, 1, "{tick:?}");
    assert!(
        took < Duration::from_secs(10),
        "the turn ended because a deadline ended it: a client with no read deadline does not fail \
         this test, it hangs in it ({took:?})"
    );
    assert_eq!(
        fixture.target.requests().len(),
        1,
        "the subscriber had the request — this is not a connection that never arrived, it is one \
         that never answers"
    );
    assert_eq!(
        (fixture.proxy.forwarded(), fixture.proxy.dropped()),
        (1, 0),
        "the proxy delivered it and passed nothing back"
    );
    let why = fixture.text("select last_error from deliveries");
    assert!(
        why.contains("sent nothing for"),
        "and the row says the read deadline fired, which is the only thing that can end this \
         failure: `{why}`"
    );
    assert_eq!(
        fixture.states(),
        "retrying",
        "the delivery is owed again rather than stuck in flight"
    );

    fixture.proxy.heal();
    fixture.drain(&pool, &sender).await;
    assert_eq!(fixture.states(), "sent");
}

/// Loss is injected *deterministically*: the same run drops the same connections.
#[tokio::test]
async fn a_dropped_connection_is_retryable_and_the_loss_is_reproducible() {
    let fixture = Fixture::start();
    fixture.accept("loss", 4);
    fixture.proxy.inject(Fault::Loss { every: 2 });
    let pool = fixture.pool().await;
    let sender = fixture.sender(policy(Duration::from_secs(2), Duration::from_secs(2)));

    let tick = fixture.tick(&pool, &sender).await;
    assert_eq!(tick.claimed, 4, "{tick:?}");
    assert_eq!(
        fixture.proxy.dropped(),
        2,
        "every second request is taken, and the same two next time this test runs — a loss rate \
         drawn from a generator is a test people learn to re-run instead of read"
    );
    assert_eq!(
        fixture.proxy.forwarded(),
        2,
        "and the other two reached the subscriber"
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'sent'"),
        2,
        "the two the subscriber was reached for: `{}`",
        fixture.states()
    );
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'retrying'"),
        2,
        "and the two the fault took are owed again: `{}`",
        fixture.states()
    );

    fixture.proxy.heal();
    fixture.drain(&pool, &sender).await;
    assert_eq!(
        fixture.count("select count(*) from deliveries where state = 'sent'"),
        4,
        "all four end as delivered, and the two that met the fault cost one extra attempt each"
    );
    assert_eq!(
        fixture.count("select sum(attempts) from deliveries"),
        2,
        "the total attempts spent on retries: the two dropped requests were the only failures"
    );
}

/// Every fault class, one after another: after the fault is healed, nothing is still owed.
#[tokio::test]
async fn nothing_is_lost_under_any_fault_in_the_matrix() {
    for fault in [
        Fault::None,
        Fault::Delay(Duration::from_millis(400)),
        Fault::Reset,
        Fault::HalfOpen,
        Fault::Loss { every: 2 },
    ] {
        let fixture = Fixture::start();
        fixture.accept("matrix", 2);
        fixture.proxy.inject(fault);
        let pool = fixture.pool().await;
        // A deadline short enough that a half-open connection cannot outlive two turns.
        let sender = fixture.sender(policy(
            Duration::from_millis(200),
            Duration::from_millis(500),
        ));

        for _ in 0..2 {
            fixture.fast_forward();
            fixture.tick(&pool, &sender).await;
        }
        fixture.proxy.heal();
        fixture.drain(&pool, &sender).await;

        let accepted = fixture.count("select count(*) from deliveries");
        assert_eq!(
            accepted,
            2,
            "{}: one delivery per accepted event, whatever the network did: `{}`",
            fault.as_str(),
            fixture.states()
        );
        assert_eq!(
            fixture.count("select count(*) from deliveries where state = 'sent'"),
            2,
            "{}: two turns are not enough to bury a delivery, so every accepted event is delivered \
             — and none of them is still owed, which is what at-least-once promises: `{}`",
            fault.as_str(),
            fixture.states()
        );
        assert_eq!(
            fixture.count("select count(*) from deliveries where state = 'claimed'"),
            0,
            "{}: and no row is left invisible: `{}`",
            fault.as_str(),
            fixture.states()
        );
        assert!(
            fixture.proxy.forwarded() + fixture.proxy.dropped() >= 2,
            "{}: the faults were injected on the path the service uses ({} attempt(s) through the \
             proxy: {} forwarded, {} taken)",
            fault.as_str(),
            fixture.proxy.forwarded() + fixture.proxy.dropped(),
            fixture.proxy.forwarded(),
            fixture.proxy.dropped()
        );
    }
}

/// A fault that never stops does not retry for ever: the budget buries the delivery, with its reason.
#[tokio::test]
async fn a_fault_that_never_stops_ends_the_deliveries_in_the_dead_letter_queue() {
    let fixture = Fixture::start();
    fixture.accept("permanent", 1);
    fixture.proxy.inject(Fault::HalfOpen);
    let pool = fixture.pool().await;
    let sender = fixture.sender(policy(Duration::from_millis(150), Duration::from_secs(5)));

    // Five attempts is what a deployment gets when it configures nothing (`Budget::default`), and
    // the turns are fast-forwarded so the test does not wait for the backoff.
    for _ in 0..6 {
        fixture.fast_forward();
        fixture.tick(&pool, &sender).await;
    }

    assert_eq!(
        fixture.states(),
        "dead",
        "the delivery stops being owed when its attempts run out: retrying a subscriber that has \
         not answered five times is a subscriber that never learns the delivery is not coming"
    );
    assert_eq!(
        fixture.count("select attempts from deliveries"),
        5,
        "and the count is the budget's"
    );
    assert_eq!(
        fixture.count("select count(*) from dead_letters"),
        1,
        "the life that ended is written down where an operator looks (`relay/2.10-dlq`)"
    );
    let reason = fixture.text("select reason from dead_letters");
    assert!(
        reason.contains("gave up after 5 attempts"),
        "with the sentence that says so: `{reason}`"
    );
    assert!(
        reason.contains("sent nothing for"),
        "and the failure that killed it, in the words of what happened: `{reason}`"
    );
    assert_eq!(
        fixture.proxy.forwarded(),
        5,
        "five requests reached the subscriber, and the sixth attempt did not happen"
    );
}
