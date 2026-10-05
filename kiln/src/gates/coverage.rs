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
//! `llvm-cov export --summary` over the test binaries cargo produced. Both the profiles and the merged
//! data are removed with the scratch directory when the gate ends. The instrumented objects do *not*
//! live in the ordinary build directory — a different `RUSTFLAGS` is a different build, and putting it
//! there would make every ordinary build that follows recompile the project (measured: 22 s cold on the
//! machine this was written on). They get a cache directory of their own, which also means the gate's
//! second run is incremental.
//!
//! Facts about the machine are reported as blocks, not failures: no tools, no profile data, no test
//! binary, an unreadable export, or an instrumented spec run that did not pass would each mean a
//! number that describes something other than the learner's code.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::{KilnError, Result};
use crate::gates::runner::{GateContext, output_detail};
use crate::gates::sources;
use crate::gates::{GateOutcome, GateSpec};
use crate::process;
use crate::scratch;

/// What a machine without the LLVM tools is told, install line included.
pub const MISSING: &str = "llvm-profdata/llvm-cov are not in the active toolchain, so line coverage \
                            cannot be measured — `rustup component add llvm-tools-preview`, then \
                            re-run this step";

/// The suffix the profile files of a run carry.
const PROFILE: &str = ".profraw";

/// The two binaries a coverage run needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    /// `llvm-profdata`, which merges what every process of the run wrote.
    pub profdata: PathBuf,
    /// `llvm-cov`, which turns a merged profile plus the binaries into per-file counts.
    pub cov: PathBuf,
}

/// Find the LLVM tools of the *active* toolchain, when this machine has them.
///
/// `rustc --print sysroot` and `rustc -vV` answer where they are: `rustup component add
/// llvm-tools-preview` installs them under `lib/rustlib/<host>/bin` of the sysroot whose rustc is
/// compiling this run. Looking on `PATH` instead would happily find a system clang of a different
/// version, and a profile written by one LLVM read by another is a measurement nobody should trust.
#[must_use]
pub fn tools(timeout: Duration) -> Option<Tools> {
    let dir = std::env::temp_dir();
    let sysroot = process::run("rustc", &["--print", "sysroot"], &dir, timeout).ok()?;
    let verbose = process::run("rustc", &["-vV"], &dir, timeout).ok()?;
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
    let Some(tools) = tools(ctx.probe_timeout()) else {
        return Ok(GateOutcome::blocked(MISSING));
    };

    let scratch = scratch::directory("coverage")?;
    let profiles = scratch.path().join("profiles");
    std::fs::create_dir_all(&profiles).map_err(|source| KilnError::Write {
        path: profiles.clone(),
        source,
    })?;
    // An instrumented build gets a build directory of its own, and it *keeps* it (`scratch::cache`
    // rather than the per-run scratch): a different `RUSTFLAGS` is a different build, so sharing the
    // ordinary directory would throw away every artifact the learner's next command needs (measured:
    // a cold rebuild of this project is 22 s, and the ordinary build would pay it again). Keeping it
    // also makes the gate's own second run incremental rather than a full rebuild.
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
    let ran = process::run_line_with_env(&run.cmd, project, ctx.timeout, &environment)?;
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
        project,
        ctx.timeout,
    )?;
    if !merged_run.success() {
        return Ok(GateOutcome::blocked(format!(
            "`llvm-profdata merge` could not read the profile of this run:\n    {}",
            output_detail(&merged_run)
        )));
    }

    let deps = target.join("debug").join("deps");
    let objects = executables(&deps);
    if objects.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "no test binary under {} — profile data can only be read back against the binaries it \
             came from, and this run left none",
            deps.display()
        )));
    }
    // Every executable at once: `llvm-cov` ignores an object with no coverage data, so what the \
    // deps directory holds besides this run's test binaries costs nothing.
    let mut export_args: Vec<String> = vec![
        "export".to_owned(),
        "--summary".to_owned(),
        "--instr-profile".to_owned(),
        lossy(&merged),
    ];
    export_args.extend(objects.iter().map(|object| lossy(object)));
    let exported = process::run(
        &lossy(&tools.cov),
        &borrowed(&export_args),
        project,
        ctx.timeout,
    )?;
    if !exported.success() {
        return Ok(GateOutcome::blocked(format!(
            "`llvm-cov export --summary` could not read this run's profile:\n    {}",
            output_detail(&exported)
        )));
    }

    let coverage = summary(&exported.stdout, project, &module)?;
    Ok(judge(required, &coverage, &module))
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

