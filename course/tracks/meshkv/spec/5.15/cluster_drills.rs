//! What `meshkv/5.15-cluster-drills` has to be true about.
//!
//! The step's subject is a schedule, and the cases below are the ways a schedule goes wrong: it
//! misses a node, names a stranger, restarts one twice, takes the cluster below its majority, or
//! starts from a cluster that is already broken.

use meshkv::cluster::NodeId;
use meshkv::restart::Plan;

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn cluster(names: &[&str]) -> Vec<NodeId> {
    names.iter().map(|name| node(name)).collect()
}

#[test]
fn a_plan_names_every_member_exactly_once() {
    let members = cluster(&["n1", "n2", "n3"]);
    let plan = Plan::new(&members, &members).expect("a plan");
    assert_eq!(plan.order().len(), 3);
    assert_eq!(plan.majority(), 2);

    let short = Plan::new(&members, &cluster(&["n1", "n2"])).expect_err("a node is missing");
    assert!(short.to_string().contains('3'), "{short}");

    let twice =
        Plan::new(&members, &cluster(&["n1", "n1", "n2"])).expect_err("one node is named twice");
    assert!(twice.to_string().contains("twice"), "{twice}");

    let stranger =
        Plan::new(&members, &cluster(&["n1", "n2", "n9"])).expect_err("n9 is not a member");
    assert!(stranger.to_string().contains("n9"), "{stranger}");
}

#[test]
fn a_rolling_restart_keeps_a_majority_at_every_step() {
    let members = cluster(&["n1", "n2", "n3"]);
    let plan = Plan::new(&members, &members).expect("a plan");
    let timeline = plan
        .run(&members)
        .expect("restarting one at a time keeps two of three up");
    assert_eq!(timeline.len(), 3);
    for step in &timeline {
        assert!(step.was_up, "each node was up before its turn");
        assert!(
            step.majority_kept,
            "and the majority held while it was down"
        );
    }
}

#[test]
fn a_cluster_of_five_can_lose_two_nodes_and_a_cluster_of_three_cannot() {
    let five = cluster(&["n1", "n2", "n3", "n4", "n5"]);
    let plan = Plan::new(&five, &five).expect("a plan");
    assert_eq!(plan.majority(), 3);
    let timeline = plan
        .run(&five)
        .expect("five nodes survive a rolling restart");
    assert_eq!(timeline.len(), 5);

    // Three nodes, two of them down: the restart of the third cannot keep a majority.
    let three = cluster(&["n1", "n2", "n3"]);
    let plan = Plan::new(&three, &three).expect("a plan");
    let error = plan
        .run(&cluster(&["n1"]))
        .expect_err("one of three is below the majority");
    assert!(error.to_string().contains("majority"), "{error}");
}

#[test]
fn a_node_that_is_already_down_is_restarted_where_its_turn_comes() {
    let members = cluster(&["n1", "n2", "n3"]);
    let plan = Plan::new(&members, &cluster(&["n2", "n1", "n3"])).expect("a plan");
    // n2 is down; the plan's first step brings it back, and the cluster keeps its majority (n1, n3).
    let timeline = plan.run(&cluster(&["n1", "n3"])).expect("a plan");
    assert_eq!(timeline[0].node, node("n2"));
    assert!(!timeline[0].was_up, "n2 was down before its turn");
    assert!(timeline[0].majority_kept);
}

#[test]
fn a_plan_that_would_stop_the_cluster_is_refused_before_it_runs() {
    // A five-node cluster with two already down: the majority is exactly the three that are up, so
    // the first restart takes it to two and the plan refuses.
    let five = cluster(&["n1", "n2", "n3", "n4", "n5"]);
    let plan = Plan::new(&five, &five).expect("a plan");
    let error = plan
        .run(&cluster(&["n1", "n2", "n3"]))
        .expect_err("restarting n1 leaves two of five");
    let message = error.to_string();
    assert!(message.contains("n1"), "{message}");
    assert!(message.contains("3"), "{message}");
}

#[test]
fn a_plan_over_an_empty_cluster_is_refused() {
    let none: Vec<NodeId> = Vec::new();
    let error = Plan::new(&none, &none).expect_err("a plan restarts at least one node");
    assert!(error.to_string().contains("at least one"), "{error}");
}
