//! probe = "dist/topology@apply"
//! title = "A node's name is what the cluster votes about, and its address is only where it answers today"
//! hint = "Parse each `id=host:port` entry strictly, keep the specification's order, refuse a name twice, and make the greeting one exact line in both directions."
//!
//! Step `meshkv/5.1-multi-node-topology` is the first time a process has to know who else is in
//! the cluster, and everything it knows is one string. This probe is that string, the names it
//! hands out, and the one line two nodes greet each other with.
//!
//! Four rules, and a deployment has paid for each of them:
//!
//! 1. **a name is not an address.** A name is what the cluster commits, votes and replicates
//!    about; an address is where the node happens to answer today. A deployment that moves a node
//!    has changed an address and not an identity, so [`NodeId`] and [`Peer`] keep the two apart
//!    and no map in the system is keyed by the thing that moves;
//! 2. **a duplicate name is refused where the text is read.** Two nodes answering to `n1` is a
//!    cluster that can commit the same index twice under one identity. That is refused by the
//!    parser, not discovered by an operator at three in the morning;
//! 3. **a cluster is data before it is a network.** The whole of what a node knows about its peers
//!    is `id=addr,id=addr`, parsed by a pure function whose refusals name the entry they refused.
//!    An entry that is not `host:port`, or a trailing comma that leaves a nameless entry, is
//!    refused before a socket exists;
//! 4. **the greeting is one line in each direction.** `<protocol> hello <id>` is what a node sends
//!    and what it accepts, and the only thing the reader forgives is the line ending the transport
//!    added. An extra token or another protocol version is refused by quoting the line, so the
//!    peer that speaks it can see what this build heard.
//!
//! ## What you write
//!
//! * [`NodeId::parse`] — the accepted alphabet, with a refusal that spells it out;
//! * [`Topology::parse`] — `id=addr,id=addr`, refusing an empty specification, a malformed entry,
//!   a duplicate name, and an address that is not `host:port`;
//! * [`Topology::lookup`] — a name nobody has is a refusal that quotes it and names the cluster;
//! * [`Topology::render`] — the canonical specification, in the order it was written;
//! * [`hello_line`] and [`parse_hello`] — the handshake, one exact line in both directions.
//!
//! [`NodeId`], [`Peer`] and the accessors on [`Topology`] are given, because they are the
//! vocabulary of every later step in this track: `dist/membership@apply` rewrites addresses and
//! keeps the names, and `dist/consensus@apply` addresses its votes and appends to exactly these
//! names. What is missing is the checking, not the shape.
//!
//! ## What the tests ask
//!
//! That the order the specification was written in survives a round trip through
//! [`Topology::render`]; that a duplicate is refused by name and a malformed entry — including the
//! empty one a trailing comma leaves — is refused by position; that a one-character name is a name
//! and an empty one is not; that a name nobody has is answered with the cluster's own names; that
//! the greeting round-trips in both directions; and that two topologies that differ only in
//! addresses have the same identities, so moving a node did not rename it.
//!
//! ## Where this is honest, and where it is not
//!
//! There is no socket here, no listener, no `Node::bind` and no `accept` loop: the step ships a
//! daemon, and this probe does not. The line-length bound the step's TRAP asks for is also absent —
//! a `&str` parameter cannot model a peer choosing how much memory this process holds, and a test
//! that pretended to check it would be measuring a string it already holds. What is re-drilled is
//! the part every later step writes against: the alphabet, the refusals, the order two nodes can
//! compare, and the exact bytes of the greeting. A duplicate name survives every network fix, which
//! is why the parser is where it is caught.

use std::fmt;

/// Why a specification or a greeting was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// The smallest thing a cluster agrees about: a node's name.
///
/// Names are compared, logged and (from `5.3`) written into the log, so the accepted alphabet is
/// narrow on purpose: ASCII letters and digits, `-` and `_`. No spaces, no addresses, no surprises
/// in a `grep`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId {
    name: String,
}

impl NodeId {
    /// Check `text` into a name, or say why it is not one.
    ///
    /// # Errors
    ///
    /// [`Error`] when the text is empty, or holds a byte outside the accepted alphabet.
    pub fn parse(text: &str) -> Result<Self, Error> {
        todo!()
    }

    /// The name, as it is compared and logged.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// One member of the cluster: the name the system commits about, and where it can be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    id: NodeId,
    addr: String,
}

