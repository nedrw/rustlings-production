//! What `meshkv/5.11-client-sdk` has to be true about — on **both** backends.
//!
//! `conformance@2` runs this suite once per implementation of [`meshkv::sdk::Backend`], with
//! `KILN_BACKEND` naming the one in `tests/support/mod.rs`. The cases are therefore written against
//! the SDK's *contract* and never against an implementation's habits: a case that could only pass on
//! the in-process cluster would be a case the SDK does not have to honour.

#[path = "support/mod.rs"]
mod support;

use meshkv::idempotency::Session;
use meshkv::sdk::{MAX_ATTEMPTS, Sdk, SdkError, command_cas, command_delete, command_set};

fn sdk(start: &str) -> Sdk<Box<dyn meshkv::sdk::Backend>> {
    let session = Session::new("sdk-suite").expect("a named session");
    Sdk::new(support::backend(), session, start)
}

#[test]
fn a_write_is_applied_and_a_read_sees_it() {
    let mut sdk = sdk("n1");
    let index = sdk
        .write(&command_set("k", b"v"))
        .expect("the leader applies it");
    assert!(index > 0, "a write has a position in the log");

    let value = sdk.read("k").expect("the leader reads it");
    assert_eq!(value.as_deref(), Some(&b"v"[..]));
}

#[test]
fn a_write_that_starts_at_a_follower_follows_the_redirect() {
    let mut sdk = sdk("n2");
    let index = sdk
        .write(&command_set("k", b"v"))
        .expect("the redirect leads to the leader");
    assert!(index > 0);
    assert_eq!(sdk.attempts(), 2, "one redirect, then the leader");
    assert_eq!(
        sdk.node(),
        "n1",
        "and the SDK remembers where the leader is"
    );
}

use std::result::Result;

/// A backend that never knows a leader: a cluster in the middle of an election that never ends.
struct Nobody;

impl meshkv::sdk::Backend for Nobody {
    fn write(
        &mut self,
        _node: &str,
        _key: &meshkv::idempotency::IdempotencyKey,
        _command: &[u8],
    ) -> Result<meshkv::sdk::Reply, SdkError> {
        Ok(meshkv::sdk::Reply::NoLeader)
    }

    fn read(&mut self, _node: &str, _key: &str) -> Result<meshkv::sdk::Reply, SdkError> {
        Ok(meshkv::sdk::Reply::NoLeader)
    }
}

#[test]
fn a_cluster_that_knows_no_leader_exhausts_the_attempts_instead_of_spinning() {
    // The bound is the SDK's own, so this case uses a backend of its own too: no implementation of
    // the port has to be able to produce an endless election for the policy to be checkable.
    let session = Session::new("sdk-suite").expect("a named session");
    let mut sdk = Sdk::new(Nobody, session, "n1");
    match sdk.write(&command_set("k", b"v")) {
        Ok(_) => panic!("nobody can serve a write with no leader"),
        Err(SdkError::RetriesExhausted { attempts, .. }) => {
            assert_eq!(
                attempts, MAX_ATTEMPTS,
                "the bound is the SDK's, and it holds"
            );
        }
        Err(other) => panic!("the SDK gives up with RetriesExhausted, not {other:?}"),
    }
}

#[test]
fn the_same_write_twice_is_applied_once_and_answers_the_same_index() {
    let mut sdk = sdk("n1");
    let first = sdk.write(&command_set("k", b"v")).expect("applied");
    let second = sdk
        .write(&command_set("k", b"v"))
        .expect("the same write is a success");
    assert_eq!(
        first, second,
        "the retry is answered with the original position: the caller asked for a write, and the \
         write exists"
    );
}

#[test]
fn a_refused_write_is_not_retried() {
    let mut sdk = sdk("n1");
    let _ = sdk.write(&command_set("k", b"v")).expect("applied");

    // A compare-and-set against a value that is not there: the store refuses it, and the SDK does
    // not spend its attempts on a refusal that retrying cannot change.
    let error = sdk
        .write(&command_cas("k", b"other", b"new"))
        .expect_err("the compare-and-set does not match");
    assert!(matches!(error, SdkError::Refused { .. }), "{error:?}");
    assert!(!error.is_retryable(), "a refusal is not retryable");
    assert_eq!(sdk.attempts(), 1, "and it was tried exactly once");
}

#[test]
fn a_delete_is_a_write_like_any_other() {
    let mut sdk = sdk("n1");
    let _ = sdk.write(&command_set("gone", b"v")).expect("applied");
    let _ = sdk.write(&command_delete("gone")).expect("applied");
    assert_eq!(sdk.read("gone").expect("the leader reads"), None);
}

#[test]
fn errors_say_what_a_caller_can_do_about_them() {
    let not_leader = SdkError::NotLeader {
        leader: Some("n1".to_owned()),
    };
    assert!(not_leader.is_retryable(), "a redirect is worth following");
    let refused = SdkError::Refused {
        reason: "no".to_owned(),
    };
    assert!(!refused.is_retryable(), "a refusal is not");
    let unavailable = SdkError::Unavailable {
        detail: "down".to_owned(),
    };
    assert!(
        unavailable.is_retryable(),
        "a node that is down may come back"
    );
}

#[test]
fn the_sdk_does_not_ask_the_caller_for_a_key() {
    // The key is the SDK's: a caller that had to build it would be a caller that could forget it,
    // and one forgotten key is one duplicate write.
    let mut sdk = sdk("n1");
    let _ = sdk.write(&command_set("k", b"1")).expect("applied");
    let _ = sdk.write(&command_set("k", b"2")).expect("applied");
    assert_eq!(
        sdk.read("k").expect("read").as_deref(),
        Some(&b"2"[..]),
        "two different writes are two writes"
    );
}
