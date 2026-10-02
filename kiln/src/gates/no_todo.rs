//! `no-todo` — no `todo!`/`unimplemented!`/`dbg!` left in the step's code (plan §6).
//!
//! Implemented with clippy's restriction lints rather than a grep, so the check understands code
//! (a `todo!` inside a string literal or a comment is not a leftover, and a macro that expands to
//! one is). It runs over every target including tests: a `todo!()` in a test is a step that is not
//! finished either.

use crate::error::Result;
use crate::gates::runner::{GateContext, clip};
use crate::gates::tool::{self, Tool};
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// Lints that flag work left for later.
pub const LINTS: &[&str] = &["clippy::todo", "clippy::unimplemented", "clippy::dbg_macro"];

/// Run clippy with the "left for later" lints denied.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let probe = tool::probe(Tool::CargoClippy, ctx.probe_timeout());
    tool::require(&probe)?;

    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let mut args = vec!["clippy", "--all-targets", "--"];
    for lint in LINTS {
        args.push("-D");
        args.push(lint);
    }

    let ran = process::run("cargo", &args, project, ctx.timeout)?;
    if ran.success() {
        return Ok(GateOutcome::passed(format!(
            "no {} anywhere in the step's code",
            LINTS.join(", ")
        )));
    }
    Ok(GateOutcome::failed(format!(
        "unfinished work found ({}):\n    {}",
        LINTS
            .iter()
            .map(|lint| lint.trim_start_matches("clippy::"))
            .collect::<Vec<_>>()
            .join(", "),
        clip(&ran.stderr, 12)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lint_targets_a_placeholder_left_behind() {
        assert_eq!(LINTS.len(), 3);
        assert!(LINTS.contains(&"clippy::todo"));
        assert!(LINTS.contains(&"clippy::unimplemented"));
        assert!(
            !LINTS.contains(&"clippy::panic"),
            "startup panics are a separate policy"
        );
    }
}
