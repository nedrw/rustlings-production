//! Spec for `relay/2.16-authz-audit`: the authorization matrix, and the trail behind it.
//!
//! `kiln run` copies this file into the project as `tests/spec_authz.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The step is judged from three sides, and each one is blind to
//! the other two:
//!
//! * **the matrix, as a matrix.** The permission model is data ([`relay::authz::GRANTS`]), so this
//!   file walks it cell by cell — every action, every role, both scopes — and asks for an answer. A
//!   `decide` that answers "no" to everything passes a one-off test and fails this one;
//! * **every route, every role.** The table below sends one request per route per role and pins the
//!   status. It is the same permission model seen from outside, which is the only place where "the
//!   route consults the matrix" is observable: a route that never asked is a route that answers
//!   `200` to somebody the matrix refuses;
//! * **the trail.** A privileged decision writes a row, a refused one writes a row too, a read
//!   writes none, and the rows are readable by the tenant they belong to and by nobody else.
//!
//! Two properties are asserted rather than described, because both are the kind of sentence that
//! is easy to write in a document and easy to get wrong in code:
//!
//! * **a row that belongs to somebody else and a row that does not exist answer the same thing**,
//!   byte for byte. A `403`, a different body or a different status for the two is a service that
//!   confirms the existence of another tenant's data to whoever guesses an id;
//! * **nothing a caller sends changes who it is.** Not a header, not a spelling of a role, not a
//!   setting: the role comes from the credential's own row, and this file tries the ways in.

mod support;

use std::collections::BTreeSet;

use relay::authz::{self, ACTIONS, Action, Decision, ROLES, Reason, Role, Scope};
use relay::config::{Config, Flags};
use serde_json::{Value, json};
use support::app::{self, App};
use support::client::{Request, Response};
use support::postgres::Postgres;
use support::var;

/// The tenant these tests act as: an administrator of its own rows.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// A second administrator: what is not yours has to be invisible to it.
const GLOBEX: (&str, &str) = ("globex", "key-globex-91b7");
/// A tenant whose key may read and may not write.
const READER: (&str, &str) = ("acme-reader", "key-acme-reader-2c4f");
/// The reader, as the configuration spells it: `name:role=key`.
const READER_IN_CONFIG: &str = "acme-reader:tenant-reader=key-acme-reader-2c4f";
/// A key nobody was ever issued.
const STRANGER: &str = "key-that-was-never-issued";
/// A subscriber address: a delivery target this service accepts when the deployment allows its own
/// networks, which is what makes registering one possible offline (`relay/2.12-ssrf-defense`).
const TARGET: &str = "http://127.0.0.1:9/hook";
/// An id that is a delivery id and is nobody's: the second half of the indistinguishability test.
const NOBODYS: &str = "00000000-0000-4000-8000-000000000000";

/// One database and one in-process service, for the length of one test.
struct Fixture {
    app: App,
    postgres: Postgres,
}

impl Fixture {
    /// Start a service with an administrator, a second administrator and a reader.
    fn start() -> Self {
        Self::start_with(&[])
    }

