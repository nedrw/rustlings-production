//! `spec-integrity` — the spec files are the ones the course shipped (plan §3 D2).
//!
//! The gate checks both directions, because either side can be edited:
//!
//! * the authoritative copy under `course/tracks/<track>/spec/` against `spec/manifest.toml`;
//! * the copy inside the learner project against the authoritative one.
//!
//! A failing gate always names the file and the command that puts it right, because "your test file
//! is not the course's" is only useful with "here is how to get it back".

use crate::error::Result;
use crate::gates::runner::GateContext;
use crate::gates::{GateOutcome, GateSpec};
use crate::spec;

/// Compare both copies of every spec file against the hashes the course recorded.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    if ctx.existing_project().is_none() {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    }

    let course = spec::check_course(ctx.course.root(), ctx.track)?;
    let learner = spec::check_learner(ctx.course.root(), ctx.track, ctx.step, &ctx.project_dir)?;
    let checked = course.checked + learner.checked;
    let problems: Vec<String> = course.lines().into_iter().chain(learner.lines()).collect();

    if problems.is_empty() {
        return Ok(GateOutcome::passed(format!(
            "{checked} spec file(s) match `spec/manifest.toml` and the project copies"
        )));
    }
    Ok(GateOutcome::failed(format!(
        "the specs are not the ones the course shipped:\n    {}",
        problems.join("\n    ")
    )))
}
