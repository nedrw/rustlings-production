//! What `meshkv/5.18-cluster-observability` has to be true about.
//!
//! The step's subject is the question only a cluster can be asked — who is behind, who is flapping —
//! and the case below are the answers that have to be arithmetic: a lag measured from two indexes,
//! an error budget in counts, and the two shapes a failing node has.

use meshkv::cluster::NodeId;
use meshkv::metrics::{NodeMetrics, Slo};

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

#[test]
fn a_lag_is_measured_from_two_indexes() {
    let mut metrics = NodeMetrics::new();
    metrics.record_peer(node("n2"), 90);
    metrics.record_peer(node("n3"), 40);
    assert_eq!(metrics.lag(&node("n2"), 100).expect("a lag"), 10);
    assert_eq!(metrics.lag(&node("n3"), 100).expect("a lag"), 60);
    assert_eq!(
        metrics.most_behind(100),
        Some((&node("n3"), 60)),
        "the node furthest behind is the one an operator looks at first"
    );

    // A peer that is ahead of the commit it knows about is not "negative behind".
    metrics.record_peer(node("n4"), 120);
    assert_eq!(metrics.lag(&node("n4"), 100).expect("a lag"), 0);

    // A peer nobody has heard from has an unknown lag, not a zero one.
    let error = metrics
        .lag(&node("n9"), 100)
        .expect_err("an unrecorded peer has no lag");
    assert!(error.to_string().contains("unknown"), "{error}");
}

#[test]
fn a_node_that_keeps_losing_elections_is_the_one_to_look_at() {
    let mut steady = NodeMetrics::new();
    steady.elections_stood = 3;
    steady.elections_won = 3;
    assert!(!steady.is_suspect());

    let mut flapping = NodeMetrics::new();
    flapping.elections_stood = 9;
    flapping.elections_won = 1;
    assert_eq!(flapping.lost_elections(), 8);
    assert!(
        flapping.is_suspect(),
        "eight losses and one win is a problem"
    );

    let mut refusing = NodeMetrics::new();
    refusing.refusals = 20;
    refusing.commits = 0;
    assert!(refusing.is_suspect(), "refusing every write is a problem");
}

#[test]
fn an_objective_is_a_ratio_and_a_budget_is_what_it_allows() {
    // 99.9% over a thousand attempts allows one failure.
    let slo = Slo::new(999_000).expect("an objective");
    assert_eq!(slo.budget_left(1_000, 1_000).expect("a budget"), 1);
    assert_eq!(
        slo.budget_left(999, 1_000).expect("a budget"),
        0,
        "one failure spends the whole budget"
    );
    assert!(slo.is_broken(999, 1_000).expect("a verdict"));
    assert!(!slo.is_broken(1_000, 1_000).expect("a verdict"));

    // And the budget grows with the window: ten thousand attempts allow ten.
    assert_eq!(slo.budget_left(10_000, 10_000).expect("a budget"), 10);
    assert_eq!(slo.budget_left(9_995, 10_000).expect("a budget"), 5);
}

#[test]
fn an_objective_that_is_not_a_ratio_is_refused() {
    for ppm in [0, 1_000_001] {
        assert!(
            Slo::new(ppm).is_err(),
            "{ppm} parts per million is not an objective"
        );
    }
    let slo = Slo::new(1_000_000).expect("100% is an objective");
    assert_eq!(
        slo.budget_left(10, 10).expect("a budget"),
        0,
        "a perfect objective has no budget at all"
    );
}

#[test]
fn a_ratio_that_is_impossible_is_refused() {
    let slo = Slo::new(999_000).expect("an objective");
    let error = slo
        .budget_left(11, 10)
        .expect_err("more successes than attempts is not a ratio");
    assert!(error.to_string().contains("ratio"), "{error}");
}
