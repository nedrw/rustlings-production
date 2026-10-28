//! Spec for `platform/9.7-rollout-drill` — every step of an observed rollout held its budgets.

use platform::drill::{Observed, Violation, drill};

fn step(ready: u32, total: u32) -> Observed {
    Observed {
        replicas: 3,
        max_surge: 1,
        max_unavailable: 1,
        ready,
        total,
    }
}

#[test]
fn a_clean_rollout_has_no_violations() {
    let steps = [step(3, 3), step(3, 4), step(4, 4), step(3, 3)];
    assert_eq!(drill(&steps), []);
}

#[test]
fn too_many_pods_is_a_violation() {
    let steps = [step(3, 3), step(3, 5)];
    assert_eq!(
        drill(&steps),
        [Violation::TooManyPods { step: 1, total: 5 }]
    );
}

#[test]
fn too_few_ready_is_a_violation() {
    let steps = [step(3, 3), step(1, 3)];
    assert_eq!(
        drill(&steps),
        [Violation::TooFewReady { step: 1, ready: 1 }]
    );
}

#[test]
fn both_budgets_can_break_at_one_step() {
    let steps = [step(1, 6)];
    assert_eq!(
        drill(&steps),
        [
            Violation::TooManyPods { step: 0, total: 6 },
            Violation::TooFewReady { step: 0, ready: 1 },
        ]
    );
}

#[test]
fn sitting_exactly_on_both_budgets_is_allowed() {
    // 4 total is the ceiling, 2 ready is the floor.
    let steps = [step(2, 4)];
    assert_eq!(drill(&steps), []);
}

#[test]
fn every_step_is_checked_not_just_the_last() {
    let steps = [step(1, 3), step(3, 3)];
    assert_eq!(
        drill(&steps),
        [Violation::TooFewReady { step: 0, ready: 1 }]
    );
}
