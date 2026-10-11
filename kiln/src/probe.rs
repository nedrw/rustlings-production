//! Probe exercises: the 3–10 minute re-checks behind `kiln review --due` and `kiln plan --fast`.
//!
//! A probe is one file per (capability, depth) slot, and it exists because memory decays on a
//! different schedule than the course does (plan §10, curriculum.md §11): a learner who passed
//! `mailer/1.15-idempotent-delivery` a year ago is not the same as one who can still write the
//! claim/send/record state machine from an empty file.
//!
//! Three properties are deliberate, and each costs something:
//!
//! * **one file, no dependencies.** A probe is copied into a scratch crate under
//!   `.kiln/probes/<slug>/` and run with plain `cargo test`. No crate to download means `kiln probe`
//!   works offline and takes seconds; a capability whose only honest check needs an external crate
//!   gets no probe at all rather than a fake one. What a probe checks is the *shape* of the habit —
//!   the layering order, the state machine, the ownership rule — not a library's API.
//! * **the exercise is the course's, the copy is the learner's.** The course file carries the
//!   `todo!()` bodies and the tests; `kiln probe` writes the first copy and never overwrites work
//!   the learner has edited (the same rule as a scaffold, authoring.md §5).
//! * **passing a probe is what `plan --fast` accepts as proof.** Evidence from a step says "I
//!   followed the scaffold once"; evidence from a probe says "I wrote it again, from an empty
//!   file, months later". Only the second one is allowed to skip a step's implementation.
//!
//! The catalogue is data, not code: `course/probes/<slug>.rs` for the exercise and
//! `course/probes/solutions/<slug>.rs` for the reference. `kiln probe --check` compiles every
//! reference (one scratch crate, one `cargo test`) so a probe that stopped compiling cannot sit
//! here quietly — the course's usual anti-rot rule applied to the review material.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::caps::{CapRequirement, Level};
use crate::error::{KilnError, Result};
use crate::process::{self, Ran};

/// Directory below `course/` that holds the exercises.
pub const PROBES_DIR: &str = "probes";
/// Directory below [`PROBES_DIR`] that holds the reference solutions.
pub const SOLUTIONS_DIR: &str = "solutions";
/// Directory below `.kiln/` where a learner's copies live (personal state, git-ignored).
pub const SCRATCH_DIR: &str = "probes";

/// One probe exercise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// File stem, `<domain>-<name>.<level>` (`delivery-idempotency.apply`).
    slug: String,
    /// Slot this probe re-checks.
    requirement: CapRequirement,
    /// One line a learner can read in a list.
    title: String,
    /// The nudge `kiln probe` prints when the exercise does not pass.
    hint: String,
}

impl Probe {
    /// Short id used in reports and in the scratch path.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// Capability and depth this probe re-checks.
    #[must_use]
    pub fn requirement(&self) -> &CapRequirement {
        &self.requirement
    }

    /// One-line description.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The nudge shown when the exercise fails.
    #[must_use]
    pub fn hint(&self) -> &str {
        &self.hint
    }

    /// The exercise as the course ships it.
    #[must_use]
    pub fn exercise_path(&self, course_dir: &Path) -> PathBuf {
        course_dir
            .join(PROBES_DIR)
            .join(format!("{}.rs", self.slug))
    }

    /// The reference solution.
    #[must_use]
    pub fn solution_path(&self, course_dir: &Path) -> PathBuf {
        course_dir
            .join(PROBES_DIR)
            .join(SOLUTIONS_DIR)
            .join(format!("{}.rs", self.slug))
    }

    /// The learner's copy of the exercise: `.kiln/probes/<slug>/`.
    #[must_use]
    pub fn copy_dir(&self, root: &Path) -> PathBuf {
        root.join(crate::state::STATE_DIR)
            .join(SCRATCH_DIR)
            .join(&self.slug)
    }

