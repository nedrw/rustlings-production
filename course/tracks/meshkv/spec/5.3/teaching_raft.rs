//! What `meshkv/5.3-teaching-raft` has to be true about.
//!
//! The step's subject is a protocol, and a protocol is only as good as the questions one asks of
//! it. The cases below are the questions that separate "the code runs" from "the code is right":
//! who wins an election, when a vote may be refused, when an entry counts as committed, and what
//! happens to a node that has fallen behind. The last three cases are about the *wire*: a message
//! that cannot be parsed is a message nobody can answer, and an encoding that breaks on a payload
//! is an encoding that loses writes.

use meshkv::cluster::NodeId;
use meshkv::raft::message::{LogPosition, Message};
use meshkv::raft::state::{Outbound, Raft, Role};
use meshkv::raft::{Entry, Term};

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn members() -> Vec<NodeId> {
    vec![id("n1"), id("n2"), id("n3")]
}

fn node(name: &str) -> Raft {
    Raft::new(id(name), members())
}

/// The votes a candidate sent out.
fn requests(messages: &[Outbound]) -> Vec<&Outbound> {
    messages
        .iter()
        .filter(|outbound| matches!(outbound.message, Message::RequestVote { .. }))
        .collect()
}

/// A message a node sent, of one kind.
fn of_kind<'a>(messages: &'a [Outbound], wanted: &str) -> Vec<&'a Message> {
    messages
        .iter()
        .map(|outbound| &outbound.message)
        .filter(|message| {
            matches!(
                (wanted, message),
                ("vote", Message::Vote { .. })
                    | ("request", Message::RequestVote { .. })
                    | ("append", Message::Append { .. })
                    | ("ack", Message::AppendAck { .. })
            )
        })
        .collect()
}

#[test]
fn a_candidate_asks_everyone_and_a_majority_makes_it_the_leader() {
    let mut n1 = node("n1");
    assert_eq!(n1.role(), Role::Follower);
    assert_eq!(n1.term(), Term::zero(), "a fresh node is in term 0");

    let asked = n1.campaign();
    assert_eq!(n1.role(), Role::Candidate);
    assert_eq!(
        n1.term(),
        Term::new(1),
        "standing for election moves to the next term"
    );
    assert_eq!(
        requests(&asked).len(),
        2,
        "a node of three asks the other two"
    );

    // The candidate's own vote counts: one of the two peers is a majority of three.
    let answered = n1.on_message(
        &id("n2"),
        Message::Vote {
            term: Term::new(1),
            granted: true,
        },
    );
    assert_eq!(
        n1.role(),
        Role::Leader,
        "its own vote and one peer's are two of three"
    );
    assert!(
        !answered.is_empty(),
        "a new leader starts replicating right away"
    );
    assert_eq!(n1.leader().map(|leader| leader.as_str()), Some("n1"));

    // A vote that was not given changes nothing.
    let mut n3 = node("n3");
    let _ = n3.campaign();
    let refused = n3.on_message(
        &id("n2"),
        Message::Vote {
            term: Term::new(1),
            granted: false,
        },
    );
    assert!(refused.is_empty());
    assert_eq!(
        n3.role(),
        Role::Candidate,
        "a refused vote leaves the candidate a candidate"
    );
}

