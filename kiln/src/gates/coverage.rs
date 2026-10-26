//! `coverage@N` — the step's own module is exercised by its own suite (handoff M2 §1).
//!
//! This is the one slow gate that is a *should*: measuring line coverage needs `llvm-profdata` and
//! `llvm-cov`, which come from a rustup component (`llvm-tools-preview`) rather than from the
//! toolchain baseline, so a machine without them reports [`GateOutcome::blocked`] with the command
//! that installs them (plan §10). A missing tool never fails a step.
//!
//! What it measures is deliberately narrow: the `.rs` files under `src/` that this step
//! *introduces* ([`sources::introduced_sources`] — the files its scaffold and its reference solution
//! create), not the whole crate. A single percentage over a whole library says nothing about the
//! step — the step is one module, and that is the module whose error branches and `match` arms the
//! specs were supposed to reach. A file an earlier step wrote is not part of it even when this step
//! edits it: its lines are exercised by that step's suite, so counting them would report this step's
//! suite as bad at something it was never about (handoff M2 §5.2: `jobq/3.26-dlq-cli` read 37% over a
//! module of other steps' files and 90% over the file it brings).
//!
//! The run: the step's own `run.cmd` under `RUSTFLAGS=-C instrument-coverage`, with
//! `LLVM_PROFILE_FILE` pointing into a scratch directory; then `llvm-profdata merge -sparse`, then one
//! `llvm-cov export --summary-only` per object cargo linked, folded into one number ([`Coverage`]).
//! Both the profiles and the merged data are removed with the scratch directory when the gate ends. The
//! instrumented objects do *not* live in the ordinary build directory — a different `RUSTFLAGS` is a
//! different build, and putting it there would make every ordinary build that follows recompile the
//! project (measured: 22 s cold on the machine this was written on). They get a cache directory of their
//! own, which also means the gate's second run is incremental. A project whose *own* cargo config
//! sets `rustflags` is refused before any of that ([`configured_rustflags`]): the environment wins
//! over the config, so the instrumented build would not be the build the learner's own commands
//! make, and a percentage about a build nobody has is the one answer this gate must not give.
//!
//! "Every profile the run wrote" includes the ones its children chose ([`profiles_of_the_run`]): a
//! suite that judges a command line starts the binary with an environment of its own (`env_clear`),
//! and that child never sees `LLVM_PROFILE_FILE` — it writes LLVM's default name into its working
//! directory, inside the tree this gate measured. Those profiles are this run's; not reading them
//! reported the file `jobq/3.26-dlq-cli` is about as `0 of 199 line(s)` for a suite that drives it on
//! every case (handoff M2 §5.2).
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
//! Only the objects *this run* linked are read ([`executables_of_the_run`]) — from both directories
//! cargo links a binary into: `target/debug/deps` for the test binaries, and the build root for the
//! binary a spec spawns — and each is read on its own: the profile's files have to be this copy's, and
//! the answer about a file has to come from the object that carries it — an `export` of several
//! binaries at once is a document about none of them (measured, see the comments there).
//!
//! Facts about the machine are reported as blocks, not failures: no tools, no profile data, no
//! executable, an unreadable export, or an instrumented spec run that did not pass would each mean a
//! number that describes something other than the learner's code.

use std::collections::BTreeMap;
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

