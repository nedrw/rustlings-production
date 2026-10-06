//! `conformance@N` — one suite, every implementation of a port (handoff M2 §1).
//!
//! A port is only a port if more than one thing implements it, and a suite is only a conformance
//! suite if it passes on **all** of them: a store that works against the in-memory implementation and
//! breaks on Postgres has a bug the tests cannot see, because the tests were written against whichever
//! implementation the developer happened to run.
//!
//! The project says which implementations exist, in `<project>/conformance.toml`:
//!
//! ```toml
//! backends = ["memory", "postgres"]
//! ```
//!
//! and the gate does the one thing a single test target cannot: it runs the step's own `run.cmd` once
//! per backend, with `KILN_BACKEND=<name>` in the environment, and requires every run to pass. The
//! convention the spec side keeps is that `tests/support/store.rs` builds the store from that
//! variable, so "the same suite on every implementation" is one suite run N times, not N suites.
//!
//! A missing or too-small declaration is a *failure*, not a block: the file is the step's own
//! deliverable, and a step about ports that lists one backend has not done the work. A missing
//! `run.cmd` is a block, because then there is no suite to run at all.
//!
//! Distinctness matters to the count. `["memory", "memory"]` is one implementation written twice, and
//! accepting it would make `conformance@2` a claim nobody checked.

use serde::Deserialize;

use crate::error::Result;
use crate::gates::harness;
use crate::gates::runner::GateContext;
use crate::gates::{GateOutcome, GateSpec};
use crate::process::{self, Ran};

/// The file a project declares its backends in.
pub const FILE: &str = "conformance.toml";

/// The environment variable the spec's store factory reads to pick an implementation.
pub const BACKEND: &str = "KILN_BACKEND";

/// The declaration, as `conformance.toml` writes it.
///
/// Only `backends` is judged; a project may keep its own notes next to it, and denying unknown fields
/// would fail a step for a comment somebody put in a table.
#[derive(Debug, Deserialize)]
struct Declared {
    /// Backend names, in the order the project lists them.
    backends: Vec<String>,
}

/// What one backend's run of the spec showed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attempt {
    /// Backend the run was made with.
    backend: String,
    /// `None` when the spec passed on it, the one-line reason when it did not.
    failure: Option<String>,
}

impl Attempt {
    /// `memory ✓` or `postgres ✗ (error: no such table)`.
    fn line(&self) -> String {
        match &self.failure {
            None => format!("{} ✓", self.backend),
            Some(reason) => format!("{} ✗ ({reason})", self.backend),
        }
    }
}

/// Run the step's spec once per declared backend.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    let required = spec.numeric_param()? as usize;
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let path = project.join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(GateOutcome::failed(format!(
            "{} does not exist — this step's deliverable is that one suite runs against several \
             implementations, and {FILE} is where the project says which: write `backends = \
             [\"memory\", \"postgres\"]`",
            ctx.project_dir.join(FILE).display()
        )));
    };
    let declared: Declared = match toml::from_str(&text) {
        Ok(declared) => declared,
        Err(source) => {
            return Ok(GateOutcome::failed(format!(
                "{FILE} does not read as a backend list ({source}) — the file this step delivers \
                 looks like `backends = [\"memory\", \"postgres\"]`"
            )));
        }
    };
    if let Some(blank) = declared
        .backends
        .iter()
        .find(|backend| backend.trim().is_empty())
    {
        return Ok(GateOutcome::failed(format!(
            "{FILE} lists an empty backend name ({blank:?}) — a backend is a word the spec's store \
             factory can read out of `{BACKEND}`"
        )));
    }

    let backends = distinct(&declared.backends);
    if backends.len() < required {
        return Ok(GateOutcome::failed(format!(
            "{FILE} declares {} backend(s) ({}) and this step requires at least {required} — the \
             point of the step is that the same suite passes on more than one implementation",
            backends.len(),
            backends.join(", ")
        )));
    }

    let Some(run) = ctx.step.run.as_ref() else {
        return Ok(GateOutcome::blocked(format!(
            "{} declares no `run.cmd`, so there is no suite to run against {} backend(s)",
            ctx.step.qualified_id(),
            backends.len()
        )));
    };

    let mut attempts = Vec::new();
    for backend in &backends {
        let environment = [(BACKEND.to_owned(), backend.clone())];
        let ran = process::run_line_with_env(&run.cmd, project, ctx.timeout, &environment)?;
        attempts.push(attempt(backend, &ran));
    }
    Ok(grade(&attempts))
}

