//! One node: its identity, the cluster it belongs to, and the handshake it answers with.
//!
//! Before a cluster can vote, replicate or commit anything, two nodes have to agree on *who* just
//! connected. That agreement is this step's whole subject, and it is one line of text in each
//! direction:
//!
//! ```text
//! MESHKV/1 hello <id>
//! ```
//!
//! A version, a word, and a name. The version is there because `5.21` will have to run two builds
//! against each other; the word is there so a stray connection is refused in the first line rather
//! than after buffering; and the name is checked against the topology — a node that is not in the
//! list is not a peer, whatever it claims.
//!
//! The connection stays open after the handshake: [`Session`] is the two ends of one authenticated
//! link, and the steps after this one put their messages through it instead of re-inventing framing.

use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};

use crate::cluster::{NodeId, Topology};
use crate::error::Result;

/// The protocol line every node opens with.
pub const HELLO_PREFIX: &str = "MESHKV/1 hello ";

/// The longest handshake line this build will read, in bytes.
///
/// A peer that sends more than this is not a node of this build — it is either a different protocol
/// or a connection that will never say anything useful, and buffering it is how a listener becomes a
/// memory leak. The limit is checked before the peer's identity is believed.
pub const MAX_LINE: usize = 256;

/// The line a node says to introduce itself.
#[must_use]
pub fn hello_line(id: &NodeId) -> String {
    todo!("meshkv/5.1-multi-node-topology: hello_line")
}

/// Read an identity out of a handshake line, or say what was wrong with it.
///
/// Accepts the line with or without its trailing line ending, because the bytes arrive however the
/// reader split them; anything else — another version, another word, extra tokens — is a refusal
/// that quotes what it read.
///
/// # Errors
///
/// [`Error::Protocol`] when the line is not this build's handshake, or its name is not a valid
/// [`NodeId`].
pub fn parse_hello(line: &str) -> Result<NodeId> {
    todo!("meshkv/5.1-multi-node-topology: parse_hello")
}

/// A node that has bound its listener and can be greeted.
#[derive(Debug)]
pub struct Node {
    id: NodeId,
    topology: Topology,
    listener: TcpListener,
}

impl Node {
    /// Bind `id`'s listener and check the node is a member of `topology`.
    ///
    /// `listen` is a `host:port`; port `0` asks the kernel for one, which is what a test does and
    /// what [`Node::local_addr`] is for.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the name is not in the topology, and [`Error::Network`] when the
    /// address cannot be parsed or bound.
    pub async fn bind(id: NodeId, topology: Topology, listen: &str) -> Result<Self> {
        todo!("meshkv/5.1-multi-node-topology: Node::bind")
    }

    /// This node's name.
    #[must_use]
    pub fn id(&self) -> &NodeId {
        todo!("meshkv/5.1-multi-node-topology: Node::id")
    }

    /// The cluster this node was handed.
    #[must_use]
    pub fn topology(&self) -> &Topology {
        todo!("meshkv/5.1-multi-node-topology: Node::topology")
    }

    /// The address the kernel actually gave this node — the one a peer can reach.
    ///
    /// # Errors
    ///
    /// [`Error::Network`] when the platform will not report it.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        todo!("meshkv/5.1-multi-node-topology: Node::local_addr")
    }

    /// Wait for the next peer and complete the handshake.
    ///
    /// A connection that fails the handshake is a refusal *of that connection*: the caller is told
    /// why, and a daemon counts it and keeps serving. The rule is the one
    /// `lakelet/4.1-ingest-protocol` wrote down for a decoder — after bytes that are not a
    /// handshake, nothing that follows can be trusted to start at a boundary.
    ///
    /// # Errors
    ///
    /// [`Error::Network`] when accepting fails; [`Error::Protocol`] when the peer's greeting is not
    /// this build's, is not UTF-8, or names a node the topology does not have.
    pub async fn accept(&self) -> Result<Session> {
        todo!("meshkv/5.1-multi-node-topology: Node::accept")
    }
}

/// One authenticated link: the peer's name, and the socket the next steps will write through.
#[derive(Debug)]
pub struct Session {
    peer: NodeId,
    stream: TcpStream,
}

impl Session {
    /// The name the other end proved with its handshake.
    #[must_use]
    pub fn peer(&self) -> &NodeId {
        todo!("meshkv/5.1-multi-node-topology: Session::peer")
    }

    /// Read one line, up to [`MAX_LINE`] bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Protocol`] when the peer sent more than a line this build will hold;
    /// [`Error::Network`] when the socket ends or fails.
    pub async fn read_line(&mut self) -> Result<String> {
        todo!("meshkv/5.1-multi-node-topology: Session::read_line")
    }

    /// Write one line.
    ///
    /// # Errors
    ///
    /// [`Error::Network`] when the socket refuses the write.
    pub async fn write_line(&mut self, line: &str) -> Result<()> {
        todo!("meshkv/5.1-multi-node-topology: Session::write_line")
    }
}

/// Connect to `addr`, shake hands as `me`, and answer the name the other end proved.
///
/// `expected` is who the caller means to reach: the node whose address this is, according to the
/// topology. A peer that answers with any other name — including a name that *is* in the cluster —
/// is refused, because reaching the wrong node is not reaching the right one late; it is a cluster
/// that has learned something false about itself.
///
/// # Errors
///
/// [`Error::Invalid`] when either name is not in the topology; [`Error::Network`] when the address
/// cannot be reached; [`Error::Protocol`] when the other end answers with the wrong identity.
pub async fn greet(
    addr: &str,
    me: &NodeId,
    expected: &NodeId,
    topology: &Topology,
) -> Result<NodeId> {
    todo!("meshkv/5.1-multi-node-topology: greet")
}