#[test]
fn a_vote_is_granted_only_to_a_candidate_that_is_caught_up() {
    let mut n2 = node("n2");
    // n2's log reaches term 1, position 1.
    n2.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(1),
            leader: id("n1"),
            prev: LogPosition::empty(),
            entries: vec![Entry::new(Term::new(1), 1)],
            commit: 0,
        },
    );

    // A candidate whose log is behind is refused.
    let behind = n2.on_message(
        &id("n3"),
        Message::RequestVote {
            term: Term::new(2),
            candidate: id("n3"),
            last: LogPosition {
                term: Term::zero(),
                index: 0,
            },
        },
    );
    let votes = of_kind(&behind, "vote");
    assert_eq!(votes.len(), 1);
    assert!(
        !matches!(votes[0], Message::Vote { granted: true, .. }),
        "a candidate with an empty log cannot ask a node with a longer one for its vote: {votes:?}"
    );

    // A candidate whose log is at least as long is granted — once.
    let caught_up = n2.on_message(
        &id("n3"),
        Message::RequestVote {
            term: Term::new(2),
            candidate: id("n3"),
            last: LogPosition {
                term: Term::new(1),
                index: 1,
            },
        },
    );
    let votes = of_kind(&caught_up, "vote");
    assert!(
        matches!(votes[0], Message::Vote { granted: true, .. }),
        "{votes:?}"
    );

    // The vote is spent: a second candidate in the same term is refused.
    let second = n2.on_message(
        &id("n1"),
        Message::RequestVote {
            term: Term::new(2),
            candidate: id("n1"),
            last: LogPosition {
                term: Term::new(1),
                index: 1,
            },
        },
    );
    let votes = of_kind(&second, "vote");
    assert!(
        !matches!(votes[0], Message::Vote { granted: true, .. }),
        "one vote per term, whichever candidate asks: {votes:?}"
    );
}

#[test]
fn a_leader_appends_its_write_and_sends_it_to_the_peers() {
    let mut n1 = leader("n1");
    let index = n1
        .propose(b"write-1".to_vec())
        .expect("a leader may propose");
    assert_eq!(
        index, 2,
        "the leader wrote one empty entry of its own term when it won"
    );
    assert_eq!(n1.log().len(), 2);

    let write = n1
        .propose(b"write-2".to_vec())
        .expect("a leader may propose");
    assert_eq!(write, 3);
    assert_eq!(
        n1.log().at(3).map(|entry| entry.payload()),
        Some(&b"write-2"[..]),
        "the write is in the leader's own log before anybody else hears of it"
    );

    let sent = n1.append_entries();
    assert_eq!(sent.len(), 2, "one message per peer");
    let appends = of_kind(&sent, "append");
    let Message::Append {
        term,
        leader,
        prev,
        entries,
        commit,
    } = appends[0]
    else {
        panic!("a leader sends appends");
    };
    assert_eq!(*term, Term::new(1));
    assert_eq!(leader, &id("n1"));
    assert_eq!(prev.index, 0, "nothing is on the peer yet");
    assert_eq!(
        entries.len(),
        3,
        "the empty entry of its own term and both writes ride along"
    );
    assert_eq!(*commit, 0, "nothing is committed yet");

    // A follower may not propose: the write has to go to the leader.
    let mut n2 = node("n2");
    let error = n2
        .propose(b"nope".to_vec())
        .expect_err("a follower has no log to write to");
    assert!(error.to_string().contains("leader"), "{error}");
}

/// A node that has won its election.
fn leader(name: &str) -> Raft {
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
    node
}

#[test]
fn a_follower_takes_the_entries_and_answers_how_far_its_log_reaches() {
    let mut n2 = node("n2");
    let answered = n2.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(1),
            leader: id("n1"),
            prev: LogPosition::empty(),
            entries: vec![Entry::new(Term::new(1), 1), Entry::new(Term::new(1), 2)],
            commit: 0,
        },
    );
    let acks = of_kind(&answered, "ack");
    assert_eq!(acks.len(), 1);
    let Message::AppendAck {
        success,
        match_index,
        ..
    } = acks[0]
    else {
        panic!("a follower answers with an ack");
    };
    assert!(*success, "the entries were accepted");
    assert_eq!(*match_index, 2, "the follower's log now reaches 2");
    assert_eq!(n2.log().len(), 2);
    assert_eq!(
        n2.commit_index(),
        0,
        "nothing is committed until the leader says so"
    );
}

