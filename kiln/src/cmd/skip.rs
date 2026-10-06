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
//!
//! `--track` names the track to walk, like every other command that walks one. It is how a track that
//! is not the course's default gets built up for `kiln verify --track <track>`: the acceptance drill
//! of a track asks for its whole tail tree, and walking a track's own steps one at a time used to need
//! them spelled out in path order (handoff M2 §4.1).

use crate::cmd::{Session, list, plan};
use crate::error::{EXIT_FAILED, EXIT_OK, KilnError, Result};
use crate::info::{Step, Track};
use crate::probe::{self, Probe};
use crate::project;

/// Skip one step, or every step `plan --fast` would skip.
pub fn run(
    session: &mut Session,
    reference: Option<&str>,
    all: bool,
    force: bool,
    track: Option<&str>,
) -> Result<u8> {
    let track = named_track(session, track)?;
    if all {
        if reference.is_some() {
            return Err(KilnError::Invalid(
                "`kiln skip --all` walks a whole track: it takes no step id".to_owned(),
            ));
        }
        // One read of the catalogue for the whole pass: the verdicts and the refusal messages both
        // name an exercise only when one exists (kiln/src/cmd/plan.rs).
        let probes = probe::load(&session.course().dir())?;
        let steps: Vec<Step> = plan::skippable(session.state(), &track, &probes)
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
            if skip_one(session, step, &probes, force)? {
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

    let step = match reference {
        Some(reference) => super::resolve_step(session, Some(reference))?,
        None => list::first_unfinished(&track, session.state())
            .cloned()
            .ok_or_else(|| {
                KilnError::Invalid(format!(
                    "every step of `{}` is done — name a step explicitly, or start the next track",
                    track.id
                ))
            })?,
    };
    if step.track != track.id {
        return Err(KilnError::Invalid(format!(
            "{} is a step of `{}`, and `--track {}` was asked for — name a step of that track, or \
             drop the flag",
            step.qualified_id(),
            step.track,
            track.id
        )));
    }
    let probes = probe::load(&session.course().dir())?;
    if skip_one(session, &step, &probes, force)? {
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_FAILED)
    }
}

/// The track a skip walks: the one `--track` names, or the course's default.
fn named_track(session: &Session, track: Option<&str>) -> Result<Track> {
    match track {
        Some(id) => Ok(session.course().track(id)?.clone()),
        None => Ok(session.course().default_track()?.clone()),
    }
}

/// Skip one step. Returns `true` when it was skipped, `false` when it could not be.
fn skip_one(session: &mut Session, step: &Step, probes: &[Probe], force: bool) -> Result<bool> {
    let id = step.qualified_id();
    if let Some(status) = plan::behind_us(session.state(), step) {
        println!("  {id}: {}", status.label());
        return Ok(status == crate::state::StepStatus::Skipped);
    }

    let reason = match plan::disposition(session.state(), step, probes) {
        plan::Disposition::Skip(reason) => reason,
        plan::Disposition::Keep(blocker) => {
            if !force {
                println!("✗ {id} is not skippable — {}", blocker.detail);
                // The verdict names the route to a proof when the catalogue has one; this line
                // must stay true when it has none, so it points at nothing above it.
                println!("  `kiln skip {id} --force` skips it anyway and records that you did");
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
