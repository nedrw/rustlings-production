//! Spec for `relay/2.10-dlq`: the deliveries this service has given up on, and how one comes back.
//!
//! `kiln run` copies this file into the project as `tests/spec_dlq.rs`; it is the course's file, not
//! the learner's (plan §3 D2). The promises here are the API's — `GET /dlq` and
//! `POST /dlq/{id}/replay` are the deliverable — and the table behind them, because "the history
//! survives a replay" is a claim about rows.
//!
//! Five properties:
//!
//! * **a poison subscriber is buried, not retried into the ground.** A delivery to an endpoint that
//!   refuses it permanently costs *one* attempt, and the deliveries of the tenant's other endpoints
//!   are unaffected — a delivery nobody can take is not allowed to be everybody's problem;
//! * **the two terminal failures land in the same place.** A permanent refusal and a budget that
//!   ran out are different sentences on the same row, and the listing shows both;
//! * **the listing is what a person needs to act**: which delivery, which event, which endpoint,
//!   what was said, how many attempts, when — and every earlier life of that delivery;
//! * **a replay is a new life, not a new delivery.** The id the receiver deduplicates on does not
//!   change, the attempts go back to zero, and what the last life cost stays readable;
//! * **the boundary of a replay is written down.** A delivery that is not dead is not reset, an
//!   unknown delivery is not found, and another tenant can neither see nor replay it.
//!
//! Nothing here sleeps: the one test that needs a retry to become due again moves it with the same
//! `UPDATE` the fixture has been using since `relay/2.6-at-least-once`.

mod support;

#[path = "support/sender.rs"]
mod sender;

use std::sync::Arc;

use relay::backoff::Budget;
use relay::db::Db;
use relay::delivery::{self, Sender, Tick};
use relay::ordering::{Lanes, spread};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;

use sender::Recording;

/// The tenant that owns the endpoints, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// A second tenant: what is not yours is invisible, including in a list of failures.
const GLOBEX: (&str, &str) = ("globex", "key-globex-91b7");
/// An endpoint that will never take an event, and the reason it gives.
const POISON: &str = "http://127.0.0.1:9/poison";
/// An endpoint that takes everything.
const GOOD: &str = "http://127.0.0.1:9/good";
/// What a subscriber that is gone says.
const REFUSED: &str = "the subscriber answered 410 Gone";

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service with two tenants, over a database of its own.
    fn start() -> Self {
        let postgres = Postgres::start();
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(
                app::BOOTSTRAP_TENANTS,
                format!("{}={},{}={}", ACME.0, ACME.1, GLOBEX.0, GLOBEX.1),
            ),
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

    /// Register a subscriber for one tenant.
    fn register(&self, key: &str, url: &str) {
        let answer = self.app.client().send(
            Request::post("/endpoints", Vec::new())
                .bearer(key)
                .json(&json!({ "url": url })),
        );
        assert_eq!(
            answer.status, 201,
            "the fixture registers `{url}`: {}",
            answer.body
        );
    }

    /// Accept `count` events for one tenant, each one delivery per endpoint it owns.
    fn accept(&self, key: &str, prefix: &str, count: usize) {
        for index in 0..count {
            let answer = self.app.client().send(
                Request::post("/events", Vec::new())
                    .bearer(key)
                    .header("idempotency-key", &format!("{prefix}-{index}"))
                    .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
            );
            assert_eq!(
                answer.status, 201,
                "accepting event {index}: {}",
                answer.body
            );
        }
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

    /// Make every delivery that is waiting to be retried due again, the way an injected clock would.
    fn move_the_clock_forward(&self) {
        self.postgres.execute(
            "update deliveries set next_attempt_at = now() - interval '1 second' \
              where state = 'retrying'",
        );
    }

    /// One turn of the worker, with the policy a deployment runs with.
    async fn turn(&self, sender: Arc<dyn Sender>) -> Tick {
        let pool = self.pool().await;
        spread(
            &pool,
            sender,
            "worker-a",
            Arc::new(Lanes::new()),
            &Budget::default(),
        )
        .await
        .expect("the worker runs")
    }

    /// One turn per delivery, until nothing is owed any more, as a worker loop would.
    async fn drain(&self, sender: Arc<dyn Sender>) -> usize {
        for round in 1..=12 {
            let turn = self.turn(Arc::clone(&sender)).await;
            let waiting = self.query("select count(*) from deliveries where state = 'retrying'");
            if turn.claimed == 0 && waiting == "0" {
                return round;
            }
            self.move_the_clock_forward();
        }
        panic!(
            "the worker never ran out of claimable work: {}",
            self.states()
        );
    }

    /// What the deliveries table holds, by state: `dead|3\nsent|3`.
    fn states(&self) -> String {
        self.query("select state || '|' || count(*) from deliveries group by state order by state")
    }

    /// The dead letters one tenant is told about.
    fn dead_letters(&self, key: &str) -> Vec<Value> {
        let answer = self.app.client().get_with_key("/dlq", key);
        assert_eq!(
            answer.status, 200,
            "`GET /dlq` lists the calling tenant's dead letters: {}",
            answer.body
        );
        answer
            .json()
            .get("dead_letters")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "the answer carries no `dead_letters` array: {}",
                    answer.body
                )
            })
            .clone()
    }

    /// Replay one dead letter as `key`.
    fn replay(&self, key: &str, delivery: &str) -> Response {
        self.app
            .client()
            .send(Request::post(&format!("/dlq/{delivery}/replay"), Vec::new()).bearer(key))
    }

    /// The delivery of one endpoint, as text: the only one, in these tests.
    fn delivery_of(&self, url: &str) -> String {
        self.query(&format!(
            "select id from deliveries where endpoint_id = \
             (select id from endpoints where url = '{url}')"
        ))
    }

    /// One delivery's state and failure count: `dead|1`.
    fn delivery_state(&self, delivery: &str) -> String {
        self.query(&format!(
            "select state || '|' || attempts from deliveries where id = '{delivery}'"
        ))
    }

    /// What `dead_letters` holds for one delivery: `1|replayed|the subscriber answered 410 Gone`.
    fn history(&self, delivery: &str) -> String {
        self.query(&format!(
            "select attempts || '|' || case when replayed_at is null then 'open' else 'replayed' \
                     end || '|' || reason from dead_letters where delivery_id = '{delivery}' order by id"
        ))
    }
}