#[test]
fn a_leader_commits_only_what_a_majority_holds() {
    // Five nodes, so that "the leader and one follower" is visibly not a majority.
    let five: Vec<NodeId> = ["n1", "n2", "n3", "n4", "n5"]
        .iter()
        .map(|name| id(name))
        .collect();
    let mut n1 = Raft::new(id("n1"), five);
    let _ = n1.campaign();
    for peer in ["n2", "n3", "n4", "n5"] {
        let _ = n1.on_message(
            &id(peer),
            Message::Vote {
                term: Term::new(1),
                granted: true,
            },
        );
    }
    assert_eq!(n1.role(), Role::Leader);
    let index = n1.propose(b"write".to_vec()).expect("propose");
    assert_eq!(n1.commit_index(), 0, "one node is not a majority");

    // One follower acknowledges: the leader and it are two of five — not a majority of three.
    let _ = n1.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(1),
            success: true,
            match_index: index,
        },
    );
    assert_eq!(
        n1.commit_index(),
        0,
        "two of five is not a majority: the write is not committed yet"
    );

    // Two more acknowledge: now it is.
    for peer in ["n3", "n4"] {
        let _ = n1.on_message(
            &id(peer),
            Message::AppendAck {
                term: Term::new(1),
                success: true,
                match_index: index,
            },
        );
    }
    assert_eq!(
        n1.commit_index(),
        index,
        "a majority holds it, so it is committed"
    );
    assert_eq!(n1.committed().len(), index as usize);
}

#[test]
fn an_entry_from_an_older_term_does_not_commit_even_with_a_majority() {
    // n1 inherits an entry of term 1, then wins the election in term 2.
    let mut n1 = Raft::new(id("n1"), members());
    let _ = n1.on_message(
        &id("n2"),
        Message::Append {
            term: Term::new(1),
            leader: id("n2"),
            prev: LogPosition::empty(),
            entries: vec![Entry::new(Term::new(1), 1)],
            commit: 0,
        },
    );
    let _ = n1.campaign();
    let _ = n1.on_message(
        &id("n2"),
        Message::Vote {
            term: Term::new(2),
            granted: true,
        },
    );
    assert_eq!(n1.role(), Role::Leader);
    assert_eq!(n1.term(), Term::new(2));
    assert_eq!(
        n1.log().at(1).map(|entry| entry.term),
        Some(Term::new(1)),
        "the inherited entry is still there"
    );

    // A follower acknowledges only the inherited entry.
    let _ = n1.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(2),
            success: true,
            match_index: 1,
        },
    );
    assert_eq!(
        n1.commit_index(),
        0,
        "a majority holds the inherited entry, and it still does not commit: only an entry of the \
         leader's own term may"
    );

    // Once the follower has the leader's own entry, both commit.
    let _ = n1.on_message(
        &id("n2"),
        Message::AppendAck {
            term: Term::new(2),
            success: true,
            match_index: 2,
        },
    );
    assert_eq!(
        n1.commit_index(),
        2,
        "the leader's own entry commits, and the inherited one comes with it"
    );
}

#[test]
fn a_follower_whose_log_diverges_is_corrected_by_the_leader() {
    // n2 believes the entry at 1 is from term 1; the leader says it is from term 2.
    let mut n2 = node("n2");
    let _ = n2.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(1),
            leader: id("n1"),
            prev: LogPosition::empty(),
            entries: vec![Entry::new(Term::new(1), 1)],
            commit: 0,
        },
    );
    assert_eq!(n2.log().at(1).map(|entry| entry.term), Some(Term::new(1)));

    // The new leader's log has a different entry at 1.
    let answered = n2.on_message(
        &id("n1"),
        Message::Append {
            term: Term::new(2),
            leader: id("n1"),
            prev: LogPosition::empty(),
            entries: vec![Entry::new(Term::new(2), 1), Entry::new(Term::new(2), 2)],
            commit: 0,
        },
    );
    let acks = of_kind(&answered, "ack");
    let Message::AppendAck { success, .. } = acks[0] else {
        panic!("a follower answers with an ack");
    };
    assert!(*success);
    assert_eq!(
        n2.log().at(1).map(|entry| entry.term),
        Some(Term::new(2)),
        "the leader's entry replaced the divergent one"
    );
    assert_eq!(
        n2.log().len(),
        2,
        "and the follower took the rest of the suffix too"
    );
}

