//! The command line both binaries share: one flag set, one refusal style.
//!
//! `cli/contract@apply — jobq/3.15` is the rule this follows: a binary answers its user with an
//! exit code and a sentence, and refuses what it does not understand *by name*. The gate that
//! holds every step to it (`exit-code`) hands each binary an argument no program can mean and
//! checks that the answer names it — which is why [`Flags::from_args`] quotes what it refused.

use crate::error::Result;

/// What both binaries tell a user who asks how to run them.
pub const USAGE: &str = "\
usage: meshkvd --cluster <spec> --node <id> [--listen <addr>] [--help] [--version]
       meshkvc topology --cluster <spec>

  --cluster <spec>   the cluster: `id=host:port` pairs, comma separated
  --node <id>        which of them this process is
  --listen <addr>    where to listen (default 127.0.0.1:7000; port 0 asks the kernel)";

/// The flags a node's command line can carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    /// The cluster specification, verbatim. Parsing it is [`crate::cluster::Topology::parse`]'s job,
    /// so the command line and a test hand a node exactly the same thing.
    pub cluster: Option<String>,
    /// The name this process answers to.
    pub node: Option<String>,
    /// Where to listen.
    pub listen: Option<String>,
    /// `--help` was asked for.
    pub help: bool,
    /// `--version` was asked for.
    pub version: bool,
}

impl Flags {
    /// Read flags, refusing anything not in [`USAGE`] by name.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for an unknown argument, a flag with no value, or a positional argument.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self> {
        todo!("meshkv/5.1-multi-node-topology: Flags::from_args")
    }
}