/// A subscriber that refuses everything is buried at one attempt, and does not touch the others.
#[tokio::test]
async fn a_poison_subscriber_is_buried_without_burning_the_other_subscribers_turns() {
    let fixture = Fixture::start();
    fixture.register(ACME.1, POISON);
    fixture.register(ACME.1, GOOD);
    fixture.accept(ACME.1, "poison", 3);

    let sender: Arc<dyn Sender> = Arc::new(Recording::refusing(
        POISON,
        delivery::SendError::Permanent(REFUSED.to_owned()),
    ));
    let turn = fixture.turn(sender).await;

    assert_eq!(
        (turn.claimed, turn.sent, turn.retried, turn.dead, turn.shed),
        (6, 3, 0, 3, 0),
        "one turn, six deliveries: the three to the subscriber that takes them were delivered, and \
         the three to the one that refuses them are over — not `retrying`, not shed, dead: {turn:?}"
    );
    assert_eq!(
        fixture.states(),
        "dead|3\nsent|3",
        "and the table agrees: {} delivered, {} given up on",
        fixture.query("select count(*) from deliveries where state = 'sent'"),
        fixture.query("select count(*) from deliveries where state = 'dead'")
    );
    assert_eq!(
        fixture.query("select distinct attempts from deliveries where state = 'dead'"),
        "1",
        "a subscriber that says \"never send me this again\" is understood the first time: a \
         permanent failure is not a failure worth another attempt, and spending the retry budget on \
         it is the budget the next delivery no longer has"
    );
    assert_eq!(
        fixture.query(
            "select count(*) from deliveries d join endpoints e on e.id = d.endpoint_id \
              where e.url = 'http://127.0.0.1:9/good' and d.state = 'sent'"
        ),
        "3",
        "and every delivery to the other endpoint was taken: one subscriber that cannot be talked to \
         must not become every subscriber's problem"
    );
}

/// The listing says what died, where, why, and how much it cost.
#[tokio::test]
async fn the_listing_names_what_died_where_and_why() {
    let fixture = Fixture::start();
    fixture.register(ACME.1, POISON);
    fixture.register(ACME.1, GOOD);
    fixture.accept(ACME.1, "poison", 2);
    let sender: Arc<dyn Sender> = Arc::new(Recording::refusing(
        POISON,
        delivery::SendError::Permanent(REFUSED.to_owned()),
    ));
    fixture.turn(sender).await;

    let listed = fixture.dead_letters(ACME.1);
    assert_eq!(
        listed.len(),
        2,
        "`GET /dlq` answers with the tenant's dead letters: {listed:?}"
    );

    let ids: Vec<String> = listed
        .iter()
        .filter_map(|letter| letter.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let dead: Vec<String> = fixture
        .query("select id from deliveries where state = 'dead' order by id")
        .lines()
        .map(str::to_owned)
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        sorted, dead,
        "the listing names every delivery the table buried, and nothing else"
    );

    let first = &listed[0];
    assert_eq!(
        first.get("url").and_then(Value::as_str),
        Some(POISON),
        "where it was going, so the operator knows which subscriber to look at: {first}"
    );
    assert_eq!(
        first.get("kind").and_then(Value::as_str),
        Some("invoice.paid"),
        "what kind of event it was: {first}"
    );
    assert!(
        first
            .get("event_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "and which event, by id: {first}"
    );
    assert_eq!(
        first.get("replays").and_then(Value::as_i64),
        Some(0),
        "it has not been replayed yet: {first}"
    );
    let lives = first
        .get("lives")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a dead letter has a history: {first}"));
    assert_eq!(lives.len(), 1, "one life lost so far: {lives:?}");
    assert_eq!(
        lives[0].get("attempts").and_then(Value::as_i64),
        Some(1),
        "and that life cost one attempt, which is the sentence an operator reads next to the \
         subscriber's answer: {lives:?}"
    );
    assert!(
        lives[0]
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.contains("410")),
        "the subscriber's own words are kept — a dead letter with no reason is a delivery nobody can \
         explain: {lives:?}"
    );
    assert!(
        lives[0].get("died_at").and_then(Value::as_str).is_some(),
        "and when it was given up on: {lives:?}"
    );
    assert!(
        lives[0].get("replayed_at").is_some_and(Value::is_null),
        "this life has not been undone: {lives:?}"
    );
}

