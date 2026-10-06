//! Which files a step's slow gates read.
//!
//! Two questions come up in more than one gate, and they are answered here so that the gates cannot
//! drift apart on them:
//!
//! * **what is "this step's module"?** A hand-kept list would go stale the day a step is added; the
//!   course already says where a step's work is — its patches. The two gates ask different questions
//!   of that, and deliberately read different sets: `coverage` measures what the step *introduces*
//!   ([`introduced_sources`]: the files its scaffold and its reference solution create), and
//!   `mutants` samples every file the reference solution touches ([`production_sources`]). The
//!   reasons are written out on each function.
//! * **where does the course keep what the gate judges against?** Course-owned files next to `spec/`,
//!   at the same trust level: readable, useless to edit, because the gate reads the course's copy.
//!   A bench baseline is per judged step (`budget/<step-id>.toml`); a public-API witness is *not* —
//!   it is established by one step and then holds for every later one, so the witness a step is
//!   judged against is the newest one at or before it ([`newest_witness`]).

use std::path::{Path, PathBuf};

use crate::error::{KilnError, Result};
use crate::info::Step;
use crate::patch::{Patch, Section};
use crate::project::{self, Scaffold};

/// The production sources a step's reference solution touches, relative to the project.
///
/// Paths stay in the order the patch lists them, so two runs of a gate see the same module in the
/// same order. A step without a reference solution answers with an empty list: the caller says what
/// that means for its own gate.
///
/// This is what `mutants` mutates: it asks whether the suite would notice if the behaviour *this
/// step touched* were wrong, and a site in a file the step only edits is a site the step is
/// answerable for (handoff M2 §1.3: `jobq/3.26-dlq-cli`'s spec grew three cases because of exactly
/// such a site).
pub fn production_sources(track_dir: &Path, step: &Step) -> Result<Vec<String>> {
    let Some(solution) = project::load_solution(track_dir, step)? else {
        return Ok(Vec::new());
    };
    Ok(touched(&solution))
}

/// The production sources a step **introduces**: the `src/**/*.rs` files its scaffold or its
/// reference solution creates.
///
/// This is what `coverage` measures, and it is a narrower question than [`production_sources`] on
/// purpose: `coverage` asks whether the step's suite walks the code **this step is about**, and that
/// code is the module the step brings with it — the scaffold's structure plus the solution that
/// fills it in. A file an earlier step wrote is not this step's code even when this step edits it: it
/// carries that step's history, its lines are exercised by that step's suite, and a percentage over
/// it is not a number about this step. Measured (handoff M2 §5.2): `jobq/3.26-dlq-cli` edits the
/// store layer, and 1015 of the 1071 lines its module left uncovered sat in files `jobq/3.5`–
/// `jobq/3.23` own — while the file it introduces read 90%.
///
/// A step that introduces no file at all — it changes only what the learner already owns
/// (authoring.md §5) — falls back to the files its patch touches: the alternative is no measurement
/// at all, and a step whose whole work is an edit is answerable for the file it edits.
pub fn introduced_sources(track_dir: &Path, step: &Step) -> Result<Vec<String>> {
    let mut introduced: Vec<String> = Vec::new();
    if let Some(Scaffold::Patch(scaffold)) = project::load_scaffold(track_dir, step)? {
        created(&scaffold, &mut introduced);
    }
    let solution = project::load_solution(track_dir, step)?;
    if let Some(solution) = &solution {
        created(solution, &mut introduced);
    }
    if introduced.is_empty()
        && let Some(solution) = &solution
    {
        return Ok(touched(solution));
    }
    Ok(introduced)
}

/// The files a patch touches that are production sources, in patch order and without repeats.
fn touched(patch: &Patch) -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    for path in patch.paths() {
        if !is_production_source(path) {
            continue;
        }
        if !sources.iter().any(|seen| seen == path) {
            sources.push(path.to_owned());
        }
    }
    sources
}

/// Add the files a patch creates to `found`, in patch order and without repeats: a file the
/// scaffold created and the solution then edits is introduced once.
fn created(patch: &Patch, found: &mut Vec<String>) {
    for file in patch.sections() {
        if file.section != Section::Create || !is_production_source(&file.path) {
            continue;
        }
        if !found.iter().any(|seen| seen == &file.path) {
            found.push(file.path.clone());
        }
    }
}

