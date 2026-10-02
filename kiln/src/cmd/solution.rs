//! `kiln solution [step]` — apply the reference implementation, and record that you did.
//!
//! Self-study courses cannot prevent this, and pretending otherwise is dishonest; what they *can* do
//! is leave a trace (plan §3 D2). So the patch is applied on top of the scaffold state, the
//! step's honesty marker is written to `.kiln/state.toml`, and `kiln list` and `kiln verify` both
//! show it from then on.

use crate::cmd::Session;
use crate::error::{EXIT_NOT_AUTHORED, EXIT_OK, KilnError, Result};
use crate::project;

/// Apply the step's reference solution.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    super::run::print_step_header(session, &step)?;

    if step.solution.is_none() {
        return Err(KilnError::NotAuthored {
            what: format!("the reference solution of {}", step.qualified_id()),
        });
    }

    let track = session.course().track(&step.track)?;
    let project_dir = track.project_dir(session.course().root());
    if !project_dir.is_dir() {
        println!();
        println!(
            "  {} does not exist yet — apply the step's scaffold first (`kiln run {}`)",
            track.project,
            step.qualified_id()
        );
        return Ok(EXIT_NOT_AUTHORED);
    }

    // The solution is a diff against the scaffold state, so that state has to be there first.
    println!();
    super::run::print_preparation(&project::prepare(session.course(), track, &step)?);
    let outcome = project::apply_solution(session.course(), track, &step)?;

    println!();
    for line in outcome.lines() {
        println!("  {line}");
    }
    session
        .state_mut()
        .record_solution_applied(&step.qualified_id(), true);
    session.save_state()?;

    println!();
    println!(
        "applied the reference solution of {} — this is not your own work, and the record says so",
        step.qualified_id()
    );
    println!(
        "`kiln run {}` now shows what the course's implementation does",
        step.qualified_id()
    );
    Ok(EXIT_OK)
}
