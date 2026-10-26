//! What `meshkv/5.17-distributed-budgets` has to be true about.
//!
//! The step's subject is arithmetic a reader can check: the quorum, the fan-out, and the two ways to
//! divide them. The cases below are the numbers themselves, because a budget that is not checked is
//! a diagram.

use meshkv::budget::{cost, workload};

#[test]
fn the_quorum_is_a_majority_and_the_fan_out_is_every_peer() {
    for (nodes, quorum) in [(1, 1), (2, 2), (3, 2), (4, 3), (5, 3), (7, 4)] {
        let cost = cost(nodes).expect("a cluster");
        assert_eq!(cost.quorum, quorum, "{nodes} nodes decide with {quorum}");
        assert_eq!(cost.messages, nodes - 1, "the leader sends to every peer");
        assert_eq!(
            cost.acks,
            quorum - 1,
            "and waits for a majority of them, itself counted"
        );
    }
    assert!(cost(0).is_err(), "a cluster of no nodes has no quorum");
}

#[test]
fn a_write_and_a_linearizable_read_each_take_one_round_trip() {
    for nodes in 1..=9 {
        let cost = cost(nodes).expect("a cluster");
        assert_eq!(cost.write_hops, 1, "a write waits for one round trip");
        assert_eq!(cost.read_hops, 1, "and so does a linearizable read");
    }
}

#[test]
fn a_larger_cluster_costs_more_messages_and_less_per_node() {
    let small = workload(3, 1000).expect("a workload");
    let large = workload(5, 1000).expect("a workload");
    assert!(
        large.messages > small.messages,
        "five nodes carry more messages than three: {} vs {}",
        large.messages,
        small.messages
    );
    let small_share = small.per_node().expect("a share");
    let large_share = large.per_node().expect("a share");
    assert!(
        large_share > small_share,
        "and the average per-node share *rises*, because the leader's messages are fixed per write \
         while the nodes multiply: {large_share} vs {small_share}"
    );
}

#[test]
fn the_leader_does_more_than_a_follower_and_the_gap_is_the_fan_out() {
    for nodes in 2..=7 {
        let counts = workload(nodes, 100).expect("a workload");
        assert_eq!(
            counts.leader_messages,
            counts.follower_messages * (nodes - 1),
            "the leader sends one message per peer per write"
        );
        assert_eq!(
            counts.leader_messages, counts.messages,
            "and it is the only sender of write messages"
        );
    }
}

#[test]
fn the_acknowledgements_are_the_decision_and_nothing_else() {
    // For every cluster size, the number of acks per write is the number that decides it: quorum - 1,
    // the leader being one of the majority. A budget that counted the fan-out as the decision would
    // overstate what the leader waits for.
    for nodes in 1..=9 {
        let cost = cost(nodes).expect("a cluster");
        assert_eq!(cost.acks + 1, cost.quorum);
        assert!(
            cost.acks <= cost.messages,
            "the decision is a subset of the fan-out"
        );
    }
}