    /// Start it with extra settings, for the test that asks whether a setting can turn the matrix
    /// off.
    fn start_with(settings: &[(&str, &str)]) -> Self {
        let postgres = Postgres::start();
        let mut vars = vec![
            var(app::DATABASE_URL, postgres.url()),
            var(
                app::BOOTSTRAP_TENANTS,
                format!(
                    "{}={},{}={},{READER_IN_CONFIG}",
                    ACME.0, ACME.1, GLOBEX.0, GLOBEX.1
                ),
            ),
            // The subscribers of this file are on `127.0.0.1`, and a service refuses an address on
            // its own networks unless the deployment says otherwise (`relay/2.12-ssrf-defense`).
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
            // This file sends one request per route per role: the quota of `relay/2.13-rate-limit`
            // is not what it is about, and a fixture that spent it would be measuring the wrong
            // refusal.
            var("RELAY__RATE_LIMIT__BURST", "100000"),
            var("RELAY__RATE_LIMIT__PER_SECOND", "100000"),
        ];
        for (name, value) in settings {
            vars.push(var(name, *value));
        }
        let app = App::start(app::config(&vars));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting — the roles below are seeded after that. One round trip
        // only the running service can answer is what tells this file it is up.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks anything of it: {}",
            serving.body
        );
        Self { app, postgres }
    }

    /// One request without a credential.
    fn anonymous(&self, method: &str, path: &str) -> Response {
        self.app.client().send(Request::new(method, path))
    }

    /// One request with a bearer key.
    fn as_(&self, method: &str, path: &str, key: &str) -> Response {
        self.app
            .client()
            .send(Request::new(method, path).bearer(key))
    }

    /// One request carrying whatever the caller sends, with a key when it has one.
    fn send(&self, request: Request, key: Option<&str>) -> Response {
        let request = match key {
            Some(key) => request.bearer(key),
            None => request,
        };
        self.app.client().send(request)
    }

    /// The body a route of this API takes.
    fn event_request() -> Request {
        Request::post("/events", event_body()).header("content-type", "application/json")
    }

    /// Register the fixture's target for a tenant.
    fn register(&self, key: &str) {
        let answer = self.send(
            Request::post("/endpoints", endpoint_body()).header("content-type", "application/json"),
            Some(key),
        );
        assert_eq!(
            answer.status, 201,
            "the fixture registers `{TARGET}`: {}",
            answer.body
        );
    }

    /// Accept one event, under an idempotency key of its own.
    fn accept(&self, key: &str, idempotency: &str) -> Response {
        self.send(
            Fixture::event_request().header("idempotency-key", idempotency),
            Some(key),
        )
    }

    /// The tenant's trail, as the API answers it.
    fn trail(&self, key: &str) -> Vec<Value> {
        let answer = self.as_("GET", "/audit", key);
        assert_eq!(
            answer.status, 200,
            "`GET /audit` answers the caller's own trail: {}",
            answer.body
        );
        answer
            .field("entries")
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("the trail is an `entries` array: {}", answer.body))
    }

    /// One line of `psql`, as text.
    fn query(&self, sql: &str) -> String {
        self.postgres.query(sql)
    }
}

/// The body `POST /events` accepts.
fn event_body() -> Vec<u8> {
    json!({ "kind": "invoice.paid", "payload": { "n": 1 } })
        .to_string()
        .into_bytes()
}

/// The body `POST /endpoints` accepts.
fn endpoint_body() -> Vec<u8> {
    json!({ "url": TARGET }).to_string().into_bytes()
}

/// One row of the route × role table: one request, and what each role gets for it.
struct Case {
    /// The route, as `POST /events`.
    what: &'static str,
    /// The request to send, before a credential is added.
    request: Request,
    /// What a caller with no key gets.
    anonymous: u16,
    /// What the reader gets.
    reader: u16,
    /// What the administrator gets.
    admin: u16,
}

/// Every route this service serves, with what each of the three roles gets.
///
/// The statuses are the permission model read from outside: `401` for a caller with no identity,
/// `403` for a caller the matrix refuses, and the route's own answer for a caller it allows. The
/// interesting column is the middle one — a read-only role is the one a service usually gets wrong,
/// and it is wrong in one direction only: it may read its own rows, and it may not write, replay,
/// or read the trail about what was read.
fn cases() -> Vec<Case> {
    let json_post = |path: &str, body: Vec<u8>| {
        Request::post(path, body).header("content-type", "application/json")
    };
    vec![
        Case {
            what: "GET /healthz",
            request: Request::get("/healthz"),
            anonymous: 200,
            reader: 200,
            admin: 200,
        },
        Case {
            what: "GET /readyz",
            request: Request::get("/readyz"),
            anonymous: 200,
            reader: 200,
            admin: 200,
        },
        Case {
            what: "GET /openapi.json",
            request: Request::get("/openapi.json"),
            anonymous: 200,
            reader: 200,
            admin: 200,
        },
        Case {
            what: "GET /endpoints",
            request: Request::get("/endpoints"),
            anonymous: 401,
            reader: 200,
            admin: 200,
        },
        Case {
            what: "POST /endpoints",
            request: json_post("/endpoints", endpoint_body()),
            anonymous: 401,
            reader: 403,
            admin: 201,
        },
        Case {
            what: "GET /events",
            request: Request::get("/events"),
            anonymous: 401,
            reader: 200,
            admin: 200,
        },
        Case {
            what: "POST /events",
            request: Fixture::event_request().header("idempotency-key", "matrix"),
            anonymous: 401,
            reader: 403,
            admin: 201,
        },
        Case {
            what: "GET /dlq",
            request: Request::get("/dlq"),
            anonymous: 401,
            reader: 200,
            admin: 200,
        },
        Case {
            what: "POST /dlq/{id}/replay",
            request: Request::post(&format!("/dlq/{NOBODYS}/replay"), Vec::new()),
            anonymous: 401,
            reader: 403,
            admin: 404,
        },
        Case {
            what: "GET /audit",
            request: Request::get("/audit"),
            anonymous: 401,
            reader: 403,
            admin: 200,
        },
    ]
}