/// The build directory an instrumented run keeps: `coverage`'s own, and claimed while it is used.
///
/// A different `RUSTFLAGS` is a different build, so this cannot be the directory the learner's own
/// commands use — and it is *shared between runs* because the dependencies in it are what a run
/// after it reuses ([`scratch::cache`], [`scratch::claim`]).
const TARGET_DIR: &str = "coverage-target";

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
    let module = sources::introduced_sources(&track_dir, ctx.step)?;
    if module.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "{} introduces no production source under `src/` and its patch touches none, so there is \
             no module to measure",
            ctx.step.qualified_id()
        )));
    }
    let Some(run) = ctx.step.run.as_ref() else {
        return Ok(GateOutcome::blocked(format!(
            "{} declares no `run.cmd`, so there is no suite whose coverage could be measured",
            ctx.step.qualified_id()
        )));
    };
    // A project that carries `rustflags` in its own cargo config cannot be measured here: this gate
    // sets `RUSTFLAGS` in the child environment, cargo lets the environment win over the config, and
    // the build would therefore not be the one the project's own commands make (see
    // [`configured_rustflags`]). Refused before anything is built, because the alternative is a
    // percentage about a tree nobody has.
    if let Some((file, key)) = configured_rustflags(project, ctx.course.root()) {
        return Ok(GateOutcome::blocked(format!(
            "{} sets `{key}`, and `RUSTFLAGS` in the environment wins over a project's cargo config \
             — the instrumented build would have flags your own commands never use, so this gate \
             will not report a percentage about a different build; move those flags to the command \
             that needs them (or drop them) and run the step again",
            file.display()
        )));
    }
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
    sandbox::copy_project_and_dependencies(ctx.course, ctx.track, project, &measured)?;
    let profiles = scratch.path().join("profiles");
    std::fs::create_dir_all(&profiles).map_err(|source| KilnError::Write {
        path: profiles.clone(),
        source,
    })?;
    // The build directory is shared between runs, so it is claimed before anything is built and held
    // for the whole gate: cargo picks an artifact by package *name and version*, so a second kiln
    // process building in it would be served this run's binaries — or leave its own for this one —
    // and the percentage would be about a tree nobody has (see [`scratch::claim`]).
    let Some(_claim) = scratch::claim(TARGET_DIR)? else {
        return Ok(GateOutcome::blocked(format!(
            "{} — two coverage runs in one build directory would serve each other's artifacts, so \
             this gate did not build there; wait for that run to finish, or delete the file if that \
             run is gone",
            scratch::contended(TARGET_DIR)
        )));
    };
    // An instrumented build gets a build directory of its own, and it *keeps* it (`scratch::cache`
    // rather than the per-run scratch): a different `RUSTFLAGS` is a different build, so sharing the
    // ordinary directory would throw away every artifact the learner's next command needs (measured:
    // a cold rebuild of this project is 22 s, and the ordinary build would pay it again). Keeping it
    // also makes the gate's own second run incremental — the copy's stamps rebuild the *package*, and
    // the dependencies, which are not in the copied tree, stay as cargo left them.
    let target = scratch::cache(TARGET_DIR)?;
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
    // older in the shared directory belongs to a run that came before (see [`executables_of_the_run`]).
    let started = SystemTime::now();
    let ran = process::run_line_with_env(&run.cmd, &measured, ctx.timeout, &environment)?;
    if !ran.success() {
        return Ok(GateOutcome::blocked(format!(
            "the spec run under `-C instrument-coverage` did not pass, so any coverage number would \
             describe a broken run:\n    {}",
            output_detail(&ran)
        )));
    }

    let raw = profiles_of_the_run(&profiles, &measured, started)?;
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

    let debug = target.join("debug");
    let objects = executables_of_the_run(&debug, started);
    if objects.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "no executable under {} was linked by this run — profile data can only be read back 
             against the binaries it came from, and this run left none",
            debug.display()
        )));
    }
    // One `llvm-cov export` per object, folded together here: see [`Coverage::absorb`]. Handing them
    // all to a single `export` is not an equivalent shortcut — that is the call shape for one binary
    // split into several object files, and with several *binaries* it silently loses the files only
    // some of them carry (measured: the module of `jobq/3.22-bench-budget` is `src/engine/budget.rs`,
    // which is linked into that step's spec binary and into none of the crate's three bins, so the
    // export answered "the profile has no executable line for the step's module" for a step whose
    // suite covers that module 64/64).
    let mut coverage = Coverage::default();
    for object in &objects {
        let export_args: Vec<String> = vec![
            "export".to_owned(),
            "--summary-only".to_owned(),
            // Only this tree's own files. The document otherwise carries every dependency's sources
            // and the toolchain's, which is both noise the gate filters out anyway and — measured —
            // more than kiln's own 256 KiB capture cap: a truncated document is an export this gate
            // cannot read at all, and the flag is the difference between a measurement and a parse
            // error.
            "--ignore-filename-regex".to_owned(),
            IGNORED_SOURCES.to_owned(),
            "--instr-profile".to_owned(),
            lossy(&merged),
            lossy(object),
        ];
        let exported = process::run(
            &lossy(&tools.cov),
            &borrowed(&export_args),
            &measured,
            ctx.timeout,
        )?;
        if !exported.success() {
            return Ok(GateOutcome::blocked(format!(
                "`llvm-cov export --summary-only` could not read this run's profile against {}:\n    {}",
                object.display(),
                output_detail(&exported)
            )));
        }
        coverage.absorb(summary(&exported.stdout, &measured, &module)?);
    }

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

