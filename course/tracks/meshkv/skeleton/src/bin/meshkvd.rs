//! `meshkvd` — one node of the cluster.
//!
//! What it does at startup, in order, and each step is a contract a spec can lean on:
//!
//! 1. read the flags and refuse what it does not understand (`cli/contract@apply — jobq/3.15`);
//! 2. parse the cluster specification and check this node is a member of it;
//! 3. bind, and print `listening on <addr>` — port `0` means the kernel chose, so the line is how
//!    anybody outside the process learns which port to use;
//! 4. accept peers, handshaking each one, until `SIGTERM` or `SIGINT`, then print `draining` and
//!    exit `0`.
//!
//! A peer that fails the handshake is *counted and dropped*: the daemon prints the refusal once and
//! keeps serving, because a listener that dies on its first bad connection is a listener anybody can
//! kill.

use std::process::ExitCode;

use meshkv::Error;
use meshkv::cli::{Flags, USAGE};
use meshkv::cluster::{NodeId, Topology};
use meshkv::node::Node;

/// Where a node listens when the command line does not say.
const DEFAULT_LISTEN: &str = "127.0.0.1:7000";

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("meshkvd: cannot start a runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("meshkvd: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Error> {
    let flags = Flags::from_args(std::env::args().skip(1))?;
    if flags.help {
        println!("{USAGE}");
        return Ok(());
    }
    if flags.version {
        println!("meshkvd {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cluster = flags.cluster.as_deref().ok_or_else(|| {
        Error::invalid("no `--cluster <spec>` given — a node starts only as a member of a cluster")
    })?;
    let name = flags.node.as_deref().ok_or_else(|| {
        Error::invalid("no `--node <id>` given — this process has to know who it is")
    })?;
    let listen = flags.listen.as_deref().unwrap_or(DEFAULT_LISTEN);
    let id = NodeId::parse(name)?;
    let topology = Topology::parse(cluster)?;
    let node = Node::bind(id, topology, listen).await?;
    let address = node.local_addr()?;
    println!("listening on {address}");

    // The signal handlers are installed before the first peer is accepted, and failing to install
    // one is a startup failure: a node that cannot be asked to stop is a node nobody can operate.
    let mut stop = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|source| Error::Network { source })?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|source| Error::Network { source })?;
    loop {
        tokio::select! {
            _ = stop.recv() => break,
            _ = interrupt.recv() => break,
            session = node.accept() => match session {
                Ok(session) => {
                    let peer = session.peer();
                    println!("peer {peer}");
                }
                // The refusal is printed and the node keeps serving: one bad connection is one
                // bad connection.
                Err(error) => eprintln!("meshkvd: peer refused: {error}"),
            },
        }
    }
    println!("draining");
    Ok(())
}
