//! Spec for `relay/2.6-at-least-once`: what the service promises a subscriber, and what it does
//! when the subscriber — or the service itself — fails.
//!
//! `kiln run` copies this file into the project as `tests/spec_at_least_once.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Every judgement here is made from outside the module:
//!
//! * **through the API**, for what the service accepted. An accepted event is owed to every
//!   endpoint its tenant has, and a *replayed* event (the same `Idempotency-Key`) is not a second
//!   debt: the fan-out belongs to the acceptance, not to the request;
//! * **through the sender**, for what a subscriber was handed. `tests/support/sender.rs` is a
//!   subscriber a test controls: it records every request, and it fails, parks or takes them;
//! * **through `psql`**, for what the service recorded. `state`, `attempts` and `last_error` are the
//!   promise's public half: "nothing was dropped" is a claim about rows, not about answers.
//!
//! The property the whole step is about is one sentence: **a delivery is forgotten only when a
//! subscriber has taken it, or when the worker has given up on it** — never because something
//! crashed. The last test measures the price of that: between the send and the record there is a
//! window, and a delivery that comes back through it is sent twice. That is allowed, and the
//! decision record has to say so.
//!
//! Nothing here sleeps: a retry that has to become due again is moved by the fixture (the same
//! `UPDATE` an injected clock would do), and the one interruption is a worker whose future is
//! dropped while a parked sender holds it.

mod support;

#[path = "support/sender.rs"]
mod sender;

use std::path::PathBuf;

use relay::claim;
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
/// Four subscribers. Nothing dials them: the sender is the fixture, and the target only has to be a
/// URL this service accepts and a name a test can talk about.
const HOOK_A: &str = "http://127.0.0.1:9/hook-a";
const HOOK_B: &str = "http://127.0.0.1:9/hook-b";
const HOOK_C: &str = "http://127.0.0.1:9/hook-c";
const HOOK_D: &str = "http://127.0.0.1:9/hook-d";
/// The decision record this step delivers, relative to the project root.
const RECORD: &str = "docs/adr/0001-at-least-once.md";

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
        // the running service can give — and a failure here says what was waited for.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks its database anything: {}",
            serving.body
        );
        Self { app, postgres }
    }

    /// Register a subscriber for the fixture's tenant.
    fn register(&self, url: &str) -> String {
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
        answer
            .field("id")
            .as_str()
            .expect("an endpoint answers with its id")
            .to_owned()
    }

    /// Accept one event under `key`. The same key twice is one event (step `relay/2.2`).
    fn accept(&self, key: &str) -> Response {
        self.app.client().send(
            Request::post("/events", Vec::new())
                .bearer(ACME.1)
                .header("idempotency-key", key)
                .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
        )
    }

    /// The pool the worker runs on: the service's own policy, so a tick in a test uses the
    /// connections a tick in production uses.
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

    /// What the deliveries table holds, by state: `sent|2`.
    fn states(&self) -> String {
        self.query("select state, count(*) from deliveries group by state order by state")
    }

    /// State and failure count of the one delivery in the table: `retrying|2`.
    fn delivery(&self) -> String {
        self.query("select state || '|' || attempts from deliveries")
    }

    /// Make every delivery that is waiting to be retried due again, the way an injected clock would.
    fn move_the_clock_forward(&self) {
        self.postgres.execute(
            "update deliveries set next_attempt_at = now() - interval '1 second' \
              where state = 'retrying'",
        );
    }

    /// Turn the worker until it has nothing left to claim, and answer how many turns that took.
    async fn drain(&self, pool: &PgPool, sender: &dyn delivery::Sender) -> usize {
        for turn in 1..=32 {
            let tick = delivery::tick(pool, sender, "worker-drain")
                .await
                .expect("the worker runs");
            let waiting = self.query("select count(*) from deliveries where state = 'retrying'");
            if tick.claimed == 0 && waiting == "0" {
                return turn;
            }
            self.move_the_clock_forward();
        }
        panic!(
            "the worker never ran out of claimable work: {}",
            self.states()
        );
    }
}