/// A `.rs` file below `src/` — production code, as opposed to a spec, a bench, a migration or a
/// manifest. Walking `src/` is also what keeps everything under `tests/` out.
#[must_use]
pub fn is_production_source(path: &str) -> bool {
    path.starts_with("src/") && path.ends_with(".rs")
}

/// The course-owned file a gate reads for one step: `<track dir>/<kind>/<step-id>.<extension>`.
///
/// The step id here is the local one (`3.22-bench-budget`), the same string the step's spec, scaffold
/// and solution files are named after, so one step names its files one way everywhere. A witness that
/// outlives its step is the exception, and [`newest_witness`] answers for that one.
#[must_use]
pub fn course_file(track_dir: &Path, step: &Step, kind: &str, extension: &str) -> PathBuf {
    track_dir
        .join(kind)
        .join(format!("{}.{extension}", step.id))
}

/// The public-API witness a step is judged against: the newest one at or before it.
///
/// A witness is written by the step that *establishes* the baseline (step 3.2 of T3) and then holds
/// for every later step of the track — `track.toml` declares `semver@baseline` from there on, and
/// asking each of them for their own file would mean writing the same API down twenty times, all of
/// them drifting apart. So the track ships witnesses in `api/` named after the step that writes them
/// (`api/<step-id>.rs`), and the gate takes the largest step number that is not past the step it is
/// judging. A step that deliberately breaks the API ships a new witness under its own number, and
/// that one is picked up with no change here.
///
/// `Ok(None)` means the track ships no witness at or before the step, which is a hole in the course
/// rather than a mistake by the learner — the caller says so.
pub fn newest_witness(track_dir: &Path, step: &str) -> Result<Option<PathBuf>> {
    let Some(judged) = step_number(step) else {
        return Err(KilnError::Invalid(format!(
            "`{step}` is not a step number: witnesses are chosen by `<phase>.<index>`"
        )));
    };
    let directory = track_dir.join("api");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        // No `api/` directory is the same answer as an empty one, and the caller turns it into the
        // sentence a person can act on.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(KilnError::Read {
                path: directory,
                source,
            });
        }
    };
    let mut candidates: Vec<((u32, u32), String, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| KilnError::Read {
            path: directory.clone(),
            source,
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        let Some(number) = witness_number(name) else {
            continue;
        };
        if number > judged {
            continue;
        }
        candidates.push((number, name.to_owned(), path));
    }
    // Sorted by number and then by name, so two witnesses under one number cannot make the answer
    // depend on the order the file system happened to answer in.
    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(candidates.pop().map(|(_, _, path)| path))
}

/// The step number a witness file is named after, when it is named after one.
///
/// `3.2-public-api-baseline.rs` is the course's naming for a step's files (authoring.md §1); a bare
/// `3.2.rs` is accepted as well, because a name that is nothing but a number can mean nothing else.
/// Anything after the number that is neither the end of the name nor a `-` makes it another kind of
/// file — which is what keeps a README, a note or a stray file from being judged as an API witness.
#[must_use]
pub fn witness_number(name: &str) -> Option<(u32, u32)> {
    let stem = name.strip_suffix(".rs")?;
    let (phase, rest) = stem.split_once('.')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (index, after) = rest.split_at(digits);
    let number = (phase.parse::<u32>().ok()?, index.parse::<u32>().ok()?);
    (after.is_empty() || after.starts_with('-')).then_some(number)
}

