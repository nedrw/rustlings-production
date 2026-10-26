//! What `meshkv/5.6-lease-reads` has to be true about.
//!
//! A read is a question about *now*, and the cases below are the ways a node can get that question
//! wrong: answering while it is a follower, answering as a leader without having confirmed it still
//! is one, answering past the lease it was given, keeping a lease after it stopped leading, and
//! claiming a position newer than the one the round actually proved.

use meshkv::cluster::NodeId;
use meshkv::raft::Term;
use meshkv::raft::message::Message;
use meshkv::raft::read::{ELECTION_TIMEOUT_MILLIS, LEASE_MILLIS, ReadAnswer, is_safe_lease};
use meshkv::raft::state::Raft;

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn members() -> Vec<NodeId> {
    vec![id("n1"), id("n2"), id("n3")]
}

/// A three-node cluster whose leader is `n1`, with `writes` entries committed.
fn leader_with(writes: usize) -> Raft {
    let mut node = Raft::new(id("n1"), members());
    let _ = node.campaign();
    let _ = node.on_message(
        &id("n2"),
        Message::Vote {
            term: Term::new(1),
            granted: true,
        },
    );
    for _ in 0..writes {
        let _ = node.propose(vec![b'x'; 4]);
    }
    if writes > 0 {
        let index = node.log().len() as meshkv::raft::Index;
        let _ = node.on_message(
            &id("n2"),
            Message::AppendAck {
                term: Term::new(1),
                success: true,
                match_index: index,
            },
        );
    }
    node
}

#[test]
fn a_follower_does_not_answer_a_read_and_says_who_does() {
    let mut follower = Raft::new(id("n2"), members());
    // It learns who the leader is from the first append it accepts.
    let _ = follower.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(1),
            leader: id("n1"),
            prev: meshkv::raft::message::LogPosition::empty(),
            entries: vec![meshkv::raft::Entry::new(Term::new(1), 1)],
            commit: 1,
        },
    );
    match follower.serve_read(0) {
        ReadAnswer::NotLeader { leader } => {
            assert_eq!(
                leader.map(|leader| leader.as_str().to_owned()),
                Some("n1".to_owned())
            );
        }
        other => panic!("a follower answers a read with `NotLeader`, not {other:?}"),
    }
}

#[test]
fn a_leader_without_a_round_asks_for_one() {
    let node = leader_with(1);
    assert_eq!(
        node.serve_read(0),
        ReadAnswer::NeedsQuorum,
        "a leader with no lease has nothing to answer with yet"
    );
    assert!(node.lease().is_none());
}

#[test]
fn a_read_round_confirms_leadership_and_answers_at_the_commit_index() {
    let mut node = leader_with(1);
    let committed = node.commit_index();
    assert!(committed > 0, "the fixture committed a write");

    let heartbeats = node.confirm_read(1_000).expect("the leader starts a round");
    assert!(
        !heartbeats.is_empty(),
        "a round is a heartbeat to every peer"
    );
    assert_eq!(
        node.read_index(),
        Some(committed),
        "the round remembered the commit index it saw"
    );

    // One follower of three is a majority with the leader itself.
    let answer = node
        .complete_read(&[id("n1"), id("n2")], 1_100)
        .expect("the leader finishes a round");
    assert_eq!(
        answer,
        ReadAnswer::Served { index: committed },
        "the answer names the position it is correct at"
    );
    assert_eq!(
        node.serve_read(1_100),
        ReadAnswer::Served { index: committed },
        "and the lease it granted answers the next read at the same position"
    );
}

#[test]
fn a_round_without_a_majority_grants_nothing() {
    let mut node = leader_with(1);
    let _ = node.confirm_read(0).expect("a round");
    let answer = node
        .complete_read(&[id("n1")], 0)
        .expect("the leader finishes a round");
    assert_eq!(
        answer,
        ReadAnswer::NeedsQuorum,
        "one of three is not a majority"
    );
    assert!(
        node.lease().is_none(),
        "a round that did not confirm leadership grants no lease"
    );
    assert_eq!(node.serve_read(0), ReadAnswer::NeedsQuorum);
}

#[test]
fn the_lease_expires_and_an_expired_lease_does_not_answer() {
    let mut node = leader_with(1);
    let committed = node.commit_index();
    let _ = node.confirm_read(1_000).expect("a round");
    let _ = node
        .complete_read(&[id("n1"), id("n2")], 1_000)
        .expect("a majority");

    let last_moment = 1_000 + LEASE_MILLIS - 1;
    assert_eq!(
        node.serve_read(last_moment),
        ReadAnswer::Served { index: committed },
        "one millisecond before the expiry is still inside the lease"
    );
    assert_eq!(
        node.serve_read(1_000 + LEASE_MILLIS),
        ReadAnswer::NeedsQuorum,
        "the instant it expires, it is over — the safe side of the comparison"
    );
}

#[test]
fn stepping_down_drops_the_lease() {
    let mut node = leader_with(1);
    let _ = node.confirm_read(0).expect("a round");
    let _ = node
        .complete_read(&[id("n1"), id("n2")], 0)
        .expect("a majority");
    assert!(node.serve_read(1).is_served());

    // A message from a newer term makes it a follower, and a follower has no lease: the promise it
    // made was about being the leader.
    let _ = node.on_message(
        &id("n2"),
        Message::RequestVote {
            term: Term::new(9),
            candidate: id("n2"),
            last: meshkv::raft::message::LogPosition::empty(),
        },
    );
    assert!(node.lease().is_none(), "the lease went with the leadership");
    assert!(
        !node.serve_read(1).is_served(),
        "and a read is refused rather than answered from a term that has ended"
    );
}

#[test]
fn the_lease_answers_the_position_the_round_proved_and_not_a_later_one() {
    let mut node = leader_with(1);
    let proved = node.commit_index();
    let _ = node.confirm_read(0).expect("a round");
    let _ = node
        .complete_read(&[id("n1"), id("n2")], 0)
        .expect("a majority");

    // A write commits after the round.
    let written = node.propose(b"after".to_vec()).expect("a write");
    let _ = node.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(1),
            success: true,
            match_index: written,
        },
    );
    assert!(node.commit_index() > proved, "the write committed");

    assert_eq!(
        node.serve_read(1),
        ReadAnswer::Served { index: proved },
        "the lease answers the position it proved, not the newer commit: a caller told `proved` \
         never reads a value committed after the round"
    );
}

#[test]
fn a_follower_cannot_start_a_read_round() {
    let mut follower = Raft::new(id("n2"), members());
    let error = follower
        .confirm_read(0)
        .expect_err("a follower has no leadership to confirm");
    assert!(error.to_string().contains("leader"), "{error}");
}

#[test]
fn a_lease_has_to_be_shorter_than_the_election_timeout() {
    assert!(
        is_safe_lease(LEASE_MILLIS, ELECTION_TIMEOUT_MILLIS),
        "the course's constant is safe"
    );
    assert!(is_safe_lease(999, 1_000));
    assert!(
        !is_safe_lease(1_000, 1_000),
        "a lease as long as the timeout can be outlived by an election that starts at the same instant"
    );
    assert!(!is_safe_lease(1_001, 1_000));
}
