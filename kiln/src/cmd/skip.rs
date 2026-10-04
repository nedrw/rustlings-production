//! `kiln skip <step>` / `kiln skip --all` — act on a `plan --fast` decision.
//!
//! Skipping is the one place where the course writes code the learner did not, so it is the one
//! place that records *why*: a step is skippable only when every slot it teaches has probe evidence
//! (plan §3 D4 — a probe is the learner's own proof, taken months later, from an empty file).
//! `--force` skips without that proof and says so in the report; the state then shows the step as
//! skipped, never as done (`kiln list` prints `»`).
//!
//! The reference solution is applied on the way past. That is not a shortcut for the learner — it is
//! what keeps the project compiling for the steps after it, exactly as a scaffold does. The
//! application is recorded (`solution_applied`), so nothing downstream can mistake it for work.

use crate::cmd::{Session, plan};
use crate::error::{EXIT_FAILED, EXIT_OK, KilnError, Result};
use crate::info::Step;
use crate::project;

/// Skip one step, or every step `plan --fast` would skip.
pub fn run(session: &mut Session, reference: Option<&str>, all: bool, force: bool) -> Result<u8> {
    if all {
        if reference.is_some() {
            return Err(KilnError::Invalid(
                "`kiln skip --all` walks a whole track: it takes no step id".to_owned(),
            ));
        }
        let track = session.course().default_track()?.clone();
        let steps: Vec<Step> = plan::skippable(session.state(), &track)
            .into_iter()
            .cloned()
            .collect();
        if steps.is_empty() {
            println!(
                "nothing to skip in {}: `kiln plan --fast` explains which steps a passed probe \
                 would unlock",
                track.id
            );
            return Ok(EXIT_OK);
        }
        let mut skipped = 0usize;
        for step in &steps {
            if skip_one(session, step, force)? {
                skipped += 1;
            }
        }
        println!();
        println!(
            "skipped {skipped} step(s) — `kiln list` shows them as `»`, and `kiln reset <step>` \
             puts any of them back"
        );
        return Ok(EXIT_OK);
    }

    let step = super::resolve_step(session, reference)?;
    if skip_one(session, &step, force)? {
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_FAILED)
    }
}

/// Skip one step. Returns `true` when it was skipped, `false` when it could not be.
fn skip_one(session: &mut Session, step: &Step, force: bool) -> Result<bool> {
    let id = step.qualified_id();
    if let Some(status) = plan::behind_us(session.state(), step) {
        println!("  {id}: {}", status.label());
        return Ok(status == crate::state::StepStatus::Skipped);
    }

    let reason = match plan::disposition(session.state(), step) {
        plan::Disposition::Skip(reason) => reason,
        plan::Disposition::Keep(blocker) => {
            if !force {
                println!("✗ {id} is not skippable — {}", blocker.detail);
                println!(
                    "  `kiln probe <capability>` is how you prove it; `kiln skip {id} --force` skips \
                     anyway and records that you did"
                );
                return Ok(false);
            }
            println!("  {id}: forced past — {}", blocker.detail);
            plan::SkipReason { proofs: Vec::new() }
        }
    };

    let track = session.course().track(&step.track)?.clone();
    let missing = project::missing_content(session.course(), &track, step);
    if !missing.is_empty() {
        for line in &missing {
            println!("  {line}");
        }
        println!("  {id}: skipped in the plan, but it has no content to apply");
        return Ok(false);
    }

    println!("  {id}: skip — {}", reason.summary());
    project::prepare(session.course(), &track, step)?;
    match project::apply_solution(session.course(), &track, step) {
        Ok(outcome) => super::run::print_preparation(&outcome),
        Err(error) => {
            println!("  ✗ {id}: the reference solution did not apply: {error}");
            return Ok(false);
        }
    }
    session.state_mut().record_solution_applied(&id, true);
    session.state_mut().record_skipped(&id, true);
    session.save_state()?;
    Ok(true)
}