/// An accepted event is owed to every endpoint that exists when it arrives — once per event.
#[tokio::test]
async fn an_accepted_event_is_owed_to_every_endpoint_of_its_tenant() {
    let fixture = Fixture::start();
    fixture.register(HOOK_A);
    fixture.register(HOOK_B);
    fixture.register(HOOK_C);

    let accepted = fixture.accept("key-1");
    assert_eq!(
        accepted.status, 201,
        "a well-formed event is accepted: {}",
        accepted.body
    );
    assert_eq!(
        fixture.states(),
        "pending|3",
        "an accepted event is work from the instant it is accepted: one pending delivery per \
         endpoint of its tenant. A fan-out that happens somewhere else — a cron job, a second \
         process, a queue nobody filled — loses the window between accepting and scheduling"
    );
    assert_eq!(
        fixture.query("select count(distinct endpoint_id) from deliveries"),
        "3",
        "and each of them is owed exactly one"
    );

    let replay = fixture.accept("key-1");
    assert_eq!(
        replay.status, 200,
        "the same key is the same event, and it is answered with the event stored then: {}",
        replay.body
    );
    assert_eq!(
        fixture.states(),
        "pending|3",
        "a replay is not new work: a fan-out that runs per *request* rather than per accepted event \
         delivers everything twice whenever a caller retries — and callers retry exactly when they \
         are unsure"
    );

    assert_eq!(
        fixture.accept("key-2").status,
        201,
        "a new key is a new event"
    );
    assert_eq!(
        fixture.states(),
        "pending|6",
        "and its own three deliveries"
    );

    fixture.register(HOOK_D);
    assert_eq!(
        fixture.states(),
        "pending|6",
        "an endpoint is owed the events that arrive while it exists: registering one is not \
         subscribing to history, and a service that owes the past to every new subscriber has to \
         deliver the past to everybody who ever registers"
    );
}

/// A failed attempt leaves the delivery owed, and the next attempt knows it is the next one.
#[tokio::test]
async fn a_failed_attempt_leaves_the_delivery_owed() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.register(HOOK_A);
    assert_eq!(fixture.accept("key-1").status, 201);
    let sender = Recording::scripted(vec![SendError::Retryable(
        "the subscriber was away".to_owned(),
    )]);

    let first = delivery::tick(&pool, &sender, "worker-a")
        .await
        .expect("the worker runs");
    assert_eq!(
        (
            first.claimed,
            first.sent,
            first.retried,
            first.dead,
            first.lost
        ),
        (1, 0, 1, 0, 0),
        "one claimed delivery, one failed attempt, nothing recorded that was not the worker's: \
         {first:?}"
    );
    assert_eq!(
        fixture.delivery(),
        "retrying|1",
        "a failed attempt is not a lost delivery: the row is owed again, with the failure counted. \
         `sent` is the only state that means the subscriber has it"
    );
    assert!(
        fixture
            .query("select last_error from deliveries")
            .contains("away"),
        "the reason is kept where an operator will read it: {}",
        fixture.query("select last_error from deliveries")
    );
    assert_eq!(
        fixture.query(
            "select count(*) from deliveries where locked_by is null and lease_expires_at is null"
        ),
        "1",
        "and a row that is waiting is nobody's: a failure that kept its lease would make the \
         delivery invisible until the lease ran out"
    );
    assert_eq!(
        sender.deliveries().len(),
        1,
        "the subscriber was asked once"
    );
    assert_eq!(
        delivery::tick(&pool, &sender, "worker-b")
            .await
            .expect("the worker runs")
            .claimed,
        0,
        "and is asked again when the retry is due, not before: a retry that ignores \
         `next_attempt_at` is a subscriber hammered while it recovers"
    );

    // The clock, moved by the fixture.
    fixture.move_the_clock_forward();
    let second = delivery::tick(&pool, &sender, "worker-b")
        .await
        .expect("the worker runs");
    assert_eq!(
        (second.claimed, second.sent, second.retried),
        (1, 1, 0),
        "the second attempt was taken: {second:?}"
    );
    assert_eq!(
        fixture.states(),
        "sent|1",
        "the delivery is done, and nothing is owed any more"
    );
    assert_eq!(
        sender.attempts(),
        vec![1, 2],
        "the subscriber saw this delivery twice, once per attempt, and each request says which \
         attempt it is — a receiver that wants to count duplicates needs to be told"
    );
    let ids = sender.deliveries();
    assert_eq!(ids.len(), 2, "two attempts, two requests: {ids:?}");
    assert_eq!(
        ids[0], ids[1],
        "both attempts carry the same delivery id, which is what the receiver deduplicates on: an \
         id that changed between attempts would make a duplicate indistinguishable from a new event"
    );
}

