//! Spec for `relay/2.2-idempotent-ingest`: the same event posted twice is one event, and the
//! caller still gets an answer.
//!
//! `kiln run` copies this file into the project as `tests/spec_ingest.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Two instruments, because the two claims cannot be told apart by
//! either one alone:
//!
//! * **HTTP** says what the service *answered* — a 201 for an event it took, a 200 for one it
//!   already had, a 400 for a request it cannot use (`Idempotency-Key` is required, and a missing
//!   or empty one is the caller's mistake, not the service's);
//! * **`psql`** says what the service *did* — how many rows exist, and what they hold. A service
//!   that answers 200 without storing anything passes the HTTP half and fails the row count, and a
//!   service that overwrites the row on conflict passes the row count and fails the value.
//!
//! Three properties are the whole step, and each one is a way to get idempotency wrong:
//!
//! * **the second POST is a replay, not a new event and not a failure** — the tenant gets back the
//!   identity of the event it already sent, *with the content it sent the first time*;
//! * **the key is scoped to one tenant** — `(tenant_id, idempotency_key)`, not `idempotency_key`:
//!   two tenants sharing a key is normal, and a global unique index would silently drop one of
//!   their events;
//! * **nothing is stored before the body is usable** — a refused request leaves the table exactly
//!   as it was, which is what makes "this event was accepted" mean something.

mod support;

use serde_json::{Value, json};
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;

/// The tenant these tests seed, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// A second tenant: an idempotency key belongs to a tenant, not to the service.
const GLOBEX: (&str, &str) = ("globex", "key-globex-91b7");

/// One database and one in-process service, for the length of one test.
///
/// The app is declared first so it is the first thing taken apart: a service still serving while its
/// database disappears writes error lines into a test that has already passed.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service seeded with `tenants`, over a database of its own.
    fn start(tenants: &[(&str, &str)]) -> Self {
        let postgres = Postgres::start();
        let seeded = tenants
            .iter()
            .map(|(name, key)| format!("{name}={key}"))
            .collect::<Vec<_>>()
            .join(",");
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, seeded),
        ]));
        Self { app, postgres }
    }

    /// `POST /events` with an API key and, when given, an idempotency key.
    fn post(&self, api_key: &str, idempotency_key: Option<&str>, body: &Value) -> Response {
        let request = Request::post("/events", Vec::new())
            .bearer(api_key)
            .json(body);
        let request = match idempotency_key {
            Some(key) => request.header("idempotency-key", key),
            None => request,
        };
        self.app.client().send(request)
    }

    /// How many events this database holds.
    fn events(&self) -> String {
        self.postgres.query("select count(*) from events")
    }
}

/// A body `POST /events` accepts: a kind, and the JSON to deliver.
fn event(kind: &str, payload: Value) -> Value {
    json!({ "kind": kind, "payload": payload })
}

/// The second POST with one key answers with the first event — content included.
///
/// The second body is *different* on purpose: a client that retries a request it is not sure about
/// sends what it sent the first time, and a client that reuses a key for new content has to get the
/// old event back rather than a new one, or the key would stop meaning anything.
#[test]
fn one_key_is_one_event_however_many_times_it_is_posted() {
    let fixture = Fixture::start(&[ACME]);
    let sent = event("invoice.paid", json!({ "id": "inv_1", "amount": 4200 }));

    let first = fixture.post(ACME.1, Some("key-6b1f"), &sent);
    assert_eq!(
        first.status, 201,
        "a well-formed event with an idempotency key is accepted: {}",
        first.body
    );
    assert_eq!(
        first.field("replayed"),
        json!(false),
        "the first POST says it stored the event: {}",
        first.body
    );
    let id = first.field("id");

    let again = fixture.post(
        ACME.1,
        Some("key-6b1f"),
        &event(
            "invoice.voided",
            json!({ "id": "inv_1", "reason": "duplicate" }),
        ),
    );
    assert_eq!(
        again.status, 200,
        "a replay is an answer, not an error: {}",
        again.body
    );
    assert_eq!(
        again.field("id"),
        id,
        "the caller gets back the event it already sent: {}",
        again.body
    );
    assert_eq!(
        again.field("kind"),
        json!("invoice.paid"),
        "the answer describes the *stored* event, not the request that asked for it again: {}",
        again.body
    );
    assert_eq!(
        again.field("replayed"),
        json!(true),
        "the caller can tell a replay from a first acceptance: {}",
        again.body
    );

    assert_eq!(
        fixture.events(),
        "1",
        "one key, one row: the unique index is the mechanism, and an `insert` that skips the \
         conflict is the implementation"
    );
    assert_eq!(
        fixture.postgres.query("select kind from events"),
        "invoice.paid",
        "the second POST must not overwrite what the first one stored"
    );
    assert_eq!(
        fixture
            .postgres
            .query("select payload->>'amount' from events"),
        "4200",
        "the payload is stored as the JSON that was sent, not as text that happens to look like \
         JSON — `payload jsonb` is what makes the column queryable"
    );
}

