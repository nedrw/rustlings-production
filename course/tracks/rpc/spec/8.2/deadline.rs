//! Spec for `rpc/8.2-deadline` — an absolute deadline, propagated, and a hop that fails before it
//! spends somebody else's budget.

use rpc::deadline::{Status, call, remaining};

#[test]
fn a_chain_with_budget_to_spare_answers_the_leftover() {
    assert_eq!(call(100, 0, &[10, 20, 30]), Ok(40));
}

#[test]
fn a_hop_may_consume_exactly_what_is_left() {
    assert_eq!(call(50, 0, &[10, 40]), Ok(0));
}

#[test]
fn a_hop_that_does_not_fit_fails() {
    assert_eq!(call(50, 0, &[10, 41]), Err(Status::DeadlineExceeded));
}

#[test]
fn a_deadline_already_past_fails_before_the_first_hop() {
    assert_eq!(call(10, 10, &[]), Err(Status::DeadlineExceeded));
    assert_eq!(call(10, 11, &[1]), Err(Status::DeadlineExceeded));
}

#[test]
fn no_hops_is_the_whole_budget() {
    assert_eq!(call(10, 3, &[]), Ok(7));
}

// Pins given code: `remaining`; the step is `call`, and its accounting is what the other tests fail on.
#[test]
fn remaining_is_an_absolute_deadline_minus_now() {
    assert_eq!(remaining(100, 30), Ok(70));
    assert_eq!(remaining(100, 100), Err(Status::DeadlineExceeded));
}
