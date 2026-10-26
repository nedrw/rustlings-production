//! probe = "dist/client-routing@apply"
//! title = "A redirect is followed to a leader, never in a circle, and never past the hop budget"
//! hint = "Ask the leader you remember before the node you were handed, follow the name a redirect gives you, and stop on a node you already asked, a node that knows no leader, or more redirects than `MAX_HOPS`."
//!
//! Step `meshkv/5.7-client-routing` is where a cluster's clients meet its rules. A client knows an
//! address — the one service discovery gave it, the one behind a load balancer, the one in a
//! configuration file that was right last week — and the cluster moves its leadership around. So
//! the client asks a node, and the node may not be the one that may serve the request. What
//! happens next is more than "ask someone else", and the four rules below are the step:
//!
//! 1. **the leader serves it, and a follower has to name one.** A redirect that names nobody is a
//!    dead end with a friendly name. A node that knows no leader says exactly that, and the client
//!    stops and says so rather than trying nodes at random until one happens to lead.
//! 2. **a redirect cycle is a failure, not a retry.** Two nodes that each believe the other leads —
//!    which happens during a leadership change — would send a client round for ever. The client
//!    remembers every node it asked *in this attempt*, and a next node it has already asked ends
//!    the attempt, named in the error. It does not ask that node a second time.
//! 3. **the chain is bounded by a number.** Even without a cycle, a chain of redirects has to end;
//!    [`MAX_HOPS`] is the number the client states, and the redirect that would exceed it is not
//!    followed. "The chain will end" is not a property a client can rely on.
//! 4. **what the client learned is worth keeping.** The leader a redirect named is what the next
//!    attempt asks *first*, before the node the caller handed in: one line, and the difference
//!    between one round trip and two. It is a hint, not a guarantee — the next attempt can be
//!    redirected again.
//!
//! ## What you write
//!
//! * [`Client::starting_node`] — where an attempt begins: the remembered leader, or the node the
//!   caller named when the client remembers none;
//! * [`Client::serve`] — the attempt: ask, follow the name a redirect gives, remember it, count
//!   the hops, and stop on a served answer, on a cycle, on a node that knows no leader, on the hop
//!   budget, or on a node the transport could not reach.
//!
//! [`Answer`], [`Transport`], [`RouteError`], [`Client::new`], [`Client::leader`] and
//! [`Client::hops`] are given. The transport is the seam — a trait with one method — because a
//! routing rule that can only be tested with a real network is a routing rule nobody tests; the
//! tests implement it with a table. [`RouteError`] is given with its four variants because the
//! vocabulary is the interface: a client that reports a node that is *down* and a node that is
//! *not the leader* the same way either retries a redirect or follows a dead node.
//!
//! ## What the tests ask
//!
//! A node that is the leader serves the request in one round trip. A follower's redirect is
//! followed, the redirect names the leader, and the client remembers it. A second request against a
//! different entry node goes straight to the remembered leader, and that attempt costs zero hops.
//! A chain that returns to its starting node is refused as a cycle with that node named, and the
//! nodes in the chain are asked exactly once. A chain that returns to a node in the *middle* is the
//! same refusal, which is what makes it a visited set rather than a comparison with the start. A
//! chain of exactly [`MAX_HOPS`] redirects is followed to the leader, and one redirect longer is
//! refused before the client asks the node that would have been next. A node that knows no leader
//! ends the attempt there. A node the transport cannot reach is a different answer from any of the
//! routing refusals, and it carries the transport's reason.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **no sockets, no cluster, no election and no daemon**. The
//! transport is a table in the tests; a redirect is a variant of an enum; the leader's own read path
//! and the server's refusal of a write at a follower are not here. The real step also changes
//! `propose` on a follower to answer with the leader's name, and that server half is what gives the
//! client a name to follow; this probe is the client half, driven from the outside.
//!
//! What is re-checked is the part that turns a routing rule into a production incident: a cycle
//! that spins, a chain that never ends, a dead end with a friendly name, and a client that pays for
//! the same redirect on every request. None of them needs a network to be wrong, and the shape is
//! identical whether the transport is a table or a socket. `dist/linearizable-read@design` is what
//! the node the client lands on still has to prove before it answers a read, and
//! `api/idempotency-key@design` is what the retry has to do when the redirect itself is retried: a
//! second attempt that reaches the leader must not be a second write.

/// How many redirects one attempt may follow before it gives up.
pub const MAX_HOPS: usize = 4;

/// What a node answers a request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The request was served; this is the value it produced.
    Served {
        /// The value the node answered with.
        value: Vec<u8>,
    },
    /// This node is not the leader: ask that one.
    Redirect {
        /// The leader this node names.
        leader: String,
    },
    /// This node does not know who the leader is — an election is probably running.
    NoLeader,
}

impl Answer {
    /// `true` when the request was served.
    #[must_use]
    pub fn is_served(&self) -> bool {
        matches!(self, Self::Served { .. })
    }
}

