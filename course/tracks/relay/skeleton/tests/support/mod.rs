//! The test harness every relay spec pulls in with `mod support;`.
//!
//! Four fixtures, one promise: nothing is shared between tests. Each is a module of its own, and a
//! spec uses the one it needs:
//!
//! | module | what it is | when a spec wants it |
//! | --- | --- | --- |
//! | [`app`] | the service *in process*: a state, a router, a thread, a port of its own | routes whose input is a body or a header |
//! | [`client`] | HTTP/1.1 over a socket, with headers on both sides | every test that has to see what came back |
//! | [`service`] | the real binary as a child process, its log, and a way to kill it | signals, recovery, log lines, fairness between two instances |
//! | [`postgres`] | one Postgres container, one empty database, per test | anything that is a claim about rows |
//!
//! The harness is the project's own file, not the course's: the specs (`tests/spec_*.rs`) are
//! read-only and come from `course/`, but this directory is the learner's, and a step that needs a
//! new fixture adds one here. Two rules hold for every fixture, and they are the reason the whole
//! suite can run in parallel:
//!
//! * **no fixture picks a port.** The kernel hands one to the specific child or thread that will
//!   use it, and the service announces it in its own log;
//! * **no fixture sleeps to synchronise.** Every wait is a condition with a deadline, so a failure
//!   says what it was waiting for instead of racing it.

pub mod app;
pub mod client;
pub mod postgres;
pub mod service;

/// One environment variable, as a fixture passes it to the service it starts.
///
/// A struct rather than a `(&str, &str)` tuple because a fixture's value is often owned (the
/// Postgres URL of a container, the key a test generated), and a tuple of borrowed strings would
/// force every caller to keep the owned value alive next to it.
#[derive(Debug, Clone)]
pub struct Var {
    /// Variable name, e.g. `RELAY__DATABASE__URL`.
    pub name: String,
    /// Value.
    pub value: String,
}

impl Var {
    /// Build one variable.
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            value: value.into(),
        }
    }
}

/// `Var::new`, for the fixture call sites.
pub fn var(name: &str, value: impl Into<String>) -> Var {
    Var::new(name, value)
}
