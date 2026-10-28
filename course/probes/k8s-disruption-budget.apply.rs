//! probe = "k8s/disruption-budget@apply"
//! title = "A budget that allows nothing is a drain that hangs"
//! hint = "Both spellings, capped at the replicas and never below zero; allowed == 0 is the number to alert on, because it blocks every voluntary disruption."
//!
//! Step `platform/9.3-disruption-budget` is the promise a workload makes to itself during a drain.
//! This probe asks for the number and for the judgement an operator makes about it.
//!
//! ## What you write
//!
//! * [`allowed`] — evictions permitted at once: `replicas - minAvailable` or `maxUnavailable`,
//!   capped at `replicas`, never below zero;
//! * [`blocks_drain`] — whether the budget allows no eviction at all.
//!
//! ## What the tests ask
//!
//! The two spellings; a budget of zero evictions; a budget larger than the workload; a floor larger
//! than the replicas; and the drain-blocking judgement.

/// One of the two spellings of a disruption budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// At least this many pods stay up.
    MinAvailable(u32),
    /// At most this many pods go down.
    MaxUnavailable(u32),
}

/// How many pods may be evicted at once.
#[must_use]
pub fn allowed(replicas: u32, budget: Budget) -> u32 {
    todo!("the difference or the number, capped at the replicas")
}

/// Whether the budget allows no eviction at all.
#[must_use]
pub fn blocks_drain(replicas: u32, budget: Budget) -> bool {
    allowed(replicas, budget) == 0 && replicas > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_available_leaves_the_difference() {
        assert_eq!(allowed(5, Budget::MinAvailable(2)), 3);
    }

    #[test]
    fn max_unavailable_is_the_number_itself() {
        assert_eq!(allowed(5, Budget::MaxUnavailable(2)), 2);
    }

    #[test]
    fn a_budget_larger_than_the_workload_is_capped() {
        assert_eq!(allowed(2, Budget::MaxUnavailable(9)), 2);
    }

    #[test]
    fn a_floor_larger_than_the_replicas_is_zero_not_negative() {
        assert_eq!(allowed(1, Budget::MinAvailable(5)), 0);
    }

    #[test]
    fn a_budget_of_zero_blocks_the_drain() {
        assert!(blocks_drain(5, Budget::MinAvailable(5)));
        assert!(blocks_drain(5, Budget::MaxUnavailable(0)));
        assert!(!blocks_drain(5, Budget::MaxUnavailable(1)));
        assert!(!blocks_drain(5, Budget::MinAvailable(0)));
    }

    #[test]
    fn an_empty_workload_has_nothing_to_block() {
        assert!(!blocks_drain(0, Budget::MaxUnavailable(0)));
    }
}
