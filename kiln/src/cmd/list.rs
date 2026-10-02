//! `kiln list` (and the bare `kiln` welcome).
//!
//! The listing is the learner's map: progress per step, what each step establishes, and the exit
//! capabilities of the track. Anything that has no content yet says so here rather than hiding.

use crate::cmd::Session;
use crate::error::{EXIT_OK, Result};
use crate::info::{Step, Track};
use crate::state::{State, StepStatus};

/// Print every authored track, or one of them.
pub fn run(session: &Session, track: Option<&str>) -> Result<u8> {
    let tracks: Vec<&Track> = match track {
        Some(id) => vec![session.course().track(id)?],
        None => session.course().tracks().iter().collect(),
    };

    let mut done = 0;
    let mut total = 0;
    for (index, track) in tracks.iter().enumerate() {
        if index > 0 {
            println!();
        }
        let (track_done, track_total) = print_track(session, track);
        done += track_done;
        total += track_total;
    }

    println!();
    println!("{done}/{total} steps done · `kiln run` judges a step, `kiln check` runs its gates");
    if let Some(note) = session.locale_note()? {
        println!("{note}");
    }
    Ok(EXIT_OK)
}

/// Bare `kiln`: greet, then point at the step to continue at.
pub fn welcome(session: &Session) -> Result<u8> {
    let manifest = session.course().manifest();
    println!("{}", session.ui().welcome(manifest).trim());
    println!();

    let track = session.course().default_track()?;
    match first_unfinished(track, session.state()) {
        Some(step) => {
            println!("next step: {} — {}", step.qualified_id(), step.title);
            println!(
                "  `kiln run {}` to judge it, `kiln hint {}` for a nudge",
                step.qualified_id(),
                step.qualified_id()
            );
        }
        None => println!("{}", session.ui().final_message(manifest).trim()),
    }
    if let Some(note) = session.locale_note()? {
        println!();
        println!("{note}");
    }
    Ok(EXIT_OK)
}

/// First step of a track that is not done yet.
pub fn first_unfinished<'a>(track: &'a Track, state: &State) -> Option<&'a Step> {
    track
        .step
        .iter()
        .find(|step| state.status(&step.qualified_id()) != StepStatus::Done)
}

/// Print one track and return `(steps done, steps total)`.
fn print_track(session: &Session, track: &Track) -> (usize, usize) {
    let done = track
        .step
        .iter()
        .filter(|step| session.state().status(&step.qualified_id()) == StepStatus::Done)
        .count();
    println!(
        "{} {} — {} ({done}/{} steps done)",
        track.label(),
        track.id,
        track.title,
        track.step.len()
    );
    println!("   project: {}", track.project);

    for step in &track.step {
        let id = step.qualified_id();
        let status = session.state().status(&id);
        println!(
            "  {} {:<20} {:<9} {:>4}  {}{}",
            status.marker(),
            step.id,
            step.kind,
            step.time_hint,
            step.title,
            honesty_marks(session, &id)
        );
        println!(
            "        teaches {}",
            step.teaches
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if !track.exit.is_empty() {
        println!(
            "   exit capabilities: {}",
            track
                .exit
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    (done, track.step.len())
}

/// " [solution applied, spec modified]" for a step whose result is not the learner's own.
fn honesty_marks(session: &Session, step_id: &str) -> String {
    let Some(recorded) = session.state().steps.get(step_id) else {
        return String::new();
    };
    let mut marks = Vec::new();
    if recorded.solution_applied {
        marks.push("solution applied");
    }
    if recorded.spec_modified {
        marks.push("spec modified");
    }
    if marks.is_empty() {
        String::new()
    } else {
        format!("  [{}]", marks.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::test_support::{TempDir, ledger_toml, load_course, step_toml, track_toml};

    fn course(temp: &TempDir) -> crate::info::Course {
        load_course(
            temp.path(),
            &track_toml(
                &["repo/toolchain-pinning@apply"],
                &[
                    step_toml("1.1-workspace", &["repo/workspace-layout@apply"]),
                    step_toml("1.2-toolchain", &["repo/toolchain-pinning@apply"]),
                ],
            ),
            &ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("repo/toolchain-pinning", "apply", "mailer/1.2"),
            ]),
        )
    }

    #[test]
    fn the_first_unfinished_step_is_the_one_to_continue_at() {
        let temp = TempDir::new("cmd-list-next");
        let course = course(&temp);
        let track = course.default_track().unwrap();
        let mut state = State::default();

        assert_eq!(
            first_unfinished(track, &state).unwrap().qualified_id(),
            "mailer/1.1-workspace"
        );

        state.record_spec("mailer/1.1-workspace", true);
        state.record_gates("mailer/1.1-workspace", true);
        assert_eq!(
            first_unfinished(track, &state).unwrap().qualified_id(),
            "mailer/1.2-toolchain"
        );

        state.record_spec("mailer/1.2-toolchain", true);
        state.record_gates("mailer/1.2-toolchain", true);
        assert!(
            first_unfinished(track, &state).is_none(),
            "a finished track has nothing to continue at"
        );
    }
}