#[test]
fn a_message_from_a_newer_term_makes_a_leader_a_follower_again() {
    let mut n1 = leader("n1");
    assert_eq!(n1.role(), Role::Leader);
    let _ = n1.on_message(
        &id("n2"),
        Message::Vote {
            term: Term::new(5),
            granted: true,
        },
    );
    assert_eq!(
        n1.role(),
        Role::Follower,
        "a node that hears a newer term steps down"
    );
    assert_eq!(n1.term(), Term::new(5));
    assert_eq!(n1.leader(), None, "and it no longer believes it leads");
}

#[test]
fn a_message_from_an_older_term_is_refused_with_this_nodes_term() {
    let mut n2 = node("n2");
    let _ = n2.campaign();
    assert_eq!(n2.term(), Term::new(1), "n2 moved on to term 1");
    let refused = n2.on_message(
        &id("n1"),
        Message::RequestVote {
            term: Term::zero(),
            candidate: id("n1"),
            last: LogPosition::empty(),
        },
    );
    let votes = of_kind(&refused, "vote");
    assert_eq!(votes.len(), 1, "the candidate gets an answer, not silence");
    let Message::Vote { term, granted } = votes[0] else {
        panic!("a vote request is answered with a vote");
    };
    assert_eq!(*term, Term::new(1), "the answer carries this node's term");
    assert!(!*granted);
}

#[test]
fn every_message_survives_the_wire() {
    let messages = [
        Message::RequestVote {
            term: Term::new(3),
            candidate: id("n2"),
            last: LogPosition {
                term: Term::new(2),
                index: 7,
            },
        },
        Message::Vote {
            term: Term::new(3),
            granted: true,
        },
        Message::Append {
            term: Term::new(3),
            leader: id("n1"),
            prev: LogPosition {
                term: Term::new(2),
                index: 7,
            },
            entries: vec![Entry::new(Term::new(3), 8)],
            commit: 5,
        },
        Message::AppendAck {
            term: Term::new(3),
            success: false,
            match_index: 2,
        },
    ];
    for message in messages {
        let line = message.encode();
        assert!(!line.contains('\n'), "a message is one line: {line}");
        let decoded = Message::decode(&line).expect("a message this build wrote, it can read");
        assert_eq!(decoded, message, "the wire changed the message: {line}");
    }
}

#[test]
fn a_write_rides_the_wire_without_breaking_the_line() {
    let payload = vec![0x00, 0x0a, 0xff, b'x'];
    let message = Message::Append {
        term: Term::new(1),
        leader: id("n1"),
        prev: LogPosition::empty(),
        entries: vec![Entry::with_payload(Term::new(1), 1, payload.clone())],
        commit: 0,
    };
    let line = message.encode();
    assert!(!line.contains('\n'), "{line}");
    let decoded = Message::decode(&line).expect("it decodes");
    let Message::Append { entries, .. } = decoded else {
        panic!("it is an append");
    };
    assert_eq!(entries[0].payload(), payload.as_slice());
}

#[test]
fn a_line_that_is_not_a_message_is_refused_by_quoting_it() {
    for line in [
        "",
        "hello",
        "vote",
        "vote 1 n2",
        "vote 1 n2 0 0 extra",
        "vote x n2 0 0",
        "vote-reply 1 2",
        "append 1 n1 0 0 0",
        "append 1 n1 0 0 0 1",
        "append 1 n1 0 0 0 1 1:1:2",
        "append-ack 1 1",
    ] {
        let error = Message::decode(line).expect_err(&format!("`{line}` is not a message"));
        assert!(
            error.to_string().contains(line.trim()) || line.trim().is_empty(),
            "the refusal quotes what it read: {error}"
        );
    }
}

#[test]
fn a_new_leader_writes_an_empty_entry_of_its_own_term() {
    // The subtlety from Raft's paper: a leader that inherited entries from an earlier term cannot
    // commit them by itself, so it writes one of its own — and commits the rest with it.
    let n1 = leader("n1");
    let own = n1.log().last().expect("the leader wrote an entry");
    assert_eq!(
        own.term,
        Term::new(1),
        "the entry is of the leader's own term"
    );
    assert!(own.payload().is_empty(), "and it carries nothing");
    assert_eq!(own.index, 1);
}
