//! Spec for `mailer/1.15-idempotent-delivery`: one confirmation email per subscriber, however many
//! times the service is asked for it.
//!
//! `kiln run` copies this file into the project as `tests/spec_idempotent_delivery.rs`; it is the
//! course's file, not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * through the route, with a **fake sender** (`tests/support/email.rs`) the test can read — the
//!   claim "exactly one email" is a claim about what the sender was given, and a mail server would
//!   be a different test entirely;
//! * through `psql` (`tests/support/postgres.rs`), because the record of a delivery is a fact about
//!   a row: `confirmation_sent_at`, `confirmation_claimed_at`, `confirmation_attempts`;
//! * with **several callers at once** (real threads on the same fixture), because a claim that only
//!   works when requests arrive one at a time is not a claim.
//!
//! The fixture is the one of step `mailer/1.13-email-port` (`tests/support/app.rs`): the state is
//! built with the sender a test chose, and the database is the one the container provides. Nothing
//! here sleeps to synchronise — every wait is a condition with a deadline.

#[path = "support/app.rs"]
mod app;
#[path = "support/email.rs"]
mod email;
#[path = "support/postgres.rs"]
mod postgres;

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use app::App;
use email::Recording;
use mailer::delivery::MAX_ATTEMPTS;
use mailer::email::Message;
use postgres::Postgres;

/// How long a test waits for something that is supposed to happen.
const DEADLINE: Duration = Duration::from_secs(10);
/// How often a waiting test looks again.
const RETRY: Duration = Duration::from_millis(20);
/// How many callers race for one address in the concurrency test.
const RACERS: usize = 8;

/// The configuration a test starts the service with: the defaults, plus what the test adds.
fn config(env: &[(String, String)]) -> mailer::config::Config {
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
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {DEADLINE:?}"
        );
        std::thread::sleep(RETRY);
    }
}

/// The messages the fake was handed, once it has been handed at least one.
fn delivered_so_far(sender: &Recording) -> Vec<Message> {
    wait_for("the sender to be handed an email", || {
        let delivered = sender.delivered();
        (!delivered.is_empty()).then_some(delivered)
    })
}

/// One column of one subscriber, read with `psql`.
fn column(postgres: &Postgres, address: &str, column: &str) -> String {
    postgres.query(&format!(
        "SELECT {column} FROM subscribers WHERE email = '{address}'"
    ))
}

#[test]
fn a_signup_records_the_delivery_it_caused() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    let delivered = delivered_so_far(&sender);
    assert_eq!(delivered.len(), 1, "one signup, one email: {delivered:?}");

    // The email is only half of it. The other half is the row that says it happened: without a
    // record, the next job for this subscriber has nothing to check, and "one email" is a property
    // of this test's timing rather than of the service.
    assert!(
        !column(&postgres, "ada@example.com", "confirmation_sent_at").is_empty(),
        "a delivered confirmation has to be recorded in the row: \
         `confirmation_sent_at` is still empty"
    );
    assert_eq!(
        column(&postgres, "ada@example.com", "confirmation_attempts"),
        "1",
        "the delivery took one claim, and the count of claims is how a message that was given up \
         on becomes visible"
    );
}

#[test]
fn a_second_signup_for_the_same_address_does_not_email_twice() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    let delivered = delivered_so_far(&sender);

    // The same address again: the answer is the `409` of step `mailer/1.11-signup` (the address is
    // taken), and the delivery is already done. Asking twice must not be a way to send twice.
    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 409, "{body}");
    let (status, body) = app.post("/signup", &signup_body("Ada@Example.com"));
    assert_eq!(status, 409, "{body}");

    assert_eq!(
        sender.delivered().len(),
        1,
        "one confirmation per subscriber, however many times the address is signed up: \
         {:?}",
        sender.delivered()
    );
    assert_eq!(
        delivered.len(),
        1,
        "the first email is still the only one: {delivered:?}"
    );
}