/// A failure that cannot be fixed does not get retried.
#[tokio::test]
async fn a_permanent_failure_buries_the_delivery() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.register(HOOK_A);
    assert_eq!(fixture.accept("key-1").status, 201);
    let sender = Recording::always(SendError::Permanent(
        "the subscriber answered 410 Gone".to_owned(),
    ));

    let tick = delivery::tick(&pool, &sender, "worker-a")
        .await
        .expect("the worker runs");
    assert_eq!(
        (tick.claimed, tick.sent, tick.retried, tick.dead, tick.lost),
        (1, 0, 0, 1, 0),
        "a permanent failure ends the delivery: {tick:?}"
    );
    assert_eq!(
        fixture.delivery(),
        "dead|1",
        "`dead` is a decision, not a state that happens: the attempt is counted and the delivery \
         stops being work"
    );
    assert!(
        fixture
            .query("select last_error from deliveries")
            .contains("410"),
        "with the reason kept, because the person who has to explain the missing delivery needs it"
    );
    fixture.move_the_clock_forward();
    assert_eq!(
        delivery::tick(&pool, &sender, "worker-a")
            .await
            .expect("the worker runs")
            .claimed,
        0,
        "and it is never handed out again — retrying a subscriber that said `do not send this \
         again` is how a service gets blocked"
    );
    assert_eq!(
        sender.deliveries().len(),
        1,
        "the subscriber was asked exactly once"
    );
}

/// A delivery that keeps failing stops being tried, rather than being retried forever.
#[tokio::test]
async fn a_delivery_that_keeps_failing_stops_being_tried() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.register(HOOK_A);
    assert_eq!(fixture.accept("key-1").status, 201);
    let sender = Recording::always(SendError::Retryable("the subscriber timed out".to_owned()));

    let mut rounds = 0;
    while rounds == 0 || fixture.query("select state from deliveries") == "retrying" {
        rounds += 1;
        assert!(
            rounds <= 12,
            "a delivery that keeps failing has to be given up on inside a handful of attempts: \
             after {rounds} rounds it is `{}` with {} failures. Retrying forever is a subscriber \
             that will never be told the delivery is not coming, and a queue that never drains",
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
        "out of attempts: the delivery is buried, with the failure count that got it there"
    );
    let attempts: i32 = fixture
        .query("select attempts from deliveries")
        .parse()
        .expect("attempts is an integer");
    assert!(
        attempts >= 2,
        "giving up after one failure is not a policy, it is a coincidence: {attempts}"
    );
    assert_eq!(
        sender.deliveries().len() as i32,
        attempts,
        "every recorded failure was one request to the subscriber"
    );
}

/// The promise, end to end: nothing that was accepted is silently dropped.
#[tokio::test]
async fn no_accepted_event_is_silently_dropped() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.register(HOOK_A);
    fixture.register(HOOK_B);
    for key in ["key-1", "key-2", "key-3"] {
        assert_eq!(fixture.accept(key).status, 201);
    }
    assert_eq!(
        fixture.states(),
        "pending|6",
        "three events, two subscribers"
    );

    // One subscriber takes everything; the other refuses everything, for a reason retrying cannot
    // fix. Both outcomes are terminal states, and neither of them is a lost event.
    let sender = Recording::refusing(
        HOOK_B,
        SendError::Permanent("the subscriber answered 404".to_owned()),
    );
    fixture.drain(&pool, &sender).await;

    assert_eq!(
        fixture.states(),
        "dead|3\nsent|3",
        "every delivery that was owed is in a terminal state: {} were sent to the subscriber that \
         takes them, {} were buried for the one that refuses. A delivery left in `pending` or \
         `claimed` after the worker has run out of work is an event nobody was ever told about",
        fixture.query("select count(*) from deliveries where state = 'sent'"),
        fixture.query("select count(*) from deliveries where state = 'dead'")
    );
    assert_eq!(
        fixture.query(
            "select count(*) from deliveries where state in ('pending', 'claimed', 'retrying')"
        ),
        "0",
        "nothing is left in flight or waiting: the queue is empty because everything reached an \
         end, not because the worker stopped looking"
    );
    assert_eq!(
        sender.deliveries().len(),
        6,
        "and every one of them was handed to a subscriber at least once — the deliveries that were \
         buried are the ones the subscriber said no to, and they are buried *because* they were \
         asked"
    );
}