/// A different key is a different event, even when the body is the same.
#[test]
fn a_different_key_is_a_different_event() {
    let fixture = Fixture::start(&[ACME]);
    let sent = event("invoice.paid", json!({ "id": "inv_2" }));

    let first = fixture.post(ACME.1, Some("key-a"), &sent);
    let second = fixture.post(ACME.1, Some("key-b"), &sent);
    assert_eq!(first.status, 201, "{}", first.body);
    assert_eq!(
        second.status, 201,
        "a new key is a new event, however much the body looks like the previous one: {}",
        second.body
    );
    assert_ne!(
        first.field("id"),
        second.field("id"),
        "two keys, two identities"
    );
    assert_eq!(fixture.events(), "2", "two keys, two rows");
}

/// A request without a usable key is refused, and stores nothing.
#[test]
fn a_request_without_a_usable_key_stores_nothing() {
    let fixture = Fixture::start(&[ACME]);
    let sent = event("invoice.paid", json!({ "id": "inv_3" }));

    let missing = fixture.post(ACME.1, None, &sent);
    assert_eq!(
        missing.status, 400,
        "`Idempotency-Key` is required, and asking for it is the caller's side of the contract \
         (a 401 is about who is calling, and this caller has a valid API key): {}",
        missing.body
    );
    assert!(
        missing.body.to_lowercase().contains("idempotency"),
        "the refusal names the header that is missing, or the caller cannot fix it: {}",
        missing.body
    );

    let empty = fixture.post(ACME.1, Some(""), &sent);
    assert_eq!(
        empty.status, 400,
        "an empty header is not a key: `Idempotency-Key: ` would make every event of a tenant \
         collide with the first one: {}",
        empty.body
    );

    assert_eq!(
        fixture.events(),
        "0",
        "a refused request leaves the table exactly as it was"
    );
}

/// The key names an event *within one tenant*: two tenants may use the same one.
#[test]
fn one_key_in_two_tenants_is_two_events() {
    let fixture = Fixture::start(&[ACME, GLOBEX]);
    let sent = event("invoice.paid", json!({ "id": "inv_4" }));

    let acme = fixture.post(ACME.1, Some("key-shared"), &sent);
    let globex = fixture.post(GLOBEX.1, Some("key-shared"), &sent);
    assert_eq!(acme.status, 201, "{}", acme.body);
    assert_eq!(
        globex.status, 201,
        "another tenant's event is new, not a replay: a unique index on the key alone would have \
         silently dropped it: {}",
        globex.body
    );
    assert_ne!(
        acme.field("id"),
        globex.field("id"),
        "two tenants, two identities"
    );
    assert_eq!(
        fixture.postgres.query(
            "select tenants.name, count(*) from events \
             join tenants on tenants.id = events.tenant_id \
             group by tenants.name order by tenants.name"
        ),
        "acme|1\nglobex|1",
        "each row belongs to the tenant that sent it"
    );
}

/// A body that is not a kind and a JSON object is refused before anything is written.
#[test]
fn the_body_has_to_be_a_kind_and_a_json_object() {
    let fixture = Fixture::start(&[ACME]);

    let refused = [
        (
            json!({ "payload": { "id": "inv_5" } }),
            "a body with no `kind`",
        ),
        (json!({ "kind": "", "payload": {} }), "an empty `kind`"),
        (
            json!({ "kind": "invoice.paid" }),
            "a body with no `payload`",
        ),
        (
            json!({ "kind": "invoice.paid", "payload": "not an object" }),
            "a `payload` that is a string",
        ),
        (
            json!({ "kind": "invoice.paid", "payload": [1, 2] }),
            "a `payload` that is an array",
        ),
    ];
    for (body, what) in refused {
        let answer = fixture.post(ACME.1, Some("key-1"), &body);
        assert_eq!(
            answer.status, 400,
            "{what} is refused with a sentence, not stored and not a 500: {}",
            answer.body
        );
    }

    let malformed = fixture.app.client().send(
        Request::post("/events", "{\"kind\": \"invoice.paid\",")
            .bearer(ACME.1)
            .header("content-type", "application/json")
            .header("idempotency-key", "key-2"),
    );
    assert_eq!(
        malformed.status, 400,
        "a body that is not JSON at all is the caller's mistake: {}",
        malformed.body
    );

    let accepted = fixture.post(
        ACME.1,
        Some("key-3"),
        &event("invoice.paid", json!({ "id": "inv_5" })),
    );
    assert_eq!(
        accepted.status, 201,
        "the refusals are about the body, not about the route: {}",
        accepted.body
    );
    assert_eq!(
        fixture.events(),
        "1",
        "only the acceptable event is stored — a body that fails halfway through must not leave \
         a row behind"
    );
}
