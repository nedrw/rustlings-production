//! `coverage@N` — the step's own module is exercised by its own suite (handoff M2 §1).
//!
//! This is the one slow gate that is a *should*: measuring line coverage needs `llvm-profdata` and
//! `llvm-cov`, which come from a rustup component (`llvm-tools-preview`) rather than from the
//! toolchain baseline, so a machine without them reports [`GateOutcome::blocked`] with the command
//! that installs them (plan §10). A missing tool never fails a step.
//!
//! What it measures is deliberately narrow: the `.rs` files under `src/` that this step's
//! reference-solution patch touches ([`sources::production_sources`]), not the whole crate. A single
//! percentage over a whole library says nothing about the step — the step is one module, and that is
//! the module whose error branches and `match` arms the specs were supposed to reach.
//!
//! The run: the step's own `run.cmd` under `RUSTFLAGS=-C instrument-coverage`, with
//! `LLVM_PROFILE_FILE` pointing into a scratch directory; then `llvm-profdata merge -sparse`, then
//! `llvm-cov export --summary-only` over the test binaries cargo produced. Both the profiles and the merged
//! data are removed with the scratch directory when the gate ends. The instrumented objects do *not*
//! live in the ordinary build directory — a different `RUSTFLAGS` is a different build, and putting it
//! there would make every ordinary build that follows recompile the project (measured: 22 s cold on the
//! machine this was written on). They get a cache directory of their own, which also means the gate's
//! second run is incremental.
//!
//! The run happens in a **copy** of the project, stamped fresh ([`sandbox::copy_project`]), and that is
//! a correctness rule rather than tidiness: cargo keys a build slot by package *name and version* rather
//! than by the directory a package lives in, so a binary in the shared cache can be one an earlier
//! sandbox built from a tree this one has never had — and the profile it writes describes *that* tree.
//! Measured (handoff M2 §5.2): `jobq/3.22-bench-budget` answered `blocked` — "the profile has no
//! executable line for the step's module (src/engine/budget.rs …)" — because its module is a file this
//! step's own patch *creates*, and the binary that ran came from a tree without it. In a stamped copy the
//! artifact can only be this run's, because every source in it is newer than anything built before it;
//! only the copy is stamped, so the dependencies it uses are still cached and a run costs one package
//! rebuild (handoff M2 §1.3). The build directory stays the gate's own, so the ordinary build the
//! learner's next command needs is untouched either way.
//!
//! Facts about the machine are reported as blocks, not failures: no tools, no profile data, no test
//! binary, an unreadable export, or an instrumented spec run that did not pass would each mean a
//! number that describes something other than the learner's code.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::error::{KilnError, Result};
use crate::gates::runner::{GateContext, output_detail};
use crate::gates::{GateOutcome, GateSpec, sandbox, sources};
use crate::process;
use crate::scratch;

/// What a machine without the LLVM tools is told, install line included.
pub const MISSING: &str = "llvm-profdata/llvm-cov are not in the active toolchain, so line coverage \
                            cannot be measured — `rustup component add llvm-tools-preview`, then \
                            re-run this step";

/// The suffix the profile files of a run carry.
const PROFILE: &str = ".profraw";

/// Sources the export leaves out: everything outside the tree being measured.
///
/// A dependency's line counts say nothing about the step's module, and the document that carries
/// them is big enough to hit the runner's capture cap — where the answer stops being "not this
/// step" and starts being "not readable".
const IGNORED_SOURCES: &str = r"(\.cargo/registry|/rustlib/|/rustup/toolchains/)";

/// The two binaries a coverage run needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    /// `llvm-profdata`, which merges what every process of the run wrote.
    pub profdata: PathBuf,
    /// `llvm-cov`, which turns a merged profile plus the binaries into per-file counts.
    pub cov: PathBuf,
}

