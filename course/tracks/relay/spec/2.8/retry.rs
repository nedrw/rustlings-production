//! Spec for `relay/2.8-retry-budget`: how long a failed delivery waits, how many attempts it gets,
//! and when this service stops talking to an endpoint.
//!
//! `kiln run` copies this file into the project as `tests/spec_retry.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Three properties run through it, and each one is a decision the
//! step's own record argues for:
//!
//! * **the wait is a function of the delivery, not of the clock.** `Budget::delay` is deterministic
//!   for one `(delivery, attempt)` pair, which is what makes a retry reproducible in a test and in a
//!   log — and what lets this file move time by *passing* it instead of pausing a runtime;
//! * **the wait is never zero.** Full jitter spreads deliveries that failed together over the whole
//!   window, zero included; the spec of `relay/2.6-at-least-once` then fails, because it asks a
//!   worker for a second turn *immediately* after a failure and expects the delivery to be waiting.
//!   The window this budget spreads over therefore starts above zero;
//! * **an endpoint that is not answering is cut off before the queue is spent on it.** The breaker's
//!   whole purpose is that an attempt it sheds is not an attempt: the row keeps its count and its
//!   place. The last two tests measure that on real rows.
//!
//! Nothing here sleeps: the pure properties pass instants in, and the two tests that run a worker
//! use the fixture's own `UPDATE` to make a retry due (the same trick `relay/2.6` uses).

mod support;

#[path = "support/sender.rs"]
mod sender;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Duration as Chrono, Utc};
use relay::backoff::{Admission, Breaker, BreakerPolicy, Budget, State};
use relay::db::Db;
use relay::delivery::{self, SendError};
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;
use uuid::Uuid;

use sender::Recording;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// The subscriber the DB tests register. Nothing dials it: the sender is the fixture, and the
/// target only has to be a URL this service accepts.
const HOOK: &str = "http://127.0.0.1:9/hook";
/// The decision record this step delivers, relative to the project root.
const RECORD: &str = "docs/adr/0002-retry-budget.md";
/// A fixed instant, so that every test in this file is about the policy and never about "now".
const NOW: i64 = 1_760_000_000;

/// The instant `NOW` names.
fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(NOW, 0).expect("the fixture's instant is a real one")
}

/// A budget that is not the default: small numbers, so a test can count what it did.
fn budget(attempts: i32, failures: u32) -> Budget {
    Budget {
        max_attempts: attempts,
        base: Duration::from_secs(30),
        cap: Duration::from_secs(6 * 60 * 60),
        breaker: Breaker::new(BreakerPolicy {
            failures,
            cooldown: Duration::from_secs(30),
        }),
    }
}

/// A deterministic series of delivery ids: the fixture is not allowed an `Rng` (`proptest` is
/// `relay/4.13`), and a fixed list is what makes "sixty deliveries" reproducible.
///
/// The same twelve-line generator `relay/2.4-cursor-pagination` uses: splitmix64 over a seed, so
/// two runs of this file look at exactly the same ids.
struct Ids(u64);

impl Ids {
    fn new() -> Self {
        Self(0x5eed_1234_5678_9abc)
    }

