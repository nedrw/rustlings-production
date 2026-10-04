//! `kiln list` (and the bare `kiln` welcome).
//!
//! The listing is the learner's map: progress per step, what each step establishes, and the exit
//! capabilities of the track. Anything that has no content yet says so here rather than hiding.

use crate::cmd::Session;
use crate::error::{EXIT_OK, Result};
use crate::info::{Step, Track};
use crate::state::{State, StepStatus};

/// What one listing line may occupy.
///
/// The listing is read in a terminal, so a row that runs past this wraps and the columns after the
/// id are lost. 100 is the width the repository writes prose at (rustfmt.toml, the step bodies).
const ROW_WIDTH: usize = 100;

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
    println!("markers: ✓ step done · ◐ spec tests pass, gates still to run · · not started");
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
            let doc = crate::i18n::resolve_step_doc(
                &session.course().dir(),
                &step.entry,
                session.locale(),
                session.source(),
            )?;
            let body = doc
                .path()
                .strip_prefix(session.course().root())
                .unwrap_or_else(|_| doc.path())
                .display()
                .to_string();
            println!("next step: {} — {}", step.qualified_id(), step.title);
            println!("  read     {body}");
            println!("  project  {}", track.project);
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
        // The id column fits the longest id of the track: a hard-coded width breaks the alignment
        // on the first longer id, which is exactly where a reader is looking for the title. The
        // kind and the time hint move to the second line, so the title keeps its room: it is what
        // the reader is scanning for.
        let id_width = track
            .step
            .iter()
            .map(|step| step.id.chars().count())
            .max()
            .unwrap_or(20);
        println!(
            "  {} {:<id_width$}  {}",
            status.marker(),
            step.id,
            clip(&step.title, ROW_WIDTH.saturating_sub(id_width + 6).max(24))
        );
        let teaches = step
            .teaches
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "        {} · {} · teaches {teaches}",
            step.kind, step.time_hint
        );
        // The honesty marks get their own line: they are the record that this result is not the
        // learner's own work (plan §3 D2), and a footnote is easier to see than a bracketed tail.
        let marks = honesty_marks(session, &id);
        if !marks.is_empty() {
            println!("        {marks}");
        }
    }

    if !track.exit.is_empty() {
        let items: Vec<String> = track.exit.iter().map(ToString::to_string).collect();
        for line in wrap_items("   exit capabilities: ", "      ", &items) {
            println!("{line}");
        }
    }
    (done, track.step.len())
}

/// `prefix` + a comma-separated list, wrapped at [`ROW_WIDTH`] with `continuation` as the indent.
fn wrap_items(prefix: &str, continuation: &str, items: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = prefix.to_owned();
    for item in items {
        let separator = if current.trim_end() == prefix.trim_end() {
            ""
        } else {
            ", "
        };
        let candidate = format!("{current}{separator}{item}");
        if candidate.chars().count() > ROW_WIDTH && !current.trim_end().ends_with(':') {
            lines.push(current.trim_end().to_owned());
            current = format!("{continuation}{item}");
        } else {
            current = candidate;
        }
    }
    lines.push(current.trim_end().to_owned());
    lines
}

/// A title cut to `width` characters, with an ellipsis when it had to lose something.
fn clip(title: &str, width: usize) -> String {
    let characters: Vec<char> = title.chars().collect();
    if characters.len() <= width {
        return title.to_owned();
    }
    let mut short: String = characters
        .into_iter()
        .take(width.saturating_sub(1))
        .collect();
    short.push('…');
    short
}

/// "[solution applied, spec modified]" for a step whose result is not the learner's own.
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
        format!("[{}]", marks.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::test_support::{TempDir, ledger_toml, load_course, one_track, step_toml};

    fn course(temp: &TempDir) -> crate::info::Course {
        load_course(
            temp.path(),
            &one_track(
                &[
                    step_toml("mailer", "1.1-workspace", &["repo/workspace-layout@apply"]),
                    step_toml("mailer", "1.2-toolchain", &["repo/toolchain-pinning@apply"]),
                ],
                &["repo/toolchain-pinning@apply"],
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
