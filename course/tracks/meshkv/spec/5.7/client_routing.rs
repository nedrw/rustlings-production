//! What `meshkv/5.7-client-routing` has to be true about.
//!
//! The step's subject is the path a request takes when the node it reaches is not the one that
//! may serve it. The cases below are the ways that path goes wrong: a redirect that names nobody, a
//! redirect that comes back to where it started, a chain that never ends, and a client that forgets
//! what it just learned.

use std::collections::BTreeMap;

use meshkv::client::{Answer, Client, MAX_HOPS, Transport};
use meshkv::cluster::NodeId;
use meshkv::error::{Error, Result};
use meshkv::raft::Term;
use meshkv::raft::message::Message;
use meshkv::raft::state::Raft;

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

/// A transport that answers from a table, and remembers where it was asked.
struct Table {
    answers: BTreeMap<NodeId, Answer>,
    asked: Vec<NodeId>,
}

impl Table {
    fn new(answers: Vec<(NodeId, Answer)>) -> Self {
        Self {
            answers: answers.into_iter().collect(),
            asked: Vec::new(),
        }
    }
}

impl Transport for Table {
    fn call(&mut self, node: &NodeId, _request: &[u8]) -> Result<Answer> {
        self.asked.push(node.clone());
        self.answers
            .get(node)
            .cloned()
            .ok_or_else(|| Error::Network {
                source: std::io::Error::other("the node is not in the table"),
            })
    }
}

#[test]
fn a_client_that_asks_the_leader_is_served() {
    let mut transport = Table::new(vec![(
        id("n1"),
        Answer::Served {
            value: b"v".to_vec(),
        },
    )]);
    let mut client = Client::new();
    let value = client
        .serve(&id("n1"), b"get", &mut transport)
        .expect("the leader serves it");
    assert_eq!(value, b"v");
    assert_eq!(transport.asked, [id("n1")], "one round trip, no detour");
    assert_eq!(client.hops(), 0, "and no redirect was followed");
}

#[test]
fn a_client_that_asks_a_follower_follows_the_redirect() {
    let mut transport = Table::new(vec![
        (id("n2"), Answer::Redirect { leader: id("n1") }),
        (
            id("n1"),
            Answer::Served {
                value: b"v".to_vec(),
            },
        ),
    ]);
    let mut client = Client::new();
    let value = client
        .serve(&id("n2"), b"get", &mut transport)
        .expect("the redirect leads to the leader");
    assert_eq!(value, b"v");
    assert_eq!(transport.asked, [id("n2"), id("n1")]);
    assert_eq!(client.hops(), 1, "one redirect was followed");
    assert_eq!(
        client.leader().map(|leader| leader.as_str().to_owned()),
        Some("n1".to_owned()),
        "and the client remembers where the leader is"
    );
}

#[test]
fn a_client_that_has_learned_the_leader_asks_it_first() {
    let mut transport = Table::new(vec![
        (id("n2"), Answer::Redirect { leader: id("n1") }),
        (
            id("n1"),
            Answer::Served {
                value: b"v".to_vec(),
            },
        ),
    ]);
    let mut client = Client::new();
    let _ = client
        .serve(&id("n2"), b"get", &mut transport)
        .expect("served");

    // The second request goes straight to the leader it learned about.
    let _ = client
        .serve(&id("n3"), b"get", &mut transport)
        .expect("served");
    assert_eq!(
        transport.asked,
        [id("n2"), id("n1"), id("n1")],
        "the remembered leader is tried before the node the caller named"
    );
}

#[test]
fn a_redirect_that_comes_back_to_its_start_is_refused() {
    // n2 and n3 both believe the other is the leader.
    let mut transport = Table::new(vec![
        (id("n2"), Answer::Redirect { leader: id("n3") }),
        (id("n3"), Answer::Redirect { leader: id("n2") }),
    ]);
    let mut client = Client::new();
    let error = client
        .serve(&id("n2"), b"get", &mut transport)
        .expect_err("a cycle ends the attempt");
    let message = error.to_string();
    assert!(message.contains("n2"), "{message}");
    assert!(
        transport.asked.len() <= 3,
        "the client stops instead of spinning: {:?}",
        transport.asked
    );
}

#[test]
fn the_number_of_redirects_is_bounded() {
    // A chain that never reaches a leader: every node points at the next one.
    let answers: Vec<(NodeId, Answer)> = (0..8)
        .map(|index| {
            (
                id(&format!("n{index}")),
                Answer::Redirect {
                    leader: id(&format!("n{}", index + 1)),
                },
            )
        })
        .collect();
    let mut transport = Table::new(answers);
    let mut client = Client::new();
    let error = client
        .serve(&id("n0"), b"get", &mut transport)
        .expect_err("the chain does not end");
    assert!(error.to_string().contains("redirect"), "{error}");
    assert!(
        transport.asked.len() <= MAX_HOPS + 1,
        "the client gave up after {} ask(s)",
        transport.asked.len()
    );
}

#[test]
fn a_node_that_does_not_know_the_leader_says_so() {
    let mut transport = Table::new(vec![(id("n2"), Answer::NoLeader)]);
    let mut client = Client::new();
    let error = client
        .serve(&id("n2"), b"get", &mut transport)
        .expect_err("there is nobody to redirect to");
    let message = error.to_string();
    assert!(message.contains("leader"), "{message}");
    assert_eq!(
        transport.asked,
        [id("n2")],
        "and the client does not keep asking"
    );
}

#[test]
fn a_node_that_cannot_be_reached_is_a_different_failure_from_a_redirect() {
    // A node that is *down* is worth retrying; a node that is *not the leader* is worth redirecting.
    // The client has to tell them apart, and the transport is where the difference comes from.
    let mut transport = Table::new(Vec::new());
    let mut client = Client::new();
    let error = client
        .serve(&id("n9"), b"get", &mut transport)
        .expect_err("the node is not there");
    assert!(
        matches!(error, Error::Network { .. }),
        "a node that cannot be reached is a network failure, not a routing one: {error}"
    );
}

#[test]
fn a_follower_refuses_a_write_and_names_the_leader() {
    let mut follower = Raft::new(id("n2"), vec![id("n1"), id("n2"), id("n3")]);
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
    let error = follower
        .propose(b"write".to_vec())
        .expect_err("a follower does not write");
    match error {
        Error::NotLeader { leader } => {
            assert_eq!(
                leader.map(|leader| leader.as_str().to_owned()),
                Some("n1".to_owned())
            );
        }
        other => panic!("the refusal names the leader, not {other:?}"),
    }
}