    fn next(&mut self) -> Uuid {
        let mut low = self.mix();
        let mut high = self.mix();
        // A version 4 UUID, so the ids this file signs are the shape a real delivery id has.
        high = (high & 0xffff_ffff_ffff_0fff) | 0x0000_0000_0000_4000;
        low = (low & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
        Uuid::from_u64_pair(high, low)
    }

    fn mix(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// The window an exponential wait with jitter may fall in: half of `base * 2^(attempt-1)`, up to
/// that bound, and never past the cap.
fn window(base: Duration, cap: Duration, attempt: i32) -> (Duration, Duration) {
    let doublings = u32::try_from(attempt.max(1) - 1).unwrap_or(0).min(31);
    let bound = base
        .saturating_mul(1u32 << doublings)
        .min(cap)
        .max(Duration::from_nanos(2));
    (bound / 2, bound)
}

/// One `(delivery, attempt)` pair's wait is always the same wait.
#[test]
fn the_same_attempt_of_the_same_delivery_waits_the_same_time() {
    let first = Budget::default();
    let second = Budget::default();
    let mut ids = Ids::new();

    for attempt in 1..=8 {
        let delivery = ids.next();
        let wait = first.delay(delivery, attempt);
        assert_eq!(
            wait,
            first.delay(delivery, attempt),
            "the same attempt of the same delivery has to answer the same wait twice in a row: a \
             jitter drawn from a random number generator makes a retry impossible to reproduce, and \
             a log line that says when the next attempt is becomes a guess"
        );
        assert_eq!(
            wait,
            second.delay(delivery, attempt),
            "two budgets — two workers, two processes — compute the same wait for the same \
             delivery: the jitter decorrelates deliveries, it does not add entropy to the service"
        );
    }
}

/// The wait grows with the attempt number and never grows past the cap.
#[test]
fn the_wait_grows_with_every_attempt_and_stops_at_the_cap() {
    let budget = Budget::default();
    let mut ids = Ids::new();
    let deliveries: Vec<Uuid> = (0..64).map(|_| ids.next()).collect();

    for attempt in 1..=6 {
        let (floor, ceiling) = window(budget.base, budget.cap, attempt);
        for delivery in &deliveries {
            let wait = budget.delay(*delivery, attempt);
            assert!(
                (floor..=ceiling).contains(&wait),
                "attempt {attempt} waits {wait:?}, which is outside the window {floor:?}..={ceiling:?} \
                 the attempt number asks for: the base wait doubles with every failure and the \
                 jitter spreads the fleet across half of that window, not all of it and not past it"
            );
        }
    }

    // The far end: an attempt number no delivery will ever reach in production, because the budget
    // gives up long before it. `1 << attempt` overflows a `u32` there, and a wait that wraps around
    // is a wait of almost zero — the one thing a backoff must never be.
    for attempt in [32, 40, 64, 1_000] {
        let (floor, ceiling) = window(budget.base, budget.cap, attempt);
        for delivery in &deliveries {
            let wait = budget.delay(*delivery, attempt);
            assert!(
                (floor..=ceiling).contains(&wait),
                "attempt {attempt} waits {wait:?}, which is outside {floor:?}..={ceiling:?}: the \
                 doubling stops at the cap instead of wrapping around, and a delay of `1 << 40` in \
                 a `u32` is not a long wait — it is a short one"
            );
        }
    }

    let (short, _) = window(budget.base, budget.cap, 1);
    let (_, long) = window(budget.base, budget.cap, 4);
    assert!(
        budget.cap >= long && budget.cap <= Duration::from_secs(24 * 60 * 60),
        "the cap is {cap:?}: it is the longest a delivery can be parked for, so it has to be at \
         least the fourth attempt's window ({long:?}) and inside a day. A cap of days is a delivery \
         that arrives late enough to be a loss, and a cap below the base is a cap that is really the \
         base",
        cap = budget.cap
    );
    assert!(
        short > Duration::ZERO,
        "the first attempt waits at least {short:?}"
    );
}

/// A wait is never zero, and never longer than the cap.
#[test]
fn a_wait_is_never_zero_and_never_longer_than_the_cap() {
    let budget = Budget::default();
    let mut ids = Ids::new();
    let mut shortest = Duration::MAX;

    for _ in 0..256 {
        let delivery = ids.next();
        for attempt in 1..=10 {
            let wait = budget.delay(delivery, attempt);
            assert!(
                wait > Duration::ZERO,
                "{delivery} waits {wait:?} after attempt {attempt}: a retry that fires in the same \
                 millisecond as the failure is a spin, not a backoff — and the whole point of \
                 spreading a fleet out is lost if the spread starts at nothing"
            );
            assert!(
                wait <= budget.cap,
                "{delivery} waits {wait:?} after attempt {attempt}, which is past the cap {:?}",
                budget.cap
            );
            shortest = shortest.min(wait);
        }
    }
    assert!(
        shortest >= budget.base / 2,
        "the shortest wait over 2560 draws is {shortest:?} and the base is {:?}: the jitter spreads \
         deliveries over the upper half of the window, so that a fleet that failed together does not \
         retry together *and* a failure does not retry immediately",
        budget.base
    );
}

/// Deliveries that failed at the same instant do not come back at the same instant.
#[test]
fn deliveries_that_failed_together_do_not_wait_together() {
    let budget = Budget::default();
    let mut ids = Ids::new();
    let mut waits = Vec::new();
    for _ in 0..60 {
        waits.push(budget.delay(ids.next(), 3));
    }

    let mut distinct = waits.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() >= 50,
        "sixty deliveries failed at the same instant and answered {} distinct waits ({distinct:?} \
         of {}): without jitter they all retry in the same millisecond, which is the thundering herd \
         this budget exists to avoid",
        distinct.len(),
        waits.len()
    );

    let shortest = waits.iter().min().copied().unwrap_or_default();
    let longest = waits.iter().max().copied().unwrap_or_default();
    let (floor, ceiling) = window(budget.base, budget.cap, 3);
    assert!(
        longest.saturating_sub(shortest) >= (ceiling - floor) / 2,
        "the sixty waits sit between {shortest:?} and {longest:?}, which is less than half of the \
         window {floor:?}..={ceiling:?}: a jitter that is deterministic is not a jitter that is \
         constant — the hash has to move across the window, or every delivery gets the same wait"
    );
}

/// Consecutive failures open the breaker; a success closes it and forgets the failures.
#[test]
fn consecutive_failures_open_the_breaker_and_a_success_closes_it() {
    let breaker = Breaker::new(BreakerPolicy {
        failures: 3,
        cooldown: Duration::from_secs(30),
    });
    let endpoint = HOOK;

    for opened in 1..3 {
        breaker.failed(endpoint, now());
        assert_eq!(
            breaker.admit(endpoint, now()),
            Admission::Try,
            "attempts ({opened}) attempt the endpoint again: a breaker opens on \
             `BreakerPolicy::failures` failures in a row, not on the first one — a subscriber that \
             hiccuped once is not a subscriber that is gone"
        );
        assert_eq!(
            breaker.state(endpoint, now()),
            State::Closed,
            "and it is not cut off, so an attempt is an ordinary attempt"
        );
    }

    breaker.failed(endpoint, now());
    assert_eq!(
        breaker.state(endpoint, now()),
        State::Open,
        "three failures in a row is the endpoint being broken, and the breaker says so"
    );
    assert_eq!(
        breaker.admit(endpoint, now()),
        Admission::Shed,
        "while it is open, an attempt is shed: not made and not counted, because it is the attempt \
         the deliveries behind it are paying for"
    );

    // A successful attempt is the only thing that says the endpoint is back.
    breaker.succeeded(endpoint);
    assert_eq!(
        breaker.state(endpoint, now()),
        State::Closed,
        "a success closes the breaker"
    );
    breaker.failed(endpoint, now());
    breaker.failed(endpoint, now());
    assert_eq!(
        breaker.admit(endpoint, now()),
        Admission::Try,
        "two failures after a success are two failures, not a continuation of the three before it: \
         the count is *consecutive* failures, and a breaker that only ever counts up never closes"
    );
}

/// The cooldown lets exactly one probe through, and the probe decides what happens next.
#[test]
fn the_cooldown_lets_exactly_one_probe_through() {
    let policy = BreakerPolicy {
        failures: 2,
        cooldown: Duration::from_secs(30),
    };
    let open_at = now();
    let breaker = Breaker::new(policy);
    breaker.failed(HOOK, open_at);
    breaker.failed(HOOK, open_at);

    let inside = open_at + Chrono::seconds(29);
    assert_eq!(
        breaker.admit(HOOK, inside),
        Admission::Shed,
        "the cooldown is thirty seconds and it is twenty-nine: the endpoint is still cut off"
    );

    let cooled = open_at + Chrono::seconds(30);
    assert_eq!(
        breaker.admit(HOOK, cooled),
        Admission::Probe,
        "once the cooldown has passed, one attempt is let through to find out whether the endpoint \
         is back"
    );
    assert_eq!(
        breaker.admit(HOOK, cooled),
        Admission::Shed,
        "and exactly one: a half-open breaker that lets every attempt through is an open breaker \
         for a slow endpoint, which is the endpoint that needed the breaker most"
    );
    assert_eq!(
        breaker.state(HOOK, cooled),
        State::HalfOpen,
        "the state says what is happening: the probe is out and its answer decides the next window"
    );

    // The probe failed: the endpoint is opened again, for a fresh cooldown.
    breaker.failed(HOOK, cooled);
    assert_eq!(
        breaker.state(HOOK, cooled),
        State::Open,
        "a failed probe is a failure"
    );
    let soon = cooled + Chrono::seconds(29);
    assert_eq!(
        breaker.admit(HOOK, soon),
        Admission::Shed,
        "and the cooldown starts again from the probe, rather than from the failure that opened \
         the breaker the first time"
    );
    let next = cooled + Chrono::seconds(30);
    assert_eq!(
        breaker.admit(HOOK, next),
        Admission::Probe,
        "one more probe"
    );

    // The probe succeeded: the endpoint is back, and ordinary attempts resume.
    breaker.succeeded(HOOK);
    assert_eq!(
        breaker.state(HOOK, next),
        State::Closed,
        "the probe found the endpoint back"
    );
    assert_eq!(
        breaker.admit(HOOK, next),
        Admission::Try,
        "and attempts are attempted again"
    );
}

/// A probe nobody ever reported does not leave an endpoint cut off forever.
#[test]
fn a_probe_that_is_never_reported_is_presumed_lost() {
    let policy = BreakerPolicy {
        failures: 1,
        cooldown: Duration::from_secs(10),
    };
    let open_at = now();
    let breaker = Breaker::new(policy);
    breaker.failed(HOOK, open_at);

    let probe = open_at + Chrono::seconds(10);
    assert_eq!(
        breaker.admit(HOOK, probe),
        Admission::Probe,
        "the first probe"
    );

    // The worker holding the probe died, and nothing ever reports it. The next window is another
    // one, because a breaker whose only probe can be lost is a breaker that never closes.
    assert_eq!(
        breaker.admit(HOOK, probe + Chrono::seconds(9)),
        Admission::Shed,
        "an unreported probe holds the endpoint for its own cooldown, like any other attempt"
    );
    assert_eq!(
        breaker.admit(HOOK, probe + Chrono::seconds(10)),
        Admission::Probe,
        "and then the breaker asks again: the alternative is an endpoint that is cut off for good \
         because one process died holding the only probe it will ever be given"
    );
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
        ]));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting. This file reads the tables, so it waits for an answer only
        // the running service can give.
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

    /// Accept one event under `key`.
    fn accept(&self, key: &str) -> Response {
        self.app.client().send(
            Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", key)
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
        )
    }

    /// The pool the worker runs on: the service's own policy, so a turn in a test uses the
    /// connections a turn in production uses.
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

    /// Make every delivery that is waiting to be retried due again, the way an injected clock would.
    fn move_the_clock_forward(&self) {
        self.postgres.execute(
            "update deliveries set next_attempt_at = now() - interval '1 second' \
              where state = 'retrying'",
        );
    }

    /// What the deliveries table holds, by state and failure count: `pending|0|13`.
    fn progress(&self) -> String {
        self.query(
            "select state || '|' || attempts || '|' || count(*) from deliveries \
              group by state, attempts order by state, attempts",
        )
    }
}