/// Find the LLVM tools of the toolchain that compiles `project`, when this machine has them.
///
/// The two `rustc` questions are asked **in the project's directory**, because that is where the
/// toolchain pin applies: a project with a `rust-toolchain.toml` compiles with the toolchain that
/// file names, and asking the same questions in a scratch directory answers about whatever the
/// machine made default instead. That is not a detail — it is the difference between measuring the
/// profile this run writes and reading it with another LLVM.
///
/// `rustc --print sysroot` and `rustc -vV` answer where the tools are: `rustup component add
/// llvm-tools-preview` installs them under `lib/rustlib/<host>/bin` of the sysroot whose rustc is
/// compiling this run. Looking on `PATH` instead would happily find a system clang of a different
/// version, and a profile written by one LLVM read by another is a measurement nobody should trust.
pub fn tools(project: &Path, timeout: Duration) -> Option<Tools> {
    let sysroot = process::run("rustc", &["--print", "sysroot"], project, timeout).ok()?;
    let verbose = process::run("rustc", &["-vV"], project, timeout).ok()?;
    tool_paths(&sysroot.stdout, &verbose.stdout)
}

/// The two binaries, out of the two answers.
fn tool_paths(sysroot: &str, verbose: &str) -> Option<Tools> {
    let host = verbose
        .lines()
        .find_map(|line| line.strip_prefix("host: "))?
        .trim();
    let bin = Path::new(sysroot.trim())
        .join("lib")
        .join("rustlib")
        .join(host)
        .join("bin");
    let profdata = bin.join("llvm-profdata");
    let cov = bin.join("llvm-cov");
    (profdata.is_file() && cov.is_file()).then_some(Tools { profdata, cov })
}

