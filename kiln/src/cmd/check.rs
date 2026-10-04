//! `kiln check [step]` — run a step's production gates, without its spec tests.
//!
//! `run` answers "does the step work?", `check` answers "would this pass review?" (plan §3 D3).
//! Gates that are catalogued but not implemented, or that a later milestone brings, are reported
//! as pending — never as passes — and only `must` gates can stop the step.

use crate::cmd::Session;
use crate::error::{EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, Result};
use crate::gates::{Outcome, all_must_passed};
use crate::project;
use crate::spec;

/// Run the gates of one step.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    super::run::print_step_header(session, &step)?;

    let track = session.course().track(&step.track)?;
    let project_dir = track.project_dir(session.course().root());
    let missing = project::missing_content(session.course(), track, &step);
    if !missing.is_empty() {
        println!();
        for line in &missing {
            println!("  {line}");
        }
        println!();
        println!("nothing to judge: no gate can run without the step's content.");
        return Ok(EXIT_NOT_AUTHORED);
    }

    println!();
    super::run::print_preparation(&project::prepare(session.course(), track, &step)?);

    println!();
    let results = session.gates(&step, false)?;
    for result in &results {
        println!("{}", result.line());
    }

    let passed = all_must_passed(&results);
    let must_declared = step.must_gates().count();
    let must_passed = results
        .iter()
        .filter(|gate| {
            gate.spec.tier == crate::gates::Tier::Must && gate.outcome == Outcome::Passed
        })
        .count();
    let blocking = results.iter().filter(|gate| gate.breaks_the_step()).count();
    let should = results
        .iter()
        .filter(|gate| gate.spec.tier == crate::gates::Tier::Should)
        .count();
    let pending = results
        .iter()
        .filter(|gate| gate.outcome == Outcome::Pending)
        .count();

    // `check` looks at the spec files too (the `spec-integrity` gate), so the honesty marker is
    // refreshed here as well: it is a fact about this tree, not a historical note (plan §3 D2).
    let drift = spec::check_learner(session.course().root(), track, &step, &project_dir)?;
    session
        .state_mut()
        .record_spec_modified(&step.qualified_id(), !drift.is_clean());
    session
        .state_mut()
        .record_gates(&step.qualified_id(), passed);
    session.save_state()?;

    println!();
    println!(
        "{must_passed}/{must_declared} must gates passed · {should} should gates run · {pending} \
         catalogue gate(s) not implemented yet"
    );
    if passed {
        println!(
            "✓ every must gate passed for {} — `kiln list` shows what is next",
            step.qualified_id()
        );
        Ok(EXIT_OK)
    } else {
        println!(
            "✗ {blocking} must gate(s) did not pass for {} — `kiln hint {}` points at what to fix",
            step.qualified_id(),
            step.qualified_id()
        );
        Ok(EXIT_FAILED)
    }
}
