//! `kiln run [step]` — judge a step's spec tests.
//!
//! The fast path of the whole course (plan §6.1): one command, one answer, under a minute of
//! wall clock. Getting there takes three steps, in this order:
//!
//! 1. the step's content has to exist at all (otherwise: exit 3, "not authored yet");
//! 2. the spec files are synced and the scaffold applied, both idempotently (authoring.md §5);
//! 3. the spec files have to *still be* the ones the course shipped — a judgement made against an
//!    edited spec would be worthless, so the run refuses instead of guessing (plan §3 D2).
//!
//! When a step has no spec or no project yet, this says so and exits 3 — "you still have work to
//! do" and "this step is not written yet" must not look the same.

use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, KilnError, Result};
use crate::i18n::{fallback_notice, resolve_step_doc};
use crate::info::Step;
use crate::process;
use crate::project;
use crate::spec;

/// Run the spec tests of one step.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    let id = step.qualified_id();
    judge(session, &id, true)
}

/// The body of `run`, shared with `watch`.
///
/// `announce` controls the header: a watcher prints it once and judges many times, and a header
/// between every turn is scrollback nobody reads.
pub(crate) fn judge(session: &mut Session, reference: &str, announce: bool) -> Result<u8> {
    let step = super::resolve_step(session, Some(reference))?;
    if announce {
        print_step_header(session, &step)?;
    }

    let track = session.course().track(&step.track)?;
    let project_label = track.project.clone();
    let project_dir = track.project_dir(session.course().root());
    let missing = project::missing_content(session.course(), track, &step);
    if !missing.is_empty() {
        println!();
        for line in &missing {
            println!("  {line}");
        }
        println!();
        println!(
            "nothing to judge: this step's content is not authored yet (metadata is in place)."
        );
        return Ok(EXIT_NOT_AUTHORED);
    }

    let Some(run) = &step.run else {
        return Err(KilnError::Invalid(format!(
            "{} declares no `run.cmd`, so its spec tests cannot be started",
            step.qualified_id()
        )));
    };

    println!();
    print_preparation(&project::prepare(session.course(), track, &step)?);

    // Refuse to judge against a spec the project's author changed: the result would say something
    // about that file, not about the step.
    let drift = spec::check_learner(session.course().root(), track, &step, &project_dir)?;
    if !drift.is_clean() {
        println!();
        for problem in &drift.problems {
            println!("✗ {problem}");
        }
        session
            .state_mut()
            .record_spec_modified(&step.qualified_id(), true);
        session.save_state()?;
        println!();
        println!("nothing was judged: a judgement against an edited spec is not a judgement.");
        return Ok(EXIT_ERROR);
    }
    session
        .state_mut()
        .record_spec_modified(&step.qualified_id(), false);

    let deadline = step.deadline(session.timeout());
    println!();
    println!(
        "> {}  (in {}, deadline {:?})",
        run.cmd, project_label, deadline
    );
    let ran = process::run_streamed_line(&run.cmd, &project_dir, deadline)?;
    print_output(&ran);

    let passed = ran.success();
    let id = step.qualified_id();
    session.state_mut().record_spec(&id, passed);
    // Spec and gates both green: the step is done, and its capabilities are proven. `check` writes
    // the same record when it is the one that completes the step; the rule lives in the state so
    // both paths agree on when "done" happened.
    if session.state().status(&id) == crate::state::StepStatus::Done {
        let teaches = step.teaches.clone();
        session.state_mut().record_step_capabilities(&teaches, &id);
    }
    session.save_state()?;

    println!();
    if passed {
        println!(
            "✓ {} spec tests pass — now run `kiln check {}` for the gates",
            step.qualified_id(),
            step.qualified_id()
        );
        Ok(EXIT_OK)
    } else {
        println!(
            "✗ {} spec tests failed — `kiln hint {}` for a nudge, `kiln solution {}` to compare",
            step.qualified_id(),
            step.qualified_id(),
            step.qualified_id()
        );
        Ok(EXIT_FAILED)
    }
}

/// Print what the runner put in place before judging, when it had to put anything there.
pub fn print_preparation(prepared: &project::Outcome) {
    if prepared.is_empty() {
        return;
    }
    for line in prepared.lines() {
        println!("  {line}");
    }
    for note in &prepared.notes {
        println!("  note: {note}");
    }
}

/// Print the step header, including where its body is (and whether it is translated).
pub fn print_step_header(session: &Session, step: &Step) -> Result<()> {
    println!(
        "{} — {}",
        step.qualified_id(),
        crate::i18n::step_title(
            &session.course().dir(),
            step,
            session.locale(),
            session.source()
        )
    );
    println!(
        "  kind: {} · origin: {} · {}",
        step.kind, step.origin, step.time_hint
    );

    let doc = resolve_step_doc(
        &session.course().dir(),
        &step.entry,
        session.locale(),
        session.source(),
    )?;
    let relative = doc
        .path()
        .strip_prefix(session.course().root())
        .unwrap_or_else(|_| doc.path());
    match doc.locale() {
        Some(locale) => println!("  body: {} ({locale})", relative.display()),
        None => println!("  body: not authored yet ({})", relative.display()),
    }
    if let Some(notice) = fallback_notice(
        &step.qualified_id(),
        session.locale(),
        &doc,
        session.source(),
    ) {
        println!("  {notice}");
    }
    Ok(())
}

/// Print a command's output without letting it swallow the report.
/// Print what the runner captured.
///
/// `kiln run` streams the child's output instead (`process::run_streamed_line`), so this only has
/// something to say when a command timed out: there is no transcript to reorder, and the note has
/// to be printed by somebody.
fn print_output(ran: &process::Ran) {
    let stdout = ran.stdout.trim_end();
    if !stdout.is_empty() {
        println!("{stdout}");
    }
    let stderr = ran.stderr.trim_end();
    if !stderr.is_empty() {
        eprintln!("{stderr}");
    }
}
