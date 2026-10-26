//! What `meshkv/5.4-snapshots` has to be true about.
//!
//! The step's subject is a log that stops growing: the state at some index is kept, the entries
//! before it are thrown away, and a node that is too far behind catches up with the state rather
//! than with the entries. The cases below are the four things that have to be true for that to be
//! safe, and the last two are the ones that make it a *snapshot* rather than a copy.

use meshkv::cluster::NodeId;
use meshkv::raft::message::{LogPosition, Message};
use meshkv::raft::snapshot::Snapshot;
use meshkv::raft::state::Raft;
use meshkv::raft::{Entry, Index, Term};

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn members() -> Vec<NodeId> {
    vec![id("n1"), id("n2"), id("n3")]
}

/// A node that has won its election and appended `count` writes.
fn leader_with(name: &str, count: usize) -> Raft {
    let mut node = Raft::new(id(name), members());
    let _ = node.campaign();
    let _ = node.on_message(
        &id("n2"),
        Message::Vote {
            term: Term::new(1),
            granted: true,
        },
    );
    let _ = node.on_message(
        &id("n3"),
        Message::Vote {
            term: Term::new(1),
            granted: true,
        },
    );
    for _ in 0..count {
        let _ = node.propose(vec![b'x'; 8]);
    }
    node
}

/// A node whose log holds `count` entries, all of term 1.
fn follower_with(count: usize) -> Raft {
    let mut node = Raft::new(id("n2"), members());
    for index in 1..=count as Index {
        let _ = node.on_message(
            &id("n1"),
            Message::Append {
                term: Term::new(1),
                leader: id("n1"),
                prev: if index == 1 {
                    LogPosition::empty()
                } else {
                    LogPosition {
                        term: Term::new(1),
                        index: index - 1,
                    }
                },
                entries: vec![Entry::new(Term::new(1), index)],
                commit: index,
            },
        );
    }
    node
}

#[test]
fn a_snapshot_says_which_entry_it_came_from() {
    let snapshot = Snapshot::new(10, Term::new(3), b"state".to_vec()).expect("a snapshot");
    assert_eq!(snapshot.last_included_index, 10);
    assert_eq!(snapshot.last_included_term, Term::new(3));
    assert_eq!(snapshot.bytes(), 5);
    assert!(snapshot.covers(10), "it covers the entry it came from");
    assert!(snapshot.covers(4), "and everything before it");
    assert!(!snapshot.covers(11), "but not the entries after it");

    // A snapshot of nothing is a snapshot that says nothing: there is no entry zero.
    let error = Snapshot::new(0, Term::zero(), Vec::new()).expect_err("index 0 is before the log");
    assert!(error.to_string().contains("index 0"), "{error}");
}

#[test]
fn a_log_compacts_to_the_snapshot_and_keeps_its_place() {
    let mut node = follower_with(5);
    assert_eq!(node.log().len(), 5);

    let snapshot = Snapshot::new(3, Term::new(1), b"state".to_vec()).expect("a snapshot");
    let dropped = node.compact(&snapshot).expect("the log holds that entry");
    assert_eq!(dropped, 3, "the three entries the snapshot covers are gone");
    assert_eq!(node.log().len(), 2, "the log keeps what comes after");
    assert_eq!(node.log().base(), 3, "and it knows where it starts");

    // The positions are still the cluster's positions.
    assert_eq!(
        node.log().at(3).map(|entry| entry.index),
        None,
        "3 is compacted away"
    );
    assert_eq!(node.log().at(4).map(|entry| entry.index), Some(4));
    assert_eq!(node.log().at(5).map(|entry| entry.index), Some(5));

    // And the next entry belongs right after the snapshot.
    let _ = node.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(1),
            leader: id("n1"),
            prev: LogPosition {
                term: Term::new(1),
                index: 5,
            },
            entries: vec![Entry::new(Term::new(1), 6)],
            commit: 6,
        },
    );
    assert_eq!(node.log().len(), 3, "the entry after the snapshot is taken");
    assert_eq!(node.log().at(6).map(|entry| entry.index), Some(6));
}

