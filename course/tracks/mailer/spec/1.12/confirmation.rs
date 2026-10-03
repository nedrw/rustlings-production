//! Spec for `mailer/1.12-token`: a signup issues a token nobody can guess, and that token confirms
//! the subscriber once — not twice, not after its window, not when it was tampered with.
//!
//! `kiln run` copies this file into the project as `tests/spec_confirmation.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * the token is read out of the table (`psql`, step `mailer/1.9-postgres`'s container harness),
//!   because it is only ever sent to the address's mailbox — the *service* never hands it to a
//!   caller, and the spec checks that too;
//! * every failure the step designs against is provoked on purpose: the expiry is moved into the
//!   past with one statement, a token is tampered with by a character, a second confirmation arrives
//!   at the same moment as the first;
//! * the row is read back after every attempt, because the interesting mistakes here are the ones
//!   that answer correctly and change the table anyway.
//!
//! Nothing here sleeps: waiting is done on values that change (the barrier below), and every
//! command the harness starts has a deadline of its own (this machine has no `timeout(1)`).

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

/// The configuration a test starts the service with: the defaults, plus what the test adds.
fn config(env: &[(String, String)]) -> Config {
    Config::load(&Flags::default(), env).expect("the fixture configuration is valid")
}

/// The setting a service reads its database from (step `mailer/1.4-config`).
fn database_setting(url: &str) -> (String, String) {
    ("MAILER__DATABASE__URL".to_owned(), url.to_owned())
}

/// One signup body, as a caller writes it.
fn signup_body(email: &str) -> String {
    json!({ "email": email, "name": "Ada Lovelace", "password": PASSWORD }).to_string()
}

/// Subscribe one address and hand back the token the service stored for it.
///
/// The token is read where it lives: the service never answers with it, so a spec that wants one
/// has to be the database's reader — which is also the only way to see what a *real* token looks
/// like rather than what the spec hopes it looks like.
fn signed_up(postgres: &Postgres, app: &TestApp, email: &str) -> String {
    let (status, body) = app.post("/signup", &signup_body(email));
    assert_eq!(
        status, 201,
        "the fixture signup failed: {body} — a signup that stops working this early is what a token \
         that is the same for everybody looks like: `subscribers.confirmation_token` is `UNIQUE`, so \
         the second subscriber collides with the first"
    );
    postgres.query(&format!(
        "SELECT confirmation_token FROM subscribers WHERE email = '{email}'"
    ))
}

/// The status a subscriber is in, and whether the confirmation timestamp is set.
fn state(postgres: &Postgres, email: &str) -> String {
    postgres.query(&format!(
        "SELECT status || '|' || (confirmed_at IS NOT NULL)::text FROM subscribers WHERE email = \
         '{email}'"
    ))
}

#[test]
fn a_signup_issues_a_token_the_answer_never_carries() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    let token = postgres.query("SELECT confirmation_token FROM subscribers");

    assert_eq!(
        token.len(),
        64,
        "a token is 32 random bytes written as hex: `{token}`"
    );
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "a token has to survive a URL, an email client and a shell: `{token}`"
    );
    assert!(
        !body.contains(&token),
        "the token goes to the address, not to the caller who typed it: {body}"
    );
    assert_eq!(
        postgres.query("SELECT status || '|' || (confirmed_at IS NULL)::text FROM subscribers"),
        "pending|true",
        "a signup is not a confirmation"
    );
    assert_eq!(
        postgres.query(
            "SELECT (confirmation_expires_at > now() + interval '23 hours')::text || '|' || \
             (confirmation_expires_at < now() + interval '25 hours')::text FROM subscribers"
        ),
        "true|true",
        "the token has to expire — a day from now, stamped by the database's own clock"
    );

    // Two subscribers, two tokens: a token that is the same for everybody is not a token.
    signed_up(&postgres, &app, "grace@example.com");
    assert_eq!(
        postgres.query("SELECT count(DISTINCT confirmation_token) FROM subscribers"),
        "2",
        "every signup has to get a token of its own"
    );
    app.stop();
}

#[test]
fn a_valid_token_confirms_the_subscriber_once() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));
    let token = signed_up(&postgres, &app, "ada@example.com");

    let (status, body) = app.get(&format!("/confirm?token={token}"));
    assert_eq!(status, 200, "a link out of the mailbox has to work: {body}");
    assert!(
        body.contains("\"active\"") && body.contains("ada@example.com"),
        "the answer has to say whose subscription became active: {body}"
    );
    assert_eq!(
        state(&postgres, "ada@example.com"),
        "active|true",
        "confirming has to set both the status and the timestamp the `CHECK` constraint pairs it \
         with"
    );
    let confirmed_at = postgres.query("SELECT confirmed_at::text FROM subscribers");

    // The same link again: the caller is told, and nothing moves — least of all `confirmed_at`,
    // which is the record of when this address was actually confirmed.
    let (status, body) = app.get(&format!("/confirm?token={token}"));
    assert_eq!(
        status, 409,
        "a used token is not a second confirmation, and it is not a 500 either: {body}"
    );
    assert!(
        body.contains("ada@example.com"),
        "the answer has to name the address that is already confirmed: {body}"
    );
    assert_eq!(
        postgres.query("SELECT confirmed_at::text FROM subscribers"),
        confirmed_at,
        "replaying a token must not move the timestamp it set"
    );
    app.stop();
}

