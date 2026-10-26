//! Who is in the cluster: node identities and the topology every node is handed.
//!
//! A node's identity is a name, not an address. The address is where the node *can* be reached
//! today; the name is what the rest of the system commits, votes and replicates about. Steps after
//! this one keep those two apart on purpose (`5.7` routes by name, `5.9` authenticates a name), so
//! this step puts the difference in the type system: [`NodeId`] is the name, [`Peer`] pairs a name
//! with an address, and a [`Topology`] is the list of pairs — the whole of what a node knows about
//! the cluster before it has talked to anybody.
//!
//! The specification is one string, `id=addr,id=addr`, so that a test, a command line and a
//! configuration file all hand a node the same thing. Parsing it is deliberately strict: a
//! topology with a duplicate name is a cluster where two nodes answer to one identity, and one with
//! an address that is not `host:port` is a node nobody can reach — both are refused here, before a
//! socket is opened.

use std::fmt;

use crate::error::Result;

/// The smallest thing a cluster agrees about: a node's name.
///
/// Names are compared, logged and (from `5.3`) written into the log, so the accepted alphabet is
/// narrow on purpose: ASCII letters and digits, `-` and `_`. No spaces, no addresses, no surprises
/// in a `grep`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    name: String,
}

impl NodeId {
    /// Check `text` into a name, or say why it is not one.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the text is empty, or holds a byte outside the accepted alphabet.
    pub fn parse(text: &str) -> Result<Self> {
        todo!("meshkv/5.1-multi-node-topology: NodeId::parse")
    }

    /// The name, as it is compared and logged.
    #[must_use]
    pub fn as_str(&self) -> &str {
        todo!("meshkv/5.1-multi-node-topology: NodeId::as_str")
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(todo!("meshkv/5.1-multi-node-topology: NodeId::fmt"))
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
        todo!("meshkv/5.1-multi-node-topology: Peer::id")
    }

    /// Where the node answers, `host:port`.
    #[must_use]
    pub fn addr(&self) -> &str {
        todo!("meshkv/5.1-multi-node-topology: Peer::addr")
    }
}

/// Every node the cluster is made of, in the order the specification named them.
///
/// The order is kept because it is the only stable fact a test and a human share with the config
/// file: a set that reordered itself would make two runs of the same command print two answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topology {
    peers: Vec<Peer>,
}

impl Topology {
    /// Parse `n1=127.0.0.1:7001,n2=127.0.0.1:7002`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the specification is empty, when an entry has no `=`, when a name or
    /// an address is not valid, when an address is not `host:port`, or when two entries share a
    /// name — each message names the entry it refused.
    pub fn parse(spec: &str) -> Result<Self> {
        todo!("meshkv/5.1-multi-node-topology: Topology::parse")
    }

    /// The peers, in specification order.
    #[must_use]
    pub fn peers(&self) -> &[Peer] {
        todo!("meshkv/5.1-multi-node-topology: Topology::peers")
    }

    /// How many nodes the cluster has.
    #[must_use]
    pub fn len(&self) -> usize {
        todo!("meshkv/5.1-multi-node-topology: Topology::len")
    }

    /// `true` when the topology is empty. Never true for a parsed one — an empty cluster is the
    /// specification that cannot name a node, which [`Topology::parse`] refuses.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        todo!("meshkv/5.1-multi-node-topology: Topology::is_empty")
    }

    /// The peer that answers to `id`, or a refusal that names the identity nobody has.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when no peer holds the name.
    pub fn lookup(&self, id: &NodeId) -> Result<&Peer> {
        todo!("meshkv/5.1-multi-node-topology: Topology::lookup")
    }

    /// The canonical specification, `id=addr,id=addr`, in the order the peers were named.
    ///
    /// This is what a node logs at startup and what `meshkvc topology` prints, so two nodes that
    /// print the same string can only disagree about the cluster if somebody lied to one of them.
    #[must_use]
    pub fn render(&self) -> String {
        todo!("meshkv/5.1-multi-node-topology: Topology::render")
    }
}