    /// Create the learner's copy if it is not there yet, without ever overwriting their work.
    ///
    /// Returns the lines a command should print: what it wrote, or that the copy was already in
    /// place. Idempotent for the same reason a scaffold is (authoring.md §5): `kiln probe` must be
    /// safe to run twice, and running it twice must not throw away an answer somebody is mid-way
    /// through.
    pub fn prepare(&self, course_dir: &Path, root: &Path) -> Result<Vec<String>> {
        let exercise = self.exercise_path(course_dir);
        let text = std::fs::read_to_string(&exercise).map_err(|source| KilnError::Read {
            path: exercise.clone(),
            source,
        })?;
        let dir = self.copy_dir(root);
        let lib = dir.join("src").join("lib.rs");
        let manifest = dir.join("Cargo.toml");

        if lib.is_file() {
            return Ok(vec![format!(
                "your copy is already at {} — it was not touched (`--reset` restores the exercise)",
                relative(root, &lib)
            )]);
        }
        let mut written = Vec::new();
        if !manifest.is_file() {
            write_file(&manifest, &self.scratch_manifest())?;
            written.push(relative(root, &manifest));
        }
        write_file(&lib, &text)?;
        written.push(relative(root, &lib));
        Ok(vec![format!(
            "wrote your copy: {} (edit the file, then run `kiln probe {}` again)",
            written.join(", "),
            self.slug
        )])
    }

    /// Put the course exercise back, discarding the learner's edits.
    pub fn reset(&self, course_dir: &Path, root: &Path) -> Result<PathBuf> {
        let exercise = self.exercise_path(course_dir);
        let text = std::fs::read_to_string(&exercise).map_err(|source| KilnError::Read {
            path: exercise.clone(),
            source,
        })?;
        let dir = self.copy_dir(root);
        let lib = dir.join("src").join("lib.rs");
        if !dir.is_dir() {
            return Err(KilnError::Invalid(format!(
                "there is no copy to reset at {} — `kiln probe {}` writes the first one",
                relative(root, &dir),
                self.slug
            )));
        }
        write_file(&lib, &text)?;
        Ok(lib)
    }

    /// Copy the reference solution over the learner's file.
    pub fn apply_solution(&self, course_dir: &Path, root: &Path) -> Result<PathBuf> {
        let solution = self.solution_path(course_dir);
        let text = std::fs::read_to_string(&solution).map_err(|source| KilnError::Read {
            path: solution.clone(),
            source,
        })?;
        let dir = self.copy_dir(root);
        if !dir.is_dir() {
            return Err(KilnError::Invalid(format!(
                "there is no copy at {} — `kiln probe {}` writes the first one",
                relative(root, &dir),
                self.slug
            )));
        }
        let lib = dir.join("src").join("lib.rs");
        write_file(&lib, &text)?;
        Ok(lib)
    }

    /// Run the learner's copy: `cargo test` in the scratch crate.
    pub fn run(&self, root: &Path, timeout: Duration) -> Result<Ran> {
        let dir = self.copy_dir(root);
        let lib = dir.join("src").join("lib.rs");
        if !lib.is_file() {
            return Err(KilnError::Invalid(format!(
                "nothing to run yet: `kiln probe {}` writes your copy first",
                self.slug
            )));
        }
        process::run("cargo", &["test", "--quiet"], &dir, timeout)
    }

    /// `Cargo.toml` of the learner's copy: no dependencies, its own workspace.
    fn scratch_manifest(&self) -> String {
        format!(
            "# Generated by `kiln probe {slug}`. Your copy: edit `src/lib.rs`, keep this file as is.\n\
             # No dependencies on purpose — a probe runs offline and in seconds (kiln/src/probe.rs).\n\
             \n\
             [package]\n\
             name = \"{name}\"\n\
             version = \"0.0.0\"\n\
             edition = \"2024\"\n\
             \n\
             [dependencies]\n\
             \n\
             [workspace]\n",
            slug = self.slug,
            name = self.slug.replace('.', "-"),
        )
    }
}

impl fmt::Display for Probe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.requirement, self.slug)
    }
}