/// A delivery that ran out of attempts is a dead letter too, with the budget's own sentence.
#[tokio::test]
async fn a_delivery_that_ran_out_of_attempts_is_a_dead_letter_too() {
    let fixture = Fixture::start();
    fixture.register(ACME.1, POISON);
    fixture.accept(ACME.1, "timeout", 1);
    let sender: Arc<dyn Sender> = Arc::new(Recording::refusing(
        POISON,
        delivery::SendError::Retryable("the subscriber timed out".to_owned()),
    ));

    let rounds = fixture.drain(Arc::clone(&sender)).await;
    assert!(
        (2..=12).contains(&rounds),
        "a subscriber that keeps timing out is retried and then given up on, inside the turns \
         `relay/2.6-at-least-once` measured: {rounds} turns, {}",
        fixture.states()
    );
    let delivery = fixture.delivery_of(POISON);
    let spent: i64 = fixture
        .query(&format!(
            "select attempts from deliveries where id = '{delivery}'"
        ))
        .parse()
        .expect("attempts is a number");

    let listed = fixture.dead_letters(ACME.1);
    assert_eq!(listed.len(), 1, "one dead letter: {listed:?}");
    let lives = listed[0]
        .get("lives")
        .and_then(Value::as_array)
        .expect("a dead letter has a history");
    assert_eq!(
        lives[0].get("attempts").and_then(Value::as_i64),
        Some(spent),
        "the listing counts the same attempts the row does: {lives:?}"
    );
    assert!(
        lives[0]
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.contains("gave up after")),
        "and the reason is the budget's, because the subscriber never changed its mind: {lives:?}"
    );
}

/// A replay is a new life for the same delivery.
#[tokio::test]
async fn a_replay_puts_one_back_in_the_queue_and_keeps_the_old_life() {
    let fixture = Fixture::start();
    fixture.register(ACME.1, POISON);
    fixture.accept(ACME.1, "replayed", 1);
    let refusing: Arc<dyn Sender> = Arc::new(Recording::refusing(
        POISON,
        delivery::SendError::Permanent(REFUSED.to_owned()),
    ));
    fixture.turn(Arc::clone(&refusing)).await;
    let delivery = fixture.delivery_of(POISON);
    assert_eq!(fixture.delivery_state(&delivery), "dead|1");

    let answer = fixture.replay(ACME.1, &delivery);
    assert_eq!(
        answer.status, 200,
        "replaying a dead letter is not an error: {}",
        answer.body
    );
    assert_eq!(
        answer.field("replayed"),
        json!(true),
        "the answer says what it did: {}",
        answer.body
    );
    assert_eq!(
        answer.field("replays"),
        json!(1),
        "and how many times this delivery has now been replayed: {}",
        answer.body
    );
    assert_eq!(
        answer.field("state"),
        json!("pending"),
        "and the state a replay leaves it in, which is the state an operator would guess next: {}",
        answer.body
    );
    assert_eq!(
        fixture.delivery_state(&delivery),
        "pending|0",
        "the delivery is owed again with nothing spent: a replay that kept the failure count would \
         spend a life it has not had, and `attempts` is what the budget is measured in"
    );
    assert_eq!(
        fixture.history(&delivery),
        "1|replayed|the subscriber answered 410 Gone",
        "and the life that was undone is still written down, with the instant it was undone"
    );
    assert!(
        fixture.dead_letters(ACME.1).is_empty(),
        "the delivery is not a dead letter any more: the listing is about what is stuck, not about \
         what was stuck once"
    );

    // The subscriber has not changed its mind, so the new life ends the same way — and that is a
    // second entry in the history of the same delivery, not a second delivery.
    fixture.turn(Arc::clone(&refusing)).await;
    assert_eq!(fixture.delivery_state(&delivery), "dead|1", "buried again");
    assert_eq!(
        fixture.history(&delivery),
        "1|replayed|the subscriber answered 410 Gone\n1|open|the subscriber answered 410 Gone",
        "two lives, the first stamped as replayed and the second open: the history is what a person \
         reads when they ask \"has this happened before?\""
    );
    let listed = fixture.dead_letters(ACME.1);
    assert_eq!(listed.len(), 1, "one delivery, one dead letter: {listed:?}");
    assert_eq!(
        listed[0].get("id").and_then(Value::as_str),
        Some(delivery.as_str()),
        "and it is the same delivery, with the id the receiver already deduplicated on"
    );
    assert_eq!(
        listed[0].get("replays").and_then(Value::as_i64),
        Some(1),
        "replayed once: {}",
        listed[0]
    );
}

