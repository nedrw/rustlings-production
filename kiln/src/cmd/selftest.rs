//! `kiln selftest [step …]` — the course's own anti-rot check (plan §10).
//!
//! The contract every step has to keep fits in one sentence (authoring.md §5.5, §6.2): its spec must
//! **fail** before the reference solution and **pass** after it, with every `must` gate green. A step
//! whose spec cannot fail is not a test, it is decoration — and that is reported as a broken step,
//! never as a pass.
//!
//! It runs in a throwaway copy of the course: `course/` is copied into a scratch directory and `app/`
//! starts empty, so the result says something about the *course* rather than about whatever the
//! learner happens to have in their project. Steps before the ones being asserted are set up
//! silently (scaffold then reference solution), because that is the state a step starts from.

use crate::cmd::{Cli, Session};
use crate::error::{EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, Result};
use crate::gates::{Outcome, Tier};
use crate::info::{COURSE_DIR, Step, Track};
use crate::process::Ran;
use crate::project;
use crate::scratch::Scratch;

/// How much of a failing command's output a report keeps.
const OUTPUT_LINES: usize = 12;

/// What one step did in the sandbox.
struct Report {
    /// Qualified step id.
    step: String,
    /// What was set up: `assert` for the steps under test, `setup` for their prerequisites.
    role: &'static str,
    /// Report lines, already phrased for a human.
    lines: Vec<String>,
    /// `false` as soon as one part of the contract was broken.
    passed: bool,
}

/// Run the course's self-check.
pub fn run(cli: &Cli, session: &Session, requested: &[String]) -> Result<u8> {
    let track = session.course().default_track()?;
    let authored: Vec<&Step> = track
        .step
        .iter()
        .filter(|step| project::missing_content(session.course(), track, step).is_empty())
        .collect();
    if authored.is_empty() {
        println!(
            "nothing to test yet: no step of {} has a body, a spec and a solution",
            track.id
        );
        return Ok(EXIT_NOT_AUTHORED);
    }
    // Which steps to *assert* on; every earlier step only has to set the state up. References are
    // resolved through the course, so `1.1`, `1.1-workspace` and `mailer/1.1-workspace` all work.
    let mut wanted: Vec<String> = Vec::new();
    for reference in requested {
        let qualified = session.course().step(reference)?.qualified_id();
        if !qualified.starts_with(&format!("{}/", track.id)) {
            return Err(crate::error::KilnError::Invalid(format!(
                "{qualified} is not part of {} — selftest walks one track at a time",
                track.id
            )));
        }
        wanted.push(qualified);
    }

    let scratch = match Scratch::new("selftest") {
        Ok(scratch) => scratch,
        Err(source) => {
            return Err(crate::error::KilnError::Invalid(format!(
                "cannot create a scratch directory for selftest: {source}"
            )));
        }
    };
    // One build directory for every command this run starts, so the dependencies of the project are
    // compiled once per machine rather than once per step: nineteen throwaway copies of one project
    // would otherwise pay for the same crates nineteen times.
    if let Some(target_dir) = crate::scratch::shared_target_dir() {
        crate::process::share_cargo_target_dir(target_dir);
    }
    project::copy_tree(&session.course().dir(), &scratch.path().join(COURSE_DIR))?;
    // The repository's own toolchain and style settings reach `app/<track>` through the directory
    // it lives in, so the sandbox needs them too; otherwise the sandbox would compile with whatever
    // toolchain happens to be the default, and the check would say something about this machine.
    for shared in ["rust-toolchain.toml", "rustfmt.toml"] {
        let from = session.course().root().join(shared);
        if from.is_file() {
            let to = scratch.path().join(shared);
            std::fs::copy(&from, &to).map_err(|source| crate::error::KilnError::Write {
                path: to.clone(),
                source,
            })?;
        }
    }
    let sandbox = Session::at(scratch.path(), cli)?;
    let track = sandbox.course().default_track()?;
    println!(
        "kiln selftest · {} · a throwaway copy at {}",
        track.id,
        scratch.path().display()
    );

    let mut reports = Vec::new();
    for step in &track.step {
        let authored = project::missing_content(sandbox.course(), track, step).is_empty();
        let assert = authored && (wanted.is_empty() || wanted.contains(&step.qualified_id()));
        if !authored {
            // Nothing to set up or judge beyond this point either: T1 is written in order.
            break;
        }
        let role = if assert { "assert" } else { "setup" };
        reports.push(check_step(&sandbox, track, step, assert, role));
        if assert && !wanted.is_empty() && wanted.iter().all(|id| id == &step.qualified_id()) {
            // Every requested step has been asserted on; nothing after it is needed as setup.
            break;
        }
    }

    println!();
    for report in &reports {
        println!(
            "{} {:<22} ({})",
            if report.passed { "✓" } else { "✗" },
            report.step,
            report.role
        );
        for line in &report.lines {
            println!("    {line}");
        }
    }

    let failed = reports.iter().filter(|report| !report.passed).count();
    let judged = reports
        .iter()
        .filter(|report| report.role == "assert")
        .count();
    println!();
    println!(
        "{}/{} step(s) hold the contract: the spec fails on the scaffold, passes on the reference \
         solution, and every must gate is green",
        judged.saturating_sub(failed),
        judged
    );
    if failed == 0 {
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_FAILED)
    }
}