/// Every route, every role: the status is the matrix's answer, seen from outside.
#[test]
fn every_route_answers_every_role_the_way_the_matrix_says() {
    let fixture = Fixture::start();
    for case in cases() {
        for (role, key, expected) in [
            ("nobody", None, case.anonymous),
            ("the reader", Some(READER.1), case.reader),
            ("an administrator", Some(ACME.1), case.admin),
        ] {
            let answer = fixture.send(case.request.clone(), key);
            assert_eq!(
                answer.status, expected,
                "{} as {role} has to answer {expected}: {}",
                case.what, answer.body
            );
        }
    }
}

/// The three refusals are three different sentences, and the caller can tell which one it got.
#[test]
fn a_refusal_says_which_of_the_three_reasons_it_is() {
    let fixture = Fixture::start();

    // No identity at all, and an identity nobody issued: the same answer, because a key that
    // belongs to no tenant identifies nobody.
    let nobody = fixture.anonymous("GET", "/events");
    let stranger = fixture.as_("GET", "/events", STRANGER);
    assert_eq!(nobody.status, 401, "{}", nobody.body);
    assert_eq!(
        stranger.status, 401,
        "a key nobody was issued is not a weaker identity, it is no identity: {}",
        stranger.body
    );
    assert_eq!(
        nobody.body, stranger.body,
        "and the two answers are the same bytes: a stranger who can tell `no key` from `wrong key` \
         has a way to sort their guesses"
    );

    // Known, and not permitted: a different answer, because it has to be — this one the caller can
    // change, by using a credential that may do it.
    let reader = fixture.accept(READER.1, "reader-writes");
    assert_eq!(reader.status, 403, "{}", reader.body);
    assert_eq!(
        reader.field("error"),
        json!("forbidden"),
        "the token is what a client switches on: {}",
        reader.body
    );
    assert!(
        reader.body.contains("write-events"),
        "and the refusal names the action the matrix refused, so the sentence is actionable: {}",
        reader.body
    );
    assert!(
        !reader.body.contains(READER.1),
        "a refusal never echoes the credential it is about: {}",
        reader.body
    );

    // The third refusal is not visible from here on purpose: it is the 404 of the next test.
    assert_eq!(
        Reason::NotVisible.status(),
        404,
        "the third reason is the one a caller learns the least from"
    );
}

/// A row that belongs to another tenant and a row that does not exist are the same answer.
#[test]
fn another_tenants_row_and_a_missing_row_are_indistinguishable() {
    let fixture = Fixture::start();
    fixture.register(ACME.1);
    let accepted = fixture.accept(ACME.1, "acme-one");
    assert_eq!(accepted.status, 201, "{}", accepted.body);
    let delivery = fixture.query("select id from deliveries limit 1");
    assert!(
        !delivery.trim().is_empty(),
        "the fixture wrote one delivery for acme, and the table has {}",
        fixture.query("select count(*) from deliveries")
    );

    // What globex sees: its own rows, which are none.
    let listing = fixture.as_("GET", "/events", GLOBEX.1);
    assert_eq!(listing.status, 200, "{}", listing.body);
    assert_eq!(
        listing
            .field("events")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "another tenant's page is empty, not somebody else's: {}",
        listing.body
    );

    // The same route, asked for a row that exists and for one that cannot: one answer.
    let theirs = fixture.as_(
        "POST",
        &format!("/dlq/{}/replay", delivery.trim()),
        GLOBEX.1,
    );
    let missing = fixture.as_("POST", &format!("/dlq/{NOBODYS}/replay"), GLOBEX.1);
    assert_eq!(
        theirs.status, 404,
        "another tenant's delivery is not a delivery this caller can see: {}",
        theirs.body
    );
    assert_eq!(missing.status, 404, "{}", missing.body);
    assert_eq!(
        theirs.body, missing.body,
        "and the two answers are byte for byte the same: a 403 for the first would tell the caller \
         that the id it guessed exists"
    );

    // The same question as acme, about its own row: visible, and not an error.
    let mine = fixture.as_("POST", &format!("/dlq/{}/replay", delivery.trim()), ACME.1);
    assert_eq!(
        mine.status, 200,
        "the owner of the row gets the row's answer: {}",
        mine.body
    );
}