/// The boundary a replay draws: only what is dead, and only what is yours.
#[tokio::test]
async fn a_replay_does_nothing_to_a_delivery_that_is_not_dead() {
    let fixture = Fixture::start();
    fixture.register(ACME.1, POISON);
    fixture.register(ACME.1, GOOD);
    fixture.accept(ACME.1, "boundary", 1);
    let sender: Arc<dyn Sender> = Arc::new(Recording::refusing(
        POISON,
        delivery::SendError::Permanent(REFUSED.to_owned()),
    ));
    fixture.turn(sender).await;

    let delivered = fixture.delivery_of(GOOD);
    let answer = fixture.replay(ACME.1, &delivered);
    assert_eq!(
        (
            answer.status,
            answer.field("replayed"),
            answer.field("state")
        ),
        (200, json!(false), json!("sent")),
        "a replay names a *dead letter*: a delivery a subscriber has taken is not one, and the \
         answer says so instead of sending it again: {}",
        answer.body
    );
    assert_eq!(
        fixture.delivery_state(&delivered),
        "sent|0",
        "and nothing about it moved: a replay that resets a delivered row is a replay that sends \
         duplicates nobody asked for"
    );

    // The same boundary, from the queue's side: replaying a delivery that is already owed again.
    let dead = fixture.delivery_of(POISON);
    assert_eq!(fixture.replay(ACME.1, &dead).field("replayed"), json!(true));
    let again = fixture.replay(ACME.1, &dead);
    assert_eq!(
        (
            again.status,
            again.field("replayed"),
            again.field("replays")
        ),
        (200, json!(false), json!(1)),
        "replaying the same id twice is safe: the second call finds a delivery that is already \
         owed again and answers the same thing a third one would: {}",
        again.body
    );
    assert_eq!(
        fixture.delivery_state(&dead),
        "pending|0",
        "so a second replay does not start a second life beside the first"
    );
    assert_eq!(
        fixture.history(&dead),
        "1|replayed|the subscriber answered 410 Gone",
        "and the history has one life with one replay on it, not two"
    );

    let unknown = fixture.replay(ACME.1, "00000000-0000-4000-8000-000000000000");
    assert_eq!(
        unknown.status, 404,
        "a delivery that does not exist is not a delivery this tenant may replay: {}",
        unknown.body
    );
}

/// One tenant's failures are not another tenant's business.
#[tokio::test]
async fn another_tenant_can_neither_see_nor_replay_a_dead_letter() {
    let fixture = Fixture::start();
    fixture.register(ACME.1, POISON);
    fixture.register(GLOBEX.1, GOOD);
    fixture.accept(ACME.1, "acme", 1);
    fixture.accept(GLOBEX.1, "globex", 1);
    let sender: Arc<dyn Sender> = Arc::new(Recording::refusing(
        POISON,
        delivery::SendError::Permanent(REFUSED.to_owned()),
    ));
    fixture.turn(sender).await;
    let dead = fixture.delivery_of(POISON);
    assert_eq!(fixture.delivery_state(&dead), "dead|1");

    assert_eq!(
        fixture.dead_letters(ACME.1).len(),
        1,
        "the owner of the endpoint sees its own failure"
    );
    assert_eq!(
        fixture.dead_letters(GLOBEX.1).len(),
        0,
        "and another tenant sees nothing: a dead letter names an endpoint and an event, which is \
         one tenant's business"
    );

    let answer = fixture.replay(GLOBEX.1, &dead);
    assert_eq!(
        answer.status, 404,
        "nor can it replay one: 404 rather than 403, because a caller must not be able to tell a \
         delivery that is not there from one that belongs to somebody else: {}",
        answer.body
    );
    assert_eq!(
        fixture.delivery_state(&dead),
        "dead|1",
        "and the delivery is exactly where it was"
    );
}
