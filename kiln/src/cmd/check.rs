//! `kiln check [step]` — run a step's production gates, without its spec tests.
//!
//! `run` answers "does the step work?", `check` answers "would this pass review?" (plan §3 D3).
//! Gates that are catalogued but not implemented, or that a later milestone brings, are reported
//! as pending — never as passes — and only `must` gates can stop the step.

use crate::cmd::Session;
use crate::error::{EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, Result};
use crate::gates::{GateContext, Outcome, all_must_passed, run_all};

/// Run the gates of one step.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    super::run::print_step_header(session, &step)?;

    let track = session.course().track(&step.track)?;
    let project = track.project_dir(session.course().root());
    if !project.is_dir() {
        println!();
        println!(
            "  the learner project {} does not exist yet — the step's project content (skeleton \
             and scaffold) is not authored yet",
            track.project
        );
        println!();
        println!("nothing to judge: no gate can run without a project.");
        return Ok(EXIT_NOT_AUTHORED);
    }

    let context = GateContext {
        course: session.course(),
        track,
        step: &step,
        project_dir: project,
        timeout: session.timeout(),
    };

    println!();
    let results = run_all(&context, &step.gates, false);
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

    session
        .state_mut()
        .record_gates(&step.qualified_id(), passed);
    session.save_state()?;

    println!();
    println!(
        "{must_passed}/{must_declared} must gates passed · {should} should gate(s) · {pending} not \
         implemented yet"
    );
    if passed {
        println!("✓ every must gate passed for {}", step.qualified_id());
        Ok(EXIT_OK)
    } else {
        println!(
            "✗ {blocking} must gate(s) did not pass for {}",
            step.qualified_id()
        );
        Ok(EXIT_FAILED)
    }
}
