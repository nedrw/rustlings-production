//! What `meshkv/5.8-idempotent-sessions` has to be true about.
//!
//! The step's subject is the ambiguous retry: a client whose write timed out cannot tell whether
//! the write happened, and the only safe move is to ask again. The cases below are the ways that
//! goes wrong — a retry applied twice, a duplicate answered differently from the original, a key
//! forgotten inside the window, a key remembered for ever, and a session that cannot tell one
//! request from another.

use std::time::Duration;

use meshkv::error::Error;
use meshkv::idempotency::{Answers, Ledger, Outcome, Session, default_window};

fn session() -> Session {
    Session::new("s-1").expect("a named session")
}

#[test]
fn a_session_has_an_id_and_refuses_to_be_anonymous() {
    let named = session();
    assert_eq!(named.id(), "s-1");
    let error = Session::new("  ").expect_err("an anonymous session is no session");
    assert!(error.to_string().contains("session"), "{error}");
}

#[test]
fn the_same_request_in_the_same_session_has_the_same_key() {
    let named = session();
    let first = named.key_for(b"set a 1");
    let second = named.key_for(b"set a 1");
    assert_eq!(
        first, second,
        "a retry is the same request with the same key"
    );

    let other = named.key_for(b"set a 2");
    assert_ne!(first, other, "a different request is a different write");

    let other_session = Session::new("s-2").expect("a named session");
    assert_ne!(
        first,
        other_session.key_for(b"set a 1"),
        "a different session is a different write"
    );
}

#[test]
fn a_retry_is_answered_with_the_original_answer_and_is_not_applied_again() {
    let mut ledger = Ledger::new(default_window());
    let key = session().key_for(b"set a 1");
    let mut applied = 0;

    let first = ledger
        .apply(key.clone(), 1_000, || {
            applied += 1;
            Ok((Some(7), b"ok".to_vec()))
        })
        .expect("the write is applied");
    assert_eq!(first, Outcome::Applied { index: Some(7) });
    assert_eq!(applied, 1);

    // The retry: the same key, a later moment, and the leader does *not* apply it again.
    let retry = ledger
        .apply(key.clone(), 1_500, || {
            applied += 1;
            Ok((Some(8), b"different".to_vec()))
        })
        .expect("the retry is answered");
    assert_eq!(
        retry,
        Outcome::Duplicate {
            answer: b"ok".to_vec()
        },
        "the answer is the *original* one"
    );
    assert_eq!(applied, 1, "and the write was applied exactly once");
}

#[test]
fn a_key_is_remembered_for_the_window_and_forgotten_after_it() {
    let window = Duration::from_millis(1_000);
    let mut ledger = Ledger::new(window);
    let key = session().key_for(b"set a 1");
    let _ = ledger
        .apply(key.clone(), 0, || Ok((Some(1), b"ok".to_vec())))
        .expect("applied");

    // Just inside the window: still remembered.
    assert!(ledger.expire(999) == 0, "nothing expired yet");
    assert!(
        ledger.answers().get(&key).is_some(),
        "and the key is still there"
    );

    // Just outside: forgotten.
    assert_eq!(ledger.expire(1_000), 1, "the key expired");
    assert!(ledger.answers().get(&key).is_none());
    assert!(ledger.answers().is_empty());
}

#[test]
fn a_forgotten_key_may_be_applied_again_and_that_is_the_trade() {
    let window = Duration::from_millis(100);
    let mut ledger = Ledger::new(window);
    let key = session().key_for(b"set a 1");
    let mut applied = 0;
    let _ = ledger
        .apply(key.clone(), 0, || {
            applied += 1;
            Ok((None, b"ok".to_vec()))
        })
        .expect("applied");
    let _ = ledger.expire(1_000);

    // The window has passed, so the leader no longer recognises the key. This is the *cost* of a
    // bounded window, and the reason the window has to be longer than a client's patience.
    let outcome = ledger
        .apply(key, 1_100, || {
            applied += 1;
            Ok((None, b"ok".to_vec()))
        })
        .expect("applied again");
    assert_eq!(outcome, Outcome::Applied { index: None });
    assert_eq!(applied, 2, "which is why the window is a decision");
}

#[test]
fn the_answers_are_bounded_by_the_window_not_by_the_number_of_writes() {
    let mut answers = Answers::new(Duration::from_millis(1_000));
    for index in 0..10u32 {
        answers.remember(format!("k{index}"), b"ok".to_vec(), u64::from(index) * 100);
    }
    assert_eq!(answers.len(), 10);
    assert_eq!(
        answers.expire(5_000),
        10,
        "everything older than the window goes"
    );
    assert!(answers.is_empty());
}

#[test]
fn a_write_that_fails_is_not_remembered() {
    let mut ledger = Ledger::new(default_window());
    let key = session().key_for(b"set a 1");
    let error = ledger
        .apply(key.clone(), 0, || {
            Err(Error::invalid("the write was refused"))
        })
        .expect_err("the write failed");
    assert!(error.to_string().contains("refused"), "{error}");
    assert!(
        ledger.answers().get(&key).is_none(),
        "a failed write leaves no answer to replay, so the key is free"
    );
}

#[test]
fn the_default_window_covers_a_reasonable_patience() {
    // The window is a decision, and this is the number it currently is: long enough for a client
    // that retries for a minute, short enough that the memory is bounded.
    assert!(default_window() >= Duration::from_secs(30));
    assert!(default_window() <= Duration::from_secs(300));
}
