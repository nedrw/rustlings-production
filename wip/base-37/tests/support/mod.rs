//! The test harness every jobq spec pulls in with `mod support;`.
//!
//! Three fixtures, one promise: nothing is shared between tests. Each is a module of its own, and a
//! spec uses the one it needs — a step that needs a database adds `postgres`, a step that has to
//! judge the port rather than one backend adds `store` — because the harness is the project's own
//! file, not the course's. The specs (`tests/spec_*.rs`) are read-only and come from `course/`; this
//! directory is the learner's.
//!
//! | module | what it is | when a spec wants it |
//! | --- | --- | --- |
//! | [`postgres`] | one Postgres container with one empty database, per test | anything a durable backend has to be measured against |
//! | [`service`] | the real `jobqd` binary as a child process: its log, its exit code, `SIGTERM` and `SIGKILL` | signals, drain order, crash recovery, two workers at once |
//! | [`store`] | the store a conformance suite runs against, chosen by `KILN_BACKEND` | a spec that judges the port rather than one backend |
//!
//! Two rules hold for every fixture in here, and they are the reason the suite can run in parallel:
//!
//! * **no fixture picks a resource another test could pick** — a port is handed out by the kernel,
//!   a container is named after the test that owns it;
//! * **no fixture sleeps to synchronise.** Every wait is a condition with a deadline, so a failure
//!   says what it was waiting for instead of racing it.

// The harness is a menu, and a spec orders from it: a fixture nothing in *this* target calls is not
// unfinished work, it is a fixture another spec calls. The relaxation belongs at the root of the
// directory rather than in each file, because there is no target in this project that compiles one
// fixture without the module that declares it (authoring.md §7.4 says why a test target may be
// relaxed, and this is that relaxation's smallest honest form).
#![allow(dead_code)]

pub mod postgres;
pub mod service;
pub mod store;

/// One environment variable, as a fixture passes it to the process it starts.
///
/// A struct rather than a `(&str, &str)` tuple because a fixture's value is often owned (the
/// connection string of a container, the name a test generated), and a tuple of borrowed strings
/// would force every caller to keep the owned value alive next to it.
#[derive(Debug, Clone)]
pub struct Var {
    /// Variable name, e.g. `JOBQ__STORE__URL`.
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
