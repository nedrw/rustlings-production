//! What `meshkv/5.5-membership` has to be true about.
//!
//! The step's subject is the window in which the cluster is two configurations at once. The cases
//! below are the questions that window has to answer: who decides during it (both sets), when it
//! ends (when the configuration entry commits), who counts afterwards (the new set), and what a
//! node that is not a voter may and may not do.

use meshkv::cluster::NodeId;
use meshkv::raft::membership::{Config, Membership};
use meshkv::raft::message::Message;
use meshkv::raft::state::Raft;
use meshkv::raft::{Index, Term};

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn config(text: &str) -> Config {
    Config::parse(text).expect("a valid configuration")
}

fn members(text: &str) -> Vec<NodeId> {
    text.split(',').map(id).collect()
}

/// A three-node cluster whose leader is `n1`.
fn leader(membership: &str) -> Raft {
    let mut node = Raft::new(id("n1"), members(membership));
    let _ = node.campaign();
    let _ = node.on_message(
        &id("n2"),
        Message::Vote {
            term: Term::new(1),
            granted: true,
        },
    );
    node
}

/// A follower that has taken `count` entries from `n1`.
fn follower(name: &str, membership: &str, count: usize) -> Raft {
    let mut node = Raft::new(id(name), members(membership));
    if count > 0 {
        let _ = node.on_message(
            &id("n1"),
            Message::Append {
                term: Term::new(1),
                leader: id("n1"),
                prev: meshkv::raft::message::LogPosition::empty(),
                entries: (1..=count as Index)
                    .map(|index| meshkv::raft::Entry::new(Term::new(1), index))
                    .collect(),
                commit: count as Index,
            },
        );
    }
    node
}

#[test]
fn a_configuration_parses_and_names_its_voters() {
    let three = config("n1,n2,n3");
    assert_eq!(three.voters().len(), 3);
    assert_eq!(three.len(), 3);
    assert!(!three.is_empty());
    assert!(three.contains(&id("n2")));
    assert!(!three.contains(&id("n9")));
    assert_eq!(three.render(), "n1,n2,n3");
    assert_eq!(three.majority(), 2, "a majority of three is two");

    for bad in ["", "n1,n1", "n1, n 2", "n1,,n2"] {
        assert!(
            Config::parse(bad).is_err(),
            "`{bad}` is not a configuration"
        );
    }
    assert!(
        Config::new(Vec::new()).is_err(),
        "a cluster of nobody decides nothing"
    );
}

#[test]
fn a_stable_membership_decides_with_a_majority_of_its_voters() {
    let membership = Membership::Stable(config("n1,n2,n3"));
    assert!(!membership.is_joint());
    assert!(
        !membership.is_met(&[&id("n1")]),
        "one of three is not a majority"
    );
    assert!(membership.is_met(&[&id("n1"), &id("n2")]));
    assert_eq!(membership.targeted().render(), "n1,n2,n3");
}

#[test]
fn a_joint_membership_needs_a_majority_of_both_sets() {
    // Three voters becoming five: the overlap is n1..n3.
    let membership = Membership::Joint {
        old: config("n1,n2,n3"),
        new: config("n1,n2,n3,n4,n5"),
    };
    assert!(membership.is_joint());
    assert!(
        !membership.is_met(&[&id("n1"), &id("n2")]),
        "a majority of the old set is not enough: the new set has seen nothing"
    );
    assert!(
        !membership.is_met(&[&id("n1"), &id("n4"), &id("n5")]),
        "a majority of the new set is not enough: the old set has seen only n1"
    );
    assert!(
        membership.is_met(&[&id("n1"), &id("n2"), &id("n4")]),
        "a majority of the old set and a majority of the new set decide"
    );
    assert_eq!(
        membership.voters().len(),
        5,
        "every voter of either set is a peer while the change is in flight"
    );
}

#[test]
fn a_leader_begins_a_change_and_the_cluster_requires_both_sets_until_it_commits() {
    let mut node = leader("n1,n2,n3");
    assert!(!node.membership().is_joint(), "it starts stable");

    let index = node
        .begin_membership(config("n1,n2,n3,n4,n5"))
        .expect("the leader may propose a change");
    assert_eq!(
        index, 2,
        "the entry follows the one the leader wrote when it won"
    );
    assert!(
        node.membership().is_joint(),
        "the change is in flight at once"
    );
    assert_eq!(
        node.membership().targeted().render(),
        "n1,n2,n3,n4,n5",
        "and the new set is the one it is moving to"
    );
    assert_eq!(
        node.peers().len(),
        4,
        "the joining nodes are peers immediately — they have to receive the entry"
    );

    // A majority of the old set alone does not commit the change.
    let _ = node.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(1),
            success: true,
            match_index: index,
        },
    );
    assert_eq!(
        node.commit_index(),
        0,
        "n1 and n2 are a majority of the old set and not of the new one"
    );
    assert!(
        node.membership().is_joint(),
        "so the change is still in flight"
    );

    // A majority of both sets does.
    for peer in ["n4", "n5"] {
        let _ = node.on_message(
            &id(peer),
            Message::AppendAck {
                term: Term::new(1),
                success: true,
                match_index: index,
            },
        );
    }
    assert!(
        !node.membership().is_joint(),
        "the committed configuration ends the change"
    );
    assert_eq!(
        node.membership().targeted().render(),
        "n1,n2,n3,n4,n5",
        "and the cluster is the new set"
    );
    assert_eq!(node.peers().len(), 4);
}