/// Scaffold, run, solve, run, gate: the contract of one step.
fn check_step(
    session: &Session,
    track: &Track,
    step: &Step,
    assert: bool,
    role: &'static str,
) -> Report {
    let mut report = Report {
        step: step.qualified_id(),
        role,
        lines: Vec::new(),
        passed: true,
    };

    if let Err(error) = project::prepare(session.course(), track, step) {
        report
            .lines
            .push(format!("could not prepare the step: {error}"));
        report.passed = false;
        return report;
    }

    if assert {
        grade_spec(session, step, true, &mut report);
    }

    match project::apply_solution(session.course(), track, step) {
        Ok(_) => {
            if assert {
                report
                    .lines
                    .push("solution ✓ reference patch applies".to_owned());
            }
        }
        Err(error) => {
            report.lines.push(format!(
                "{} the reference solution did not apply: {error}",
                if assert { "solution ✗" } else { "setup ✗" }
            ));
            report.passed = false;
            return report;
        }
    }

    if assert {
        grade_spec(session, step, false, &mut report);
        grade_gates(session, step, &mut report);
    }
    report
}

/// Run the step's spec tests and grade the outcome for the given side of the contract.
fn grade_spec(session: &Session, step: &Step, before_solution: bool, report: &mut Report) {
    let side = if before_solution {
        "scaffold"
    } else {
        "solution"
    };
    let ran = match session.spec(step) {
        Ok(ran) => ran,
        Err(error) => {
            report
                .lines
                .push(format!("{side} ✗ the spec could not run: {error}"));
            report.passed = false;
            return;
        }
    };
    match (before_solution, ran.success()) {
        (true, false) => {
            let structural = if complaint(&ran).contains("could not compile") {
                " — note: it does not compile on the scaffold, so the scaffold is missing a signature \
                 the spec uses (authoring.md §5.4)"
            } else {
                ""
            };
            report.lines.push(format!(
                "scaffold ✓ the spec fails as it must ({}){structural}",
                summary(&ran)
            ));
        }
        (false, true) => report.lines.push("solution ✓ the spec passes".to_owned()),
        (true, true) => {
            report.lines.push(
                "BROKEN STEP: the spec passes on the scaffold, so it cannot tell the two states \
                 apart"
                    .to_owned(),
            );
            report.lines.push(indent(&clip(&ran)));
            report.passed = false;
        }
        (false, false) => {
            report.lines.push(format!(
                "solution ✗ the spec still fails: {}",
                summary(&ran)
            ));
            report.lines.push(indent(&clip(&ran)));
            report.passed = false;
        }
    }
}

/// Run the step's gates and grade every `must` one.
fn grade_gates(session: &Session, step: &Step, report: &mut Report) {
    let results = match session.gates(step, true) {
        Ok(results) => results,
        Err(error) => {
            report.lines.push(format!("gates ✗ could not run: {error}"));
            report.passed = false;
            return;
        }
    };
    let must: Vec<&crate::gates::GateResult> = results
        .iter()
        .filter(|result| result.spec.tier == Tier::Must)
        .collect();
    let green = must
        .iter()
        .filter(|result| result.outcome == Outcome::Passed)
        .count();
    let cannot_judge: Vec<String> = must
        .iter()
        .filter(|result| matches!(result.outcome, Outcome::Blocked | Outcome::Pending))
        .map(|result| format!("{} is {}", result.spec, result.outcome.as_str()))
        .collect();

    report.lines.push(format!(
        "gates {} {}/{} must green{}",
        if green == must.len() { "✓" } else { "✗" },
        green,
        must.len(),
        if cannot_judge.is_empty() {
            String::new()
        } else {
            format!(" ({})", cannot_judge.join(", "))
        }
    ));
    for result in &results {
        if result.breaks_the_step() {
            report.passed = false;
            report.lines.push(format!(
                "{} {}\n{}",
                result.outcome.marker(),
                result.spec,
                indent(&clip_text(&result.detail))
            ));
        }
    }
}

