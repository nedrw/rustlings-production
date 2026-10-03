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
use std::time::Duration;

use crate::cmd::{Session, run_step_once, tree_fingerprint};
use crate::error::{KilnError, Result};

/// How often the project is looked at.
///
/// A tenth of a second: fast enough that a save feels like it started the run, slow enough that the
/// fingerprint (reading every file of a small project) is not the loop's own bottleneck.
const POLL: Duration = Duration::from_millis(100);

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
        "watching {} — `{}` runs after every change to {} (Ctrl-C to stop)",
        step.qualified_id(),
        run.cmd,
        track.project
    );
    let mut known = tree_fingerprint(&project_dir);
    loop {
        println!();
        // The code is deliberately dropped: a watch does not end with an answer, it ends when
        // somebody stops it, and the run before that said everything there was to say.
        let _code = run_step_once(session, &step)?;
        known = wait_for_change(&project_dir, &known, POLL);
        println!();
        println!("— the project changed; running it again —");
    }
}

/// Wait until the project stops looking like `known`, and answer what it looks like then.
///
/// Public inside the crate so a test can watch a directory it changes itself: the loop above cannot
/// be tested in one piece (it never ends), and this is the piece of it that has a right answer.
pub(crate) fn wait_for_change(dir: &Path, known: &str, poll: Duration) -> String {
    loop {
        std::thread::sleep(poll);
        let now = tree_fingerprint(dir);
        if now != known {
            return now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wait_for_change;
    use std::time::Duration;

    #[test]
    fn a_watcher_answers_when_the_project_changes() {
        let dir = std::env::temp_dir().join(format!("kiln-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("a scratch directory");
        std::fs::write(dir.join("src/lib.rs"), "//! before\n").expect("write");

        let before = crate::cmd::tree_fingerprint(&dir);
        let writer = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::write(dir.join("src/lib.rs"), "//! after\n").expect("write");
            })
        };

        let after = wait_for_change(&dir, &before, Duration::from_millis(10));
        assert_ne!(before, after, "the change is what the watcher waited for");
        writer.join().expect("the writer finishes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
