//! `kiln reset [step]` — take a step's scaffold back out.
//!
//! Reset is not "delete my work": it removes what the *course* put there — an inserted line, a file
//! nobody touched — and keeps everything the learner wrote, saying which files it kept and why. The
//! usual sequence is `kiln reset <step>` and then `kiln run <step>`, which puts the scaffold back.
//!
//! The step's recorded *progress* is forgotten with it, because the learner is about to earn it
//! again; the honesty markers stay, because those are the record of what actually happened (plan
//! §3 D2).

use crate::cmd::Session;
use crate::error::{EXIT_NOT_AUTHORED, EXIT_OK, Result};
use crate::project;

/// Revert the step's scaffold, keeping the learner's own files.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    super::run::print_step_header(session, &step)?;

    let track = session.course().track(&step.track)?;
    let project_dir = track.project_dir(session.course().root());
    if !project_dir.is_dir() {
        println!();
        println!(
            "  {} does not exist yet — there is nothing to reset",
            track.project
        );
        return Ok(EXIT_NOT_AUTHORED);
    }

    let outcome = project::reset_scaffold(session.course(), track, &step)?;
    println!();
    if outcome.changes.is_empty() && outcome.notes.is_empty() {
        println!("nothing to revert: the scaffold of this step is not in the project.");
    }
    for line in outcome.lines() {
        println!("  {line}");
    }
    for note in &outcome.notes {
        println!("  note: {note}");
    }

    session.state_mut().forget_progress(&step.qualified_id());
    session.save_state()?;

    println!();
    println!(
        "forgot what was recorded for {} — `kiln run {}` puts the scaffold back",
        step.qualified_id(),
        step.qualified_id()
    );
    let state = crate::state::State::path(session.course().root());
    let state = state
        .strip_prefix(session.course().root())
        .unwrap_or(&state);
    println!(
        "(the honesty markers in {} are kept: they are the record of what happened)",
        state.display()
    );
    Ok(EXIT_OK)
}
