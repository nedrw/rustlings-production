//! probe = "broker/rebalance@apply"
//! title = "Contiguous, complete, and independent of the order you ask in"
//! hint = "Sort the members, hand out contiguous ranges, give the remainder to the first ones — and a member with no partition still appears."
//!
//! Step `broker/7.3-rebalance` is the range assignor: every partition owned exactly once, each
//! member's partitions a contiguous run, and the answer independent of the order the members happen
//! to be listed in.
//!
//! ## What you write
//!
//! [`assign`] — sorted members, `partitions / members` each, the remainder to the first ones, an
//! empty list for a member with nothing.
//!
//! ## What the tests ask
//!
//! One member gets everything; the answer does not depend on input order; every partition exactly
//! once across several sizes; more members than partitions leaves the later ones empty; zero
//! partitions is every member with nothing; and each member's list is a contiguous run.

use std::collections::BTreeMap;

/// The partitions each member owns, by member name.
#[must_use]
pub fn assign(partitions: usize, members: &[&str]) -> BTreeMap<String, Vec<usize>> {
    let mut sorted: Vec<&str> = members.to_vec();
    sorted.sort_unstable();
    let mut assignment: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    if sorted.is_empty() {
        return assignment;
    }
    let base = partitions / sorted.len();
    let extra = partitions % sorted.len();
    let mut next = 0usize;
    for (index, member) in sorted.iter().enumerate() {
        // The remainder goes to the earliest members: an uneven split has to put it somewhere, and
        // this is the rule every other consumer expects.
        let count = base + usize::from(index < extra);
        assignment.insert((*member).to_owned(), (next..next + count).collect());
        next += count;
    }
    assignment
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owners(assignment: &BTreeMap<String, Vec<usize>>) -> Vec<usize> {
        let mut all: Vec<usize> = assignment.values().flatten().copied().collect();
        all.sort_unstable();
        all
    }

    #[test]
    fn one_member_owns_every_partition() {
        let assignment = assign(5, &["a"]);
        assert_eq!(assignment["a"], [0, 1, 2, 3, 4]);
    }

    #[test]
    fn the_answer_does_not_depend_on_the_order_of_the_members() {
        let forward = assign(6, &["a", "b", "c"]);
        let shuffled = assign(6, &["c", "a", "b"]);
        assert_eq!(forward, shuffled);
    }

    #[test]
    fn every_partition_is_owned_exactly_once() {
        for members in 1..=5 {
            let names: Vec<String> = (0..members).map(|index| format!("m{index}")).collect();
            let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
            let assignment = assign(9, &borrowed);
            assert_eq!(owners(&assignment), (0..9).collect::<Vec<usize>>(), "{members} members");
        }
    }

    #[test]
    fn more_members_than_partitions_leaves_the_later_ones_empty() {
        let assignment = assign(2, &["a", "b", "c"]);
        assert_eq!(assignment["a"], [0]);
        assert_eq!(assignment["b"], [1]);
        assert!(assignment["c"].is_empty());
    }

    #[test]
    fn zero_partitions_is_every_member_with_nothing() {
        let assignment = assign(0, &["a", "b"]);
        assert_eq!(assignment.len(), 2);
        assert!(assignment.values().all(Vec::is_empty));
    }

    #[test]
    fn each_members_partitions_are_a_contiguous_run() {
        let assignment = assign(11, &["a", "b", "c"]);
        for (member, partitions) in &assignment {
            assert!(
                partitions.windows(2).all(|pair| pair[1] == pair[0] + 1),
                "{member}: {partitions:?}"
            );
        }
    }
}