/// Every probe the course ships, sorted by slug.
///
/// The file name is checked against the header rather than trusted: a probe renamed on disk without
/// its header (or the other way round) is a probe whose catalogue entry and whose file disagree,
/// which is the kind of drift this repository has rules about.
pub fn load(course_dir: &Path) -> Result<Vec<Probe>> {
    let dir = course_dir.join(PROBES_DIR);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|source| KilnError::Read {
            path: dir.clone(),
            source,
        })?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();

    let mut probes = Vec::with_capacity(files.len());
    for path in files {
        probes.push(parse_file(&path)?);
    }
    let mut slugs: Vec<String> = Vec::with_capacity(probes.len());
    for probe in &probes {
        if slugs.contains(&probe.slug) {
            return Err(KilnError::Invalid(format!(
                "two probes claim the slug `{}`",
                probe.slug
            )));
        }
        slugs.push(probe.slug.clone());
    }
    Ok(probes)
}

/// Find one probe by capability (`delivery/idempotency`), slot (`…@apply`) or slug.
pub fn find<'a>(probes: &'a [Probe], reference: &str) -> Result<&'a Probe> {
    let wanted = reference.trim();
    let matches: Vec<&Probe> = probes
        .iter()
        .filter(|probe| {
            probe.slug == wanted
                || probe.requirement.id.as_str() == wanted
                || probe.requirement.to_string() == wanted
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(KilnError::Invalid(format!(
            "no probe for `{wanted}`{} — `kiln probe` lists the ones there are",
            suggestion(probes, wanted)
        ))),
        many => {
            // A capability with several depths and no depth named in the reference: say which ones.
            let slots = many
                .iter()
                .map(|probe| probe.requirement.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let example = many
                .first()
                .map_or_else(String::new, |probe| probe.requirement.to_string());
            Err(KilnError::Invalid(format!(
                "`{wanted}` has {n} probes ({slots}) — name the depth, e.g. `{example}`",
                n = many.len(),
            )))
        }
    }
}

/// Closest slugs, for an error message that does not make the learner guess.
fn suggestion(probes: &[Probe], wanted: &str) -> String {
    let needle = wanted.split_once('/').map_or(wanted, |(name, _)| name);
    let needle = needle
        .trim_end_matches("@apply")
        .trim_end_matches("@design");
    let close: Vec<&str> = probes
        .iter()
        .map(Probe::slug)
        .filter(|slug| slug.contains(needle))
        .take(3)
        .collect();
    if close.is_empty() {
        String::new()
    } else {
        format!(" (did you mean {}?)", close.join(", "))
    }
}

/// Which exercise answers a slot, given the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exercise<'a> {
    /// An exercise at that depth or deeper: passing it proves the slot.
    AtDepth(&'a Probe),
    /// Only a shallower exercise exists: it re-checks the habit, not the depth asked about.
    Shallower(&'a Probe),
    /// The catalogue has nothing for this capability.
    Missing,
}

/// The exercise that re-checks `capability` at `level`, if the catalogue has one.
///
/// The answer is the exercise *closest* to the depth asked about: at or above that depth it is the
/// proof (the shallowest one that suffices), below it the closest a re-check gets (the deepest one
/// there is). Two commands print a line about a slot a learner has not proven — `kiln plan --fast`
/// when it keeps a step, `kiln review --due` when it lists a stale proof — and a line that names a
/// command has to name one that works: `kiln probe <capability>@<level>` exits 2 when nothing in
/// `course/probes/` answers to that slot. Both ask this function, so the answer and the wording
/// cannot drift apart.
#[must_use]
pub fn exercise<'a>(probes: &'a [Probe], capability: &str, level: Level) -> Exercise<'a> {
    let mut at_depth: Option<&Probe> = None;
    let mut shallower: Option<&Probe> = None;
    for probe in probes
        .iter()
        .filter(|probe| probe.requirement().id.as_str() == capability)
    {
        if probe.requirement().level.rank() >= level.rank() {
            if at_depth.is_none_or(|best| closer(probe, best, level)) {
                at_depth = Some(probe);
            }
        } else if shallower.is_none_or(|best| closer(probe, best, level)) {
            shallower = Some(probe);
        }
    }
    match (at_depth, shallower) {
        (Some(probe), _) => Exercise::AtDepth(probe),
        (None, Some(probe)) => Exercise::Shallower(probe),
        (None, None) => Exercise::Missing,
    }
}

