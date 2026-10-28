//! probe = "rpc/deadline@apply"
//! title = "Every hop pays from the same deadline"
//! hint = "One absolute budget, subtracted hop by hop; a hop that needs more than what is left fails, and a chain that lands exactly on zero has finished."
//!
//! Step `rpc/8.2-deadline` propagates one instant, not a duration per hop. This probe asks for the
//! arithmetic alone: does the chain fit, and what is left.
//!
//! ## What you write
//!
//! [`fits`] — the leftover budget after every hop, or `None` at the first hop that needs more than
//! what is left.
//!
//! ## What the tests ask
//!
//! A chain with room to spare; a chain that lands exactly on zero; one that does not fit; no hops;
//! a zero-cost hop; and the budget running out in the middle of the chain.

/// The leftover budget, or `None` when the chain does not fit.
#[must_use]
pub fn fits(budget: u64, hops: &[u64]) -> Option<u64> {
    let mut left = budget;
    for hop in hops {
        // `>` and not `>=`: a hop that consumes exactly what is left has finished, which is what a
        // well-tuned last hop does on every call.
        if *hop > left {
            return None;
        }
        left -= hop;
    }
    Some(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_with_room_answers_the_leftover() {
        assert_eq!(fits(100, &[10, 20, 30]), Some(40));
    }

    #[test]
    fn landing_exactly_on_zero_has_finished() {
        assert_eq!(fits(50, &[10, 40]), Some(0));
    }

    #[test]
    fn a_hop_that_does_not_fit_fails_the_chain() {
        assert_eq!(fits(50, &[10, 41]), None);
    }

    #[test]
    fn no_hops_is_the_whole_budget() {
        assert_eq!(fits(7, &[]), Some(7));
    }

    #[test]
    fn a_zero_cost_hop_changes_nothing() {
        assert_eq!(fits(10, &[0, 5, 0]), Some(5));
    }

    #[test]
    fn the_budget_can_run_out_in_the_middle() {
        assert_eq!(fits(30, &[10, 10, 10, 1]), None);
    }
}