impl Peer {
    /// The node's name.
    #[must_use]
    pub fn id(&self) -> &NodeId {
        &self.id
    }

    /// Where the node answers, `host:port`.
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.addr
    }
}

/// Every node the cluster is made of, in the order the specification named them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topology {
    peers: Vec<Peer>,
}

impl Topology {
    /// Parse `n1=127.0.0.1:7001,n2=127.0.0.1:7002`.
    ///
    /// # Errors
    ///
    /// [`Error`] when the specification is empty, when an entry has no `=`, when a name is not a
    /// name, when an address is not `host:port`, or when two entries share a name — each message
    /// names the entry it refused.
    pub fn parse(spec: &str) -> Result<Self, Error> {
        todo!()
    }

    /// The peers, in specification order.
    #[must_use]
    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    /// How many nodes the cluster has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// `true` when the topology is empty. Never true for a parsed one — an empty cluster is the
    /// specification that cannot name a node, which [`Topology::parse`] refuses.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// The peer that answers to `id`, or a refusal that names the identity nobody has.
    ///
    /// # Errors
    ///
    /// [`Error`] when no peer holds the name.
    pub fn lookup(&self, id: &NodeId) -> Result<&Peer, Error> {
        todo!()
    }

    /// The canonical specification, `id=addr,id=addr`, in the order the peers were named.
    ///
    /// This is what a node logs at startup and what `meshkvc topology` prints, so two nodes that
    /// print the same string can only disagree about the cluster if somebody lied to one of them.
    #[must_use]
    pub fn render(&self) -> String {
        todo!()
    }
}

/// The one protocol version this build speaks.
pub const PROTOCOL: &str = "MESHKV/1";

/// The line a node sends to say who it is: `MESHKV/1 hello <id>`.
#[must_use]
pub fn hello_line(id: &NodeId) -> String {
    todo!()
}

