//! Spec for `mailer/1.18-authz-boundary`: a subscriber reads their own data and cannot tell whether
//! anybody else's exists.
//!
//! `kiln run` copies this file into the project as `tests/spec_authz.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The route under test is `GET /subscribers/{id}/deliveries`, and
//! everything is judged from outside a **real process** (`tests/support/service.rs`) with `psql` for
//! the setup: the claim is about who may see what, and that is a claim about answers.
//!
//! What the tests are careful about:
//!
//! * **the id in the path is never trusted.** The fixture asks for somebody else's id and for an id
//!   that does not exist, and the two answers have to be indistinguishable — including the body,
//!   because a sentence that names the resource is a sentence that says it is there;
//! * **a session is still required.** Being authenticated does not make every id yours: no session is
//!   a `401`, and somebody else's id is a `404`;
//! * **the list is the caller's own.** A subscriber's history is the copies *they* were owed, in the
//!   order they were published.

#[path = "support/postgres.rs"]
mod postgres;
#[path = "support/service.rs"]
mod service;

use postgres::Postgres;
use serde_json::Value;
use service::Service;

/// The password the fixture subscribes with.
const PASSWORD: &str = "correct horse battery staple";

/// The body of `POST /signup`.
fn signup_body(email: &str) -> String {
    format!(r#"{{"email":"{email}","name":"Ada Lovelace","password":"{PASSWORD}"}}"#)
}

/// The body of `POST /login`.
fn login_body(email: &str) -> String {
    format!(r#"{{"email":"{email}","password":"{PASSWORD}"}}"#)
}

/// The body of `POST /newsletter`.
fn issue_body(subject: &str) -> String {
    serde_json::json!({ "subject": subject, "body": "Hello, subscribers." }).to_string()
}

/// Subscribe an address and confirm it, so there is a subscriber who may log in.
fn confirmed_subscriber(service: &Service, postgres: &Postgres, email: &str) {
    let answer = service.post("/signup", &[], &signup_body(email));
    assert_eq!(
        answer.status, 201,
        "the fixture signup failed: {}",
        answer.body
    );
    let token = postgres.query(&format!(
        "SELECT confirmation_token FROM subscribers WHERE email = '{email}'"
    ));
    let answer = service.get(&format!("/confirm?token={token}"));
    assert_eq!(
        answer.status, 200,
        "the fixture confirmation failed: {}",
        answer.body
    );
}

/// Log one subscriber in and answer the cookie header their session travels in.
fn session_header(service: &Service, email: &str) -> String {
    let answer = service.post("/login", &[], &login_body(email));
    assert_eq!(
        answer.status, 200,
        "the fixture login failed: {}",
        answer.body
    );
    let cookie = answer
        .header("set-cookie")
        .expect("a login sets a cookie")
        .to_owned();
    let token = cookie
        .split_once(';')
        .and_then(|(pair, _)| pair.split_once('='))
        .map(|(_, value)| value.trim().to_owned())
        .expect("`Set-Cookie` is `name=value` with attributes after it");
    format!("session={token}")
}

/// The id of a subscriber, read with `psql`.
fn subscriber_id(postgres: &Postgres, email: &str) -> i64 {
    postgres
        .query(&format!(
            "SELECT id FROM subscribers WHERE email = '{email}'"
        ))
        .trim()
        .parse()
        .expect("`id` is a number")
}

/// Publish one issue, as an authenticated client.
fn publish(service: &Service, cookie: &str, subject: &str) {
    let answer = service.post("/newsletter", &[("Cookie", cookie)], &issue_body(subject));
    assert_eq!(
        answer.status, 202,
        "the fixture publish failed: {} {}",
        answer.status, answer.body
    );
}

/// A service with two confirmed subscribers, one published issue, and both sessions.
fn fixture() -> (Postgres, Service, i64, i64, String) {
    let postgres = Postgres::start();
    let service = Service::start(&[
        ("MAILER__DATABASE__URL", &postgres.url()),
        ("MAILER__LOG__FORMAT", "json"),
    ]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");
    confirmed_subscriber(&service, &postgres, "grace@example.com");
    let ada = subscriber_id(&postgres, "ada@example.com");
    let grace = subscriber_id(&postgres, "grace@example.com");

    let cookie = session_header(&service, "ada@example.com");
    publish(&service, &cookie, "First issue");
    (postgres, service, ada, grace, cookie)
}

#[test]
fn a_subscriber_reads_their_own_deliveries() {
    let (_postgres, service, ada, _grace, cookie) = fixture();

    let answer = service.get_with(
        &format!("/subscribers/{ada}/deliveries"),
        &[("Cookie", &cookie)],
    );
    assert_eq!(
        answer.status, 200,
        "a subscriber may read their own history: {} {}",
        answer.status, answer.body
    );
    let history: Value = serde_json::from_str(&answer.body).expect("the history is JSON");
    let rows = history.as_array().expect("the history is a list");
    assert_eq!(
        rows.len(),
        1,
        "one published issue is one copy for this subscriber: {}",
        answer.body
    );
    assert_eq!(
        rows[0]["subject"], "First issue",
        "and the copy says which issue it is: {}",
        answer.body
    );
    assert!(
        rows[0]["sent_at"].is_string(),
        "the fixture's issue was delivered, and the row says when: {}",
        answer.body
    );
}

#[test]
fn somebody_elses_deliveries_do_not_exist_for_this_caller() {
    let (_postgres, service, _ada, grace, cookie) = fixture();

    let answer = service.get_with(
        &format!("/subscribers/{grace}/deliveries"),
        &[("Cookie", &cookie)],
    );
    assert_eq!(
        answer.status, 404,
        "another subscriber's history is not this caller's to read: {} {}",
        answer.status, answer.body
    );
    assert!(
        !answer.body.contains("grace"),
        "and the answer must not even say whose data was asked for: {}",
        answer.body
    );

    // An id that was never issued has to be the same answer, in the same shape: the difference
    // between "that is somebody else's" and "that does not exist" is exactly what an attacker would
    // use this route to learn.
    let missing = service.get_with("/subscribers/999999/deliveries", &[("Cookie", &cookie)]);
    assert_eq!(
        missing.status, 404,
        "a subscriber who does not exist is the same answer: {} {}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.body, answer.body,
        "the two answers have to be indistinguishable, body included"
    );
}

#[test]
fn the_boundary_needs_a_session_and_a_session_is_not_enough() {
    let (_postgres, service, ada, _grace, _cookie) = fixture();

    let without = service.get(&format!("/subscribers/{ada}/deliveries"));
    assert_eq!(
        without.status, 401,
        "without a session there is nobody to authorize: {} {}",
        without.status, without.body
    );

    let invented = service.get_with(
        &format!("/subscribers/{ada}/deliveries"),
        &[("Cookie", &format!("session={}", "0".repeat(64)))],
    );
    assert_eq!(
        invented.status, 401,
        "and a token nobody was issued is not a session: {} {}",
        invented.status, invented.body
    );
}

#[test]
fn one_subscribers_history_is_not_another_ones() {
    let (postgres, service, ada, grace, cookie) = fixture();

    // A second issue, published by grace, so the two histories cannot be the same list by accident.
    let grace_cookie = session_header(&service, "grace@example.com");
    publish(&service, &grace_cookie, "Second issue");

    let adas: Value = serde_json::from_str(
        &service
            .get_with(
                &format!("/subscribers/{ada}/deliveries"),
                &[("Cookie", &cookie)],
            )
            .body,
    )
    .expect("the history is JSON");
    let graces: Value = serde_json::from_str(
        &service
            .get_with(
                &format!("/subscribers/{grace}/deliveries"),
                &[("Cookie", &grace_cookie)],
            )
            .body,
    )
    .expect("the history is JSON");

    let subjects = |history: &Value| -> Vec<String> {
        history
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|row| row["subject"].as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(
        subjects(&adas),
        vec!["Second issue".to_owned(), "First issue".to_owned()],
        "one issue, one copy: every subscriber's history is the list of issues *they* were owed"
    );
    assert_eq!(
        subjects(&graces),
        vec!["Second issue".to_owned(), "First issue".to_owned()],
        "and it is a list of copies, not of issues: both subscribers were owed both issues"
    );

    // The rows in the database are the source of that answer, and they belong to one subscriber
    // each: a query that forgot its `WHERE` would return every copy to everybody.
    assert_eq!(
        postgres.query(
            "SELECT count(*) FROM issue_deliveries \
              JOIN subscribers ON subscribers.id = issue_deliveries.subscriber_id \
             WHERE subscribers.email = 'ada@example.com'"
        ),
        "2",
        "the fixture has two copies for ada, so a leaked history would be visible"
    );
}
