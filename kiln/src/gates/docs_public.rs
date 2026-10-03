//! `docs:public` — public items carry documentation, and fallible ones say so (plan §6).
//!
//! Two lints, one idea: the part of a crate other people can call is part of its API, and an API
//! nobody can read is not finished. `missing_docs` asks for a sentence; `missing_errors_doc` asks a
//! fallible function to list what it returns on the error path, which is the half that a docblock
//! usually skips.
//!
//! Which targets it judges is the rung distinction of authoring.md §7.4: **library and binary code
//! is judged, test code is not**. A test that returns `Result` for `?` is not an API, and demanding
//! an `# Errors` section there would teach the wrong reflex. The gate denies both lints on the
//! command line instead of trusting the workspace's own lint table: a project that turned
//! `missing_docs` off would otherwise pass the gate by not being checked.

use crate::error::Result;
use crate::gates::runner::{GateContext, clip};
use crate::gates::tool::{self, Tool};
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// The lints this gate denies, as clippy spells them.
pub const LINTS: &[&str] = &["missing_docs", "clippy::missing_errors_doc"];

/// Run clippy over the library and binary targets with the documentation lints denied.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let probe = tool::probe(Tool::CargoClippy, ctx.probe_timeout());
    tool::require(&probe)?;

    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let mut args = vec!["clippy", "--lib", "--bins", "--"];
    for lint in LINTS {
        args.push("-D");
        args.push(lint);
    }

    let ran = process::run("cargo", &args, project, ctx.timeout)?;
    if ran.success() {
        return Ok(GateOutcome::passed(format!(
            "every public item is documented ({} on `--lib --bins`)",
            LINTS.join(", ")
        )));
    }
    Ok(GateOutcome::failed(format!(
        "the public API is not documented ({}):\n    {}",
        LINTS.join(", "),
        clip(&ran.stderr, 12)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_asks_for_prose_and_for_the_error_path() {
        assert!(LINTS.contains(&"missing_docs"));
        assert!(LINTS.contains(&"clippy::missing_errors_doc"));
    }
}