/// A step number (`3.4`) as the pair it is ordered by.
#[must_use]
pub fn step_number(text: &str) -> Option<(u32, u32)> {
    let (phase, index) = text.split_once('.')?;
    Some((phase.parse().ok()?, index.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, ledger_toml, load_course, one_track};

    /// A course with one step, optionally declaring a reference solution.
    fn fixture(label: &str, solution: Option<&str>) -> (TempDir, Step) {
        fixture_parts(label, None, solution)
    }

    /// The same, with a scaffold as well: the two patches a step's module can come from.
    fn fixture_parts(
        label: &str,
        scaffold: Option<&str>,
        solution: Option<&str>,
    ) -> (TempDir, Step) {
        let temp = TempDir::new(label);
        let declared = |kind: &str, path: Option<&str>| {
            path.map_or(String::new(), |path| format!("{kind} = \"{path}\"\n"))
        };
        let patches = format!(
            "{}{}",
            declared("scaffold", scaffold),
            declared("solution", solution)
        );
        let step = format!(
            "\n[[step]]\nid = \"3.22-bench-budget\"\ntitle = \"Budget\"\nkind = \"milestone\"\n\
             origin = \"teach\"\nentry = \"docs/steps/en/mailer/3.22-bench-budget.md\"\n\
             time_hint = \"60m\"\nteaches = [\"perf/bench-budget@apply\"]\n\
             run = {{ cmd = \"cargo test -p jobq --test spec_budget\" }}\n\
             gates = [\"fmt\"]\n{patches}"
        );
        let course = load_course(
            temp.path(),
            &one_track(&[step], &["perf/bench-budget@apply"]),
            &ledger_toml(&[("perf/bench-budget", "apply", "mailer/3.22")]),
        );
        let step = course
            .default_track()
            .expect("the fixture has a default track")
            .step
            .first()
            .expect("the fixture has one step")
            .clone();
        (temp, step)
    }

    /// A patch that touches one file of every kind the filter has to tell apart.
    ///
    /// Written with `concat!` rather than one long literal: a patch line *starts* with the space or
    /// the `+` that says what it is, and a line continuation in a Rust literal would eat exactly that
    /// character.
    const PATCH: &str = concat!(
        "--- a/src/store.rs\n",
        "+++ b/src/store.rs\n",
        "@@ -1,1 +1,2 @@\n",
        " use std::fmt;\n",
        "+/// New.\n",
        "--- a/tests/spec_spec.rs\n",
        "+++ b/tests/spec_spec.rs\n",
        "@@ -1,1 +1,2 @@\n",
        " use x;\n",
        "+y;\n",
        "--- a/migrations/001.sql\n",
        "+++ b/migrations/001.sql\n",
        "@@ -1,1 +1,2 @@\n",
        " a\n",
        "+b\n",
        "--- a/Cargo.toml\n",
        "+++ b/Cargo.toml\n",
        "@@ -1,1 +1,2 @@\n",
        " a\n",
        "+b\n",
        "--- /dev/null\n",
        "+++ b/src/bin/jobqc.rs\n",
        "@@ -0,0 +1,1 @@\n",
        "+fn main() {}\n",
    );

    /// A scaffold that creates the module the step is about.
    const INTRODUCES: &str = concat!(
        "--- /dev/null\n",
        "+++ b/src/engine/budget.rs\n",
        "@@ -0,0 +1,1 @@\n",
        "+pub fn budget() {}\n",
    );

    /// A patch that only edits a file the learner already owns.
    const MODIFIES: &str = concat!(
        "--- a/src/store.rs\n",
        "+++ b/src/store.rs\n",
        "@@ -1,1 +1,2 @@\n",
        " use std::fmt;\n",
        "+/// New.\n",
    );

    #[test]
    fn the_module_coverage_measures_is_what_the_step_introduces() {
        let (temp, step) = fixture_parts(
            "sources-introduced",
            Some("scaffold/3.22-bench-budget.patch"),
            Some("solutions/3.22-bench-budget.patch"),
        );
        let track_dir = temp.path().join("course/tracks/mailer");
        temp.write(
            "course/tracks/mailer/scaffold/3.22-bench-budget.patch",
            INTRODUCES,
        );
        temp.write(
            "course/tracks/mailer/solutions/3.22-bench-budget.patch",
            PATCH,
        );

        let introduced = introduced_sources(&track_dir, &step).expect("the patches parse");
        assert_eq!(
            introduced,
            ["src/engine/budget.rs", "src/bin/jobqc.rs"],
            "the scaffold's file first, then the solution's: what the step brings"
        );
        // `mutants` still reads the whole touched set: sampling widely is that gate's point.
        assert_eq!(
            production_sources(&track_dir, &step).expect("the patch parses"),
            ["src/store.rs", "src/bin/jobqc.rs"]
        );
    }

    #[test]
    fn a_step_that_introduces_no_file_is_measured_over_what_it_touches() {
        let (temp, step) = fixture("sources-touches", Some("solutions/3.22-bench-budget.patch"));
        let track_dir = temp.path().join("course/tracks/mailer");
        temp.write(
            "course/tracks/mailer/solutions/3.22-bench-budget.patch",
            MODIFIES,
        );

        assert_eq!(
            introduced_sources(&track_dir, &step).expect("the patch parses"),
            ["src/store.rs"],
            "an edit-only step is answerable for the file it edits"
        );
    }

    #[test]
    fn only_the_production_sources_of_a_patch_define_the_module() {
        let (temp, step) = fixture("sources", Some("solutions/3.22-bench-budget.patch"));
        let track_dir = temp.path().join("course/tracks/mailer");
        temp.write(
            "course/tracks/mailer/solutions/3.22-bench-budget.patch",
            PATCH,
        );

        let sources = production_sources(&track_dir, &step).expect("the patch parses");
        assert_eq!(sources, ["src/store.rs", "src/bin/jobqc.rs"]);
    }

    #[test]
    fn a_step_without_a_reference_solution_has_no_module() {
        let (temp, step) = fixture("sources-none", None);
        let track_dir = temp.path().join("course/tracks/mailer");
        let sources = production_sources(&track_dir, &step).expect("no solution is not an error");
        assert!(sources.is_empty(), "{sources:?}");
    }

    #[test]
    fn a_course_file_is_named_after_the_step_and_its_kind() {
        let (temp, step) = fixture("sources-file", None);
        let track_dir = temp.path().join("course/tracks/mailer");
        assert_eq!(
            course_file(&track_dir, &step, "budget", "toml"),
            track_dir.join("budget/3.22-bench-budget.toml")
        );
    }

    #[test]
    fn a_witness_is_named_after_the_step_that_writes_it() {
        assert_eq!(witness_number("3.2-public-api-baseline.rs"), Some((3, 2)));
        assert_eq!(witness_number("3.10-cron-schedules.rs"), Some((3, 10)));
        assert_eq!(witness_number("10.2-later-track.rs"), Some((10, 2)));
        assert_eq!(witness_number("3.2.rs"), Some((3, 2)));

        // Not witnesses: a note, a name that only looks like a number, and a `3.2.1` that is neither.
        assert_eq!(witness_number("README.md"), None);
        assert_eq!(witness_number("api-notes.rs"), None);
        assert_eq!(witness_number("3.2.1-hotfix.rs"), None);
        assert_eq!(witness_number("3.2"), None);
    }

    #[test]
    fn a_step_is_judged_against_the_newest_witness_at_or_before_it() {
        let (temp, _) = fixture("sources-witness", None);
        let track_dir = temp.path().join("course/tracks/mailer");

        // Nothing shipped yet: the caller gets `None` and says what that means (a course hole).
        assert_eq!(newest_witness(&track_dir, "3.4").unwrap(), None);

        for name in [
            "3.2-public-api-baseline.rs",
            "3.6-bumped.rs",
            "3.4-notes.md",
        ] {
            temp.write(&format!("course/tracks/mailer/api/{name}"), "//! witness\n");
        }
        // 3.2 established the baseline and 3.6 bumps it: a step in between takes 3.2, a later step
        // takes 3.6, and the step that bump it takes its own.
        assert!(
            newest_witness(&track_dir, "3.2")
                .unwrap()
                .is_some_and(|path| {
                    path.file_name() == Some(std::ffi::OsStr::new("3.2-public-api-baseline.rs"))
                })
        );
        assert!(
            newest_witness(&track_dir, "3.5")
                .unwrap()
                .is_some_and(|path| {
                    path.file_name() == Some(std::ffi::OsStr::new("3.2-public-api-baseline.rs"))
                })
        );
        assert!(
            newest_witness(&track_dir, "3.6")
                .unwrap()
                .is_some_and(|path| {
                    path.file_name() == Some(std::ffi::OsStr::new("3.6-bumped.rs"))
                })
        );
        assert!(
            newest_witness(&track_dir, "4.1")
                .unwrap()
                .is_some_and(|path| {
                    path.file_name() == Some(std::ffi::OsStr::new("3.6-bumped.rs"))
                })
        );

        // A step before every witness is blocked with a reason rather than judged against nothing.
        assert_eq!(newest_witness(&track_dir, "3.1").unwrap(), None);

        // The `.md` file in the same directory is not a witness, and a step number that is not one
        // is a mistake in the metadata rather than an answer.
        assert!(newest_witness(&track_dir, "3.4").is_ok());
        assert!(newest_witness(&track_dir, "three.four").is_err());
    }

    #[test]
    fn a_production_source_is_a_rust_file_under_src() {
        assert!(is_production_source("src/lib.rs"));
        assert!(is_production_source("src/service/queue.rs"));
        assert!(!is_production_source("tests/spec_spec.rs"));
        assert!(!is_production_source("src/notes.md"));
        assert!(!is_production_source("benches/bench.rs"));
    }
}