/// The default budget of this service still gives up on a delivery the step before measured.
///
/// `relay/2.6-at-least-once` counts turns and expects a delivery that keeps failing to be buried
/// inside twelve of them. The budget that replaced its constant has to keep that promise, and this
/// test is the place the promise is kept: it is the same loop, run against whatever the default
/// budget now says.
#[tokio::test]
async fn the_default_budget_gives_up_inside_the_turns_the_step_before_measured() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.register(HOOK);
    assert_eq!(fixture.accept("key-1").status, 201);
    let sender = Recording::always(SendError::Retryable("the subscriber timed out".to_owned()));

    let mut rounds = 0;
    while rounds == 0 || fixture.query("select state from deliveries") == "retrying" {
        rounds += 1;
        assert!(
            rounds <= 12,
            "the default budget has to give up on a delivery inside twelve turns — that is what \
             `relay/2.6-at-least-once` measured, and a budget that spends more turns than that is a \
             subscriber that never learns the delivery is not coming. After {rounds} turns the row is \
             `{}` with {} failures",
            fixture.query("select state from deliveries"),
            fixture.query("select attempts from deliveries")
        );
        delivery::tick(&pool, &sender, "worker-a")
            .await
            .expect("the worker runs");
        fixture.move_the_clock_forward();
    }

    assert_eq!(
        fixture.query("select state from deliveries"),
        "dead",
        "out of attempts, with the count that got it there"
    );
    let attempts: i32 = fixture
        .query("select attempts from deliveries")
        .parse()
        .expect("attempts is an integer");
    assert!(
        attempts >= 2,
        "giving up after one failure is not a budget: {attempts} attempts"
    );
}

