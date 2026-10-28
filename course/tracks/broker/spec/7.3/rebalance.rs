//! Spec for `broker/7.3-rebalance` — range assignment: every partition owned exactly once,
//! contiguously, and survivors re-own everything when a member leaves.

use std::collections::BTreeMap;

use broker::assignment::assign;

fn owners(assignment: &BTreeMap<String, Vec<usize>>) -> Vec<usize> {
    let mut all: Vec<usize> = assignment.values().flatten().copied().collect();
    all.sort_unstable();
    all
}

#[test]
fn one_member_owns_every_partition() {
    let assignment = assign(6, &["a"]);
    assert_eq!(assignment["a"], [0, 1, 2, 3, 4, 5]);
}

#[test]
fn two_members_split_into_contiguous_halves() {
    let assignment = assign(6, &["a", "b"]);
    assert_eq!(assignment["a"], [0, 1, 2]);
    assert_eq!(assignment["b"], [3, 4, 5]);
}

#[test]
fn an_uneven_split_gives_the_earlier_member_the_extra() {
    let assignment = assign(5, &["a", "b"]);
    assert_eq!(assignment["a"], [0, 1, 2]);
    assert_eq!(assignment["b"], [3, 4]);
}

#[test]
fn every_partition_is_owned_exactly_once() {
    for members in [1, 2, 3, 5] {
        let names: Vec<String> = (0..members).map(|index| format!("m{index}")).collect();
        let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
        let assignment = assign(7, &borrowed);
        assert_eq!(owners(&assignment), (0..7).collect::<Vec<usize>>());
    }
}

#[test]
fn more_members_than_partitions_leaves_some_empty() {
    let assignment = assign(2, &["a", "b", "c"]);
    assert_eq!(assignment["a"], [0]);
    assert_eq!(assignment["b"], [1]);
    assert!(assignment["c"].is_empty());
}

#[test]
fn a_member_leaving_moves_its_partitions_to_the_survivors() {
    let before = assign(4, &["a", "b"]);
    assert_eq!(before["b"], [2, 3]);
    let after = assign(4, &["a"]);
    assert_eq!(after["a"], [0, 1, 2, 3]);
    assert!(!after.contains_key("b"));
}

#[test]
fn no_members_is_an_empty_assignment() {
    assert!(assign(4, &[]).is_empty());
}