/// Measure line coverage of the step's module.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    let required = spec.numeric_param()?;
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };
    let track_dir = ctx.track.dir(ctx.course.root());
    let module = sources::production_sources(&track_dir, ctx.step)?;
    if module.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "{}'s reference solution touches no production source under `src/`, so there is no module \
             to measure",
            ctx.step.qualified_id()
        )));
    }
    let Some(run) = ctx.step.run.as_ref() else {
        return Ok(GateOutcome::blocked(format!(
            "{} declares no `run.cmd`, so there is no suite whose coverage could be measured",
            ctx.step.qualified_id()
        )));
    };
    // The tools first: on a machine without them nothing else matters, and the answer has to be the
    // install line rather than a run nobody can measure.
    let Some(tools) = tools(project, ctx.probe_timeout()) else {
        return Ok(GateOutcome::blocked(MISSING));
    };

    let scratch = scratch::directory("coverage")?;
    // The tree that is built, measured and reported on is a copy of the project, never the project
    // itself: see the module docs and [`sandbox::copy_project`]. The copy is what every path below is
    // answered against — the report's numbers, and the files the export has to name.
    let measured = scratch.path().join("project");
    sandbox::copy_project(project, &measured)?;
    let profiles = scratch.path().join("profiles");
    std::fs::create_dir_all(&profiles).map_err(|source| KilnError::Write {
        path: profiles.clone(),
        source,
    })?;
    // An instrumented build gets a build directory of its own, and it *keeps* it (`scratch::cache`
    // rather than the per-run scratch): a different `RUSTFLAGS` is a different build, so sharing the
    // ordinary directory would throw away every artifact the learner's next command needs (measured:
    // a cold rebuild of this project is 22 s, and the ordinary build would pay it again). Keeping it
    // also makes the gate's own second run incremental — the copy's stamps rebuild the *package*, and
    // the dependencies, which are not in the copied tree, stay as cargo left them.
    let target = scratch::cache("coverage-target")?;
    let environment = [
        ("RUSTFLAGS".to_owned(), instrumented()),
        ("CARGO_TARGET_DIR".to_owned(), target.display().to_string()),
        (
            "LLVM_PROFILE_FILE".to_owned(),
            profiles
                .join("kiln-%p-%m.profraw")
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    // The instant before the run: everything cargo links from here on is this copy's, and everything
    // older in the shared directory belongs to a run that came before (see [`executables`]).
    let started = SystemTime::now();
    let ran = process::run_line_with_env(&run.cmd, &measured, ctx.timeout, &environment)?;
    if !ran.success() {
        return Ok(GateOutcome::blocked(format!(
            "the spec run under `-C instrument-coverage` did not pass, so any coverage number would \
             describe a broken run:\n    {}",
            output_detail(&ran)
        )));
    }

    let raw = profiles_written(&profiles)?;
    if raw.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "the instrumented run wrote no `{PROFILE}` data, so there is nothing to measure — the \
             suite may not have started a single instrumented binary"
        )));
    }
    let merged = scratch.path().join("kiln.profdata");
    let mut merge_args: Vec<String> = vec!["merge".to_owned(), "-sparse".to_owned()];
    merge_args.extend(raw.iter().map(|path| lossy(path)));
    merge_args.push("-o".to_owned());
    merge_args.push(lossy(&merged));
    let merged_run = process::run(
        &lossy(&tools.profdata),
        &borrowed(&merge_args),
        &measured,
        ctx.timeout,
    )?;
    if !merged_run.success() {
        return Ok(GateOutcome::blocked(format!(
            "`llvm-profdata merge` could not read the profile of this run:\n    {}",
            output_detail(&merged_run)
        )));
    }

    let deps = target.join("debug").join("deps");
    let objects = executables(&deps, started);
    if objects.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "no test binary under {} was linked by this run — profile data can only be read back 
             against the binaries it came from, and this run left none",
            deps.display()
        )));
    }
    // Every executable this run left, at once: the objects carry one coverage mapping each, and
    // `llvm-cov` reads them together with the merged profile.
    let mut export_args: Vec<String> = vec![
        "export".to_owned(),
        "--summary-only".to_owned(),
        // Only this tree's own files. The document otherwise carries every dependency's sources and
        // the toolchain's, which is both noise the gate filters out anyway and — measured — more than
        // kiln's own 256 KiB capture cap: a truncated document is an export this gate cannot read at
        // all, and the flag is the difference between a measurement and a parse error.
        "--ignore-filename-regex".to_owned(),
        IGNORED_SOURCES.to_owned(),
        "--instr-profile".to_owned(),
        lossy(&merged),
    ];
    export_args.extend(objects.iter().map(|object| lossy(object)));
    let exported = process::run(
        &lossy(&tools.cov),
        &borrowed(&export_args),
        &measured,
        ctx.timeout,
    )?;
    if !exported.success() {
        return Ok(GateOutcome::blocked(format!(
            "`llvm-cov export --summary-only` could not read this run's profile:\n    {}",
            output_detail(&exported)
        )));
    }

    let coverage = summary(&exported.stdout, &measured, &module)?;
    Ok(judge(required, &coverage, &module, &measured))
}

/// The `RUSTFLAGS` the instrumented run uses: the project's own, plus instrumentation.
///
/// Appending rather than replacing keeps a project's own flags (a `--cfg`, a target feature) in the
/// build; replacing them would measure a build the project does not have.
fn instrumented() -> String {
    let existing = std::env::var("RUSTFLAGS").unwrap_or_default();
    format!("{} -C instrument-coverage", existing.trim())
        .trim()
        .to_owned()
}

/// Every profile file a run wrote, in path order.
fn profiles_written(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(directory).map_err(|source| KilnError::Read {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| KilnError::Read {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "profraw")
        {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// Every runnable file of a target directory that `since` did not come after, in path order.
///
/// Test binaries live in `target/debug/deps` with a hash in the name, and they are the executables
/// there: the `.d` files beside them are not, so the executable bit is what tells the two apart.
///
/// They are also not all of *this* run. cargo keys an artifact by package name and version, so a
/// shared build directory — and this gate's is shared by every step that declares it — keeps the
/// test binaries of every run that ever used it, each one carrying the coverage mapping of the tree
/// it was built from. Handing all of them to `llvm-cov` does not merely cost time: it pairs this
/// run's profile with an object of another tree and answers with *that* tree's files (measured: the
/// export named a copy of a previous run, every line of it uncovered, which the guard below then
/// turned into a `blocked` for a step whose work had been measured perfectly well). The run builds
/// in a copy whose every source is stamped ahead of the clock, so cargo links the test binary again
/// — and a file it links during the run is newer than the instant before the run, which is the test
/// for "this run's": what is older is what an earlier run left behind.
fn executables(directory: &Path, since: SystemTime) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| process::is_executable(path))
        .filter(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified >= since)
        })
        .collect();
    found.sort();
    found
}