/// While the breaker is open, the deliveries behind the broken endpoint keep their attempts.
#[tokio::test]
async fn a_broken_endpoint_stops_spending_the_attempts_of_the_deliveries_behind_it() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.register(HOOK);
    for index in 0..20 {
        assert_eq!(
            fixture.accept(&format!("key-{index}")).status,
            201,
            "twenty deliveries, all due at once"
        );
    }
    assert_eq!(
        fixture.progress(),
        "pending|0|20",
        "nothing has been attempted yet"
    );

    // Three attempts, and a breaker that opens on the third: the endpoint is broken and the budget
    // knows it after three requests instead of sixteen.
    let budget = budget(3, 3);
    let sender = Recording::always(SendError::Retryable("the subscriber timed out".to_owned()));
    let sender: Arc<dyn delivery::Sender> = Arc::new(sender);
    let first = delivery::tick_with_budget(&pool, sender.clone(), "worker-a", &budget)
        .await
        .expect("the worker runs");

    assert_eq!(
        first.shed, 13,
        "one turn claims a batch of sixteen, three of them learn that the endpoint is not \
         answering, and the other thirteen are shed: not attempted, not counted. The turn reported \
         {first:?}"
    );
    assert_eq!(
        (first.retried, first.dead),
        (3, 0),
        "the three attempts that were made failed in a way that is worth another attempt: {first:?}"
    );
    assert_eq!(
        fixture.progress(),
        "pending|0|17\nretrying|1|3",
        "and the rows say the same thing: the thirteen behind the breaker are untouched — a state, \
         a failure count of zero, still owed — plus the four the claim never reached, while the \
         three that were tried carry their one failure"
    );
    assert_eq!(
        fixture.query("select count(*) from deliveries where state = 'claimed'"),
        "0",
        "a shed delivery is handed straight back to the queue: a row left `claimed` is a delivery \
         nobody will look at until its lease runs out"
    );

    // A second turn, immediately: the breaker is still open, so it spends nothing at all.
    let second = delivery::tick_with_budget(&pool, sender, "worker-b", &budget)
        .await
        .expect("the worker runs");
    assert!(
        second.claimed > 0,
        "the turn found work: the thirteen rows are back in the queue and due: {second:?}"
    );
    assert_eq!(
        (second.sent, second.retried, second.dead),
        (0, 0, 0),
        "and attempted none of it, because the endpoint is still cut off: {second:?}"
    );
    assert_eq!(
        fixture.progress(),
        "pending|0|17\nretrying|1|3",
        "nothing moved: three attempts were spent on a broken endpoint, and the seventeen deliveries \
         waiting behind them cost nothing while it stays broken. This is the whole difference \
         between a retry budget and a retry loop"
    );
}