/// How a client reaches one node: the seam a test drives with a table and a daemon drives with a
/// socket.
pub trait Transport {
    /// Ask `node` to serve `request`, and answer what it said.
    ///
    /// # Errors
    ///
    /// A reason when the node could not be reached at all. A node that is *down* is a different
    /// failure from a node that is *not the leader*, and the client has to tell them apart: one is
    /// worth retrying, the other is worth following.
    fn call(&mut self, node: &str, request: &[u8]) -> Result<Answer, String>;
}

/// Why an attempt ended without a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// A node said it does not know who leads: there is nowhere to redirect to.
    NoLeader,
    /// The redirects came back to a node this attempt had already asked.
    Cycle {
        /// The node that would have been asked twice.
        node: String,
    },
    /// The chain was longer than [`MAX_HOPS`].
    TooManyHops {
        /// How many redirects the attempt followed before it stopped.
        hops: usize,
    },
    /// A node could not be reached at all, and this is what the transport said.
    Unreachable {
        /// The node the client was asking.
        node: String,
        /// The transport's reason.
        reason: String,
    },
}

/// A client that finds the leader by asking, and remembers where it found one.
#[derive(Debug, Default)]
pub struct Client {
    leader: Option<String>,
    hops: usize,
}

impl Client {
    /// A client that does not know anything yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The leader this client last saw, if it has seen one.
    #[must_use]
    pub fn leader(&self) -> Option<&str> {
        self.leader.as_deref()
    }

    /// How many redirects the last attempt followed.
    #[must_use]
    pub fn hops(&self) -> usize {
        self.hops
    }

    /// The node this attempt asks first: the leader this client remembers, or the node it was
    /// handed when it remembers none.
    ///
    /// A remembered leader is a hint, not a guarantee: the next attempt can still be redirected
    /// again, but starting at the last known leader saves the redirect when it has not moved.
    #[must_use]
    pub fn starting_node(&self, node: &str) -> String {
        todo!()
    }

    /// Ask `node` to serve `request`, following redirects.
    ///
    /// The client asks [`Client::starting_node`] first, then each name a redirect gives it. A node
    /// is asked at most once per attempt: a redirect that would ask one twice is
    /// [`RouteError::Cycle`], and more than [`MAX_HOPS`] redirects is [`RouteError::TooManyHops`].
    /// The leader a redirect names is remembered for the next attempt, and [`Client::hops`] is what
    /// this attempt cost.
    ///
    /// # Errors
    ///
    /// [`RouteError::NoLeader`] when a node cannot resolve a leader, the two routing refusals above,
    /// or [`RouteError::Unreachable`] when the transport could not reach a node at all.
    pub fn serve(
        &mut self,
        node: &str,
        request: &[u8],
        transport: &mut impl Transport,
    ) -> Result<Vec<u8>, RouteError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A transport that answers from a table and remembers what it was asked, in order.
    struct Table {
        answers: BTreeMap<String, Answer>,
        asked: Vec<String>,
    }