/// One line coverage number over the step's module.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Coverage {
    /// Executable lines in the module's files.
    lines: u64,
    /// Lines the run executed.
    covered: u64,
    /// The module's files the export had data for.
    files: Vec<String>,
    /// Module line(s) the export named *outside* the copy this run measured.
    ///
    /// This used to be a guard against a number from another sandbox (cargo keys a build slot by
    /// package name and version, so a tree that had built before this one could answer here). The
    /// run now builds in a copy of this step's own tree, stamped fresh, so there is no other tree
    /// left to answer; what is counted here is a fault rather than a foreign measurement — a path
    /// that ends with the module's name and does not live in the copy, which means the profile came
    /// from something other than this run.
    foreign: u64,
}

/// What an `llvm-cov export --summary-only` document carries.
#[derive(Debug, Deserialize)]
struct Export {
    /// One entry per object file handed to `llvm-cov`.
    #[serde(default)]
    data: Vec<Data>,
}

/// One object's share of the export.
#[derive(Debug, Deserialize)]
struct Data {
    /// The files that object has coverage mappings for.
    #[serde(default)]
    files: Vec<FileCoverage>,
}

/// One file of the export.
#[derive(Debug, Deserialize)]
struct FileCoverage {
    /// Absolute path, as the compiler saw it.
    filename: String,
    /// Its counts.
    summary: Summary,
}

/// The summary of one file.
#[derive(Debug, Deserialize)]
struct Summary {
    /// Line coverage.
    lines: Lines,
}

/// Line counts.
#[derive(Debug, Deserialize)]
struct Lines {
    /// Lines that could be executed.
    count: u64,
    /// Lines that were.
    covered: u64,
}

/// Line coverage of the step's module, out of an export document.
///
/// `measured` is the root the profile's files have to live under — the copy this run built in — and
/// `module` holds the step's files as paths relative to it.
fn summary(document: &str, measured: &Path, module: &[String]) -> Result<Coverage> {
    let export: Export = serde_json::from_str(document).map_err(|source| {
        KilnError::Invalid(format!(
            "`llvm-cov export --summary-only` answered with something this build cannot read: {source}"
        ))
    })?;
    let mut coverage = Coverage::default();
    let root = resolved(measured);
    for file in export.data.iter().flat_map(|data| &data.files) {
        // The export answers with the path the compiler used, which is absolute; joining keeps a
        // relative one working too, because `join` replaces the path when it is already absolute.
        let absolute = measured.join(&file.filename);
        let Some(relative) = module
            .iter()
            .find(|relative| absolute.ends_with(relative.as_str()))
        else {
            continue;
        };
        // A path that ends with the module's name but does not live in the copy this run built in is
        // a build of something else: counting it would report another tree's lines as this step's.
        if !resolved(&absolute).starts_with(&root) {
            coverage.foreign += 1;
            continue;
        }
        coverage.lines += file.summary.lines.count;
        coverage.covered += file.summary.lines.covered;
        if !coverage.files.iter().any(|seen| seen == relative) {
            coverage.files.push(relative.clone());
        }
    }
    Ok(coverage)
}

