//! Spec for `mailer/1.13-email-port`: a signup hands the confirmation email to the sender the service
//! was built with, and a sender that refuses does not turn a signup into a loss.
//!
//! `kiln run` copies this file into the project as `tests/spec_email_port.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * through the route, with a **fake sender** built into the app for the test (a constructor
//!   argument, not a mocking framework — `tests/support/email.rs`), so what the service tried to send
//!   is a value the test can read instead of a mail server it would have to run;
//! * through `psql` (`tests/support/postgres.rs`), because "the subscriber survived a failed send" is
//!   a claim about a row, not about an answer;
//! * the port itself is judged once, over *every* adapter, because a fake that does not behave like
//!   the thing it replaces is a fake that will lie to a test one day.
//!
//! The harness this step needs is `tests/support/app.rs`: the factory of step
//! `mailer/1.8-test-harness` builds its state with `AppState::start`, and this step's seam is the
//! other constructor, so the fixture that takes a sender lives next to it. Nothing here sleeps, and
//! every wait is a connection that either succeeds or has a deadline.

#[path = "support/app.rs"]
mod app;
#[path = "support/email.rs"]
mod email;
#[path = "support/postgres.rs"]
mod postgres;

use std::sync::Arc;
use std::time::{Duration, Instant};

use app::App;
use email::Recording;
use mailer::config::Config;
use mailer::domain::Email;
use mailer::email::{EmailSender, Logging, Message};
use postgres::Postgres;

/// The configuration a test starts the service with: the defaults, plus what the test adds.
fn config(env: &[(String, String)]) -> Config {
    app::config(env)
}

/// The setting a service reads its database from (step `mailer/1.4-config`).
fn database_setting(url: &str) -> (String, String) {
    app::database_setting(url)
}

/// One signup body, as a caller writes it.
fn signup_body(email: &str) -> String {
    app::signup_body(email)
}

/// Wait until `check` answers something, or fail loudly with what the test was waiting for.
///
/// A spec cannot assert on what the sender was handed the moment a `201` arrives: on this tree the
/// send happens inside the request, and step `mailer/1.14-worker` is where it moves onto a queue
/// whose worker runs *after* the answer. Waiting for the condition instead of sleeping a guessed
/// amount is what keeps one spec honest on both trees (authoring.md §6.4): a service that never
/// sends fails after the deadline with the right sentence, instead of hanging or passing late.
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "{what} did not happen within 5s");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The messages the fake was handed, once it has been handed at least one.
fn delivered_so_far(sender: &Recording) -> Vec<mailer::email::Message> {
    wait_for("the sender to be handed an email", || {
        let delivered = sender.delivered();
        (!delivered.is_empty()).then_some(delivered)
    })
}

/// The token the service stored for an address: the only thing its confirmation link can carry.
fn token_of(postgres: &Postgres, address: &str) -> String {
    postgres.query(&format!(
        "SELECT confirmation_token FROM subscribers WHERE email = '{address}'"
    ))
}

#[test]
fn a_signup_hands_one_confirmation_email_to_the_sender() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body(" Ada@Example.COM "));
    assert_eq!(status, 201, "the fixture signup failed: {body}");

    let delivered = delivered_so_far(&sender);
    assert_eq!(
        delivered.len(),
        1,
        "a signup sends exactly one email, and this one sent {}: {delivered:?}",
        delivered.len()
    );
    let message = &delivered[0];
    assert_eq!(
        message.to.as_str(),
        "ada@example.com",
        "the email goes to the address as it was stored, not as it was typed"
    );
    assert!(
        !message.subject.is_empty(),
        "an email without a subject is an email that gets reported as spam"
    );
    assert!(
        message.body.contains(&format!(
            "/confirm?token={}",
            token_of(&postgres, "ada@example.com")
        )),
        "the body has to carry the link with the token that is in the row — the only thing \
         `/confirm` accepts: {}",
        message.body
    );
    assert!(
        !sender.attempted().is_empty(),
        "the fake has to be what the route called, not a second path through the service"
    );
}

#[test]
fn an_address_that_is_already_subscribed_gets_no_second_email() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 409, "{body}");

    // The second signup sent nothing, and that is a claim about a *non*-event: the only honest way
    // to assert it is to wait for the first email to have been handed over (so the sender is in use)
    // and then look at what else it was given.
    let delivered = delivered_so_far(&sender);
    assert_eq!(
        delivered.len(),
        1,
        "the second signup must not email anybody: the address already has a subscriber, and a \
         second confirmation link is a link that confirms nothing new ({delivered:?})"
    );
}

#[test]
fn a_sender_that_refuses_does_not_lose_the_subscriber() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    sender.fail_with("the mail server is away");
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(
        status, 201,
        "a mail server that is away is not a signup that failed: the subscriber exists, and the \
         message is a delivery problem ({body})"
    );
    // It has to try — and on the tree where the send happens on a queue, "try" includes the
    // retries that queue brings with it (step `mailer/1.14-worker`), so the claim is "at least one
    // attempt", not "exactly one".
    let attempted = wait_for("the sender to be handed the email", || {
        let attempted = sender.attempted();
        (!attempted.is_empty()).then_some(attempted)
    });
    assert!(
        !attempted.is_empty(),
        "the service has to try: a sender that is never called is a message nobody missed"
    );
    assert_eq!(
        sender.delivered().len(),
        0,
        "the fixture sender was told to fail"
    );
    assert_eq!(
        postgres.query("SELECT status FROM subscribers WHERE email = 'ada@example.com'"),
        "pending",
        "the subscriber has to be there, waiting: a signup that is undone by a failed send loses an \
         address this service already has"
    );
}

#[test]
fn a_send_is_attempted_again_after_the_sender_recovers() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    sender.fail_with("the mail server is away");
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    sender.clear_failure();
    let (status, body) = app.post("/signup", &signup_body("grace@example.com"));
    assert_eq!(status, 201, "{body}");

    // Grace's email has to arrive. Ada's may or may not, depending on how far its retries got before
    // the sender was told to work again, so the assertions are about grace and about the fact that
    // sending did not stay broken.
    let delivered = wait_for("grace's confirmation email", || {
        let delivered = sender.delivered();
        delivered
            .iter()
            .any(|message| message.to.as_str() == "grace@example.com")
            .then_some(delivered)
    });
    assert!(
        sender.attempted().len() >= 2,
        "two signups, at least two attempts: {:?}",
        sender.attempted()
    );
    assert!(
        delivered
            .iter()
            .any(|message| message.to.as_str() == "grace@example.com"),
        "the message that could go out did: {delivered:?}"
    );
}

#[tokio::test]
async fn every_sender_keeps_the_same_contract() {
    // The port is only worth having if an adapter can be swapped for another one: every
    // implementation has to take a message it can deliver, and the fake is the only one that has to
    // be able to refuse — which is the whole reason it exists.
    let message = Message {
        to: Email::parse("ada@example.com").expect("the fixture address is valid"),
        subject: "Confirm your subscription".to_owned(),
        body: "Confirm your subscription by opening the link.".to_owned(),
    };
    let senders: Vec<Box<dyn EmailSender>> = vec![Box::new(Logging), Box::new(Recording::new())];
    for sender in &senders {
        assert!(
            sender.send(message.clone()).await.is_ok(),
            "a message an adapter can deliver has to be taken by every adapter: {sender:?}"
        );
    }

    let refusing = Recording::new();
    refusing.fail_with("the mail server is away");
    assert!(
        refusing.send(message).await.is_err(),
        "a fake that cannot fail cannot be used to test what happens when sending fails"
    );
}
