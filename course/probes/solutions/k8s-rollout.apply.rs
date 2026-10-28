//! probe = "k8s/rollout@apply"
//! title = "Can the rollout move?"
//! hint = "Progress is a pod created within the surge or one terminated above the floor; when both budgets are spent the rollout waits, and that is a configuration to read, not a bug to retry."
//!
//! Step `platform/9.2-rollout` is the arithmetic of two budgets. This probe asks the question an
//! operator asks of a stuck rollout: given the counts, can anything move?
//!
//! ## What you write
//!
//! [`progress`] — whether one more pod may be created (total below `replicas + surge`) or one more
//! old pod terminated (ready - 1 at or above `replicas - unavailable`).
//!
//! ## What the tests ask
//!
//! A full deck with room to surge; no surge and no unavailable budget (stuck); an unavailable budget
//! that lets a pod go; a surge already taken; and a workload below its replica count.

/// Whether the rollout can do anything right now.
#[must_use]
pub fn progress(replicas: u32, surge: u32, unavailable: u32, ready: u32, total: u32) -> bool {
    let may_create = total < replicas.saturating_add(surge);
    let floor = replicas.saturating_sub(unavailable);
    // The pod that leaves takes its readiness: after the move the floor must still be there.
    let may_terminate = total > 0 && ready.saturating_sub(1) >= floor;
    may_create || may_terminate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_deck_with_surge_can_create() {
        assert!(progress(3, 1, 0, 3, 3));
    }

    #[test]
    fn no_surge_and_no_unavailable_budget_is_stuck() {
        assert!(!progress(3, 0, 0, 3, 3), "nothing may leave and nothing may be created");
    }

    #[test]
    fn an_unavailable_budget_lets_a_pod_go() {
        assert!(progress(3, 0, 1, 3, 3));
    }

    #[test]
    fn a_taken_surge_and_a_reached_floor_is_stuck() {
        // One extra pod is already created (total 4 = 3 + 1) and it is not ready yet: nothing may
        // leave, and the surge is spent.
        assert!(!progress(3, 1, 0, 3, 4));
    }

    #[test]
    fn a_workload_below_its_replica_count_can_create() {
        // Scale-up: two pods running towards three, no surge needed because there is room.
        assert!(progress(3, 0, 0, 2, 2));
    }

    #[test]
    fn a_ready_pod_with_a_surge_slot_can_replace() {
        // The new pod is ready and the surge has room: the old one may go next.
        assert!(progress(3, 1, 1, 4, 4));
    }
}
