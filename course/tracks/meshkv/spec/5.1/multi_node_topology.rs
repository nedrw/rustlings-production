//! What `meshkv/5.1-multi-node-topology` has to be true about.
//!
//! The step hands out identities and a topology, and the cases below are the sentence "three nodes
//! start and each learns who else is in the cluster" broken into everything that can go wrong on
//! the way: a name that is not a name, a cluster with a duplicate, an address nobody can reach, a
//! peer that answers with the wrong identity, a handshake from another build, and a command line
//! that disagrees with the library about what a specification means.

use std::time::Duration;

use meshkv::cli::Flags;
use meshkv::cluster::{NodeId, Topology};
use meshkv::node::{Node, greet, hello_line, parse_hello};
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// The cluster every case here builds, with the addresses the specs use for parsing. The three-node
/// case rebinds the addresses the kernel actually handed out and keeps these names.
const SPEC: &str = "n1=127.0.0.1:7101,n2=127.0.0.1:7102,n3=127.0.0.1:7103";

/// Five seconds is a cluster test's patience: anything that has not answered by then is a hang, and
/// a hanging test is a failure with a name rather than a dead CI job.
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("the cluster answered within five seconds")
}

fn id(text: &str) -> NodeId {
    NodeId::parse(text).expect("a valid name")
}

fn topology() -> Topology {
    Topology::parse(SPEC).expect("a valid topology")
}

#[test]
fn the_topology_keeps_the_order_it_was_written_in_and_renders_it_back() {
    let shuffled = "n3=127.0.0.1:3,n1=127.0.0.1:1,n2=127.0.0.1:2";
    let topology = Topology::parse(shuffled).expect("a valid topology");
    let names: Vec<&str> = topology
        .peers()
        .iter()
        .map(|peer| peer.id().as_str())
        .collect();
    assert_eq!(
        names,
        ["n3", "n1", "n2"],
        "the order is the specification's"
    );
    assert_eq!(topology.len(), 3);
    assert!(!topology.is_empty());
    assert_eq!(
        topology.render(),
        shuffled,
        "render writes the canonical specification: `id=addr`, in order, comma separated"
    );
    assert_eq!(topology.peers()[1].addr(), "127.0.0.1:1");
}

#[test]
fn a_duplicate_name_is_refused_and_the_name_is_quoted() {
    let error =
        Topology::parse("n1=127.0.0.1:1,n1=127.0.0.1:2").expect_err("duplicates cannot load");
    let message = error.to_string();
    assert!(message.contains("n1"), "{message}");
    assert!(
        message.contains("twice") || message.contains("duplicate"),
        "the refusal says what is wrong with the name: {message}"
    );
}

#[test]
fn an_entry_that_is_not_a_host_and_port_is_refused_by_name() {
    for (spec, blamed) in [
        ("n1", "n1"),
        ("n1=", "n1"),
        ("n1=localhost", "n1"),
        ("n1=127.0.0.1:1,", ""),
    ] {
        let error = Topology::parse(spec).expect_err(&format!("`{spec}` is not a cluster"));
        let message = error.to_string();
        assert!(
            message.contains(blamed),
            "the refusal of `{spec}` quotes the entry it refused: {message}"
        );
    }
    let error = Topology::parse("  ").expect_err("an empty specification names no node");
    assert!(error.to_string().contains("empty"), "{error}");
}

#[test]
fn a_name_has_an_alphabet_and_the_refusal_says_which() {
    assert_eq!(
        id("n-1_a").as_str(),
        "n-1_a",
        "letters, digits, `-` and `_`"
    );
    for text in ["", "n 1", "n1=127.0.0.1:1", "n/1", "näme"] {
        let error = NodeId::parse(text).expect_err(&format!("`{text}` is not a name"));
        assert!(
            error.to_string().contains(text) || error.to_string().contains("name"),
            "the refusal of `{text}` says what a name is: {error}"
        );
    }
}

#[test]
fn a_name_nobody_has_is_not_a_peer_and_the_refusal_quotes_it() {
    let topology = topology();
    assert_eq!(
        topology.lookup(&id("n2")).expect("a member").addr(),
        "127.0.0.1:7102"
    );
    let error = topology
        .lookup(&id("n9"))
        .expect_err("n9 is not in this cluster");
    let message = error.to_string();
    assert!(message.contains("n9"), "{message}");
    assert!(
        message.contains("n1") && message.contains("n2") && message.contains("n3"),
        "the refusal names the cluster it checked against: {message}"
    );
}

#[tokio::test]
async fn a_node_cannot_bind_under_a_name_the_cluster_does_not_have() {
    let error = Node::bind(id("ghost"), topology(), "127.0.0.1:0")
        .await
        .expect_err("ghost is not a member");
    assert!(error.to_string().contains("ghost"), "{error}");
}