/// A reader may read its own rows, and the matrix has nothing else for it.
#[test]
fn a_reader_reads_and_does_not_write() {
    let fixture = Fixture::start();
    for path in ["/endpoints", "/events", "/dlq"] {
        let answer = fixture.as_("GET", path, READER.1);
        assert_eq!(
            answer.status, 200,
            "a reader reads its own `{path}`: {}",
            answer.body
        );
    }
    for (what, request) in [
        (
            "register an endpoint",
            Request::post("/endpoints", endpoint_body()).header("content-type", "application/json"),
        ),
        (
            "accept an event",
            Fixture::event_request().header("idempotency-key", "reader"),
        ),
        (
            "replay a dead letter",
            Request::post(&format!("/dlq/{NOBODYS}/replay"), Vec::new()),
        ),
        ("read the trail", Request::get("/audit")),
    ] {
        let answer = fixture.send(request, Some(READER.1));
        assert_eq!(
            answer.status, 403,
            "a reader may not {what}: {}",
            answer.body
        );
    }

    // Reading the trail is the one that is easy to leave open: the rows are about this tenant, and
    // who did what is exactly what a read-only credential is not allowed to see.
    assert_eq!(
        fixture.as_("GET", "/audit", READER.1).status,
        403,
        "the trail is about the data, not a part of it"
    );
}

/// The matrix as data: it is total, it grants nothing outside a tenant, and it is readable.
#[test]
fn the_matrix_grants_nothing_outside_a_tenant() {
    for grant in authz::GRANTS {
        assert_eq!(
            grant.scope,
            Scope::Own,
            "the matrix has no cell for another tenant's rows: {grant:?}"
        );
    }
    for role in ROLES {
        for action in ACTIONS {
            let decision = authz::decide(role, action, Scope::Any);
            assert!(
                matches!(decision, Decision::Deny(_)),
                "nobody — {} included — may do {action:?} to another tenant's rows",
                role.as_str()
            );
            // A role that *may* do this to its own rows, and asks about somebody else's, gets the
            // refusal that says nothing. A role that may not do it at all is refused for the other
            // reason: telling an anonymous caller "not visible" would be telling it that there is
            // something to be visible.
            if authz::granted(role, action).is_some() {
                assert_eq!(
                    decision,
                    Decision::Deny(Reason::NotVisible),
                    "{} may do {action:?} to its own rows, and asking for somebody else's has to \
                     answer with the refusal that reveals nothing",
                    role.as_str()
                );
            }
        }
    }

    // A table that refuses everything for everybody is not a matrix, it is an outage.
    assert!(
        authz::GRANTS.len() >= ROLES.len(),
        "the matrix has to grant something, or this service serves nobody: {}",
        authz::GRANTS.len()
    );

    // Every action is granted to some role, or the route that serves it is dead code.
    let ungranted: Vec<Action> = ACTIONS
        .iter()
        .copied()
        .filter(|action| !authz::GRANTS.iter().any(|grant| grant.action == *action))
        .collect();
    assert!(
        ungranted.is_empty(),
        "these actions are served by a route and granted to nobody: {ungranted:?}"
    );

    // The words: a role round-trips through text, and `anonymous` is not a role a tenant can be
    // given — that is what a missing key is.
    for role in [Role::Reader, Role::Admin] {
        assert_eq!(
            Role::parse(role.as_str()).ok(),
            Some(role),
            "the role's text is what configuration writes and what the database stores"
        );
    }
    assert!(
        Role::parse("anonymous").is_err(),
        "no configuration makes a tenant anonymous: a request with no usable key is, and that is \
         not a property of a row"
    );
    assert!(Role::parse("admin").is_err(), "one spelling per role");

    // The configuration spells a role the same way, on the entry that names the tenant.
    let env = vec![(
        app::BOOTSTRAP_TENANTS.to_owned(),
        format!("{READER_IN_CONFIG},{}={}", ACME.0, ACME.1),
    )];
    let config = Config::load(&Flags::default(), &env).expect("the fixture's tenants are valid");
    let roles: Vec<(&str, Role)> = config
        .bootstrap
        .tenants
        .iter()
        .map(|tenant| (tenant.name.as_str(), tenant.role))
        .collect();
    assert_eq!(
        roles,
        vec![("acme-reader", Role::Reader), ("acme", Role::Admin)],
        "an entry that names no role is an administrator, and one that names a role is that role"
    );
    assert!(
        Config::load(
            &Flags::default(),
            &[(
                app::BOOTSTRAP_TENANTS.to_owned(),
                "acme:tenant-owner=key-acme-5f3d".to_owned()
            )],
        )
        .is_err(),
        "a role this service does not have is a configuration error, not a default"
    );
}