#[test]
fn a_snapshot_that_does_not_match_the_log_is_refused() {
    let mut node = follower_with(5);

    // A snapshot of an entry the log does not hold.
    let beyond = Snapshot::new(9, Term::new(1), Vec::new()).expect("a snapshot");
    let error = node.compact(&beyond).expect_err("the log ends at 5");
    assert!(error.to_string().contains('9'), "{error}");

    // A snapshot of the same index from a different term: another history.
    let other = Snapshot::new(3, Term::new(2), Vec::new()).expect("a snapshot");
    let error = node.compact(&other).expect_err("the term does not match");
    let message = error.to_string();
    assert!(message.contains("term"), "{message}");
    assert_eq!(node.log().len(), 5, "and the log is untouched");
}

#[test]
fn a_snapshot_is_taken_of_the_committed_state_and_of_nothing_else() {
    let mut node = leader_with("n1", 2);
    // Nothing is committed yet: the two writes are in the log, but no follower acknowledged them.
    let error = node
        .take_snapshot(b"state".to_vec())
        .expect_err("an uncommitted state is not a state yet");
    assert!(error.to_string().contains("committed"), "{error}");

    // A follower acknowledges: now there is a committed state.
    let _ = node.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(1),
            success: true,
            match_index: node.log().len() as Index,
        },
    );
    let snapshot = node.take_snapshot(b"state".to_vec()).expect("a snapshot");
    assert_eq!(
        snapshot.last_included_index,
        node.commit_index(),
        "a snapshot is of the committed state"
    );
    assert_eq!(snapshot.last_included_term, Term::new(1));
}

#[test]
fn a_node_that_installs_a_snapshot_replaces_its_log() {
    let mut node = follower_with(3);
    assert_eq!(node.log().len(), 3);
    assert_eq!(node.commit_index(), 3);

    // A snapshot from further ahead: this node is too far behind to be caught up by entries.
    let snapshot = Snapshot::new(9, Term::new(2), b"state".to_vec()).expect("a snapshot");
    node.install_snapshot(&snapshot).expect("it installs");

    assert_eq!(
        node.log().len(),
        0,
        "the log is gone: the state replaced it"
    );
    assert_eq!(
        node.log().base(),
        9,
        "and the log starts after the snapshot"
    );
    assert_eq!(node.commit_index(), 9, "the state is applied");
    assert_eq!(
        node.snapshot().map(|held| held.last_included_index),
        Some(9),
        "and the node holds the snapshot it installed"
    );

    // The next entry belongs right after the snapshot.
    let _ = node.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(2),
            leader: id("n1"),
            prev: LogPosition {
                term: Term::new(2),
                index: 9,
            },
            entries: vec![Entry::new(Term::new(2), 10)],
            commit: 10,
        },
    );
    assert_eq!(node.log().at(10).map(|entry| entry.index), Some(10));
}

#[test]
fn a_snapshot_that_arrives_late_is_refused_rather_than_undoing_a_state() {
    let mut node = follower_with(5);
    let error = node
        .install_snapshot(&Snapshot::new(3, Term::new(1), Vec::new()).expect("a snapshot"))
        .expect_err("this node has already committed 5");
    assert!(error.to_string().contains("committed"), "{error}");
    assert_eq!(node.commit_index(), 5, "and the state is untouched");
}

#[test]
fn a_leader_sends_a_snapshot_to_a_peer_that_is_too_far_behind() {
    let mut leader = leader_with("n1", 3);
    // The leader commits and snapshots, then compacts the log it replaced.
    let _ = leader.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(1),
            success: true,
            match_index: leader.log().len() as Index,
        },
    );
    let snapshot = leader.take_snapshot(b"state".to_vec()).expect("a snapshot");
    leader.compact(&snapshot).expect("the log holds that entry");

    // n3 never acknowledged anything: it is at the beginning of the log, which is gone.
    assert!(
        leader.needs_snapshot(&id("n3")),
        "a peer behind the snapshot cannot be caught up by entries"
    );

    let sent = leader.append_entries();
    let snapshots: Vec<&Message> = sent
        .iter()
        .map(|outbound| &outbound.message)
        .filter(|message| matches!(message, Message::InstallSnapshot { .. }))
        .collect();
    assert_eq!(
        snapshots.len(),
        1,
        "the peer that is behind gets the snapshot, and the other gets entries: {sent:?}"
    );
    let Message::InstallSnapshot { snapshot, .. } = snapshots[0] else {
        panic!("it is a snapshot");
    };
    assert_eq!(snapshot.last_included_index, leader.commit_index());
}