/// The project's own `rustflags`, when a cargo config it carries sets any: the file, and the key.
///
/// [`instrumented`] puts `RUSTFLAGS` into the child environment, and cargo's rule is that the
/// **environment wins over a project's config** — so a project that configures `[build] rustflags`
/// (or `[target.…] rustflags`) is built by this gate with flags its own commands never use. That is
/// not a measurement with a caveat, it is a percentage about a tree nobody has (review finding #6,
/// handoff M2 §5.2), and the gate cannot merge two flag sets it does not understand. So the caller
/// refuses the run instead of answering about the wrong build.
///
/// Cargo reads configuration from the directory its command runs in and from that directory's
/// ancestors, which is why the walk starts at the project and goes up: this course's projects live
/// at `app/<track>` below the repository root, and a `rustflags` at any level in between belongs to
/// the build the learner's own commands make and not to the one this gate can reproduce. The walk
/// stops at `root` (the course's own tree) — the machine's `$CARGO_HOME/config.toml` is deliberately
/// not read: it is the machine's business rather than the course's, and a gate that blocked on it
/// would answer about a file the learner cannot see in the project (handoff M2 §1.3 states that
/// boundary). Both spellings cargo accepts are looked for, and a config that cannot be read or
/// parsed is left to cargo — it reports its own error about the file, and a gate that guessed at a
/// broken one would answer about a build that does not exist either.
fn configured_rustflags(project: &Path, root: &Path) -> Option<(PathBuf, String)> {
    let mut directory = Some(project);
    while let Some(here) = directory {
        if let Some(found) = ["config.toml", "config"]
            .iter()
            .find_map(|name| rustflags_in(&here.join(".cargo").join(name)))
        {
            return Some(found);
        }
        directory = if here == root { None } else { here.parent() };
    }
    None
}

/// The `rustflags` key of one cargo config file, when it has one.
fn rustflags_in(file: &Path) -> Option<(PathBuf, String)> {
    let text = std::fs::read_to_string(file).ok()?;
    let config: toml::Value = toml::from_str(&text).ok()?;
    key_setting(&config).map(|key| (file.to_path_buf(), key))
}

/// Which key of a parsed cargo config sets `rustflags`, named the way the file writes it.
fn key_setting(config: &toml::Value) -> Option<String> {
    if config
        .get("build")
        .and_then(|build| build.get("rustflags"))
        .is_some()
    {
        return Some("build.rustflags".to_owned());
    }
    // Every `[target.…]` entry: a triple, a `cfg(…)` expression, or a literal like `unix`. A table
    // that names `rustflags` is a finding whether the flags are a list or a table of their own.
    config
        .get("target")?
        .as_table()?
        .iter()
        .find_map(|(name, entry)| {
            entry
                .get("rustflags")
                .map(|_| format!("target.{name}.rustflags"))
        })
}