#[tokio::test]
async fn three_nodes_start_and_each_learns_who_else_is_in_the_cluster() {
    // Every node binds the same specification; port 0 asks the kernel for a port, so the addresses
    // the nodes are *reachable* at are only known after binding — which is exactly the point of
    // keeping names apart from addresses.
    let mut nodes: Vec<Arc<Node>> = Vec::new();
    for name in ["n1", "n2", "n3"] {
        let node = Node::bind(id(name), topology(), "127.0.0.1:0")
            .await
            .expect("a member binds");
        nodes.push(Arc::new(node));
    }
    let reachable = nodes
        .iter()
        .map(|node| {
            let address = node.local_addr().expect("the kernel reports the port");
            let id = node.id();
            format!("{id}={address}")
        })
        .collect::<Vec<_>>()
        .join(",");
    let reachable = Topology::parse(&reachable).expect("the reachable addresses are a topology");

    for (source, node) in nodes.iter().enumerate() {
        let me = node.id().clone();
        let mut learned = Vec::new();
        for (target, peer) in nodes.iter().enumerate() {
            if source == target {
                continue;
            }
            let peer_id = peer.id().clone();
            let address = peer
                .local_addr()
                .expect("a bound node has an address")
                .to_string();
            // The listener's half runs as its own task: a handshake is two awaits, one on each
            // side of the connection, so the test drives one side and spawns the other.
            let acceptor = {
                let peer = Arc::clone(peer);
                tokio::spawn(async move { peer.accept().await })
            };
            let greeted = within(greet(&address, &me, &peer_id, &reachable))
                .await
                .expect("the handshake completes");
            let served = within(acceptor)
                .await
                .expect("the accept task ran")
                .expect("the handshake completes");
            assert_eq!(served.peer(), &me, "the listener learned who connected");
            assert_eq!(greeted, peer_id, "the caller learned who answered");
            learned.push(greeted.as_str().to_owned());
        }
        learned.sort();
        let mut expected: Vec<String> = ["n1", "n2", "n3"]
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        expected.retain(|name| name != me.as_str());
        assert_eq!(learned, expected, "each node learned every other node");
    }
}
#[tokio::test]
async fn a_peer_that_answers_with_another_identity_is_refused() {
    // n2 is up, but the caller means to reach n3. Reaching the *wrong* node is not reaching the
    // right one late; it is a cluster that has learned something false about itself.
    let n2 = Arc::new(
        Node::bind(id("n2"), topology(), "127.0.0.1:0")
            .await
            .expect("n2 binds"),
    );
    let address = n2.local_addr().expect("an address").to_string();
    let cluster = topology();
    let acceptor = {
        let node = Arc::clone(&n2);
        tokio::spawn(async move { node.accept().await })
    };
    let greeted = within(greet(&address, &id("n1"), &id("n3"), &cluster))
        .await
        .expect_err("n2 answered, but not as n3");
    let _ = within(acceptor).await.expect("the accept task ran");
    let message = greeted.to_string();
    assert!(
        message.contains("n3") && message.contains("n2"),
        "the refusal names who was expected and who answered: {message}"
    );
}
#[tokio::test]
async fn a_handshake_from_another_build_is_refused_and_the_listener_keeps_serving() {
    let n1 = Arc::new(
        Node::bind(id("n1"), topology(), "127.0.0.1:0")
            .await
            .expect("n1 binds"),
    );
    let address = n1.local_addr().expect("an address").to_string();

    // Another version's greeting: refused, and the refusal quotes the line it read.
    let mut strange = TcpStream::connect(&address).await.expect("n1 is listening");
    strange
        .write_all(b"MESHKV/2 hello n1\n")
        .await
        .expect("the test can speak");
    let refused = within(n1.accept())
        .await
        .expect_err("another build is not this build");
    let message = refused.to_string();
    assert!(message.contains("MESHKV/2"), "{message}");
    drop(strange);

    // The refusal belonged to the connection, not to the node: the next peer is served normally.
    let cluster = topology();
    let acceptor = {
        let node = Arc::clone(&n1);
        tokio::spawn(async move { node.accept().await })
    };
    let greeted = within(greet(&address, &id("n2"), &id("n1"), &cluster))
        .await
        .expect("n2 reached n1");
    let served = within(acceptor)
        .await
        .expect("the accept task ran")
        .expect("the good handshake completes");
    assert_eq!(served.peer().as_str(), "n2");
    assert_eq!(greeted.as_str(), "n1");
}
#[test]
fn the_handshake_is_one_line_in_each_direction() {
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
    for line in ["MESHKV/1 hello n1 extra", "MESHKV/1 n1", "hello n1", "n1"] {
        let error = parse_hello(line).expect_err(&format!("`{line}` is not this build's greeting"));
        assert!(
            error.to_string().contains(line),
            "the refusal quotes what it read: {error}"
        );
    }
}

#[test]
fn the_cli_and_the_library_read_one_specification_the_same_way() {
    let flags = Flags::from_args([
        "--cluster".to_owned(),
        SPEC.to_owned(),
        "--node".to_owned(),
        "n1".to_owned(),
    ])
    .expect("a node command line");
    assert_eq!(flags.cluster.as_deref(), Some(SPEC));
    assert_eq!(flags.node.as_deref(), Some("n1"));
    let from_cli =
        Topology::parse(flags.cluster.as_deref().expect("the spec")).expect("a topology");
    assert_eq!(
        from_cli,
        topology(),
        "the command line hands the library its bytes unchanged"
    );

    // And a flag nobody knows is refused by name — the `exit-code` gate's whole subject.
    let error = Flags::from_args(["--kiln-exit-code-probe".to_owned()])
        .expect_err("an unknown flag is refused");
    assert!(
        error.to_string().contains("--kiln-exit-code-probe"),
        "{error}"
    );
}