/// Every runnable file of a target directory, in path order.
///
/// Test binaries live in `target/debug/deps` with a hash in the name, and they are the executables
/// there: the `.d` files beside them are not, so the executable bit is what tells the two apart.
fn executables(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| process::is_executable(path))
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
}

/// What an `llvm-cov export --summary` document carries.
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
fn summary(document: &str, project: &Path, module: &[String]) -> Result<Coverage> {
    let export: Export = serde_json::from_str(document).map_err(|source| {
        KilnError::Invalid(format!(
            "`llvm-cov export --summary` answered with something this build cannot read: {source}"
        ))
    })?;
    let mut coverage = Coverage::default();
    for file in export.data.iter().flat_map(|data| &data.files) {
        // The export answers with the path the compiler used, which is absolute; joining keeps a
        // relative one working too, because `join` replaces the path when it is already absolute.
        let absolute = project.join(&file.filename);
        let Some(relative) = module
            .iter()
            .find(|relative| absolute.ends_with(relative.as_str()))
        else {
            continue;
        };
        coverage.lines += file.summary.lines.count;
        coverage.covered += file.summary.lines.covered;
        if !coverage.files.iter().any(|seen| seen == relative) {
            coverage.files.push(relative.clone());
        }
    }
    Ok(coverage)
}

/// The gate's answer, from what the export says about the module.
fn judge(required: u32, coverage: &Coverage, module: &[String]) -> GateOutcome {
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

    /// An `llvm-cov export --summary` document with two files, one of them the step's.
    const EXPORT: &str = r#"{
      "data": [
        {
          "files": [
            {
              "filename": "/work/app/jobq/src/store.rs",
              "summary": { "lines": { "count": 40, "covered": 34 } }
            },
            {
              "filename": "/work/app/jobq/src/main.rs",
              "summary": { "lines": { "count": 10, "covered": 10 } }
            }
          ]
        }
      ],
      "type": "llvm.coverage.json.export",
      "version": "2.3.1"
    }"#;

    #[test]
    fn only_the_steps_module_counts_towards_its_coverage() {
        let module = ["src/store.rs".to_owned(), "src/queue.rs".to_owned()];
        let coverage =
            summary(EXPORT, Path::new("/work/app/jobq"), &module).expect("the fixture reads");
        assert_eq!(coverage.lines, 40, "main.rs is not part of this step");
        assert_eq!(coverage.covered, 34);
        assert_eq!(coverage.files, ["src/store.rs"]);

        let passed = judge(80, &coverage, &module);
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(passed.detail.contains("85%"), "{}", passed.detail);
        assert!(
            passed.detail.contains("no data for src/queue.rs"),
            "{}",
            passed.detail
        );

        let failed = judge(90, &coverage, &module);
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.detail.contains("below the 90%"), "{}", failed.detail);
    }

    #[test]
    fn a_module_with_no_data_is_blocked_not_passed() {
        let coverage = summary(
            EXPORT,
            Path::new("/work/app/jobq"),
            &["src/other.rs".to_owned()],
        )
        .expect("the fixture reads");
        assert_eq!(coverage.lines, 0);
        let outcome = judge(1, &coverage, &["src/other.rs".to_owned()]);
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
        if let Some(tools) = tools(Duration::from_secs(30)) {
            assert!(tools.profdata.is_file(), "{}", tools.profdata.display());
            assert!(tools.cov.is_file(), "{}", tools.cov.display());
        }
    }
}