/// Whether `candidate` answers `level` better than `best`: nearer in depth, then smaller slug.
///
/// The slug is the tie-break so that the answer never depends on the order `load` happened to read
/// the catalogue in — a report may not change between two runs over the same files.
fn closer(candidate: &Probe, best: &Probe, level: Level) -> bool {
    let distance = |probe: &Probe| probe.requirement().level.rank().abs_diff(level.rank());
    (distance(candidate), candidate.slug()) < (distance(best), best.slug())
}

/// Parse one exercise file: header, then the checks the catalogue depends on.
fn parse_file(path: &Path) -> Result<Probe> {
    let text = std::fs::read_to_string(path).map_err(|source| KilnError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut probe = None;
    let mut title = None;
    let mut hint = None;
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("//!") else {
            if line.trim().is_empty() {
                continue;
            }
            break;
        };
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "probe" => probe = Some(value.to_owned()),
            "title" => title = Some(value.to_owned()),
            "hint" => hint = Some(value.to_owned()),
            _ => {}
        }
    }

    let missing = |fields: &[&str]| {
        KilnError::Invalid(format!(
            "{}: the probe header needs {}",
            path.display(),
            fields
                .iter()
                .map(|field| format!("`//! {field} = \"…\"`"))
                .collect::<Vec<_>>()
                .join(" and ")
        ))
    };
    let mut absent: Vec<&str> = Vec::new();
    if probe.is_none() {
        absent.push("probe");
    }
    if title.is_none() {
        absent.push("title");
    }
    if hint.is_none() {
        absent.push("hint");
    }
    if !absent.is_empty() {
        return Err(missing(&absent));
    }
    let requirement: CapRequirement = probe.unwrap_or_default().parse()?;
    let title = title.unwrap_or_default();
    let hint = hint.unwrap_or_default();

    let slug = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let expected = expected_slug(&requirement);
    if slug != expected {
        return Err(KilnError::Invalid(format!(
            "{}: header says {requirement} but the file name should be `{expected}.rs`",
            path.display()
        )));
    }
    if requirement.level == Level::Aware {
        return Err(KilnError::Invalid(format!(
            "{}: `aware` slots get no probe (curriculum.md §11) — they are re-used by later steps, \
             so a three-minute quiz would re-teach rather than re-check",
            path.display()
        )));
    }

    Ok(Probe {
        slug,
        requirement,
        title,
        hint,
    })
}

/// File stem a probe for this slot must use.
#[must_use]
pub fn expected_slug(requirement: &CapRequirement) -> String {
    format!(
        "{}.{}",
        requirement.id.as_str().replace('/', "-"),
        requirement.level
    )
}

/// File stem one probe's test target gets in the throwaway crate of [`verify_references`].
///
/// Cargo names a test target after the file it is discovered from, and that name has to be a valid
/// crate name — a probe slug carries its depth after a dot (`delivery-idempotency.apply`) and rustc
/// refuses one (`error: invalid character '.' in crate name`). The learner's copy of the same
/// exercise carries the same normalisation in [`Probe::scratch_manifest`], which replaces the dot
/// with `-` because there the slug becomes a package name; this half is a file, so it is `_`.
#[must_use]
fn test_target_stem(slug: &str) -> String {
    slug.replace('.', "_")
}

/// What `kiln probe --check` found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// Probes whose reference solution ran and passed.
    pub passed: Vec<String>,
    /// Probes whose reference solution failed, with the reason.
    pub failed: Vec<(String, String)>,
}

