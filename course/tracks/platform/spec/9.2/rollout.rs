//! Spec for `platform/9.2-rollout` — the two budgets of a rolling update.

use platform::rollout::{available_floor, may_create, may_terminate};

// Pins given code: `available_floor`, including its clamp at zero; the step is `may_terminate`.
#[test]
fn the_floor_is_replicas_minus_unavailable() {
    assert_eq!(available_floor(3, 0), 3);
    assert_eq!(available_floor(3, 2), 1);
    assert_eq!(available_floor(1, 5), 0, "the floor never goes below zero");
}

#[test]
fn with_no_unavailable_budget_nothing_leaves_until_something_is_ready() {
    // Three replicas, three ready: removing one drops below the floor.
    assert!(!may_terminate(3, 0, 3, 3));
    // A new pod is ready and exists: now an old one may go.
    assert!(may_terminate(3, 0, 4, 4));
}

#[test]
fn an_unavailable_budget_lets_an_old_pod_go_immediately() {
    assert!(may_terminate(3, 1, 3, 3));
}

#[test]
fn a_single_replica_with_an_unavailable_budget_may_go() {
    // The documented downside: no surge and an unavailable budget of one lets the only pod leave,
    // and the service is down for the length of the swap.
    assert!(may_terminate(1, 1, 1, 1));
    assert!(
        !may_terminate(1, 0, 1, 1),
        "without the budget the pod stays"
    );
    assert!(
        !may_terminate(1, 1, 0, 0),
        "a pod that does not exist cannot leave"
    );
}

// Pins given code: `may_create`; the step is `may_terminate`.
#[test]
fn creation_is_bounded_by_surge() {
    assert!(may_create(3, 1, 3));
    assert!(!may_create(3, 1, 4), "the surge is already taken");
    assert!(!may_create(3, 0, 3), "no surge means no extra pod");
}

#[test]
fn a_ready_pod_with_no_room_to_surge_can_still_be_replaced() {
    // maxUnavailable 1: three ready, three total, one may go.
    assert!(may_terminate(3, 1, 3, 3));
    // and the replacement then needs a slot, which the removal made.
    assert!(may_create(3, 0, 2));
}