/// A path with its symlinks resolved, or as written when it does not resolve at all.
///
/// A text prefix is not identity, and this is not a hypothetical: on a machine whose temporary
/// directory is reached through a symlink the compiler records the *resolved* path, so a run in
/// `/var/folders/…` answers with files named `/private/var/folders/…` — the same file, and a prefix
/// test calls it another tree (measured: it did, for the gate's own copy, which is why the integration
/// test used to have to accept `blocked` as an answer). A path that does not resolve is not a file of
/// the copy — an artifact of a tree that is gone answers nothing — so it keeps the form it came in,
/// which the prefix test then refuses.
fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The gate's answer, from what the export says about the module.
fn judge(required: u32, coverage: &Coverage, module: &[String], measured: &Path) -> GateOutcome {
    if coverage.lines == 0 && coverage.foreign > 0 {
        return GateOutcome::blocked(format!(
            "the profile names {} line(s) of this step's module outside the copy this run measured \
             ({}), so the instrumented build was not this run's: every file of that copy is stamped \
             newer than any build before it, which is what keeps an artifact of another tree from \
             answering here. A module reached through a symlink, or named by an absolute path, is \
             the shape of that fault — the module is `{}`",
            coverage.foreign,
            measured.display(),
            module.join(", ")
        ));
    }
    if coverage.lines == 0 {
        return GateOutcome::blocked(format!(
            "the profile has no executable line for the step's module ({}), so coverage cannot be \
             measured — the spec run's binaries do not carry this module's coverage data",
            module.join(", ")
        ));
    }
    let measured = coverage.covered * 100 / coverage.lines;
    let unmeasured: Vec<&str> = module
        .iter()
        .filter(|relative| !coverage.files.iter().any(|seen| seen == *relative))
        .map(String::as_str)
        .collect();
    let note = if unmeasured.is_empty() {
        String::new()
    } else {
        format!(" (no data for {})", unmeasured.join(", "))
    };
    if coverage.covered * 100 >= u64::from(required) * coverage.lines {
        return GateOutcome::passed(format!(
            "{} of {} line(s) of this step's module are covered ({measured}% ≥ {required}%){note}",
            coverage.covered, coverage.lines
        ));
    }
    GateOutcome::failed(format!(
        "line coverage of this step's module is {measured}% ({} of {} line(s)), below the \
         {required}% this step asks for — the suite reaches {} and misses the rest{note}",
        coverage.covered,
        coverage.lines,
        coverage.files.join(", ")
    ))
}

