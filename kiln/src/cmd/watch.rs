//! `kiln watch [step]` — judge a step, then judge it again whenever the project changes.
//!
//! The loop a learner actually works in is "change something, run the spec, read the failure,
//! repeat". `kiln run` is one turn of it, and typing it is the part that adds nothing: this command
//! is that command in a loop, with the waiting done by watching the project rather than by asking.
//!
//! Three decisions, and each of them is a way to make a watcher useless:
//!
//! * **what is watched is the *content* of the project** (the fingerprint `kiln` already uses to
//!   decide whether a gate has to run again), not modification times: a spec sync that rewrites a
//!   file with the same bytes has changed nothing, and a save that changes nothing must not trigger
//!   a run — a watcher that runs on its own writes is a watcher that never stops;
//! * **`target/` is not part of it**, for the same reason: the compiler writing its own output is
//!   not the learner writing code, and a watcher that reacted to it would rerun on every build
//!   forever;
//! * **the first run happens immediately**, before waiting for anything: the most useful moment to
//!   know where you stand is when you sit down, and a watcher that stays quiet until you touch a
//!   file is a watcher that looks broken.
//!
//! It never exits on its own: an interrupted process (Ctrl-C) is how a watch ends, and the exit code
//! is whatever the last run answered — which matters when somebody pipes it into something that
//! cares.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::cmd::{FilePrint, Session, project_files, run_step_once};
use crate::error::{KilnError, Result};

/// How often the project is looked at.
///
/// A tenth of a second: fast enough that a save feels like it started the run, slow enough that the
/// fingerprint (reading every file of a small project) is not the loop's own bottleneck.
const POLL: Duration = Duration::from_millis(100);

/// How many changed paths one turn names before it says "and N more".
const NAMED_CHANGES: usize = 3;

/// Judge one step, and keep judging it while the project changes.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    let track = session.course().track(&step.track)?;
    let project_dir = track.project_dir(session.course().root());
    let run = step.run.as_ref().ok_or_else(|| {
        KilnError::Invalid(format!(
            "{} declares no `run.cmd`, so there is nothing to watch",
            step.qualified_id()
        ))
    })?;

    println!(
        "watching {} — `{}` runs after every change to {}",
        step.qualified_id(),
        run.cmd,
        track.project
    );
    println!(
        "  the project's *contents* are watched, `target/` never is: a save that changes no bytes \
         is not a change\n  Ctrl-C stops the watch"
    );
    println!();
    super::run::print_step_header(session, &step)?;

    let started = Instant::now();
    let mut known = project_files(&project_dir);
    loop {
        println!();
        println!("[{}] running the spec tests", clock(started));
        // The code is deliberately dropped: a watch does not end with an answer, it ends when
        // somebody stops it, and the run before that said everything there was to say.
        let _code = run_step_once(session, &step, false)?;
        let changed = wait_for_change(&project_dir, &known, POLL);
        let (paths, now) = changed;
        known = now;
        println!();
        println!(
            "[{}] {} changed — running again",
            clock(started),
            describe(&paths)
        );
    }
}

/// Minutes and seconds since the watch started.
///
/// Phase rather than a wall clock on purpose: the standard library cannot format local time, and a
/// third-party clock dependency for a progress line is not worth it. Read it as "this line is N into
/// the session" — which is what tells a learner returning to the tab whether the last answer is
/// seconds or an hour old.
fn clock(started: Instant) -> String {
    let seconds = started.elapsed().as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// `src/lib.rs`, `src/lib.rs and 2 more`, or `nothing` (a change that only removed files).
fn describe(paths: &[String]) -> String {
    match paths {
        [] => "the project".to_owned(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [first, rest @ ..] => {
            let hidden = rest.len().saturating_sub(NAMED_CHANGES - 1);
            if hidden == 0 {
                format!("{first} and {}", rest.join(" and "))
            } else {
                format!("{first} and {hidden} more")
            }
        }
    }
}

/// Wait until the project stops looking like `known`, and answer what changed and what it looks
/// like then.
///
/// Public inside the crate so a test can watch a directory it changes itself: the loop above cannot
/// be tested in one piece (it never ends), and this is the piece of it that has a right answer.
pub(crate) fn wait_for_change(
    dir: &Path,
    known: &[FilePrint],
    poll: Duration,
) -> (Vec<String>, Vec<FilePrint>) {
    loop {
        std::thread::sleep(poll);
        let now = project_files(dir);
        if now != known {
            return (changed_paths(known, &now), now);
        }
    }
}

/// Paths that differ between two listings: added, changed, or gone.
fn changed_paths(before: &[FilePrint], after: &[FilePrint]) -> Vec<String> {
    let mut changed: Vec<String> = Vec::new();
    for (path, hash) in after {
        let same = before
            .iter()
            .any(|(known, known_hash)| known == path && known_hash == hash);
        if !same {
            changed.push(path.clone());
        }
    }
    for (path, _) in before {
        if !after.iter().any(|(known, _)| known == path) {
            changed.push(format!("{path} (removed)"));
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::{changed_paths, wait_for_change};
    use std::time::Duration;

    #[test]
    fn a_watcher_answers_when_the_project_changes() {
        let dir = std::env::temp_dir().join(format!("kiln-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("a scratch directory");
        std::fs::write(dir.join("src/lib.rs"), "//! before\n").expect("write");

        let before = crate::cmd::project_files(&dir);
        let writer = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::write(dir.join("src/lib.rs"), "//! after\n").expect("write");
            })
        };

        let (changed, after) = wait_for_change(&dir, &before, Duration::from_millis(10));
        assert_eq!(
            changed,
            vec!["src/lib.rs".to_owned()],
            "the watcher names the file that changed, not just that something did"
        );
        assert_ne!(before, after, "the change is what the watcher waited for");
        writer.join().expect("the writer finishes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_is_named_by_path_including_one_that_removed_a_file() {
        let before = vec![
            ("src/lib.rs".to_owned(), "aaaa".to_owned()),
            ("src/gone.rs".to_owned(), "bbbb".to_owned()),
        ];
        let after = vec![
            ("src/lib.rs".to_owned(), "cccc".to_owned()),
            ("src/new.rs".to_owned(), "dddd".to_owned()),
        ];
        assert_eq!(
            changed_paths(&before, &after),
            vec![
                "src/lib.rs".to_owned(),
                "src/new.rs".to_owned(),
                "src/gone.rs (removed)".to_owned()
            ]
        );
        assert!(changed_paths(&after, &after).is_empty());
    }
}
