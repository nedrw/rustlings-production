//! Spec for `mailer/1.17-newsletter`: publishing an issue is one write to the database and a
//! background fan-out, and every confirmed subscriber is owed exactly one copy.
//!
//! `kiln run` copies this file into the project as `tests/spec_newsletter.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * through a **real process** (`tests/support/service.rs`), because publishing is an
//!   authenticated route: it needs a session cookie, and the answer is what a client would see;
//! * through `psql` (`tests/support/postgres.rs`), because "this issue is owed to these subscribers"
//!   is a claim about rows — the issue, the copies, and what the copies say about themselves;
//! * through the service's **own log**, because the adapter this build runs with writes what it was
//!   given there (step `mailer/1.13-email-port`), and "who received a copy" is exactly what those
//!   lines name.
//!
//! Nothing here sleeps to synchronise: the log is waited for, and every wait has a deadline.

#[path = "support/postgres.rs"]
mod postgres;
#[path = "support/service.rs"]
mod service;

use std::thread;
use std::time::{Duration, Instant};

use postgres::Postgres;
use serde_json::Value;
use service::{Response, Service};

/// How often a waiting test looks again.
const RETRY: Duration = Duration::from_millis(20);
/// How long a test gives the service to log something that should *not* happen.
const SETTLE: Duration = Duration::from_millis(1000);

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
fn issue_body(subject: &str, body: &str) -> String {
    serde_json::json!({ "subject": subject, "body": body }).to_string()
}

/// Subscribe an address and confirm it, so there is a subscriber an issue is owed to.
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

/// Publish one issue, as an authenticated client.
fn publish(service: &Service, cookie: &str, subject: &str, body: &str) -> Response {
    service.post(
        "/newsletter",
        &[("Cookie", cookie)],
        &issue_body(subject, body),
    )
}

/// Every delivery of one subject the service has logged, with repeats: one line per message the
/// adapter was given.
///
/// The subject is what tells an issue apart from a confirmation email in the log — both are the same
/// adapter writing the same kind of line (step `mailer/1.13-email-port`), and a test that did not say
/// which one it was counting would count both.
fn delivered_recipients(service: &Service, subject: &str) -> Vec<String> {
    service
        .log_lines()
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["fields"]["message"] == "an email would be sent")
        .filter(|event| event["fields"]["subject"] == subject)
        .filter_map(|event| event["fields"]["to"].as_str().map(str::to_owned))
        .collect()
}

/// Wait until `count` deliveries have been logged, then give the service a moment to log any that
/// should not exist, and answer every recipient line in order.
///
/// The wait is the positive claim ("two copies were sent") and the settle window is the budget the
/// *negative* one needs ("and nobody else was written to"): a line that should not exist can only be
/// observed after the moment it would have arrived. The window is short on purpose — the fan-out
/// queues everything it is going to queue before it answers, so the lines follow within milliseconds.
fn settled_recipients(service: &Service, subject: &str, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    // Reading the log *drains* it, so what has been read is kept here: a helper that looked twice
    // and threw the first look away would forget the lines it was waiting for.
    let mut seen = delivered_recipients(service, subject);
    while seen.len() < count {
        assert!(
            Instant::now() < deadline,
            "only {} of {count} copies were delivered within 10s: {seen:?}",
            seen.len()
        );
        thread::sleep(RETRY);
        seen.extend(delivered_recipients(service, subject));
    }
    thread::sleep(SETTLE);
    seen.extend(delivered_recipients(service, subject));
    seen
}

/// One column of one issue, read with `psql`.
fn count(postgres: &Postgres, query: &str) -> String {
    postgres.query(query)
}

