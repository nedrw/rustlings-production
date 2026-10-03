//! Spec for `mailer/1.19-test-layering`: the email a person would receive, sent by the real binary, to
//! a real mail server, and the link it carries.
//!
//! `kiln run` copies this file into the project as `tests/spec_integration.rs`; it is the course's
//! file, not the learner's (plan §3 D2). This is the top of the test pyramid and it is deliberately
//! thin: a **real process** (`tests/support/service.rs`), a **real mail server**
//! (`tests/support/mailpit.rs`), a **real database** (`tests/support/postgres.rs`), and assertions
//! that would be meaningless in any of the layers below.
//!
//! What is worth testing here and nowhere else:
//!
//! * **the protocol.** The fake sender answers what this service asks; a mail server answers what
//!   SMTP says. A message that never leaves the process cannot be wrong about headers, encodings or
//!   envelope addresses — and those are the parts that break in production;
//! * **the link.** The whole point of the confirmation email is that a person can click it, so the
//!   spec takes the link out of the delivered body and follows it: a token that is not the one in the
//!   row, or a URL that lost a character on the way, fails here and could not fail anywhere else;
//! * **the failure**, now that the adapter can fail: a mail server that is away is a delivery that is
//!   retried and given up on (steps `mailer/1.14-worker` and `mailer/1.15-idempotent-delivery`), and
//!   one that accepts and never answers is a process that runs out of drain deadline and says so with
//!   its exit code.

#[path = "support/mailpit.rs"]
mod mailpit;
#[path = "support/postgres.rs"]
mod postgres;
#[path = "support/service.rs"]
mod service;

use std::net::TcpListener;
use std::time::{Duration, Instant};

use mailpit::Mailpit;
use postgres::Postgres;
use serde_json::Value;
use service::Service;

/// The password the fixture subscribes with.
const PASSWORD: &str = "correct horse battery staple";

