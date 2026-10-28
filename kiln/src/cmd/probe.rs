//! `kiln probe [capability]` — the review exercises, and the evidence `--fast` accepts.
//!
//! Three jobs in one command, because they are one loop:
//!
//! * `kiln probe` lists the catalogue with what this learner last proved;
//! * `kiln probe <capability>` runs *their* copy of the exercise (writing it the first time, never
//!   overwriting it) and records `last_verified` when it passes;
//! * `kiln probe --check` validates the *course*: every reference solution compiles and passes, in
//!   one scratch crate. That is the anti-rot rule the steps have, applied to the review material.

use std::path::{Path, PathBuf};

use crate::caps::Level;
use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, KilnError, Result};
use crate::hash;
use crate::probe::{self, Probe};
use crate::scratch;
use crate::state::{Evidence, days_between, unix_now};

/// The name this command's build directories live under: `<cache>/kiln/probe-check/`.
///
/// Every reference compiles as its own test target, so the first run of a catalogue is the
/// expensive one; [`check_directory`] keeps that work between runs.
const CHECK_DIR: &str = "probe-check";

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

    // The catalogue is one `cargo test` of every reference, so the run after this one should not pay
    // for what this one built: the directory is the machine's cache, one per course checkout —
    // [`check_directory`] says why it is neither shared between checkouts nor the shared build
    // directory the sandboxed runs use.
    let dir = check_directory(&course.dir())?;
    println!(
        "  compiling the {} reference(s) in {} — kept between runs",
        probes.len(),
        dir.display()
    );
    let report = probe::verify_references(
        &probes,
        &course.dir(),
        &dir,
        session.timeout().max(std::time::Duration::from_secs(180)),
    )?;

    println!(
        "probe --check · {} exercise(s) · {} reference(s) passed",
        probes.len(),
        report.passed.len()
    );
    // Coverage, printed rather than enforced while it is being closed (docs/m6-plan.md, M6.0). A
    // slot at `apply` depth with no probe is one `plan --fast` cannot skip and `review --due` will
    // never re-check; when the last one is written, this becomes a failure instead of a number.
    let mut apply_slots = 0usize;
    let mut uncovered: Vec<String> = Vec::new();
    for (capability, level, _) in course.ledger().slots() {
        if level != Level::Apply {
            continue;
        }
        apply_slots += 1;
        let covered = probes.iter().any(|probe| {
            probe.requirement().level == Level::Apply && probe.requirement().id == *capability
        });
        if !covered {
            uncovered.push(format!("{capability}@apply"));
        }
    }
    uncovered.sort();
    if apply_slots > 0 {
        println!(
            "  coverage: {}/{} apply slot(s) have a probe",
            apply_slots - uncovered.len(),
            apply_slots
        );
        if !uncovered.is_empty() {
            let shown: Vec<&str> = uncovered.iter().take(6).map(String::as_str).collect();
            let more = uncovered.len().saturating_sub(shown.len());
            println!(
                "  without one: {}{} — docs/m6-plan.md says which batch writes them",
                shown.join(", "),
                if more > 0 {
                    format!(" … +{more}")
                } else {
                    String::new()
                }
            );
        }
    }
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

/// The directory this check compiles in: `<cache>/kiln/probe-check/<course>`.
///
/// Kept, because every reference is compiled here as its own test target: the first run pays for
/// all of them and the next one compiles what changed ([`probe::verify_references`] writes only
/// changed bytes and removes targets the catalogue no longer has). One directory per course
/// *checkout* rather than one per machine, because two checkouts are two catalogues and a shared
/// directory would have each run undoing the other's build.
///
/// It is deliberately not the shared `<cache>/kiln/target` a sandboxed run uses: this crate has no
/// dependencies, so there is nothing for that directory to save, and pointing cargo at it puts every
/// check behind the builds of a twenty-four-step project (measured: the same check took 63 s with
/// the machine to itself and over three minutes at a load average of 17 on eight cores).
///
/// # Errors
///
/// [`KilnError::Write`] when the machine has nowhere to put a cache, or the directory cannot be
/// created.
fn check_directory(course: &Path) -> Result<PathBuf> {
    let dir = scratch::cache(CHECK_DIR)?.join(check_key(course));
    std::fs::create_dir_all(&dir).map_err(|source| KilnError::Write {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// The name one course checkout's build lives under: eight hex characters of its path.
///
/// Plain hex, without the `sha256:` the course writes down: macOS's loader splits
/// `DYLD_FALLBACK_LIBRARY_PATH` — which cargo builds out of the target directory — on `:`, so a
/// directory called `sha256:2cc5c06e` made every check fail with `failed to join paths from
/// $DYLD_FALLBACK_LIBRARY_PATH` (measured, not guessed). Eight characters are enough to tell two
/// checkouts apart and short enough to read in the line the command prints.
fn check_key(course: &Path) -> String {
    let key = hash::bytes(course.to_string_lossy().as_bytes());
    key[hash::PREFIX.len()..hash::PREFIX.len() + 8].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_directory_name_is_one_path_segment_the_same_course_keeps() {
        let one = check_key(Path::new("/tmp/course-one"));
        assert_eq!(
            one,
            check_key(Path::new("/tmp/course-one")),
            "the same course keeps its build directory between runs"
        );
        assert_ne!(
            one,
            check_key(Path::new("/tmp/course-two")),
            "two checkouts are two catalogues, so they do not undo each other's build"
        );
        assert_eq!(one.len(), 8, "eight characters, and no prefix: {one}");
        assert!(
            one.chars().all(|character| character.is_ascii_hexdigit()),
            "the name is a path segment the loader cannot split: {one}"
        );
    }
}
