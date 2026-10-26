//! What `meshkv/5.22-consistenticy-audit` has to be true about.
//!
//! The step's subject is the last sentence of the track — what the cluster committed is what every
//! node holds — and the cases below are the ways the audit can say it: agreement, a node that is
//! ahead, a node that diverged, and a report that names the node.

use meshkv::audit::{State, audit};
use meshkv::cluster::NodeId;

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

#[test]
fn a_cluster_that_agrees_is_healthy() {
    let states = vec![
        State::new(node("n1"), 100, 7),
        State::new(node("n2"), 100, 7),
        State::new(node("n3"), 100, 7),
    ];
    let report = audit(&states).expect("an audit");
    assert_eq!(report.common_prefix, 100);
    assert!(report.is_healthy());
    assert_eq!(report.audited, 3);
    assert!(
        report.summary().contains("all agree"),
        "{}",
        report.summary()
    );
}

#[test]
fn a_node_that_is_ahead_is_not_a_finding() {
    // n1 is at 120 whose state includes the prefix; the audit compares at the common prefix, which is
    // 100. Being ahead is what a leader looks like, not a problem.
    let states = vec![
        State::new(node("n1"), 120, 9),
        State::new(node("n2"), 100, 7),
        State::new(node("n3"), 100, 7),
    ];
    let report = audit(&states).expect("an audit");
    assert_eq!(report.common_prefix, 100);
    assert!(report.is_healthy(), "{report:?}");
    assert!(report.diverged.is_empty());
}

#[test]
fn a_node_that_diverged_is_named() {
    let states = vec![
        State::new(node("n1"), 100, 7),
        State::new(node("n2"), 100, 7),
        State::new(node("n3"), 100, 8),
    ];
    let report = audit(&states).expect("an audit");
    assert!(!report.is_healthy());
    assert_eq!(report.diverged, vec![node("n3")]);
    let summary = report.summary();
    assert!(summary.contains("n3"), "{summary}");
    assert!(summary.contains("100"), "{summary}");
}

#[test]
fn the_majority_digest_is_the_one_that_counts() {
    // Two nodes agree and one does not: the audit sides with the majority and names the odd one.
    let states = vec![
        State::new(node("n1"), 50, 1),
        State::new(node("n2"), 50, 1),
        State::new(node("n3"), 50, 2),
    ];
    let report = audit(&states).expect("an audit");
    assert_eq!(report.diverged, vec![node("n3")]);
}

#[test]
fn an_audit_of_nobody_is_refused() {
    let error = audit(&[]).expect_err("an audit needs an answer");
    assert!(error.to_string().contains("at least one"), "{error}");
}

#[test]
fn the_same_node_answering_twice_is_refused() {
    let states = vec![State::new(node("n1"), 10, 1), State::new(node("n1"), 10, 1)];
    let error = audit(&states).expect_err("two answers from one node");
    assert!(error.to_string().contains("twice"), "{error}");
}

#[test]
fn a_cluster_where_every_node_disagrees_with_every_other_still_names_one() {
    // No majority at all: the audit has to say *something* actionable rather than "healthy". With
    // three different digests, the one with the highest count (all of them, one each) is chosen by
    // the tie-break, and the other two are the diverged ones — which is the honest answer: nobody
    // agrees.
    let states = vec![
        State::new(node("n1"), 10, 1),
        State::new(node("n2"), 10, 2),
        State::new(node("n3"), 10, 3),
    ];
    let report = audit(&states).expect("an audit");
    assert!(!report.is_healthy(), "{report:?}");
    assert_eq!(report.diverged.len(), 2, "{report:?}");
}