/// The price of at-least-once, measured: the window between the send and the record.
#[tokio::test]
async fn the_window_between_sending_and_recording_lets_a_duplicate_through() {
    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    fixture.register(HOOK_A);
    assert_eq!(fixture.accept("key-1").status, 201);

    // The subscriber takes the request and the worker stops existing at that instant: the future
    // below is dropped while the sender holds it, which is what `kill -9` between the send and the
    // record leaves behind — the send happened, the row still says it did not.
    let (called, was_called) = tokio::sync::oneshot::channel();
    let crashed = Recording::stuck(called);
    let mut working = Box::pin(delivery::tick(&pool, &crashed, "worker-crashed"));
    tokio::select! {
        finished = working.as_mut() => panic!(
            "the worker finished, so this test measured nothing: the fixture's sender parks after \
             recording, and it is the *interruption* between the send and the record that this test \
             is about ({finished:?})"
        ),
        _ = was_called => {}
    }
    drop(working);

    assert_eq!(
        crashed.deliveries().len(),
        1,
        "the subscriber was handed the request before the worker died"
    );
    assert_eq!(
        fixture.states(),
        "claimed|1",
        "and the row still says the worker has it: the delivery is neither `sent` (nothing recorded \
         it) nor lost (the row is there, owed). That gap is the whole duplicate window"
    );

    // Bringing the row back is the lease's job (step `relay/2.15-shutdown-leases`); here the fixture
    // hands it back the way its owner would, so that what this test measures is the window itself
    // and not the recovery policy.
    let delivery_id: Uuid = fixture
        .query("select id from deliveries")
        .parse()
        .expect("one delivery");
    assert!(
        claim::release(&pool, delivery_id, "worker-crashed")
            .await
            .expect("the release runs"),
        "the row is handed back"
    );

    let recovered = Recording::new();
    delivery::tick(&pool, &recovered, "worker-b")
        .await
        .expect("the worker runs");
    assert_eq!(
        recovered.deliveries(),
        vec![delivery_id],
        "the same delivery, sent again"
    );
    assert_eq!(
        fixture.states(),
        "sent|1",
        "and this time it is recorded, so the debt is settled"
    );
    assert_eq!(
        crashed.deliveries().len() + recovered.deliveries().len(),
        2,
        "the subscriber has now seen this delivery twice, for one accepted event: a duplicate is \
         possible, and the service allows it. What makes it *harmless* is that both requests carry \
         the same delivery id, so a receiver can tell a duplicate from a new event — which is the \
         sentence the decision record has to contain"
    );
}

/// The decision, written down where a reader can disagree with it.
#[test]
fn the_decision_record_names_the_guarantee_and_links_what_it_talks_about() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let page = root.join(RECORD);
    let text = std::fs::read_to_string(&page).unwrap_or_else(|error| {
        panic!(
            "this step's deliverable includes the decision record `{RECORD}` (`{}`): {error}. The \
             argument for at-least-once, and what it costs, is a page a reviewer reads — not a \
             comment nobody opens",
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
    assert!(
        lowered.contains("duplicate"),
        "`{RECORD}` has to name the duplicate window: the send that succeeded and the record that \
         never happened is the cost this guarantee pays, and a reader has to be able to find it"
    );

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
