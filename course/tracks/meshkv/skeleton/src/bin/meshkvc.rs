//! `meshkvc` — the operator's view of the cluster.
//!
//! One command so far: `topology` prints the cluster the specification describes, in canonical
//! form, one `id addr` per line. It exists before the cluster can do anything because that is the
//! question an operator asks first and the answer a distrusted config file has to be checked
//! against: what did the two nodes believe, and did they believe the same thing?

use std::process::ExitCode;

use meshkv::Error;
use meshkv::cli::{Flags, USAGE};
use meshkv::cluster::Topology;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("meshkvc: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(Error::invalid(format!("no command given — {USAGE}")));
    };
    match command.as_str() {
        "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        "topology" => {
            let flags = Flags::from_args(args)?;
            let cluster = flags.cluster.as_deref().ok_or_else(|| {
                Error::invalid("topology needs `--cluster <spec>`, the specification to print")
            })?;
            let topology = Topology::parse(cluster)?;
            for peer in topology.peers() {
                let (id, addr) = (peer.id(), peer.addr());
                println!("{id} {addr}");
            }
            Ok(())
        }
        other => Err(Error::invalid(format!(
            "unknown command `{other}` — {USAGE}"
        ))),
    }
}