#[test]
fn a_second_change_is_refused_while_one_is_in_flight() {
    let mut node = leader("n1,n2,n3");
    let _ = node
        .begin_membership(config("n1,n2,n3,n4"))
        .expect("the first change");
    let error = node
        .begin_membership(config("n1,n2"))
        .expect_err("one change at a time");
    assert!(error.to_string().contains("in flight"), "{error}");
}

#[test]
fn a_follower_cannot_begin_a_change() {
    let mut node = follower("n2", "n1,n2,n3", 1);
    let error = node
        .begin_membership(config("n1,n2"))
        .expect_err("only the leader proposes");
    assert!(error.to_string().contains("leader"), "{error}");
}

#[test]
fn a_node_that_leaves_stops_counting_once_the_change_commits() {
    let mut node = leader("n1,n2,n3");
    let index = node.begin_membership(config("n1,n2")).expect("n3 leaves");
    for peer in ["n2", "n3"] {
        let _ = node.on_message(
            &id(peer),
            Message::AppendAck {
                term: Term::new(1),
                success: true,
                match_index: index,
            },
        );
    }
    assert!(!node.membership().is_joint(), "the change committed");
    assert_eq!(node.membership().targeted().render(), "n1,n2");
    assert!(
        !node.peers().contains(&id("n3")),
        "the node that left is no longer a peer: it is not asked for anything"
    );

    // And its acknowledgement no longer decides anything: n1 and n2 are the cluster now.
    let next = node.propose(b"after".to_vec()).expect("a write");
    let _ = node.on_message(
        &id("n3"),
        Message::AppendAck {
            term: Term::new(1),
            success: true,
            match_index: next,
        },
    );
    assert_eq!(
        node.commit_index(),
        index,
        "an acknowledgement from a node that left does not commit the new write"
    );
    let _ = node.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(1),
            success: true,
            match_index: next,
        },
    );
    assert_eq!(node.commit_index(), next, "the two voters decide it");
}

#[test]
fn a_follower_adopts_the_committed_configuration_from_the_append_it_receives() {
    let mut node = follower("n4", "n1,n2,n3", 0);
    // The leader sends the configuration entry, not yet committed.
    let config_entry = meshkv::raft::Entry::with_payload(
        Term::new(1),
        1,
        meshkv::raft::membership::encode(&config("n1,n2,n3,n4")),
    );
    let _ = node.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(1),
            leader: id("n1"),
            prev: meshkv::raft::message::LogPosition::empty(),
            entries: vec![config_entry.clone()],
            commit: 0,
        },
    );
    assert_eq!(
        node.membership().targeted().render(),
        "n1,n2,n3",
        "a configuration entry that is not committed changes nothing"
    );

    // The same entry, now committed: this node is a voter of the new configuration.
    let _ = node.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(1),
            leader: id("n1"),
            prev: meshkv::raft::message::LogPosition {
                term: Term::new(1),
                index: 1,
            },
            entries: Vec::new(),
            commit: 1,
        },
    );
    assert_eq!(
        node.membership().targeted().render(),
        "n1,n2,n3,n4",
        "a committed configuration entry moves the node to the new set"
    );
}

#[test]
fn a_change_in_flight_still_replicates_a_write_to_both_sets() {
    let mut node = leader("n1,n2,n3");
    let _ = node
        .begin_membership(config("n1,n2,n3,n4"))
        .expect("a change");
    let write = node
        .propose(b"during".to_vec())
        .expect("the leader still writes");
    let sent = node.append_entries();
    assert_eq!(
        sent.len(),
        3,
        "one message per peer, including the joining node"
    );
    assert!(
        sent.iter().any(|outbound| outbound.to == id("n4")),
        "the joining node receives the write: it will be a voter when the change commits"
    );
    // A majority of both sets commits the write.
    for peer in ["n2", "n4"] {
        let _ = node.on_message(
            &id(peer),
            Message::AppendAck {
                term: Term::new(1),
                success: true,
                match_index: write,
            },
        );
    }
    assert_eq!(
        node.commit_index(),
        write,
        "the old majority (n1, n2) and the new majority (n1, n2, n4) both hold it"
    );
}
