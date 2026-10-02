//! `caps` — the step's capability declarations are legal (plan §3 D4).
//!
//! One gate, one rule set: `kiln lint-caps` already knows how to judge the ledger and the track
//! files (R0-R6), so this gate runs that linter and reports the step's own findings. A step whose
//! declarations break a rule cannot be finished, which is what keeps the ledger honest at the point
//! where a step is judged rather than only in CI.

use crate::caps::lint;
use crate::error::Result;
use crate::gates::runner::GateContext;
use crate::gates::{GateOutcome, GateSpec};

/// Run the capability rules and report the ones that concern this step.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let report = lint::lint(ctx.course)?;
    let step = ctx.step.qualified_id();
    let issues = report.issues_for_step(ctx.step);

    if issues.is_empty() {
        return Ok(GateOutcome::passed(format!(
            "{} capabilities · {} slots: no rule R0-R6 issue for {step}",
            report.capabilities, report.slots
        )));
    }
    Ok(GateOutcome::failed(format!(
        "the capability declarations of {step} break {} rule(s):\n    {}",
        issues.len(),
        issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n    ")
    )))
}