/// Every profile file a run wrote: the ones the gate asked for, and the ones its children chose.
///
/// The gate points `LLVM_PROFILE_FILE` into a directory of its own, so a process that inherits the
/// environment writes where it is told. A process a *suite* starts with an environment it picks
/// itself does not: `env_clear` is how a spec that judges a command line asks what a script sees
/// (`jobq/3.15-cli-contract`, `jobq/3.26-dlq-cli`), and a child that never sees the variable writes
/// LLVM's default name — in its working directory, which is inside the tree this run measured.
/// Measured (handoff M2 §5.2): `jobq/3.26-dlq-cli` is that shape, and the file its suite drives on
/// every case was reported as `0 of 199 line(s)`, because the coverage of the real binary was in
/// the project root and nowhere the gate looked.
///
/// Only this run's files: the copy a run builds in holds no profile file at all ([`sandbox::copy_project`]
/// leaves them behind), so the ones a walk finds are the ones a process of this run wrote, and the
/// timestamp test is a second lock on the same door. `target/` is skipped with the reason it is
/// skipped everywhere else — it is not part of the copy, and a build directory is not source.
fn profiles_of_the_run(directory: &Path, tree: &Path, since: SystemTime) -> Result<Vec<PathBuf>> {
    let mut found = profiles_written(directory)?;
    found.extend(stray_profiles(tree, since)?);
    found.sort();
    found.dedup();
    Ok(found)
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
        if is_profile(&path) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// The profile files of a tree that `since` did not come after, in path order.
fn stray_profiles(tree: &Path, since: SystemTime) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut queue = vec![tree.to_path_buf()];
    while let Some(directory) = queue.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|source| KilnError::Read {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| KilnError::Read {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                queue.push(path);
            } else if is_profile(&path)
                && std::fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| modified >= since)
            {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Is this file a profile one of the run's processes wrote?
fn is_profile(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "profraw")
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

/// Every executable this run linked under the build root (`target/debug`), in path order.
///
/// Both directories cargo links a binary into are read, and both are needed. A test binary lands in
/// `deps/` under a hashed name; the binary a package *declares* (`[[bin]]`, so `target/debug/<bin>`)
/// lands in the build root itself. The second one is the binary a spec **spawns** — `jobq/3.26-dlq-cli`
/// runs `jobq` with an environment it picks itself — and the module that step brings is reached by
/// that binary, so reading `deps/` alone leaves the measurement depending on luck: cargo happens to
/// also build a **test-harness copy** of a `[[bin]]` target into `deps/`, and that copy carries the
/// same code. It is a copy cargo makes for its own reasons, not part of the interface this gate may
/// rely on: a target declared with `harness = false`, or a cargo that stops making it, turns the same
/// measurement into a `blocked` or into a denominator made of the subset one harness copy kept.
/// Reading the build root changes no number today — `jobq/3.26-dlq-cli`'s `src/cli/dlq.rs` stays at
/// 181/199, the value the harness copy was already answering with — and that is the point: the
/// number comes from the binary the suite runs instead of from a copy of it.
fn executables_of_the_run(build_root: &Path, since: SystemTime) -> Vec<PathBuf> {
    let mut found = executables(&build_root.join("deps"), since);
    found.extend(executables(build_root, since));
    found.sort();
    found.dedup();
    found
}

/// One line coverage number over the step's module, gathered from every object the run left.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Coverage {
    /// The module's files the exports had data for, with the counts of the object that covered each
    /// of them most (see [`Coverage::absorb`]).
    files: BTreeMap<String, Lines>,
    /// Module line(s) the exports named *outside* the copy this run measured.
    ///
    /// This used to be a guard against a number from another sandbox (cargo keys a build slot by
    /// package name and version, so a tree that had built before this one could answer here). The
    /// run now builds in a copy of this step's own tree, stamped fresh, and is read back against the
    /// objects *that run* linked, so there is no other tree left to answer; what is counted here is a
    /// fault rather than a foreign measurement — a path that ends with the module's name and does not
    /// live in the copy, which means the profile came from something other than this run.
    foreign: u64,
}

impl Coverage {
    /// Executable lines in the module's files.
    fn lines(&self) -> u64 {
        self.files.values().map(|lines| lines.count).sum()
    }

    /// Lines of them the run executed.
    fn covered(&self) -> u64 {
        self.files.values().map(|lines| lines.covered).sum()
    }

    /// Fold another object's view of the module into this one.
    ///
    /// The same file is compiled into every object that links the code it belongs to: a library file
    /// is in the crate's bins *and* in the test binaries, and each of them says something different
    /// about it — a binary that was built and never run reports every line of it uncovered, and no two
    /// compilations of one file map the same lines (the test build has the file's own `#[cfg(test)]`
    /// code, a bin's copy has whatever the linker kept). The best view wins: the one that covered the
    /// file most, and among those the one whose mapping is most complete (`(covered, count)`, so an
    /// uncovered file is counted as the file the run's build holds rather than as the part of it one
    /// object happened to keep). Summing the views instead would count the file once per object:
    /// `src/lib.rs` is one small file and four objects, and a percentage over four copies of it is not
    /// a number about anything.
    fn absorb(&mut self, other: Self) {
        self.foreign += other.foreign;
        for (file, lines) in other.files {
            let seen = self.files.entry(file).or_insert(lines);
            if (lines.covered, lines.count) > (seen.covered, seen.count) {
                *seen = lines;
            }
        }
    }
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

/// Line counts of one file.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
struct Lines {
    /// Lines that could be executed.
    count: u64,
    /// Lines that were.
    covered: u64,
}

/// Line coverage of the step's module, out of one object's export document.
///
/// `measured` is the root the profile's files have to live under — the copy this run built in — and
/// `module` holds the step's files as paths relative to it. A file of the export is a file of the
/// module when its path relative to that root *is* the path the module names: ending with that path
/// is not the same thing as being it.
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
        let path = resolved(&absolute);
        // A path that ends with the module's name and does not live in the copy this run built in is
        // a build of something else: counting it would report another tree's lines as this step's.
        let Ok(rest) = path.strip_prefix(&root) else {
            if module
                .iter()
                .any(|relative| absolute.ends_with(relative.as_str()))
            {
                coverage.foreign += 1;
            }
            continue;
        };
        // A path that ends with the module's path is not the module's file. The copy this run builds
        // in can hold `backup/src/cli/dlq.rs` where the module is `src/cli/dlq.rs`, and a suffix test
        // counts that second file, and its lines, in a denominator that has one file in it. The file
        // is this file when the path relative to the copy *is* the path the module names, and nothing
        // else counts.
        let Some(relative) = module
            .iter()
            .find(|relative| rest == Path::new(relative.as_str()))
        else {
            continue;
        };
        coverage.files.insert(relative.clone(), file.summary.lines);
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

/// The gate's answer, from what the exports say about the module.
fn judge(required: u32, coverage: &Coverage, module: &[String], measured: &Path) -> GateOutcome {
    let (lines, covered) = (coverage.lines(), coverage.covered());
    if lines == 0 && coverage.foreign > 0 {
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
    if lines == 0 {
        return GateOutcome::blocked(format!(
            "the profile has no executable line for the step's module ({}), so coverage cannot be \
             measured — the spec run's binaries do not carry this module's coverage data",
            module.join(", ")
        ));
    }
    let percent = covered * 100 / lines;
    let unmeasured: Vec<&str> = module
        .iter()
        .filter(|relative| !coverage.files.contains_key(*relative))
        .map(String::as_str)
        .collect();
    let note = if unmeasured.is_empty() {
        String::new()
    } else {
        format!(" (no data for {})", unmeasured.join(", "))
    };
    if covered * 100 >= u64::from(required) * lines {
        return GateOutcome::passed(format!(
            "{covered} of {lines} line(s) of this step's module are covered ({percent}% ≥ \
             {required}%){note}"
        ));
    }
    GateOutcome::failed(format!(
        "line coverage of this step's module is {percent}% ({covered} of {lines} line(s)), below the \
         {required}% this step asks for — the suite reaches {} and misses the rest{note}",
        coverage
            .files
            .keys()
            .cloned()
            .collect::<Vec<String>>()
            .join(", ")
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
        assert_eq!(coverage.lines(), 40, "main.rs is not part of this step");
        assert_eq!(coverage.covered(), 34);
        assert_eq!(coverage.files.keys().collect::<Vec<_>>(), ["src/store.rs"]);

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
    fn the_best_view_of_a_file_is_the_one_that_counts() {
        // What several objects of one run say about one file: the library is linked into every
        // binary, a binary that was built and never run reports all of it uncovered, and no two
        // compilations map the same lines (the test build holds the file's own `#[cfg(test)]` code,
        // a bin's copy whatever the linker kept). The file is counted once, as the best view of it.
        let view = |count: u64, covered: u64| {
            let document = EXPORT.replace(
                "\"count\": 40, \"covered\": 34",
                &format!("\"count\": {count}, \"covered\": {covered}"),
            );
            summary(&document, Path::new(MEASURED), &["src/store.rs".to_owned()])
                .expect("the fixture reads")
        };
        let exercised = view(40, 34);
        let smaller = view(30, 0);
        let built_only = view(40, 0);

        let mut coverage = built_only.clone();
        coverage.absorb(exercised.clone());
        assert_eq!(coverage.lines(), 40, "one file, counted once");
        assert_eq!(coverage.covered(), 34, "the view that ran is the one kept");

        // A view that covered nothing does not win over one that did, and among the views that
        // covered nothing the most complete mapping of the file is the one that counts — a
        // compilation that kept less of the file is not a smaller file.
        let mut coverage = smaller.clone();
        coverage.absorb(built_only.clone());
        assert_eq!((coverage.lines(), coverage.covered()), (40, 0));

        let mut coverage = exercised;
        coverage.absorb(built_only);
        coverage.absorb(smaller);
        assert_eq!((coverage.lines(), coverage.covered()), (40, 34));
    }

    #[test]
    fn a_module_named_outside_the_copy_is_reported_as_a_fault_not_as_a_number() {
        // What the copy turns from a routine answer into a fault: the same file name, one directory
        // over — a profile that cannot come from the run this gate made.
        let foreign = EXPORT.replace(MEASURED, "/work/other-sandbox/project");
        let module = ["src/store.rs".to_owned()];
        let coverage = summary(&foreign, Path::new(MEASURED), &module).expect("the fixture reads");
        assert_eq!(coverage.lines(), 0);
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
        assert_eq!(coverage.lines(), 0);
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
    fn a_project_that_configures_its_own_rustflags_is_refused_not_measured() {
        // The environment wins over the config, so an instrumented build of this project is not the
        // build its own commands make. The gate has to *find* those flags (review finding #6) rather
        // than report a percentage about that build.
        let temp = TempDir::new("coverage-rustflags");
        let root = temp.path().to_path_buf();
        let project = temp.path().join("app/jobq");
        std::fs::create_dir_all(&project).unwrap();
        assert_eq!(
            configured_rustflags(&project, &root),
            None,
            "no `.cargo/` at all: nothing to find"
        );

        temp.write(
            "app/jobq/.cargo/config.toml",
            "[build]\nrustflags = [\"--cfg\", \"x\"]\n",
        );
        let (file, key) =
            configured_rustflags(&project, &root).expect("`[build] rustflags` is found");
        assert_eq!(key, "build.rustflags");
        assert_eq!(file, project.join(".cargo/config.toml"));

        // The other shape: a target section rather than `build`, named by a `cfg(…)` expression.
        temp.write(
            "app/jobq/.cargo/config.toml",
            "[target.'cfg(unix)']\nrustflags = [\"-C\", \"opt-level=1\"]\n",
        );
        let (_, key) =
            configured_rustflags(&project, &root).expect("a target's rustflags is found");
        assert_eq!(key, "target.cfg(unix).rustflags");

        // `config` is the other file name cargo accepts, and a table of its own under the target is
        // a config that sets the key too.
        std::fs::remove_file(project.join(".cargo/config.toml")).unwrap();
        temp.write("app/jobq/.cargo/config", "[target.'cfg(unix)'.rustflags]\n");
        let (file, key) =
            configured_rustflags(&project, &root).expect("the dotted form is found too");
        assert_eq!(key, "target.cfg(unix).rustflags");
        assert_eq!(file, project.join(".cargo/config"));
    }

    #[test]
    fn the_directory_above_the_project_is_read_as_well() {
        // Cargo reads the config of the directory its command runs in and of that directory's
        // ancestors, and this course's projects live at `app/<track>` — so a config in `app/`, or in
        // the course's own root, is one the learner's own commands use and the copy the gate builds
        // in does not have. Above the root is the machine's business, not the course's (handoff M2
        // §1.3 states that boundary).
        let temp = TempDir::new("coverage-rustflags-parent");
        let root = temp.path().join("course");
        let project = root.join("app/jobq");
        std::fs::create_dir_all(&project).unwrap();
        temp.write(
            "course/app/jobq/.cargo/config.toml",
            "[build]\njobs = 4\n[alias]\ncheck = \"clippy\"\n",
        );
        assert_eq!(
            configured_rustflags(&project, &root),
            None,
            "keys that are not `rustflags` are not flags"
        );

        temp.write(
            "course/app/.cargo/config.toml",
            "[build]\nrustflags = \"-C target-cpu=native\"\n",
        );
        let (file, key) =
            configured_rustflags(&project, &root).expect("the directory above sets them");
        assert_eq!(key, "build.rustflags");
        assert_eq!(file, root.join("app/.cargo/config.toml"));

        // The course's own root counts too: a `rustflags` there is part of the build a learner's
        // commands make, and the walk reaches it.
        std::fs::remove_file(root.join("app/.cargo/config.toml")).unwrap();
        temp.write(
            "course/.cargo/config.toml",
            "[target.'cfg(unix)']\nrustflags = [\"--cfg\", \"course\"]\n",
        );
        let (file, key) =
            configured_rustflags(&project, &root).expect("the course root sets them too");
        assert_eq!(key, "target.cfg(unix).rustflags");
        assert_eq!(file, root.join(".cargo/config.toml"));

        // One level above the course root is where the walk stops: it is not a file the project can
        // be answerable for.
        std::fs::remove_file(root.join(".cargo/config.toml")).unwrap();
        temp.write(
            "above/.cargo/config.toml",
            "[build]\nrustflags = [\"--cfg\", \"outside\"]\n",
        );
        assert_eq!(
            configured_rustflags(&project, &root),
            None,
            "the walk is bounded at the course root"
        );
    }

    #[test]
    fn a_config_cargo_cannot_read_is_left_to_cargo() {
        // Not this gate's answer to give: cargo reports its own error about a file it cannot parse,
        // and a gate that guessed at a broken one would answer about a build that does not exist
        // either. Unreadable and unparsable are the same answer as no config at all.
        let temp = TempDir::new("coverage-rustflags-broken");
        let project = temp.path().join("app/jobq");
        temp.write("app/jobq/.cargo/config.toml", "[build\nrustflags = \n");
        assert_eq!(configured_rustflags(&project, temp.path()), None);
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
    fn a_child_that_did_not_inherit_the_profile_variable_is_still_this_runs_data() {
        // The shape of `jobq/3.26-dlq-cli`: the suite starts the real binary with `env_clear`, so
        // `LLVM_PROFILE_FILE` never reaches it and the profile lands in the tree under LLVM's default
        // name. That file is this run's, and leaving it out reports the step's own module as uncovered.
        let temp = TempDir::new("coverage-stray");
        let tree = temp.path().join("project");
        crate::test_support::write(&tree, "src/lib.rs", "pub fn f() {}\n");
        crate::test_support::write(&tree, "default_1_0_2.profraw", "the real binary's data\n");
        crate::test_support::write(&tree, "target/debug/x.profraw", "a build directory\n");
        crate::test_support::write(&tree, "yesterday.profraw", "an earlier run's data\n");
        // The directory the gate points `LLVM_PROFILE_FILE` at: it exists, and this run put nothing
        // in it, which is the case the stray file is there for.
        std::fs::create_dir_all(tree.join("profiles")).expect("the fixture directory is writable");
        let since = SystemTime::now();
        let fresh = since + Duration::from_secs(1);
        stamp(&tree.join("default_1_0_2.profraw"), fresh);
        stamp(&tree.join("target/debug/x.profraw"), fresh);
        stamp(
            &tree.join("yesterday.profraw"),
            since - Duration::from_secs(86_400),
        );

        let found = stray_profiles(&tree, since).expect("the tree is readable");
        assert_eq!(
            found,
            vec![tree.join("default_1_0_2.profraw")],
            "the run's own profile data, not another run's and not a build directory"
        );
        assert_eq!(
            profiles_of_the_run(&tree.join("profiles"), &tree, since)
                .expect("the tree is readable")
                .len(),
            1,
            "the gate's own directory holds none here, and the stray one is the run's"
        );
    }

    /// The objects of a run, out of a build root: the test binaries cargo leaves in `deps/` *and*
    /// the binary a package declares, which cargo leaves in the build root itself.
    #[cfg(unix)]
    #[test]
    fn the_binary_a_spec_spawns_is_read_from_the_build_root_and_not_from_a_copy_of_it() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new("coverage-objects");
        let debug = temp.path().join("debug");
        // The test binary of the run, next to the `.d` file cargo also writes there.
        let harness = crate::test_support::write(&debug, "deps/jobq-1a2b3c4d", "");
        let build_root_binary = crate::test_support::write(&debug, "jobq", "");
        // Left over from an earlier run of another tree: never this run's object.
        let yesterday = crate::test_support::write(&debug, "jobq-yesterday", "");
        // Written by the same run, and not an object: a `.d` file is not executable.
        let data = crate::test_support::write(&debug, "jobq.d", "target/debug/jobq: src/main.rs\n");
        for path in [&harness, &build_root_binary, &yesterday] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("the fixture can be made executable");
        }

        let since = SystemTime::now();
        let fresh = since + Duration::from_secs(1);
        stamp(&harness, fresh);
        stamp(&build_root_binary, fresh);
        stamp(&data, fresh);
        stamp(&yesterday, since - Duration::from_secs(86_400));

        let found = executables_of_the_run(&debug, since);
        assert_eq!(
            found,
            vec![harness.clone(), build_root_binary.clone()],
            "the run's two executables: the test binary in `deps/` and the binary a spec spawns in \
             the build root, and neither the older one nor the data file written beside it"
        );
        assert!(
            !found.contains(&data),
            "a `.d` file is not an object: the executable bit is what tells them apart"
        );
    }

    #[test]
    fn a_module_file_of_the_copy_is_the_file_the_module_names() {
        // The copy a run builds in can hold a file at the module's path under a directory of its own:
        // `backup/src/store.rs` where the module is `src/store.rs`. Counting it adds a second, larger
        // file to a denominator that has one file in it.
        let temp = TempDir::new("coverage-module");
        let project = temp.path().join("project");
        crate::test_support::write(&project, "src/store.rs", "pub fn get() {}\n");
        crate::test_support::write(&project, "backup/src/store.rs", "pub fn get() {}\n");
        // And the shape only a *byte* suffix would keep: `xsrc/store.rs` does not end with the
        // module's path component by component, so it is here to stay that way if the comparison is
        // ever rewritten as a string test.
        crate::test_support::write(&project, "xsrc/store.rs", "pub fn get() {}\n");
        let module = ["src/store.rs".to_owned()];
        let document = format!(
            r#"{{"data": [{{"files": [
              {{"filename": "{project}/src/store.rs",
                "summary": {{"lines": {{"count": 40, "covered": 34}}}}}},
              {{"filename": "{project}/backup/src/store.rs",
                "summary": {{"lines": {{"count": 90, "covered": 90}}}}}},
              {{"filename": "{project}/xsrc/store.rs",
                "summary": {{"lines": {{"count": 99, "covered": 99}}}}}}
            ]}}]}}"#,
            project = project.display()
        );

        let coverage = summary(&document, &project, &module).expect("the fixture reads");
        assert_eq!(
            coverage.files.keys().collect::<Vec<_>>(),
            ["src/store.rs"],
            "the module is one file, and the copy that carries the same tail is not it"
        );
        assert_eq!(
            (coverage.lines(), coverage.covered()),
            (40, 34),
            "so the number is the module's own lines, not the backup copy's"
        );
        assert_eq!(coverage.foreign, 0);

        // A module file that is not in the copy is still the fault it was, not a number: it has to
        // be a file of the tree this run built in. `other-sandbox/project` is not one.
        let outside = format!(
            r#"{{"data": [{{"files": [
              {{"filename": "{other}/src/store.rs",
                "summary": {{"lines": {{"count": 40, "covered": 34}}}}}}
            ]}}]}}"#,
            other = temp.path().join("other-sandbox/project").display()
        );
        let coverage = summary(&outside, &project, &module).expect("the fixture reads");
        assert_eq!(coverage.lines(), 0);
        assert_eq!(
            coverage.foreign, 1,
            "a module file outside the copy is the fault `judge` reports, not a measurement"
        );
    }

    /// Put a file's modification time where the test needs it.
    fn stamp(path: &Path, when: SystemTime) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the fixture file is writable")
            .set_modified(when)
            .expect("the fixture timestamp is settable");
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