#[test]
fn a_confirmation_that_failed_is_offered_again_and_still_sent_once() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    sender.fail_with("the mail server is away");
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");

    // The worker retries (step `mailer/1.14-worker`) and then gives up: `MAX_ATTEMPTS` claims later,
    // the message is owed and nobody is sending it, and the row has to say exactly that.
    let attempts: u32 = wait_for("the failed confirmation to be given up on", || {
        let counted = column(&postgres, "ada@example.com", "confirmation_attempts");
        let attempts = counted.parse::<u32>().unwrap_or(0);
        (attempts >= MAX_ATTEMPTS).then_some(attempts)
    });
    assert_eq!(
        attempts, MAX_ATTEMPTS,
        "a message that keeps failing is claimed {MAX_ATTEMPTS} times, not forever"
    );
    assert!(
        column(&postgres, "ada@example.com", "confirmation_claimed_at").is_empty(),
        "a claim that was given up on has to be released, or the subscriber is stuck as \
         `someone is on it` and no later job can pick it up"
    );
    assert!(
        column(&postgres, "ada@example.com", "confirmation_sent_at").is_empty(),
        "nothing was sent, so nothing may be recorded as sent"
    );
    assert_eq!(
        sender.delivered().len(),
        0,
        "the fixture sender was told to fail"
    );
    assert_eq!(
        sender.attempted().len(),
        MAX_ATTEMPTS as usize,
        "and the mail server was really asked {MAX_ATTEMPTS} times — one attempt per claim, or the          row's count and the sender's disagree"
    );

    // The mail server is well again, and the subscriber signs up again (the only way this service
    // has to say "try that again"): the delivery that was owed goes out — once.
    sender.clear_failure();
    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 409, "{body}");
    let delivered = wait_for("the confirmation that was owed", || {
        let delivered = sender.delivered();
        (!delivered.is_empty()).then_some(delivered)
    });
    assert!(
        delivered
            .iter()
            .any(|message| message.to.as_str() == "ada@example.com"),
        "the message this service already owed has to go out: {delivered:?}"
    );
    assert!(
        !column(&postgres, "ada@example.com", "confirmation_sent_at").is_empty(),
        "and the row says it went out"
    );
    assert_eq!(
        sender.attempted().len(),
        MAX_ATTEMPTS as usize + 1,
        "the message was offered once more, by the job the second signup asked for: an attempt is a          claim, and both of them are in the count"
    );

    // And a third signup does nothing at all: the delivery is no longer owed.
    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        sender.delivered().len(),
        1,
        "a delivered confirmation is not delivered again: {:?}",
        sender.delivered()
    );
}

#[test]
fn callers_racing_for_one_address_produce_one_confirmation() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    // Eight callers, one address, no ordering: exactly one may win the insert (step
    // `mailer/1.11-signup` is the reason the database decides that), and every one of them is a
    // request for the confirmation email. The queue may therefore hold several jobs for the same
    // subscriber — and the claim is what has to turn them into one email.
    let answers: Vec<(u16, String)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..RACERS)
            .map(|_| scope.spawn(|| app.post("/signup", &signup_body("ada@example.com"))))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a racer panicked"))
            .collect()
    });
    let created = answers.iter().filter(|(status, _)| *status == 201).count();
    let conflicts = answers.iter().filter(|(status, _)| *status == 409).count();
    assert_eq!(
        created, 1,
        "the unique constraint creates exactly one subscriber out of {RACERS} racers: {answers:?}"
    );
    assert_eq!(
        conflicts,
        RACERS - 1,
        "and the rest are told the address is taken: {answers:?}"
    );

    delivered_so_far(&sender);
    // Give the queue a moment's worth of work to get wrong: every duplicate asked for a delivery,
    // so the jobs are there and one of them must win the claim. The count below is read after the
    // row says the delivery happened, which is the only moment at which it could have happened
    // twice — and the count is what the sender was given, so a second send would be in it.
    wait_for("the delivery to be recorded", || {
        (!column(&postgres, "ada@example.com", "confirmation_sent_at").is_empty()).then_some(())
    });
    let delivered = sender.delivered();
    assert_eq!(
        delivered.len(),
        1,
        "one subscriber, one confirmation email: the jobs of {RACERS} racing signups have to be \
         deduplicated by the delivery, not by luck: {delivered:?}"
    );
}
