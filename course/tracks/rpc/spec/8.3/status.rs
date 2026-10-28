//! Spec for `rpc/8.3-status-codes` — the canonical code, and what a client may retry.

use rpc::status::{Code, Failure, code_for, retryable};

#[test]
fn every_failure_has_its_canonical_code() {
    assert_eq!(code_for(Failure::BadRequest), Code::InvalidArgument);
    assert_eq!(code_for(Failure::MissingRow), Code::NotFound);
    assert_eq!(code_for(Failure::Duplicate), Code::AlreadyExists);
    assert_eq!(code_for(Failure::Overloaded), Code::ResourceExhausted);
    assert_eq!(code_for(Failure::Bug), Code::Internal);
    assert_eq!(code_for(Failure::Downstream), Code::Unavailable);
}

#[test]
fn transient_codes_are_retryable() {
    for code in [
        Code::Unavailable,
        Code::ResourceExhausted,
        Code::DeadlineExceeded,
    ] {
        assert!(retryable(code), "{code:?}");
    }
}

#[test]
fn a_caller_mistake_is_not_retryable() {
    for code in [Code::InvalidArgument, Code::NotFound, Code::AlreadyExists] {
        assert!(!retryable(code), "{code:?}");
    }
}

#[test]
fn the_servers_bug_is_not_retryable() {
    assert!(!retryable(Code::Internal));
}

#[test]
fn success_is_not_a_retry() {
    assert!(!retryable(Code::Ok));
}

#[test]
fn an_overloaded_server_and_a_dead_dependency_are_retryable_but_bugs_are_not() {
    assert!(retryable(code_for(Failure::Overloaded)));
    assert!(retryable(code_for(Failure::Downstream)));
    assert!(!retryable(code_for(Failure::Bug)));
    assert!(!retryable(code_for(Failure::BadRequest)));
}
