//! Spec for `mailer/1.11-signup`: a signup stores a subscriber whose password is hashed, and the
//! unique constraint — not a read in the application — decides who is already subscribed.
//!
//! `kiln run` copies this file into the project as `tests/spec_signup.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * through the route itself (`TestApp`, step `mailer/1.8-test-harness`), because "signup works"
//!   is a claim about a request;
//! * through Postgres (`psql`, step `mailer/1.9-postgres`'s container harness), because "the
//!   password is hashed" and "there is one row" are claims about what was stored — a service that
//!   answers 201 and stores nothing passes a test that only reads the answer;
//! * **at the same time** for the race: two requests for one address arrive together (the barrier
//!   below is what makes "together" mean together), and exactly one of them may win.
//!
//! Nothing here sleeps its way out of a race, and nothing waits without a deadline: a service that
//! is wrong has to fail this file loudly, not hang it (the deadline is enforced in Rust, since this
//! machine has no `timeout(1)`).

#[path = "support/postgres.rs"]
mod postgres;
#[allow(dead_code)]
mod support;

use std::sync::Barrier;
use std::thread;

use mailer::config::{Config, Flags};
use postgres::Postgres;
use serde_json::json;
use support::TestApp;

/// A password the domain accepts, used wherever the test is not about the password.
const PASSWORD: &str = "correct horse battery staple";
/// How many signups arrive at once in the race test.
///
/// More than two on purpose: the property is "however many arrive together, exactly one wins", and
/// four callers make the window between a `SELECT` and an `INSERT` — the shape this step is against
/// — much harder to slip through.
const RACERS: usize = 4;

/// The configuration a test starts the service with: the defaults, plus what the test adds.
fn config(env: &[(String, String)]) -> Config {
    Config::load(&Flags::default(), env).expect("the fixture configuration is valid")
}

/// The setting a service reads its database from (step `mailer/1.4-config`).
fn database_setting(url: &str) -> (String, String) {
    ("MAILER__DATABASE__URL".to_owned(), url.to_owned())
}

/// One signup body, as a caller writes it.
fn signup_body(email: &str, name: &str, password: &str) -> String {
    json!({ "email": email, "name": name, "password": password }).to_string()
}

/// The row count of the one table this step writes.
fn rows(postgres: &Postgres) -> String {
    postgres.query("SELECT count(*) FROM subscribers")
}

#[test]
fn a_signup_stores_the_subscriber_it_answered_for() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    // Sent the way a form sends it: the address in another case, the name with padding and runs of
    // spaces. What is stored is what step `mailer/1.10-domain-types` parsed, not what arrived.
    let (status, body) = app.post(
        "/signup",
        &signup_body(" Ada@Example.COM ", "  Ada   Lovelace ", PASSWORD),
    );
    assert_eq!(status, 201, "a valid signup has to be created: {body}");
    assert!(
        body.contains("\"pending\""),
        "the answer has to say where the subscriber is: {body}"
    );
    assert!(
        body.contains("ada@example.com"),
        "the answer has to name the address as it was stored: {body}"
    );
    assert!(
        !body.contains(PASSWORD),
        "a password must never come back out of the service: {body}"
    );
    assert!(
        !body.contains("$argon2"),
        "the stored hash is not the caller's business either: {body}"
    );

    assert_eq!(
        postgres.query("SELECT email FROM subscribers"),
        "ada@example.com",
        "the address has to be stored normalised (the unique constraint compares bytes)"
    );
    assert_eq!(
        postgres.query("SELECT name FROM subscribers"),
        "Ada Lovelace",
        "the name has to be stored as the parser produced it"
    );
    assert_eq!(
        postgres.query("SELECT status FROM subscribers"),
        "pending",
        "a signup starts unconfirmed: `active` means somebody proved the address"
    );

    let hash = postgres.query("SELECT password_hash FROM subscribers");
    assert_ne!(
        hash, PASSWORD,
        "the password was stored as it was typed: a table like that is a breach the day it is read"
    );
    assert!(
        hash.starts_with("$argon2id$"),
        "the stored value has to be a PHC string of the algorithm this service chose: {hash}"
    );

    // The point of the salt: one password, two subscribers, two stored values.
    let (status, body) = app.post(
        "/signup",
        &signup_body("grace@example.com", "Grace Hopper", PASSWORD),
    );
    assert_eq!(status, 201, "{body}");
    assert_eq!(
        postgres.query("SELECT count(DISTINCT password_hash) FROM subscribers"),
        "2",
        "the same password twice has to be two different stored values"
    );
    app.stop();
}

