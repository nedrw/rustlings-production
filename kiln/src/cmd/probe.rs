//! `kiln probe [capability]` — the review exercises, and the evidence `--fast` accepts.
//!
//! Three jobs in one command, because they are one loop:
//!
//! * `kiln probe` lists the catalogue with what this learner last proved;
//! * `kiln probe <capability>` runs *their* copy of the exercise (writing it the first time, never
//!   overwriting it) and records `last_verified` when it passes;
//! * `kiln probe --check` validates the *course*: every reference solution compiles and passes, in
//!   one scratch crate. That is the anti-rot rule the steps have, applied to the review material.

use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, KilnError, Result};
use crate::probe::{self, Probe};
use crate::scratch::Scratch;
use crate::state::{Evidence, days_between, unix_now};

/// Print the catalogue, run one exercise, or check the course.
pub fn run(
    session: &mut Session,
    reference: Option<&str>,
    solution: bool,
    reset: bool,
    check: bool,
) -> Result<u8> {
    if check {
        if reference.is_some() {
            return Err(KilnError::Invalid(
                "`kiln probe --check` validates the whole catalogue: it takes no capability"
                    .to_owned(),
            ));
        }
        return check_course(session);
    }

    let probes = probe::load(&session.course().dir())?;
    let Some(reference) = reference else {
        if solution || reset {
            return Err(KilnError::Invalid(
                "`--solution` and `--reset` act on one capability: name it".to_owned(),
            ));
        }
        return list(session, &probes);
    };

    let probe = probe::find(&probes, reference)?;
    if reset {
        let dir = session.course().dir();
        let root = session.course().root();
        let path = probe.reset(&dir, root)?;
        println!(
            "reset {} to the course exercise at {}",
            probe.slug(),
            path.display()
        );
        return Ok(EXIT_OK);
    }
    if solution {
        return run_reference(session, probe);
    }
    run_learner(session, probe)
}

/// The catalogue with the learner's state: what is proven, how long ago, and how to re-check it.
fn list(session: &Session, probes: &[Probe]) -> Result<u8> {
    if probes.is_empty() {
        println!("no probe exercises yet (course/probes/)");
        return Ok(EXIT_NOT_AUTHORED);
    }
    let now = unix_now();
    println!(
        "probes · {} exercise(s) · `kiln probe <capability>` runs one, `kiln review --due` picks \
         what is stale",
        probes.len()
    );
    println!();
    for probe in probes {
        let record = session.state().capability(&probe.requirement().id);
        let status = match record.and_then(|record| record.last_verified) {
            Some(then) => format!("last proved {} days ago", days_between(now, then)),
            None => "never proved".to_owned(),
        };
        println!(
            "  {:<40} {:<7} {status}",
            probe.requirement().id.as_str(),
            probe.requirement().level
        );
        println!("      {} · {}", probe.title(), probe.slug());
    }
    println!();
    println!(
        "evidence: a passed probe is what `kiln plan --fast` accepts as proof you may skip a step"
    );
    Ok(EXIT_OK)
}

/// Run the learner's copy and record the result when it passes.
fn run_learner(session: &mut Session, probe: &Probe) -> Result<u8> {
    let course_dir = session.course().dir();
    let root = session.course().root().to_path_buf();
    println!("probe · {} · {}", probe.requirement(), probe.title());
    for line in probe.prepare(&course_dir, &root)? {
        println!("  {line}");
    }
    if let Some(target) = crate::scratch::shared_target_dir() {
        crate::process::share_cargo_target_dir(target);
    }

    println!();
    println!("  cargo test  (in .kiln/probes/{})", probe.slug());
    let ran = probe.run(&root, session.timeout())?;
    print_output(&ran);
    if !ran.success() {
        println!();
        println!("✗ the exercise does not pass yet — {}", probe.hint());
        println!(
            "  your copy:  {}",
            probe.copy_dir(&root).join("src/lib.rs").display()
        );
        println!(
            "  stuck?     `kiln probe {} --solution` runs the reference (it does not count as \
             evidence)",
            probe.slug()
        );
        return Ok(EXIT_FAILED);
    }

    // Only the learner's own pass is recorded. `--solution` deliberately never reaches here.
    session
        .state_mut()
        .record_capability(probe.requirement(), Evidence::Probe(probe.slug()));
    session.save_state()?;

    println!();
    println!(
        "✓ {} re-verified by probe — `last_verified` written, and `kiln plan --fast` now counts this \
         as proof",
        probe.requirement()
    );
    Ok(EXIT_OK)
}