/// A privileged decision writes exactly one row, and it says which request it was.
#[test]
fn a_privileged_action_leaves_one_traceable_row() {
    let fixture = Fixture::start();
    let given = "1f2e3d4c-5b6a-4978-8899-aabbccddeeff";
    let accepted = fixture.send(
        Fixture::event_request()
            .header("idempotency-key", "audited")
            .header("x-request-id", given),
        Some(ACME.1),
    );
    assert_eq!(accepted.status, 201, "{}", accepted.body);
    assert_eq!(
        accepted.header("x-request-id"),
        Some(given),
        "the correlation id is the one the trail will carry"
    );

    let trail = fixture.trail(ACME.1);
    let row = trail
        .iter()
        .find(|entry| entry.get("request_id") == Some(&json!(given)))
        .unwrap_or_else(|| panic!("the trail has a row for the request it answered: {trail:?}"));
    assert_eq!(row.get("action"), Some(&json!("write-events")), "{row}");
    assert_eq!(row.get("decision"), Some(&json!("allow")), "{row}");
    assert_eq!(row.get("role"), Some(&json!("tenant-admin")), "{row}");
    assert_eq!(row.get("tenant"), Some(&json!("acme")), "{row}");
    assert_eq!(row.get("target"), Some(&json!("POST /events")), "{row}");
    assert!(
        row.get("at")
            .and_then(Value::as_str)
            .is_some_and(|at| at.len() >= 20 && at.contains('T')),
        "the row says when, in a form a reader can sort: {row}"
    );

    // One row for one decision: a trail that writes two rows per request doubles what an operator
    // reads and hides the second request in it.
    let mine: Vec<&Value> = trail
        .iter()
        .filter(|entry| entry.get("action") == Some(&json!("write-events")))
        .collect();
    assert_eq!(mine.len(), 1, "one decision, one row: {trail:?}");

    // The row is written *before* the action, so a request the handler then refuses still has one:
    // a trail written afterwards is silent about exactly the requests worth reading.
    let refused = fixture.send(
        Fixture::event_request()
            .body(br#"{"kind": 1}"#.to_vec())
            .header("idempotency-key", "bad-body")
            .header("x-request-id", "0badc0de-0000-4000-8000-000000000000"),
        Some(ACME.1),
    );
    assert_eq!(
        refused.status, 400,
        "the body is not an event: {}",
        refused.body
    );
    assert!(
        fixture.trail(ACME.1).iter().any(|entry| {
            entry.get("request_id") == Some(&json!("0badc0de-0000-4000-8000-000000000000"))
                && entry.get("decision") == Some(&json!("allow"))
        }),
        "the decision was made before the body was read, and the trail says so"
    );
}

/// A refused privileged action is in the trail too, and a read is not.
#[test]
fn a_refused_privileged_action_is_written_down_and_a_read_is_not() {
    let fixture = Fixture::start();
    let refused = fixture.send(
        Fixture::event_request()
            .header("idempotency-key", "refused")
            .header("x-request-id", "aaaabbbb-cccc-4ddd-8eee-ffff00001111"),
        Some(READER.1),
    );
    assert_eq!(refused.status, 403, "{}", refused.body);

    // Read with `psql` rather than through the API: a reader may not read the trail, and the row it
    // is about is the one this test is looking for.
    let stored = fixture.query(
        "select role || ' ' || action || ' ' || decision || ' ' || request_id \
           from audit_log order by at",
    );
    assert_eq!(
        stored.trim(),
        "tenant-reader write-events not-permitted aaaabbbb-cccc-4ddd-8eee-ffff00001111",
        "the refusal is a decision like any other, and it names the action that was refused"
    );

    // Reading leaves nothing behind: a trail of every read is a trail nobody can afford to keep.
    for path in [
        "/endpoints",
        "/events",
        "/dlq",
        "/openapi.json",
        "/healthz",
        "/audit",
    ] {
        fixture.as_("GET", path, ACME.1);
    }
    fixture.anonymous("GET", "/events");
    assert_eq!(
        fixture.query("select count(*) from audit_log"),
        "1",
        "the only row is the refused write: {}",
        fixture.query("select action || ' ' || decision from audit_log")
    );
    assert!(
        ACTIONS
            .iter()
            .filter(|action| action.is_privileged())
            .count()
            < ACTIONS.len(),
        "some actions are privileged and some are not — a model where everything is has no \
         privileged action to write down"
    );
}

/// The trail is one tenant's, and another tenant cannot see into it.
#[test]
fn the_trail_is_filtered_by_tenant() {
    let fixture = Fixture::start();
    let accepted = fixture.send(
        Fixture::event_request()
            .header("idempotency-key", "mine")
            .header("x-request-id", "11111111-2222-4333-8444-555555555555"),
        Some(ACME.1),
    );
    assert_eq!(accepted.status, 201, "{}", accepted.body);
    let theirs = fixture.send(
        Fixture::event_request()
            .header("idempotency-key", "theirs")
            .header("x-request-id", "66666666-7777-4888-8999-aaaaaaaaaaaa"),
        Some(GLOBEX.1),
    );
    assert_eq!(theirs.status, 201, "{}", theirs.body);

    let mine = fixture.trail(ACME.1);
    let theirs = fixture.trail(GLOBEX.1);
    let ids = |trail: &[Value]| -> BTreeSet<String> {
        trail
            .iter()
            .filter_map(|entry| entry.get("request_id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    };
    assert!(
        ids(&mine).contains("11111111-2222-4333-8444-555555555555"),
        "a tenant reads its own trail: {mine:?}"
    );
    assert!(
        !ids(&mine).contains("66666666-7777-4888-8999-aaaaaaaaaaaa"),
        "and not the one of the tenant next door: {mine:?}"
    );
    assert!(
        !ids(&theirs).contains("11111111-2222-4333-8444-555555555555"),
        "in either direction: {theirs:?}"
    );
    assert!(
        mine.iter()
            .all(|entry| entry.get("tenant") == Some(&json!("acme"))),
        "every row names the tenant it belongs to: {mine:?}"
    );
    assert!(
        theirs
            .iter()
            .all(|entry| entry.get("tenant") == Some(&json!("globex"))),
        "{theirs:?}"
    );
}

/// No header and no setting changes who the caller is.
#[test]
fn nothing_the_caller_sends_changes_who_it_is() {
    let invented = [
        ("x-role", "tenant-admin"),
        ("x-tenant-id", "acme"),
        ("x-tenant", "acme"),
        ("x-forwarded-tenant", "acme"),
        ("x-relay-role", "tenant-admin"),
        ("x-admin", "true"),
        ("role", "tenant-admin"),
    ];
    let fixture = Fixture::start();
    for (name, value) in invented {
        let answer = fixture.send(
            Fixture::event_request()
                .header(name, value)
                .header("idempotency-key", name),
            Some(READER.1),
        );
        assert_eq!(
            answer.status, 403,
            "`{name}: {value}` is a header, not an identity: {}",
            answer.body
        );
    }

    // A setting naming a role, or promising that there is none, is not read either: the keys below
    // are not settings of this service, and a service that grew one would have to grow it here.
    let settings = [
        ("RELAY__AUTHZ__ENABLED", "false"),
        ("RELAY__AUTHZ__BYPASS", "true"),
        ("RELAY__AUTH__BYPASS", "true"),
        ("RELAY__AUTHZ__TRUST_HEADERS", "true"),
        ("RELAY__AUTHZ__ROLE", "tenant-admin"),
        ("RELAY__SECURITY__DISABLED", "true"),
    ];
    let lax = Fixture::start_with(&settings);
    for (name, value) in settings {
        let answer = lax.send(
            Fixture::event_request()
                .header("idempotency-key", name)
                .header(name.to_lowercase().replace("__", "-").as_str(), value),
            Some(READER.1),
        );
        assert_eq!(
            answer.status, 403,
            "`{name}={value}` does not turn the matrix off: {}",
            answer.body
        );
    }
    assert_eq!(
        lax.anonymous("GET", "/events").status,
        401,
        "and nobody becomes somebody by setting a variable"
    );
}

/// The trail holds the decision, and never the credential it was made about.
#[test]
fn the_trail_never_carries_a_credential() {
    let fixture = Fixture::start();
    let created = fixture.send(
        Request::post("/endpoints", endpoint_body()).header("content-type", "application/json"),
        Some(ACME.1),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let secret = created
        .field("secret")
        .as_str()
        .expect("the endpoint's signing secret")
        .to_owned();
    assert!(secret.len() >= 32, "{secret}");

    let accepted = fixture.accept(ACME.1, "secret-free");
    assert_eq!(accepted.status, 201, "{}", accepted.body);

    let rendered = serde_json::to_string(&fixture.trail(ACME.1)).expect("the trail is JSON");
    for credential in [ACME.1, secret.as_str(), "Bearer"] {
        assert!(
            !rendered.contains(credential),
            "the trail is not a second copy of the credential: `{credential}` is in {rendered}"
        );
    }
    let stored = fixture.query(
        "select coalesce(string_agg(role || target || decision || request_id, ' '), '') \
           from audit_log",
    );
    for credential in [ACME.1, secret.as_str()] {
        assert!(
            !stored.contains(credential),
            "nor is any column of the table a place to keep one: `{credential}` is in {stored}"
        );
    }

    // The trail is read per tenant, newest first, and it is indexed for exactly that: without an
    // index in that shape the listing walks every decision the service has ever made.
    let indexes = fixture.query("select indexdef from pg_indexes where tablename = 'audit_log'");
    assert!(
        indexes
            .lines()
            .any(|line| line.contains("tenant_id") && line.contains("at")),
        "reading one tenant's trail, newest first, is the one query this table has: the index has \
         to be in that shape. The indexes here are:\n{indexes}"
    );
}

/// A tenant's role is a property of its row, and the row is the only thing that grants it.
#[test]
fn a_role_lives_in_the_tenant_row() {
    let fixture = Fixture::start();
    let roles = fixture.query("select name || '=' || role from tenants order by name");
    assert_eq!(
        roles.lines().map(str::trim).collect::<Vec<_>>(),
        vec![
            "acme=tenant-admin",
            "acme-reader=tenant-reader",
            "globex=tenant-admin"
        ],
        "the configuration seeded the roles it names"
    );
    assert!(
        fixture
            .query(
                "select count(*) from information_schema.table_constraints \
                  where table_name = 'tenants' and constraint_type = 'CHECK'"
            )
            .trim()
            .parse::<i64>()
            .unwrap_or_default()
            >= 1,
        "a role the service does not have is not a role a row may hold"
    );
    assert_eq!(
        authz::decide(Role::Reader, Action::WriteEvents, Scope::Own),
        Decision::Deny(Reason::NotPermitted),
        "and the refusal a reader gets for a write is the one the route answers with"
    );
    assert_eq!(
        authz::decide(Role::Anonymous, Action::ReadEvents, Scope::Own),
        Decision::Deny(Reason::Unauthenticated),
        "while a request with no usable key is refused for the other reason"
    );
}