/// The body of `POST /signup`.
fn signup_body(email: &str) -> String {
    format!(r#"{{"email":"{email}","name":"Ada Lovelace","password":"{PASSWORD}"}}"#)
}

/// One column of one subscriber, read with `psql`.
fn column(postgres: &Postgres, email: &str, column: &str) -> String {
    postgres.query(&format!(
        "SELECT {column} FROM subscribers WHERE email = '{email}'"
    ))
}

/// Wait until `check` answers something, or fail loudly with what the test was waiting for.
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within 20s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The settings that point a service at one mail server (step `mailer/1.4-config`).
fn smtp_settings(mailpit: &Mailpit) -> Vec<(String, String)> {
    let (host, port) = mailpit.smtp();
    vec![
        ("MAILER__EMAIL__SMTP_HOST".to_owned(), host),
        ("MAILER__EMAIL__SMTP_PORT".to_owned(), port.to_string()),
        ("MAILER__LOG__FORMAT".to_owned(), "json".to_owned()),
    ]
}

/// A service started with a database, a mail server and the settings that connect them.
fn service_with(settings: &[(String, String)], database: &str) -> Service {
    let mut env: Vec<(&str, &str)> = vec![("MAILER__DATABASE__URL", database)];
    env.extend(
        settings
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    );
    Service::start(&env)
}

/// The confirmation link in a message body.
fn link_in(body: &str) -> String {
    body.split_whitespace()
        .find(|word| word.contains("/confirm?token="))
        .unwrap_or_else(|| panic!("the body has to carry a confirmation link: `{body}`"))
        .trim_end_matches(['.', ','])
        .to_owned()
}

#[test]
fn a_signup_sends_a_confirmation_link_a_person_can_click() {
    let postgres = Postgres::start();
    let mailpit = Mailpit::start();
    let settings = smtp_settings(&mailpit);
    let service = service_with(&settings, &postgres.url());

    let answer = service.post("/signup", &[], &signup_body("ada@example.com"));
    assert_eq!(answer.status, 201, "{}", answer.body);

    // The message has to *arrive*, which is the claim every layer below took on trust.
    let message = wait_for("the confirmation email to arrive", || {
        mailpit.messages().first().cloned()
    });
    assert!(
        message["To"][0]["Address"] == "ada@example.com",
        "the message has to go to the address as it was stored: {message}"
    );
    assert!(
        message["Subject"]
            .as_str()
            .is_some_and(|subject| !subject.is_empty()),
        "a message without a subject is a message that gets reported as spam: {message}"
    );

    let id = message["ID"].as_str().expect("a message has an id");
    let body = mailpit.text_of(id);
    let link = link_in(&body);
    let token = column(&postgres, "ada@example.com", "confirmation_token");
    assert!(
        link.ends_with(&token),
        "the link in the delivered body has to carry the token that is in the row: `{link}`"
    );

    // And the link works: this is the one assertion no fake could make.
    let path = link
        .split_once("/confirm")
        .map(|(_, rest)| format!("/confirm{rest}"))
        .expect("the link points at /confirm");
    let answer = service.get(&path);
    assert_eq!(
        answer.status, 200,
        "the link a mail server delivered has to confirm the subscriber: {} {}",
        answer.status, answer.body
    );
    assert_eq!(
        column(&postgres, "ada@example.com", "status"),
        "active",
        "and the row says so"
    );
}

#[test]
fn a_mail_server_that_is_away_is_a_delivery_that_was_given_up_on() {
    let postgres = Postgres::start();
    // A port nothing listens on: the mail server that is not there, as a client experiences it.
    let closed = TcpListener::bind("127.0.0.1:0")
        .expect("a free port can be found")
        .local_addr()
        .expect("a bound listener has an address");
    let settings = vec![
        (
            "MAILER__EMAIL__SMTP_HOST".to_owned(),
            "127.0.0.1".to_owned(),
        ),
        (
            "MAILER__EMAIL__SMTP_PORT".to_owned(),
            closed.port().to_string(),
        ),
        ("MAILER__LOG__FORMAT".to_owned(), "json".to_owned()),
    ];
    let service = service_with(&settings, &postgres.url());

    let answer = service.post("/signup", &[], &signup_body("ada@example.com"));
    assert_eq!(
        answer.status, 201,
        "a mail server that is away is a delivery problem, not a signup that failed: {}",
        answer.body
    );

    // The worker retries, gives up, and releases the claim (step `mailer/1.15-idempotent-delivery`):
    // every one of those is visible in the row, and none of them was observable through a fake that
    // could not fail.
    let attempts: u32 = wait_for("the failed delivery to be given up on", || {
        let counted = column(&postgres, "ada@example.com", "confirmation_attempts");
        let attempts = counted.parse::<u32>().unwrap_or(0);
        (attempts >= 3).then_some(attempts)
    });
    assert_eq!(
        attempts, 3,
        "three attempts, then the message is left owed rather than retried forever"
    );
    assert_eq!(
        column(&postgres, "ada@example.com", "confirmation_sent_at"),
        "",
        "nothing was delivered, so nothing may be recorded as delivered"
    );
    assert_eq!(
        column(&postgres, "ada@example.com", "status"),
        "pending",
        "and the subscriber is still waiting for a link that never came"
    );
}

#[test]
fn a_mail_server_that_never_answers_runs_out_of_drain_deadline() {
    let postgres = Postgres::start();

    // A mail server that accepts the connection and then says nothing: the case the drain deadline
    // of step `mailer/1.14-worker` exists for. Nothing reads or accepts on this listener, so the
    // connection is established (the backlog holds it) and the conversation never starts — which is
    // exactly what a wedged mail server looks like from inside the sending process.
    let silent = TcpListener::bind("127.0.0.1:0").expect("a free port can be found");
    let silent_port = silent
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    let settings = vec![
        (
            "MAILER__EMAIL__SMTP_HOST".to_owned(),
            "127.0.0.1".to_owned(),
        ),
        (
            "MAILER__EMAIL__SMTP_PORT".to_owned(),
            silent_port.to_string(),
        ),
        ("MAILER__LOG__FORMAT".to_owned(), "json".to_owned()),
    ];
    let mut service = service_with(&settings, &postgres.url());

    let answer = service.post("/signup", &[], &signup_body("ada@example.com"));
    assert_eq!(answer.status, 201, "{}", answer.body);

    // The worker is now in an SMTP conversation that will not finish. Stopping the process is
    // supposed to be a drain with a deadline, not a wait: ten seconds later it gives up, says so,
    // and exits with the failure code of step `mailer/1.5-errors`.
    service.stop();
    let code = service.wait_for_exit();
    assert_eq!(
        code,
        1,
        "a process that could not finish what it started has to say so with its exit code \
         (0 would be a lie): {:?}",
        service.log_lines()
    );
    let lines = service.log_lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("did not drain before its deadline")),
        "the reason has to be in the log, not only in the exit code: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"message\":\"an email is being sent\"")),
        "and the log has to show the send it was waiting for: {lines:?}"
    );
}

#[test]
fn the_logging_adapter_is_still_the_default_when_nothing_is_configured() {
    // Every step before this one runs the binary with no mail server at all, and their specs read
    // `an email would be sent` out of the log. Adding a real adapter must not have changed that: a
    // service with no SMTP host configured is the course's own default, not a misconfiguration.
    let postgres = Postgres::start();
    let service = Service::start(&[
        ("MAILER__DATABASE__URL", &postgres.url()),
        ("MAILER__LOG__FORMAT", "json"),
    ]);

    let answer = service.post("/signup", &[], &signup_body("ada@example.com"));
    assert_eq!(answer.status, 201, "{}", answer.body);

    let lines = wait_for("the confirmation email to be written to the log", || {
        let lines = service.log_lines();
        lines
            .iter()
            .any(|line| line.contains("an email would be sent"))
            .then_some(lines)
    });
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"to\":\"ada@example.com\"")),
        "the envelope goes to the log, with the address: {lines:?}"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("email would be sent"))
            .count(),
        1,
        "one signup, one message: {lines:?}"
    );
}

/// The spec's own check that a `Value` helper reads as written.
#[allow(dead_code)]
fn first_message(mailpit: &Mailpit) -> Value {
    mailpit
        .messages()
        .first()
        .cloned()
        .expect("a message has arrived")
}