/// Run the reference in the learner's copy: a comparison, never evidence.
fn run_reference(session: &Session, probe: &Probe) -> Result<u8> {
    let course = session.course();
    println!("probe · {} · the reference solution", probe.requirement());
    let path = probe.apply_solution(&course.dir(), course.root())?;
    println!(
        "  wrote the reference over {} — `kiln probe {} --reset` puts the exercise back",
        path.display(),
        probe.slug()
    );
    if let Some(target) = crate::scratch::shared_target_dir() {
        crate::process::share_cargo_target_dir(target);
    }
    let ran = probe.run(course.root(), session.timeout())?;
    print_output(&ran);
    println!();
    if ran.success() {
        println!(
            "✓ the reference passes (nothing recorded: this is where you compare, not what you \
             proved)"
        );
        Ok(EXIT_OK)
    } else {
        println!(
            "✗ the reference did not pass, which is a broken probe — `kiln probe --check` says \
                  the same"
        );
        Ok(EXIT_FAILED)
    }
}

/// `kiln probe --check`: the course's own anti-rot check for the review material.
fn check_course(session: &Session) -> Result<u8> {
    let course = session.course();
    let probes = probe::load(&course.dir())?;
    if probes.is_empty() {
        println!("no probe exercises yet (course/probes/)");
        return Ok(EXIT_NOT_AUTHORED);
    }

    // Two checks that cost nothing, and one that costs a compile.
    let mut problems = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for probe in &probes {
        if seen.contains(&probe.requirement().to_string()) {
            problems.push(format!("two probes claim {}", probe.requirement()));
        }
        seen.push(probe.requirement().to_string());
        if course
            .ledger()
            .slot(&probe.requirement().id, probe.requirement().level)
            .is_none()
        {
            problems.push(format!(
                "{} is not a slot in the ledger (course/capabilities.toml)",
                probe.requirement()
            ));
        }
    }

    let scratch = Scratch::new("probe-check").map_err(|source| KilnError::Write {
        path: std::env::temp_dir(),
        source,
    })?;
    if let Some(target) = crate::scratch::shared_target_dir() {
        crate::process::share_cargo_target_dir(target);
    }
    let report = probe::verify_references(
        &probes,
        &course.dir(),
        scratch.path(),
        session.timeout().max(std::time::Duration::from_secs(180)),
    )?;

    println!(
        "probe --check · {} exercise(s) · {} reference(s) passed",
        probes.len(),
        report.passed.len()
    );
    for problem in &problems {
        println!("  ✗ {problem}");
    }
    for (slug, why) in &report.failed {
        println!("  ✗ {slug}: {why}");
    }
    if problems.is_empty() && report.is_clean() {
        println!("✓ every probe reference compiles and passes");
        return Ok(EXIT_OK);
    }
    println!(
        "✗ {} problem(s) — a probe that cannot pass is worse than no probe: \
         `kiln review --due` would send learners to a broken exercise",
        problems.len() + report.failed.len()
    );
    Ok(EXIT_ERROR)
}

fn print_output(ran: &crate::process::Ran) {
    for stream in [&ran.stdout, &ran.stderr] {
        let text = stream.trim_end();
        if text.is_empty() {
            continue;
        }
        println!("{text}");
    }
}
