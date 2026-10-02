//! `kiln run [step]` — judge a step's spec tests.
//!
//! The fast path of the whole course (plan §6.1): one command, one answer, under a minute of
//! wall clock. When a step has no spec or no project yet, this says so and exits 3 — "you still
//! have work to do" and "this step is not written yet" must not look the same.

use crate::cmd::Session;
use crate::error::{EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, KilnError, Result};
use crate::i18n::{fallback_notice, resolve_step_doc};
use crate::info::Step;
use crate::process;

/// Run the spec tests of one step.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    print_step_header(session, &step)?;

    let track = session.course().track(&step.track)?;
    let project = track.project_dir(session.course().root());
    let missing = missing_content(session, &step, &project);
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
    println!(
        "> {}  (in {}, deadline {:?})",
        run.cmd,
        track.project,
        session.timeout()
    );
    let ran = process::run_line(&run.cmd, &project, session.timeout())?;
    print_output(&ran);

    let passed = ran.success();
    session
        .state_mut()
        .record_spec(&step.qualified_id(), passed);
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
        println!("✗ {} spec tests failed", step.qualified_id());
        Ok(EXIT_FAILED)
    }
}

/// Print the step header, including where its body is (and whether it is translated).
pub fn print_step_header(session: &Session, step: &Step) -> Result<()> {
    println!("{} — {}", step.qualified_id(), step.title);
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

/// Everything that stops this step from being judged, phrased for the learner.
fn missing_content(session: &Session, step: &Step, project: &std::path::Path) -> Vec<String> {
    let mut missing = Vec::new();
    if !project.is_dir() {
        missing.push(format!(
            "learner project {} does not exist yet",
            project.display()
        ));
    }
    let track_dir = session
        .course()
        .track(&step.track)
        .map(|track| track.dir(session.course().root()));
    if let Ok(track_dir) = track_dir {
        for spec in &step.spec {
            let path = track_dir.join(spec);
            if !path.is_file() {
                missing.push(format!("spec file is missing: {}", path.display()));
            }
        }
        if step.spec.is_empty() {
            missing.push("the step declares no spec file".to_owned());
        }
    }
    missing
}

/// Print a command's output without letting it swallow the report.
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