impl CheckReport {
    /// `true` when every reference passed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Compile and run every reference solution in one crate.
///
/// One `cargo test` for the whole catalogue: the probes are dependency-free single files, so the
/// expensive part of this check is cargo itself — and the *first* run, because every reference is
/// compiled here as its own test target. `scratch` is therefore worth keeping: a file is written
/// only when its bytes change, and a target the catalogue no longer has is removed, so the run
/// after this one compiles what changed and nothing else. A fresh directory per run pays for the
/// whole catalogue every time, and a catalogue of twenty-four more references is what made this
/// check outgrow its own deadline. `course_dir` is the course the probes live in.
///
/// # Errors
///
/// [`KilnError::Write`] when the scratch crate cannot be written, and whatever [`process::run`]
/// reports when `cargo` cannot be started at all.
pub fn verify_references(
    probes: &[Probe],
    course_dir: &Path,
    scratch: &Path,
    timeout: Duration,
) -> Result<CheckReport> {
    if probes.is_empty() {
        return Ok(CheckReport::default());
    }
    std::fs::create_dir_all(scratch.join("src"))
        .and_then(|()| std::fs::write(scratch.join("src/lib.rs"), "//! probe references\n"))
        .map_err(|source| KilnError::Write {
            path: scratch.to_path_buf(),
            source,
        })?;
    write_if_changed(&scratch.join("Cargo.toml"), CHECK_MANIFEST)?;
    let tests = scratch.join("tests");
    std::fs::create_dir_all(&tests).map_err(|source| KilnError::Write {
        path: tests.clone(),
        source,
    })?;

    let mut missing = Vec::new();
    let mut wanted = Vec::with_capacity(probes.len());
    for probe in probes {
        let solution = probe.solution_path(course_dir);
        let Ok(text) = std::fs::read_to_string(&solution) else {
            missing.push((
                probe.slug.clone(),
                format!(
                    "no reference solution at {}",
                    relative(course_dir, &solution)
                ),
            ));
            continue;
        };
        // The reference is a `lib.rs`; as its own test target it gets the same `cfg(test)` compile
        // it would have in the learner's crate, which is where its `#[cfg(test)] mod tests` lives.
        let stem = test_target_stem(&probe.slug);
        write_if_changed(&tests.join(format!("{stem}.rs")), &text)?;
        wanted.push(stem);
    }
    // A target a catalogue that has moved on left behind would be compiled and judged as if it
    // were still a probe: a renamed or deleted exercise would keep its old reference running here,
    // and a red one would be a failure [`report_of`] cannot name, because it only knows the probes
    // that are here now.
    prune_targets(&tests, &wanted)?;
    if !missing.is_empty() {
        return Ok(CheckReport {
            passed: Vec::new(),
            failed: missing,
        });
    }

    let ran = process::run("cargo", &["test", "--quiet"], scratch, timeout)?;
    Ok(report_of(probes, &ran, timeout))
}

/// The report one `cargo test` run is, given the catalogue it ran.
///
/// A run the deadline killed is not a report about probes at all: libtest never reached its
/// per-target lines, so the names in the truncated output belong to whatever was being compiled
/// when the kill landed — and this check used to print one of them, which sends a learner to a
/// probe that is fine. A first run of a large catalogue is exactly that case, so the message names
/// the deadline and the size instead of a slug.
fn report_of(probes: &[Probe], ran: &Ran, timeout: Duration) -> CheckReport {
    if ran.success() {
        return CheckReport {
            passed: probes.iter().map(|probe| probe.slug.clone()).collect(),
            failed: Vec::new(),
        };
    }
    if ran.timed_out {
        let seconds = timeout.as_secs();
        let count = probes.len();
        return CheckReport {
            passed: Vec::new(),
            failed: vec![(
                "probe --check".to_owned(),
                format!(
                    "killed after {seconds}s: the catalogue is {count} reference(s), each compiled \
                     as its own test target, and a first run pays for every one of them — raise \
                     `--timeout` and run it again, because the build directory is kept and the run \
                     after this one compiles only what changed. This says nothing about any single \
                     probe"
                ),
            )],
        };
    }
    // One failing reference makes the whole run red, so the report names the ones that are red
    // rather than guessing: libtest prints one line per failing test target.
    let output = format!("{}\n{}", ran.stdout, ran.stderr);
    let failed = probes
        .iter()
        .filter(|probe| output.contains(&format!("tests/{}.rs", test_target_stem(&probe.slug))))
        .map(|probe| {
            (
                probe.slug.clone(),
                "the reference solution did not pass".to_owned(),
            )
        })
        .collect::<Vec<_>>();
    if failed.is_empty() {
        return CheckReport {
            passed: Vec::new(),
            failed: vec![(
                "cargo test".to_owned(),
                first_lines(&output, 8).unwrap_or_else(|| "no output".to_owned()),
            )],
        };
    }
    CheckReport {
        passed: Vec::new(),
        failed,
    }
}

/// Write `text` to `path` unless the file already holds exactly those bytes.
///
/// Cargo decides what to rebuild from the files' modification times, so writing a reference with the
/// bytes it already had would compile the whole catalogue again on every run — which is the
/// difference between a check that is cheap enough to run after every change and one that is not.
/// The answer is whether it wrote, which is what the test asserts on.
fn write_if_changed(path: &Path, text: &str) -> Result<bool> {
    if std::fs::read(path).is_ok_and(|found| found == text.as_bytes()) {
        return Ok(false);
    }
    write_file(path, text)?;
    Ok(true)
}

/// Remove every `tests/*.rs` that is not one of `wanted`, and nothing else.
fn prune_targets(tests: &Path, wanted: &[String]) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(tests) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_target = path.extension().is_some_and(|ext| ext == "rs");
        let kept = path
            .file_stem()
            .is_some_and(|stem| wanted.iter().any(|keep| keep == &stem.to_string_lossy()));
        if !is_target || kept {
            continue;
        }
        std::fs::remove_file(&path).map_err(|source| KilnError::Write {
            path: path.clone(),
            source,
        })?;
    }
    Ok(())
}