    impl Table {
        fn new(answers: &[(&str, Answer)]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|(node, answer)| ((*node).to_owned(), answer.clone()))
                    .collect(),
                asked: Vec::new(),
            }
        }
    }

    impl Transport for Table {
        fn call(&mut self, node: &str, _request: &[u8]) -> Result<Answer, String> {
            self.asked.push(node.to_owned());
            self.answers
                .get(node)
                .cloned()
                .ok_or_else(|| format!("{node} is not reachable"))
        }
    }

    /// A redirect to `leader`.
    fn redirects_to(leader: &str) -> Answer {
        Answer::Redirect {
            leader: leader.to_owned(),
        }
    }

    #[test]
    fn a_client_that_asks_the_leader_is_served() {
        let mut transport = Table::new(&[(
            "n1",
            Answer::Served {
                value: b"v".to_vec(),
            },
        )]);
        let mut client = Client::new();
        assert_eq!(
            client.serve("n1", b"get", &mut transport),
            Ok(b"v".to_vec()),
            "the leader serves the request, and the client returns the value"
        );
        assert_eq!(
            transport.asked,
            ["n1"],
            "one round trip, no detour: the node the caller named was already the leader"
        );
        assert_eq!(client.hops(), 0, "no redirect was followed");
    }

    #[test]
    fn a_client_that_asks_a_follower_follows_the_redirect() {
        let mut transport = Table::new(&[
            ("n2", redirects_to("n1")),
            (
                "n1",
                Answer::Served {
                    value: b"v".to_vec(),
                },
            ),
        ]);
        let mut client = Client::new();
        assert_eq!(
            client.serve("n2", b"get", &mut transport),
            Ok(b"v".to_vec()),
            "the redirect leads to the node that serves it"
        );
        assert_eq!(
            transport.asked,
            ["n2", "n1"],
            "the follower named the leader and the client asked it"
        );
        assert_eq!(client.hops(), 1, "one redirect was followed");
        assert_eq!(
            client.leader(),
            Some("n1"),
            "and the client remembers where the leader is"
        );
    }

    #[test]
    fn a_client_that_has_learned_the_leader_asks_it_first() {
        let mut transport = Table::new(&[
            ("n2", redirects_to("n1")),
            (
                "n1",
                Answer::Served {
                    value: b"v".to_vec(),
                },
            ),
        ]);
        let mut client = Client::new();
        assert_eq!(
            client.starting_node("n3"),
            "n3",
            "before it has learned anything, the client asks the node it was given"
        );
        assert_eq!(
            client.serve("n2", b"get", &mut transport),
            Ok(b"v".to_vec())
        );

        assert_eq!(
            client.starting_node("n3"),
            "n1",
            "the leader a redirect named is what the client asks first next time"
        );
        assert_eq!(
            client.serve("n3", b"get", &mut transport),
            Ok(b"v".to_vec()),
            "and the remembered leader serves the second request"
        );
        assert_eq!(
            transport.asked,
            ["n2", "n1", "n1"],
            "the remembered leader is tried before the node the caller named"
        );
        assert_eq!(
            client.hops(),
            0,
            "the second attempt was served without a redirect, so it cost no hops"
        );
    }

    #[test]
    fn a_redirect_that_comes_back_to_its_start_is_refused() {
        let mut transport = Table::new(&[("n2", redirects_to("n3")), ("n3", redirects_to("n2"))]);
        let mut client = Client::new();
        assert_eq!(
            client.serve("n2", b"get", &mut transport),
            Err(RouteError::Cycle {
                node: "n2".to_owned(),
            }),
            "the chain came back to where it started: asking n2 again would spin for ever"
        );
        assert_eq!(
            transport.asked,
            ["n2", "n3"],
            "the client stops before asking a node twice"
        );
    }

    #[test]
    fn a_redirect_that_returns_to_a_middle_node_is_a_cycle_too() {
        let mut transport = Table::new(&[
            ("n2", redirects_to("n3")),
            ("n3", redirects_to("n4")),
            ("n4", redirects_to("n3")),
        ]);
        let mut client = Client::new();
        assert_eq!(
            client.serve("n2", b"get", &mut transport),
            Err(RouteError::Cycle {
                node: "n3".to_owned(),
            }),
            "a cycle is a node that was already asked, not only the node the client started at"
        );
        assert_eq!(transport.asked, ["n2", "n3", "n4"]);
    }

    #[test]
    fn exactly_max_hops_is_followed_and_one_more_is_refused() {
        // n0 -> n1 -> n2 -> n3 -> n4, where n4 serves it: four redirects, which is the budget.
        let mut transport = Table::new(&[
            ("n0", redirects_to("n1")),
            ("n1", redirects_to("n2")),
            ("n2", redirects_to("n3")),
            ("n3", redirects_to("n4")),
            (
                "n4",
                Answer::Served {
                    value: b"v".to_vec(),
                },
            ),
        ]);
        let mut client = Client::new();
        assert_eq!(
            client.serve("n0", b"get", &mut transport),
            Ok(b"v".to_vec()),
            "a chain of exactly `MAX_HOPS` redirects still ends at the leader"
        );
        assert_eq!(
            client.hops(),
            MAX_HOPS,
            "and it cost the whole budget, not one hop more"
        );
        assert_eq!(
            transport.asked.len(),
            MAX_HOPS + 1,
            "the client asked every node in the chain"
        );

        // One redirect further, and the chain has no end the client is willing to pay for.
        let mut transport = Table::new(&[
            ("n0", redirects_to("n1")),
            ("n1", redirects_to("n2")),
            ("n2", redirects_to("n3")),
            ("n3", redirects_to("n4")),
            ("n4", redirects_to("n5")),
        ]);
        let mut client = Client::new();
        let error = client
            .serve("n0", b"get", &mut transport)
            .expect_err("a chain longer than the budget is refused");
        assert!(
            matches!(error, RouteError::TooManyHops { .. }),
            "the client gave up because the chain is too long, not because it found a leader: \
             {error:?}"
        );
        assert_eq!(
            transport.asked,
            ["n0", "n1", "n2", "n3", "n4"],
            "the redirect that would exceed the budget is not followed: the client stops before \
             asking n5"
        );
    }

    #[test]
    fn a_node_that_knows_no_leader_ends_the_attempt() {
        let mut transport = Table::new(&[("n2", Answer::NoLeader)]);
        let mut client = Client::new();
        assert_eq!(
            client.serve("n2", b"get", &mut transport),
            Err(RouteError::NoLeader),
            "a node that knows no leader gives the client nothing to redirect to: an election is \
             probably running"
        );
        assert_eq!(
            transport.asked,
            ["n2"],
            "and the client does not ask nodes at random"
        );
    }

    #[test]
    fn a_node_that_cannot_be_reached_is_not_a_routing_refusal() {
        let mut transport = Table::new(&[("n8", Answer::NoLeader)]);
        let mut client = Client::new();
        assert_eq!(
            client.serve("n9", b"get", &mut transport),
            Err(RouteError::Unreachable {
                node: "n9".to_owned(),
                reason: "n9 is not reachable".to_owned(),
            }),
            "a node that is down is worth retrying, while a redirect is worth following: the \
             client keeps the two apart and keeps the transport's reason"
        );
    }
}