/// A path as a command line takes it.
fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A `Vec<String>` of arguments as the borrowed form [`process::run`] takes.
fn borrowed(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::Outcome;
    use crate::test_support::TempDir;

    /// An `llvm-cov export --summary-only` document with two files, one of them the step's.
    const EXPORT: &str = r#"{
      "data": [
        {
          "files": [
            {
              "filename": "/work/sandbox/project/src/store.rs",
              "summary": { "lines": { "count": 40, "covered": 34 } }
            },
            {
              "filename": "/work/sandbox/project/src/main.rs",
              "summary": { "lines": { "count": 10, "covered": 10 } }
            }
          ]
        }
      ],
      "type": "llvm.coverage.json.export",
      "version": "2.3.1"
    }"#;

    /// The copy a run builds in, as [`summary`] and [`judge`] are told about it.
    const MEASURED: &str = "/work/sandbox/project";

    #[test]
    fn only_the_steps_module_counts_towards_its_coverage() {
        let module = ["src/store.rs".to_owned(), "src/queue.rs".to_owned()];
        let coverage = summary(EXPORT, Path::new(MEASURED), &module).expect("the fixture reads");
        assert_eq!(coverage.lines, 40, "main.rs is not part of this step");
        assert_eq!(coverage.covered, 34);
        assert_eq!(coverage.files, ["src/store.rs"]);

        let passed = judge(80, &coverage, &module, Path::new(MEASURED));
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(passed.detail.contains("85%"), "{}", passed.detail);
        assert!(
            passed.detail.contains("no data for src/queue.rs"),
            "{}",
            passed.detail
        );

        let failed = judge(90, &coverage, &module, Path::new(MEASURED));
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.detail.contains("below the 90%"), "{}", failed.detail);
    }

    #[test]
    fn a_module_named_outside_the_copy_is_reported_as_a_fault_not_as_a_number() {
        // What the copy turns from a routine answer into a fault: the same file name, one directory
        // over — a profile that cannot come from the run this gate made.
        let foreign = EXPORT.replace(MEASURED, "/work/other-sandbox/project");
        let module = ["src/store.rs".to_owned()];
        let coverage = summary(&foreign, Path::new(MEASURED), &module).expect("the fixture reads");
        assert_eq!(coverage.lines, 0);
        assert_eq!(coverage.foreign, 1);

        let outcome = judge(1, &coverage, &module, Path::new(MEASURED));
        assert_eq!(outcome.outcome, Outcome::Blocked);
        assert!(
            outcome.detail.contains(MEASURED) && outcome.detail.contains("stamped"),
            "the report has to name the copy it measured: {}",
            outcome.detail
        );
    }

    #[test]
    fn a_module_with_no_data_is_blocked_not_passed() {
        let coverage = summary(EXPORT, Path::new(MEASURED), &["src/other.rs".to_owned()])
            .expect("the fixture reads");
        assert_eq!(coverage.lines, 0);
        let outcome = judge(
            1,
            &coverage,
            &["src/other.rs".to_owned()],
            Path::new(MEASURED),
        );
        assert_eq!(outcome.outcome, Outcome::Blocked);
        assert!(
            outcome.detail.contains("src/other.rs"),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn a_document_that_is_not_an_export_is_reported_not_ignored() {
        let error = summary("not json", Path::new("/work"), &[]).expect_err("not an export");
        assert!(error.to_string().contains("llvm-cov export"), "{error}");
        assert!(summary("{\"data\": []}", Path::new("/work"), &[]).is_ok());
    }

    #[test]
    fn the_tools_come_from_the_sysroot_the_active_rustc_names() {
        let scratch = TempDir::new("coverage-tools");
        let bin = scratch.path().join("lib/rustlib/aarch64-apple-darwin/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("llvm-profdata"), "").unwrap();

        let verbose = "rustc 1.98.1\nbinary: rustc\nhost: aarch64-apple-darwin\nrelease: 1.98.1\n";
        let sysroot = scratch.path().to_string_lossy();
        assert!(
            tool_paths(&sysroot, verbose).is_none(),
            "one tool is not both"
        );

        std::fs::write(bin.join("llvm-cov"), "").unwrap();
        let found = tool_paths(&sysroot, verbose).expect("both tools are there");
        assert!(
            found
                .profdata
                .ends_with("lib/rustlib/aarch64-apple-darwin/bin/llvm-profdata")
        );
        assert!(
            found
                .cov
                .ends_with("lib/rustlib/aarch64-apple-darwin/bin/llvm-cov")
        );

        // A `rustc` that does not say which host it is cannot be reasoned about.
        assert!(tool_paths(&sysroot, "rustc 1.98.1\n").is_none());
    }

    #[test]
    fn the_flags_keep_what_the_project_already_asked_for() {
        // The environment is what the test can set, and it is process-wide: only assert the shape.
        let flags = instrumented();
        assert!(flags.ends_with("-C instrument-coverage"), "{flags}");
        assert!(!flags.starts_with(' '), "{flags}");
    }

    #[test]
    fn a_populated_toolchain_answers_with_files_that_exist() {
        // Machine-honest: this either finds the component or reports that it is missing, and both are
        // correct answers. What it may not do is name a path that is not there.
        // The probe runs in a directory that is a Rust project (this repository), because that is
        // where a toolchain pin would apply — the same reason the gate asks in the project it judges.
        let project = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("kiln lives one directory below the repository root");
        if let Some(tools) = tools(project, Duration::from_secs(30)) {
            assert!(tools.profdata.is_file(), "{}", tools.profdata.display());
            assert!(tools.cov.is_file(), "{}", tools.cov.display());
        }
    }
}
