//! Spec for `platform/9.3-disruption-budget` — how many pods a drain may take.

use platform::budget::{Budget, allowed};

#[test]
fn min_available_leaves_the_difference_for_evictions() {
    assert_eq!(allowed(5, Budget::MinAvailable(2)), 3);
}

#[test]
fn max_unavailable_is_the_number_itself() {
    assert_eq!(allowed(5, Budget::MaxUnavailable(1)), 1);
}

#[test]
fn a_budget_that_allows_nothing_blocks_every_drain() {
    assert_eq!(allowed(5, Budget::MinAvailable(5)), 0);
}

#[test]
fn a_budget_larger_than_the_workload_cannot_evict_more_than_there_is() {
    assert_eq!(allowed(2, Budget::MaxUnavailable(9)), 2);
}

#[test]
fn min_available_zero_allows_the_whole_workload() {
    assert_eq!(allowed(4, Budget::MinAvailable(0)), 4);
}

#[test]
fn min_available_larger_than_replicas_is_zero_not_negative() {
    assert_eq!(allowed(1, Budget::MinAvailable(5)), 0);
}