/// One line summarising a command's outcome.
fn summary(ran: &Ran) -> String {
    if ran.timed_out {
        return format!("timed out after {:?}", ran.elapsed);
    }
    match ran.code {
        Some(0) => "exit code 0".to_owned(),
        Some(code) => format!("exit code {code}: {}", first_line(ran)),
        None => "killed by a signal".to_owned(),
    }
}

fn first_line(ran: &Ran) -> String {
    let line = ran.first_message().unwrap_or("no output").to_owned();
    if line.chars().count() > 120 {
        let head: String = line.chars().take(117).collect();
        return format!("{head}...");
    }
    line
}

/// Everything a command printed.
/// The stream that explains a failure, for the report.
///
/// A test binary writes the panic, the assertion message and the failing test's name to *stdout*,
/// while the tool that started it narrates on *stderr* (`Compiling`, `Running`, and one line saying
/// `test failed`). Preferring stderr because it is not empty therefore hides the answer — a report
/// that says a spec still fails without saying which test failed is a report nobody can act on.
fn complaint(ran: &Ran) -> String {
    let stdout = ran.stdout.trim();
    if stdout.lines().any(crate::process::names_a_failure) {
        return ran.stdout.clone();
    }
    if ran.stderr.trim().is_empty() {
        ran.stdout.clone()
    } else {
        ran.stderr.clone()
    }
}

fn clip(ran: &Ran) -> String {
    clip_text(&complaint(ran))
}

fn clip_text(text: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(OUTPUT_LINES)
        .collect();
    let total = text.lines().filter(|line| !line.trim().is_empty()).count();
    let mut joined = kept.join("\n");
    if total > kept.len() {
        joined.push_str(&format!("\n… {} more line(s)", total - kept.len()));
    }
    joined
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::complaint;
    use crate::process::names_a_failure;

    fn ran(stdout: &str, stderr: &str) -> crate::process::Ran {
        crate::process::Ran {
            code: Some(101),
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            timed_out: false,
            elapsed: std::time::Duration::from_secs(1),
        }
    }

    /// `cargo test` puts the answer on stdout and the narration on stderr; the report has to show
    /// the answer, or a flaky step is undiagnosable (this happened on `mailer/1.18-authz-boundary`:
    /// the report showed four lines of cargo progress and no failing test).
    #[test]
    fn the_report_shows_the_test_that_failed_not_the_tool_that_ran_it() {
        let cargo = "   Compiling mailer v0.1.0\n    Finished `test` profile\n     Running tests/spec_authz.rs\nerror: test failed, to rerun pass `-p mailer --test spec_authz`\n";
        let libtest = "\nrunning 4 tests\ntest a_subscriber_reads_their_own ... FAILED\n\nfailures:\n\n---- a_subscriber_reads_their_own stdout ----\nthread 'a_subscriber_reads_their_own' panicked at tests/spec_authz.rs:44:5:\nthe answer for somebody else's delivery is 404, not 200\n\ntest result: FAILED. 3 passed; 1 failed\n";
        let text = complaint(&ran(libtest, cargo));
        assert!(
            text.contains("thread 'a_subscriber_reads_their_own'"),
            "{text}"
        );
        assert!(!text.contains("Compiling"), "{text}");
    }

    /// A command whose only output is on stderr (a tool that cannot run at all) still reports it.
    #[test]
    fn a_failure_without_test_output_still_comes_from_stderr() {
        let text = complaint(&ran("", "error: no library targets found\n"));
        assert_eq!(text.trim(), "error: no library targets found");
        let text = complaint(&ran("", ""));
        assert_eq!(text, "");
    }

    #[test]
    fn a_line_names_a_failure_only_when_it_says_what_broke() {
        assert!(names_a_failure("thread 'x' panicked at src/lib.rs:1:1:"));
        assert!(names_a_failure("test result: FAILED. 3 passed; 1 failed"));
        assert!(names_a_failure("---- x stdout ----"));
        assert!(!names_a_failure("    Finished `test` profile"));
        assert!(!names_a_failure("   Compiling mailer v0.1.0"));
    }
}