#[test]
fn a_publish_records_the_issue_and_the_copies_it_is_owed_to() {
    let postgres = Postgres::start();
    let service = Service::start(&[
        ("MAILER__DATABASE__URL", &postgres.url()),
        // The adapter's lines are this spec's observations, so the service has to log them in a
        // shape a test can read (step `mailer/1.6-logging`).
        ("MAILER__LOG__FORMAT", "json"),
    ]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");
    confirmed_subscriber(&service, &postgres, "grace@example.com");
    // Somebody who signed up and never confirmed is not a subscriber an issue is owed to: the
    // address was never proven, and confirmation would mean nothing if this were the exception.
    let answer = service.post("/signup", &[], &signup_body("unconfirmed@example.com"));
    assert_eq!(answer.status, 201, "{}", answer.body);

    let cookie = session_header(&service, "ada@example.com");
    let answer = publish(&service, &cookie, "First issue", "Hello, subscribers.");
    assert_eq!(
        answer.status, 202,
        "publishing accepts the work rather than finishing it: {} {}",
        answer.status, answer.body
    );
    let body: Value = serde_json::from_str(&answer.body).expect("the answer is JSON");
    assert_eq!(
        body["deliveries"], 2,
        "two confirmed subscribers are owed this issue, and the answer says so: {}",
        answer.body
    );

    assert_eq!(
        count(&postgres, "SELECT count(*) FROM issues"),
        "1",
        "one publish is one issue"
    );
    assert_eq!(
        count(
            &postgres,
            "SELECT count(*) FROM issue_deliveries \
              JOIN subscribers ON subscribers.id = issue_deliveries.subscriber_id \
             WHERE subscribers.status = 'active'"
        ),
        "2",
        "the issue is owed to exactly the confirmed subscribers"
    );
    assert_eq!(
        count(
            &postgres,
            "SELECT count(*) FROM issue_deliveries \
              JOIN subscribers ON subscribers.id = issue_deliveries.subscriber_id \
             WHERE subscribers.email = 'unconfirmed@example.com'"
        ),
        "0",
        "an address nobody confirmed gets no copy of anything"
    );
    assert_eq!(
        count(
            &postgres,
            "SELECT count(*) FROM issue_deliveries WHERE issue_id IS NULL"
        ),
        "0",
        "every copy points at the issue it belongs to: the two rows are one write, not two"
    );
}

#[test]
fn an_unauthenticated_publish_is_refused() {
    let postgres = Postgres::start();
    let service = Service::start(&[
        ("MAILER__DATABASE__URL", &postgres.url()),
        // The adapter's lines are this spec's observations, so the service has to log them in a
        // shape a test can read (step `mailer/1.6-logging`).
        ("MAILER__LOG__FORMAT", "json"),
    ]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");

    let answer = service.post(
        "/newsletter",
        &[],
        &issue_body("First issue", "Hello, subscribers."),
    );
    assert_eq!(
        answer.status, 401,
        "publishing is an act by a subscriber, and a request without a session is nobody: {}",
        answer.body
    );
    assert_eq!(
        count(&postgres, "SELECT count(*) FROM issues"),
        "0",
        "a refused publish writes nothing"
    );

    // A session that is not one is the same answer as no session at all.
    let answer = service.post(
        "/newsletter",
        &[("Cookie", &format!("session={}", "0".repeat(64)))],
        &issue_body("First issue", "Hello, subscribers."),
    );
    assert_eq!(answer.status, 401, "{}", answer.body);
}

#[test]
fn a_rejected_issue_leaves_nothing_behind() {
    let postgres = Postgres::start();
    let service = Service::start(&[
        ("MAILER__DATABASE__URL", &postgres.url()),
        // The adapter's lines are this spec's observations, so the service has to log them in a
        // shape a test can read (step `mailer/1.6-logging`).
        ("MAILER__LOG__FORMAT", "json"),
    ]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");
    let cookie = session_header(&service, "ada@example.com");

    // A subject that is not a subject: refused before anything is written, with the field named
    // (step `mailer/1.10-domain-types` made the same promise for a signup body).
    let answer = publish(&service, &cookie, "   ", "Hello, subscribers.");
    assert_eq!(
        answer.status, 400,
        "an issue has to have a subject: {} {}",
        answer.status, answer.body
    );
    assert!(
        answer.body.contains("subject"),
        "and the answer has to name the field: {}",
        answer.body
    );

    assert_eq!(
        count(&postgres, "SELECT count(*) FROM issues"),
        "0",
        "a publish that was refused wrote an issue nobody will receive"
    );
    assert_eq!(
        count(&postgres, "SELECT count(*) FROM issue_deliveries"),
        "0",
        "and no copies of it"
    );
}

#[test]
fn every_confirmed_subscriber_receives_one_copy() {
    let postgres = Postgres::start();
    let service = Service::start(&[
        ("MAILER__DATABASE__URL", &postgres.url()),
        // The adapter's lines are this spec's observations, so the service has to log them in a
        // shape a test can read (step `mailer/1.6-logging`).
        ("MAILER__LOG__FORMAT", "json"),
    ]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");
    confirmed_subscriber(&service, &postgres, "grace@example.com");
    let answer = service.post("/signup", &[], &signup_body("unconfirmed@example.com"));
    assert_eq!(answer.status, 201, "{}", answer.body);

    let cookie = session_header(&service, "ada@example.com");
    let answer = publish(&service, &cookie, "First issue", "Hello, subscribers.");
    assert_eq!(answer.status, 202, "{}", answer.body);

    let mut recipients = settled_recipients(&service, "First issue", 2);
    recipients.sort();
    assert_eq!(
        recipients,
        vec!["ada@example.com".to_owned(), "grace@example.com".to_owned()],
        "an issue goes to every confirmed subscriber once, and to nobody else"
    );
    assert_eq!(
        recipients.len(),
        2,
        "one copy per subscriber: a second delivery would be a second line"
    );

    // And the rows agree with the log: a copy that was sent says so, and the count of attempts is
    // how hard the worker had to try (step `mailer/1.15-idempotent-delivery`).
    assert_eq!(
        count(
            &postgres,
            "SELECT count(*) FROM issue_deliveries WHERE sent_at IS NOT NULL AND claimed_at IS NULL"
        ),
        "2",
        "a delivered copy is recorded as sent, and nobody is still holding it"
    );
    assert_eq!(
        count(
            &postgres,
            "SELECT count(*) FROM issue_deliveries WHERE sent_at IS NOT NULL AND attempts = 1"
        ),
        "2",
        "each copy was claimed once and sent once: a second attempt would be an attempt the sender \
         never had to make"
    );
    assert_eq!(
        count(
            &postgres,
            "SELECT count(*) FROM issue_deliveries \
              JOIN subscribers ON subscribers.id = issue_deliveries.subscriber_id \
             WHERE subscribers.email = 'unconfirmed@example.com'"
        ),
        "0",
        "the unconfirmed address was never on the receiving end of anything"
    );
}