/// The name in a greeting line, or a refusal that quotes what was read.
///
/// The line ending is optional because the bytes arrive however the reader split them; the line
/// ending is not part of the name. Everything else — another protocol version, a missing word, an
/// extra token — is refused rather than guessed at.
///
/// # Errors
///
/// [`Error`] when the line is not exactly this build's greeting around a valid name.
pub fn parse_hello(line: &str) -> Result<NodeId, Error> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "n1=127.0.0.1:7101,n2=127.0.0.1:7102,n3=127.0.0.1:7103";

    fn id(text: &str) -> NodeId {
        NodeId::parse(text).expect("a valid name")
    }

    fn topology() -> Topology {
        Topology::parse(SPEC).expect("a valid topology")
    }

    #[test]
    fn a_topology_keeps_the_specifications_order_and_renders_it_back() {
        let shuffled = "n3=127.0.0.1:3, n1=127.0.0.1:1 ,n2=127.0.0.1:2";
        let topology = Topology::parse(shuffled).expect("a valid topology");
        let names: Vec<&str> = topology
            .peers()
            .iter()
            .map(|peer| peer.id().as_str())
            .collect();
        assert_eq!(
            names,
            ["n3", "n1", "n2"],
            "the order is the specification's: a set that reordered itself would make two runs of \
             one command print two answers"
        );
        assert_eq!(topology.len(), 3);
        assert!(!topology.is_empty());
        assert_eq!(
            topology.render(),
            "n3=127.0.0.1:3,n1=127.0.0.1:1,n2=127.0.0.1:2",
            "render is the canonical specification, so two nodes that print the same string can \
             only disagree about the cluster if somebody lied to one of them"
        );
        assert_eq!(topology.peers()[1].addr(), "127.0.0.1:1");
    }

    #[test]
    fn a_duplicate_name_is_refused_by_name() {
        let error = Topology::parse("n1=127.0.0.1:1,n1=127.0.0.1:2")
            .expect_err("two nodes cannot answer to one name");
        let message = error.to_string();
        assert!(
            message.contains("n1"),
            "the refusal quotes the name that appeared twice: {message}"
        );
        assert!(
            message.contains("twice") || message.contains("duplicate"),
            "a duplicate name is worse than a missing one, because two nodes answering to n1 can \
             commit the same index twice under one identity: {message}"
        );
    }

    #[test]
    fn an_entry_that_is_not_host_and_port_is_refused_by_position() {
        for (spec, blamed) in [
            ("n1", "n1"),
            ("n1=", "n1"),
            ("n1=localhost", "n1"),
            ("n1=127.0.0.1:1,", "entry 2"),
        ] {
            let error = Topology::parse(spec).expect_err(&format!("`{spec}` is not a cluster"));
            assert!(
                error.to_string().contains(blamed),
                "the refusal of `{spec}` names the entry it refused (`{blamed}`), so an operator \
                 can find it in the file: {error}"
            );
        }
        let error = Topology::parse("  ").expect_err("an empty specification names no node");
        assert!(
            error.to_string().contains("empty"),
            "a cluster of no nodes has no identity to hand out: {error}"
        );
    }

    #[test]
    fn a_name_has_an_alphabet_with_an_exact_boundary() {
        assert_eq!(
            id("n").as_str(),
            "n",
            "a one-character name is a name: the minimum is one, not two"
        );
        assert_eq!(
            id("n-1_a").as_str(),
            "n-1_a",
            "letters, digits, `-` and `_` are the whole alphabet"
        );
        for text in ["", "n 1", "n1=127.0.0.1:1", "n/1", "näme"] {
            let error = NodeId::parse(text).expect_err(&format!("`{text}` is not a name"));
            let message = error.to_string();
            assert!(
                message.contains(text) || message.contains("name"),
                "the refusal of `{text}` says what a name is, so the next reader does not have to \
                 guess which byte was wrong: {message}"
            );
        }
    }

    #[test]
    fn a_name_nobody_has_is_answered_with_the_clusters_names() {
        let topology = topology();
        assert_eq!(
            topology.lookup(&id("n2")).expect("a member").addr(),
            "127.0.0.1:7102"
        );
        let error = topology
            .lookup(&id("n9"))
            .expect_err("n9 is not in this cluster");
        let message = error.to_string();
        assert!(
            message.contains("n9"),
            "the refusal quotes the name nobody has: {message}"
        );
        assert!(
            message.contains("n1") && message.contains("n2") && message.contains("n3"),
            "and it names the cluster it checked against, so an operator can compare the \
             specification the node holds with the one they thought it had: {message}"
        );
    }

    #[test]
    fn a_greeting_is_one_line_and_round_trips_in_both_directions() {
        assert_eq!(hello_line(&id("n1")), "MESHKV/1 hello n1");
        assert_eq!(
            parse_hello("MESHKV/1 hello n1")
                .expect("a greeting")
                .as_str(),
            "n1"
        );
        assert_eq!(
            parse_hello("MESHKV/1 hello n1\n")
                .expect("the line ending is optional")
                .as_str(),
            "n1",
            "the bytes arrive however the reader split them; the line ending is not part of the name"
        );

        for name in ["n1", "n-2_a"] {
            let line = hello_line(&id(name));
            assert_eq!(
                parse_hello(&line)
                    .expect("a line this build wrote, it can read")
                    .as_str(),
                name,
                "a greeting round-trips, so a node can recognise itself in what it hears"
            );
            assert_eq!(
                hello_line(&parse_hello(&line).expect("the name back")),
                line,
                "and the canonical line is the same bytes in both directions"
            );
        }

        for line in [
            "MESHKV/1 hello n1 extra",
            "MESHKV/1 n1",
            "hello n1",
            "MESHKV/2 hello n1",
            "n1",
        ] {
            let error = parse_hello(line).expect_err(&format!("`{line}` is not a greeting"));
            assert!(
                error.to_string().contains(line),
                "the refusal quotes what it read, so the peer knows which build this one speaks: \
                 {error}"
            );
        }
    }

    #[test]
    fn moving_a_node_changes_its_address_and_not_its_identity() {
        let before = topology();
        let moved = Topology::parse("n1=10.0.0.1:7001,n2=10.0.0.2:7002,n3=10.0.0.3:7003")
            .expect("a valid topology");
        let names = |topology: &Topology| {
            topology
                .peers()
                .iter()
                .map(|peer| peer.id().as_str().to_owned())
                .collect::<Vec<String>>()
        };
        assert_eq!(
            names(&before),
            names(&moved),
            "a deployment that moves a node has changed an address, not who that node is"
        );
        assert_eq!(
            moved.lookup(&id("n2")).expect("still a member").addr(),
            "10.0.0.2:7002",
            "the name finds the node at the address it answers at today, so nothing was keyed by \
             the address that moved"
        );
    }
}