/// Grade one run of the spec.
fn attempt(backend: &str, ran: &Ran) -> Attempt {
    Attempt {
        backend: backend.to_owned(),
        failure: (!ran.success()).then(|| harness::failure_reason(ran)),
    }
}

/// The gate's answer from what the runs showed.
fn grade(attempts: &[Attempt]) -> GateOutcome {
    let verdicts = attempts
        .iter()
        .map(Attempt::line)
        .collect::<Vec<String>>()
        .join(", ");
    let failed = attempts
        .iter()
        .filter(|attempt| attempt.failure.is_some())
        .count();
    if failed == 0 {
        return GateOutcome::passed(format!(
            "{} backend(s) pass the same suite: {verdicts}",
            attempts.len()
        ));
    }
    GateOutcome::failed(format!(
        "the suite fails on {failed} of {} backend(s): {verdicts}",
        attempts.len()
    ))
}

/// The backends of a declaration, in order and without repeats.
///
/// A name written twice is one implementation, not two, and counting it twice would let a step pass
/// the `@N` part of this gate without a second store existing.
fn distinct(backends: &[String]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for backend in backends {
        let name = backend.trim();
        if unique.iter().any(|seen| seen == name) {
            continue;
        }
        unique.push(name.to_owned());
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::Outcome;
    use std::time::Duration;

    fn ran(code: Option<i32>, stderr: &str, timed_out: bool) -> Ran {
        Ran {
            code,
            stdout: String::new(),
            stderr: stderr.to_owned(),
            timed_out,
            elapsed: Duration::from_secs(1),
        }
    }

    #[test]
    fn the_declaration_is_read_the_way_the_handoff_writes_it() {
        let declared: Declared = toml::from_str(
            "# One suite, many implementations.\nbackends = [\"memory\", \"postgres\"]\n",
        )
        .expect("the fixture parses");
        assert_eq!(declared.backends, ["memory", "postgres"]);

        let notes: Declared =
            toml::from_str("backends = [\"memory\"]\nnote = \"slow on CI\"\n").expect("parses");
        assert_eq!(notes.backends, ["memory"]);

        assert!(toml::from_str::<Declared>("backend = [\"memory\"]\n").is_err());
    }

    #[test]
    fn a_backend_named_twice_is_one_backend() {
        let backends = distinct(&[
            "memory".to_owned(),
            " memory ".to_owned(),
            "postgres".to_owned(),
        ]);
        assert_eq!(backends, ["memory", "postgres"]);
    }

    #[test]
    fn every_backend_that_passes_is_named_in_the_report() {
        let passed = grade(&[attempt("memory", &ran(Some(0), "", false))]);
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(passed.detail.contains("memory ✓"), "{}", passed.detail);
    }

    #[test]
    fn one_backend_that_fails_names_itself_and_its_reason() {
        let failed = grade(&[
            attempt("memory", &ran(Some(0), "", false)),
            attempt(
                "postgres",
                &ran(Some(1), "error: no such table: jobs", false),
            ),
        ]);
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.detail.contains("memory ✓"), "{}", failed.detail);
        assert!(failed.detail.contains("postgres ✗"), "{}", failed.detail);
        assert!(
            failed.detail.contains("error: no such table: jobs"),
            "{}",
            failed.detail
        );
        assert!(failed.detail.contains("1 of 2"), "{}", failed.detail);
    }

    #[test]
    fn a_backend_that_never_finishes_is_a_failure_of_that_backend() {
        let stuck = attempt("postgres", &ran(None, "", true));
        assert!(stuck.failure.is_some(), "{stuck:?}");
        assert!(stuck.line().contains("timed out"), "{}", stuck.line());
    }
}
