//! `fmt` — formatting is uniform.
//!
//! Deliberately the dumbest gate: the repository owns the settings (`rustfmt.toml`), the gate owns
//! the check, and nobody spends review time on whitespace.

use crate::error::Result;
use crate::gates::runner::{GateContext, output_detail};
use crate::gates::tool::{self, Tool};
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// Run `cargo fmt --all --check` in the learner's project.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let probe = tool::probe(Tool::CargoFmt, ctx.probe_timeout());
    tool::require(&probe)?;

    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let ran = process::run("cargo", &["fmt", "--all", "--check"], project, ctx.timeout)?;
    if ran.success() {
        return Ok(GateOutcome::passed("`cargo fmt --all --check` is clean"));
    }
    Ok(GateOutcome::failed(format!(
        "`cargo fmt --all --check` is not clean — run `cargo fmt --all`:\n    {}",
        output_detail(&ran)
    )))
}