/// `Cargo.toml` of the scratch crate that compiles every reference.
const CHECK_MANIFEST: &str = "\
# Generated by `kiln probe --check`. Throwaway: the references are compiled, not edited.
[package]
name = \"kiln-probe-check\"
version = \"0.0.0\"
edition = \"2024\"

[dependencies]

[workspace]
";

fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, text).map_err(|source| KilnError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn first_lines(text: &str, count: usize) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(
        lines
            .iter()
            .take(count)
            .copied()
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// A probe exercise with the header a real one carries.
    const EXERCISE: &str = "\
//! probe = \"delivery/idempotency@apply\"
//! title = \"Claim, send, record\"
//! hint = \"Record the attempt before you send.\"
//!
//! Prose a learner reads goes here.

pub fn claim() -> u32 {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {}
}
";

    fn course_with(slug: &str, body: &str) -> TempDir {
        let temp = TempDir::new("probe-load");
        temp.write(&format!("course/probes/{slug}.rs"), body);
        temp
    }

    #[test]
    fn a_probe_file_carries_the_slot_it_re_checks() {
        let temp = course_with("delivery-idempotency.apply", EXERCISE);
        let probes = load(&temp.path().join("course")).expect("the catalogue should load");
        assert_eq!(probes.len(), 1);
        let probe = &probes[0];
        assert_eq!(probe.slug(), "delivery-idempotency.apply");
        assert_eq!(
            probe.requirement().to_string(),
            "delivery/idempotency@apply"
        );
        assert_eq!(probe.title(), "Claim, send, record");
        assert!(probe.hint().starts_with("Record the attempt"));
    }

    #[test]
    fn a_probe_is_found_by_capability_slot_or_slug_and_ambiguity_is_named() {
        let temp = TempDir::new("probe-find");
        temp.write("course/probes/delivery-idempotency.apply.rs", EXERCISE);
        temp.write(
            "course/probes/delivery-idempotency.design.rs",
            &EXERCISE.replace(
                "probe = \"delivery/idempotency@apply\"",
                "probe = \"delivery/idempotency@design\"",
            ),
        );
        let course = temp.path().join("course");
        let probes = load(&course).expect("the catalogue should load");

        let by_slot = find(&probes, "delivery/idempotency@apply").expect("by slot");
        assert_eq!(by_slot.slug(), "delivery-idempotency.apply");
        assert_eq!(
            find(&probes, "delivery-idempotency.design")
                .expect("by slug")
                .requirement()
                .level,
            Level::Design
        );

        let ambiguous = find(&probes, "delivery/idempotency").unwrap_err();
        assert!(
            ambiguous.to_string().contains("name the depth"),
            "{ambiguous}"
        );

        let unknown = find(&probes, "sec/hmac-signing@apply").unwrap_err();
        assert!(unknown.to_string().contains("no probe for"), "{unknown}");
    }

    #[test]
    fn a_slot_is_answered_by_the_closest_exercise_or_says_there_is_none() {
        let temp = TempDir::new("probe-exercise");
        temp.write("course/probes/delivery-idempotency.apply.rs", EXERCISE);
        temp.write(
            "course/probes/delivery-idempotency.design.rs",
            &EXERCISE.replace(
                "probe = \"delivery/idempotency@apply\"",
                "probe = \"delivery/idempotency@design\"",
            ),
        );
        let probes = load(&temp.path().join("course")).expect("the catalogue should load");

        // At and above the depth asked about, the exercise is the proof.
        for level in [Level::Apply, Level::Design] {
            let Exercise::AtDepth(probe) = exercise(&probes, "delivery/idempotency", level) else {
                panic!("an exercise at {level} re-checks the slot itself");
            };
            assert_eq!(probe.requirement().level, level);
        }

        // Deeper than anything in the catalogue: the deepest shallow one is worth naming as itself,
        // which is what keeps a report from sending a learner to a command that would fail.
        let Exercise::Shallower(probe) = exercise(&probes, "delivery/idempotency", Level::Operate)
        else {
            panic!("only shallower exercises exist");
        };
        assert_eq!(probe.requirement().level, Level::Design);

        assert_eq!(
            exercise(&probes, "sec/hmac-signing", Level::Apply),
            Exercise::Missing
        );
    }

    #[test]
    fn a_probe_whose_file_name_disagrees_with_its_header_is_rejected() {
        let temp = course_with("delivery-idempotency.design", EXERCISE);
        let error = load(&temp.path().join("course")).unwrap_err();
        assert!(
            error.to_string().contains("delivery-idempotency.apply.rs"),
            "{error}"
        );
    }

    #[test]
    fn an_aware_slot_gets_no_probe() {
        let temp = course_with(
            "http-rate-limit.aware",
            &EXERCISE.replace("@apply", "@aware"),
        );
        let error = load(&temp.path().join("course")).unwrap_err();
        assert!(error.to_string().contains("aware"), "{error}");
    }

    #[test]
    fn a_header_missing_a_field_says_which_one() {
        let temp = course_with(
            "delivery-idempotency.apply",
            "//! probe = \"delivery/idempotency@apply\"\n\npub fn x() {}\n",
        );
        let error = load(&temp.path().join("course")).unwrap_err();
        assert!(error.to_string().contains("title"), "{error}");
        assert!(error.to_string().contains("hint"), "{error}");
    }

    #[test]
    fn a_copy_is_written_once_and_never_overwritten() {
        let temp = course_with("delivery-idempotency.apply", EXERCISE);
        let course = temp.path().join("course");
        let probes = load(&course).unwrap();
        let probe = &probes[0];

        let first = probe.prepare(&course, temp.path()).expect("first copy");
        assert!(first[0].contains("wrote your copy"), "{first:?}");
        let lib = probe.copy_dir(temp.path()).join("src/lib.rs");
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), EXERCISE);

        // The learner edits it; a second `kiln probe` must keep those edits.
        std::fs::write(&lib, "// my answer\n").unwrap();
        let second = probe.prepare(&course, temp.path()).expect("second copy");
        assert!(second[0].contains("already at"), "{second:?}");
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), "// my answer\n");

        // And `--reset` is the way back to the exercise.
        probe.reset(&course, temp.path()).expect("reset");
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), EXERCISE);
    }

    #[test]
    fn a_check_target_name_is_a_crate_name_cargo_accepts() {
        // Every probe slug carries its depth after a dot (`delivery-idempotency.apply`), and cargo
        // discovers a test target by the name of its file: without this, `probe --check` answered
        // `error: invalid character '.' in crate name` for every reference in the catalogue.
        let stem = test_target_stem("delivery-idempotency.apply");
        assert_eq!(stem, "delivery-idempotency_apply");
        assert!(!stem.contains('.'), "{stem}");
        assert!(
            stem.chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character)),
            "a test target's name has to be a valid crate name: {stem}"
        );
    }

    #[test]
    fn a_missing_reference_is_reported_without_starting_cargo() {
        let temp = course_with("delivery-idempotency.apply", EXERCISE);
        let course = temp.path().join("course");
        let probes = load(&course).unwrap();
        let scratch = TempDir::new("probe-check-missing");
        let report = verify_references(
            &probes,
            &course,
            scratch.path(),
            std::time::Duration::from_secs(5),
        )
        .expect("the report should be produced");
        assert!(!report.is_clean());
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].1.contains("no reference solution"));
    }

    #[test]
    fn a_reference_whose_bytes_did_not_change_is_not_written_again() {
        // Cargo rebuilds what carries a new modification time, so rewriting the same bytes would
        // compile the whole catalogue on every run — which is what the check used to do, with a
        // fresh scratch directory each time.
        let temp = TempDir::new("probe-write-if-changed");
        let path = temp.path().join("tests/delivery-idempotency_apply.rs");
        assert!(
            write_if_changed(&path, "// one\n").unwrap(),
            "the first write happens"
        );
        assert!(
            !write_if_changed(&path, "// one\n").unwrap(),
            "the same bytes are not written again"
        );
        assert!(
            write_if_changed(&path, "// two\n").unwrap(),
            "a changed reference is"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "// two\n");
    }

    #[test]
    fn a_target_the_catalogue_no_longer_has_is_removed() {
        let temp = TempDir::new("probe-prune");
        let tests = temp.path().join("tests");
        std::fs::create_dir_all(&tests).unwrap();
        std::fs::write(tests.join("kept_apply.rs"), "// kept\n").unwrap();
        std::fs::write(tests.join("renamed_apply.rs"), "// stale\n").unwrap();
        std::fs::write(tests.join("notes.md"), "not a target\n").unwrap();

        prune_targets(&tests, &["kept_apply".to_owned()]).unwrap();

        assert!(
            tests.join("kept_apply.rs").is_file(),
            "a target this catalogue has stays"
        );
        assert!(
            !tests.join("renamed_apply.rs").exists(),
            "one the catalogue dropped goes: a stale reference would be judged as a probe"
        );
        assert!(
            tests.join("notes.md").is_file(),
            "and nothing that is not a test target is touched"
        );
    }

    #[test]
    fn a_killed_check_is_reported_as_a_deadline_and_names_no_probe() {
        // The failure this pins: a killed run printed the first probe whose path happened to be in
        // the truncated output, so the report sent a learner to an exercise that was fine.
        let temp = course_with("delivery-idempotency.apply", EXERCISE);
        let course = temp.path().join("course");
        let probes = load(&course).unwrap();
        let killed = Ran {
            code: None,
            stdout: "Compiling tests/delivery-idempotency_apply.rs\n".to_owned(),
            stderr: String::new(),
            timed_out: true,
            elapsed: Duration::from_secs(300),
        };

        let report = report_of(&probes, &killed, Duration::from_secs(300));

        assert!(!report.is_clean());
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert_eq!(report.failed[0].0, "probe --check");
        assert!(
            report.failed[0].1.contains("killed after 300s"),
            "the deadline is the subject: {}",
            report.failed[0].1
        );
        assert!(
            !report.failed[0].1.contains("delivery-idempotency"),
            "and no probe that was merely compiling is named: {}",
            report.failed[0].1
        );
    }

    #[test]
    fn a_failing_reference_is_named_from_what_cargo_printed() {
        let temp = course_with("delivery-idempotency.apply", EXERCISE);
        let course = temp.path().join("course");
        let probes = load(&course).unwrap();
        let failed = Ran {
            code: Some(101),
            stdout: String::new(),
            stderr: "error: test failed, to rerun pass `--test delivery-idempotency_apply`\n  \
                     --> tests/delivery-idempotency_apply.rs:1:1"
                .to_owned(),
            timed_out: false,
            elapsed: Duration::from_secs(2),
        };

        let report = report_of(&probes, &failed, Duration::from_secs(300));

        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert_eq!(report.failed[0].0, "delivery-idempotency.apply");
        assert!(report.passed.is_empty());
    }
}