/// The decision, written down where a reader can disagree with it.
#[test]
fn the_decision_record_names_the_budget_and_links_what_it_talks_about() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let page = root.join(RECORD);
    let text = std::fs::read_to_string(&page).unwrap_or_else(|error| {
        panic!(
            "this step's deliverable includes the decision record `{RECORD}` (`{}`): {error}. The \
             argument for this budget — the jitter, the cap, the breaker — is a page a reviewer \
             reads, not a comment nobody opens",
            page.display()
        )
    });
    let lowered = text.to_lowercase();
    for section in ["decision", "alternatives", "consequences"] {
        assert!(
            lowered.contains(&format!("\n## {section}")),
            "`{RECORD}` needs a `## {section}` section: what was decided, what else could have been \
             decided, and what the decision costs. A record without the alternatives is a \
             description, and a description cannot be argued with"
        );
    }
    for word in ["jitter", "breaker"] {
        assert!(
            lowered.contains(word),
            "`{RECORD}` has to name the `{word}`: the two mechanisms this step adds are the \
             decision, and a reader has to be able to find out what they cost"
        );
    }

    let targets = links(&text);
    assert!(
        !targets.is_empty(),
        "`{RECORD}` links nothing: a record is read next to the code it is about, and the reader \
         needs a way to get there"
    );
    let directory = page.parent().unwrap_or(&root);
    for target in &targets {
        if target.starts_with('#') || target.contains("://") || target.starts_with("mailto:") {
            continue;
        }
        let path = target.split('#').next().unwrap_or(target);
        assert!(
            directory.join(path).exists(),
            "`{RECORD}` links `{target}`, which is not in this project (from `{}`). A record whose \
             links do not resolve sends a reader looking for a file that was renamed away",
            directory.display()
        );
    }
    assert!(
        targets.iter().any(|target| target.contains("src/")),
        "`{RECORD}` links the code it is about (a page under `src/`): the links are what make it a \
         record of *this* service rather than an essay. Targets: {targets:?}"
    );
}

/// Every target a markdown page links, as written.
fn links(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once("](") {
        let Some((target, tail)) = after.split_once(')') else {
            break;
        };
        let target = target.split_whitespace().next().unwrap_or_default().trim();
        if !target.is_empty() {
            targets.push(target.to_owned());
        }
        rest = tail;
    }
    targets
}