#[test]
fn an_address_that_already_has_a_subscriber_is_a_conflict() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    let (status, body) = app.post(
        "/signup",
        &signup_body("ada@example.com", "Ada Lovelace", PASSWORD),
    );
    assert_eq!(status, 201, "{body}");

    // The same address, spelled differently: normalisation is what makes this a conflict rather
    // than a second subscriber (step `mailer/1.10-domain-types`).
    let (status, body) = app.post(
        "/signup",
        &signup_body("ADA@Example.com", "Somebody Else", PASSWORD),
    );
    assert_eq!(
        status, 409,
        "a second signup for a taken address is a conflict, not a 201 and not a 500: {body}"
    );
    assert!(
        body.contains("ada@example.com"),
        "the answer has to name the address that is taken: {body}"
    );
    assert_eq!(
        rows(&postgres),
        "1",
        "the first signup's row is the only one"
    );
    assert_eq!(
        postgres.query("SELECT name || '|' || status FROM subscribers"),
        "Ada Lovelace|pending",
        "the second request must not have changed the subscriber that was there"
    );
    app.stop();
}

#[test]
fn concurrent_signups_for_one_address_leave_exactly_one_subscriber() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    // Every thread waits at the barrier, so the requests leave together: the last line of a race is
    // the one that decides, and this is how a test gets to the last line on purpose.
    let barrier = Barrier::new(RACERS);
    let bodies: Vec<String> = (0..RACERS)
        .map(|racer| signup_body("race@example.com", &format!("Racer {racer}"), PASSWORD))
        .collect();

    let answers: Vec<(u16, String)> = thread::scope(|scope| {
        let racing: Vec<_> = bodies
            .iter()
            .map(|body| {
                scope.spawn(|| {
                    barrier.wait();
                    app.post("/signup", body)
                })
            })
            .collect();
        racing
            .into_iter()
            .map(|racer| {
                racer
                    .join()
                    .expect("a racing request must answer rather than panic")
            })
            .collect()
    });

    let mut codes: Vec<u16> = answers.iter().map(|(status, _)| *status).collect();
    codes.sort_unstable();
    let mut expected = vec![201];
    expected.extend(std::iter::repeat_n(409, RACERS - 1));
    assert_eq!(
        codes,
        expected,
        "concurrent signups for one address have to be decided by the unique constraint: one 201 \
         and {} 409, never two winners and never a 500 ({})",
        RACERS - 1,
        answers
            .iter()
            .map(|(status, body)| format!("{status}: {body}"))
            .collect::<Vec<_>>()
            .join("; ")
    );
    assert_eq!(
        rows(&postgres),
        "1",
        "a race that leaves two rows is a race the database lost"
    );
    app.stop();
}

#[test]
fn a_value_the_domain_refuses_is_a_bad_request_and_writes_nothing() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    // (email, name, password, the field the answer has to name, a value that must not be repeated).
    let refused = [
        (
            "not-an-address",
            "Ada Lovelace",
            PASSWORD,
            "email",
            "not-an-address",
        ),
        (
            "ada@example.com",
            "Ada\nLovelace",
            PASSWORD,
            "name",
            "Ada\nLovelace",
        ),
        ("ada@example.com", "Ada Lovelace", "a-9", "password", "a-9"),
    ];
    for (email, name, password, field, secret) in refused {
        let (status, body) = app.post("/signup", &signup_body(email, name, password));
        assert_eq!(
            status, 400,
            "a refused {field} is the caller's to fix, not a 500: {body}"
        );
        assert!(
            body.contains(field),
            "the answer has to name the field it refused: {body}"
        );
        assert!(
            !body.contains(secret),
            "the answer must not repeat the value it refused — it may be a password: {body}"
        );
    }
    assert_eq!(
        rows(&postgres),
        "0",
        "a request the domain refused must not leave a row behind"
    );
    app.stop();
}

#[test]
fn a_body_this_route_cannot_use_is_a_bad_request() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    let unusable = [
        // A field is missing.
        r#"{"email":"ada@example.com","name":"Ada Lovelace"}"#,
        // Not JSON at all.
        "definitely not json",
        // JSON, and not an object a signup can come from.
        "[1, 2, 3]",
        // Nothing.
        "",
    ];
    for body in unusable {
        let (status, answer) = app.post("/signup", body);
        assert_eq!(status, 400, "`{body}` is not a signup: {answer}");
        assert!(
            !answer.contains("definitely not json"),
            "the answer must not quote the body back: {answer}"
        );
    }
    assert_eq!(rows(&postgres), "0", "none of this can have created a row");
    app.stop();
}

#[test]
fn a_signup_without_a_database_is_not_ready() {
    // No database setting: the service starts anyway (step `mailer/1.9-postgres`), and this is the
    // request it cannot answer. It is a 503 — "come back when I am ready" — and not a 500: nothing
    // is broken, a dependency is simply not there yet.
    let app = TestApp::start(config(&[]));
    let (status, body) = app.post(
        "/signup",
        &signup_body("ada@example.com", "Ada Lovelace", PASSWORD),
    );
    assert_eq!(status, 503, "{body}");
    assert_eq!(
        app.get("/healthz").0,
        200,
        "the process is alive: liveness is not readiness"
    );
    app.stop();
}