#[test]
fn an_expired_token_confirms_nothing() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));
    let token = signed_up(&postgres, &app, "ada@example.com");

    // The window is a day long, so the spec does not wait for it: it moves the row's expiry into the
    // past, which is the same state a link found in a mailbox a week later is in.
    postgres.execute(
        "UPDATE subscribers SET confirmation_expires_at = now() - interval '1 minute' WHERE email = \
         'ada@example.com'",
    );

    let (status, body) = app.get(&format!("/confirm?token={token}"));
    assert_eq!(
        status, 410,
        "an expired link is gone, and the caller has to be able to tell that from a forged one: \
         {body}"
    );
    assert_eq!(
        state(&postgres, "ada@example.com"),
        "pending|false",
        "an expired token must not confirm the subscriber it belongs to"
    );
    assert!(
        body.contains("expired"),
        "the answer has to say what is wrong with the link: {body}"
    );
    app.stop();
}

#[test]
fn a_token_nobody_was_given_is_refused() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));
    let token = signed_up(&postgres, &app, "ada@example.com");

    // A well-formed token that no subscriber holds: one character of this one is different, which is
    // what tampering with a link (or guessing a token) produces.
    let last = if token.ends_with('0') { '1' } else { '0' };
    let tampered = format!("{}{last}", &token[..token.len() - 1]);
    // (status, what the URL looks like, why)
    let refused = [
        (404, tampered.clone(), "a tampered token"),
        (404, "a".repeat(64), "a token nobody was given"),
        (400, "nope".to_owned(), "something that is not a token"),
        (400, token.to_uppercase(), "hex is not this service's hex"),
        (
            400,
            token[..token.len() - 1].to_owned(),
            "a token that lost its last character",
        ),
    ];
    for (expected, value, what) in refused {
        let (status, body) = app.get(&format!("/confirm?token={value}"));
        assert_eq!(status, expected, "{what} answered {status}: {body}");
        assert!(
            !body.contains(&token),
            "{what}: the answer must not echo the token it was given: {body}"
        );
    }

    // A missing parameter is not a token at all.
    let (status, body) = app.get("/confirm");
    assert_eq!(status, 400, "`/confirm` with no token: {body}");

    assert_eq!(
        state(&postgres, "ada@example.com"),
        "pending|false",
        "not one of these attempts may have confirmed the subscriber"
    );
    app.stop();
}

#[test]
fn two_confirmations_at_once_confirm_once() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));
    let token = signed_up(&postgres, &app, "ada@example.com");

    // Both requests wait at the barrier, so they leave together: this is the same race as two people
    // clicking one link, and the answer has to be decided by the row, not by who asked first.
    let barrier = Barrier::new(2);
    let (first, second) = thread::scope(|scope| {
        let one = scope.spawn(|| {
            barrier.wait();
            app.get(&format!("/confirm?token={token}"))
        });
        let two = scope.spawn(|| {
            barrier.wait();
            app.get(&format!("/confirm?token={token}"))
        });
        (
            one.join().expect("a confirmation must answer, not panic"),
            two.join().expect("a confirmation must answer, not panic"),
        )
    });

    let mut codes = [first.0, second.0];
    codes.sort_unstable();
    assert_eq!(
        codes,
        [200, 409],
        "two confirmations of one token have to be decided by the database: one 200 and one 409, \
         never two successes and never a 500 ({}; {})",
        first.1,
        second.1
    );
    assert_eq!(
        postgres.query("SELECT count(*) FROM subscribers WHERE status = 'active'"),
        "1",
        "one subscriber, confirmed once"
    );
    assert_eq!(
        postgres.query("SELECT (confirmed_at IS NOT NULL)::text FROM subscribers"),
        "true",
        "and the timestamp of that one confirmation is set"
    );
    app.stop();
}

#[test]
fn a_confirmation_without_a_database_is_not_ready() {
    // No database setting: the service starts anyway (step `mailer/1.9-postgres`), and this is the
    // request it cannot answer — a 503, not a 500: nothing is broken, a dependency is not there.
    let app = TestApp::start(config(&[]));
    let (status, body) = app.get(&format!("/confirm?token={}", "a".repeat(64)));
    assert_eq!(status, 503, "{body}");
    assert_eq!(
        app.get("/healthz").0,
        200,
        "the process is alive: liveness is not readiness"
    );
    app.stop();
}
