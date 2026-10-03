//! The command line surface.
//!
//! Argument parsing lives in [`Cli`], the shared context in [`Session`], and each command in its
//! own module. Exit codes are part of the contract:
//!
//! ```text
//! 0  the step (or the whole run) passed
//! 1  the runner judged the step and it did not pass
//! 2  the runner could not judge: bad metadata, unknown step, missing tool, timeout
//! 3  the requested content is not authored yet — nothing was judged
//! ```
//!
//! Code 3 exists so the difference between "you still have work to do" and "this step has no
//! content yet" survives in scripts, which is the honest answer while M0 is being written.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::error::{EXIT_ERROR, KilnError, Result};
use crate::gates::{GateContext, GateResult, run_all};
use crate::i18n::{Locale, Ui, status};
use crate::info::{Course, Step};
use crate::process;
use crate::state::State;

pub mod caps;
pub mod check;
pub mod doctor;
pub mod hint;
pub mod i18n;
pub mod images;
pub mod lint_caps;
pub mod list;
pub mod reset;
pub mod run;
pub mod selftest;
pub mod solution;
pub mod spec;
pub mod verify;
pub mod watch;

/// Command line interface of the course runner.
#[derive(Debug, Parser)]
#[command(
    name = "kiln",
    version,
    about = "Run and judge the steps of the rustlings-production course",
    long_about = "Run and judge the steps of the rustlings-production course.\n\nThe course \
                  metadata under `course/` is the single source of truth; kiln reads it, never \
                  invents it. Exit codes: 0 passed, 1 failed, 2 cannot judge, 3 content not \
                  authored yet."
)]
pub struct Cli {
    /// Course content language (`zh-Hans -> zh -> en` fallback; `KILN_LANG` also works).
    #[arg(long, global = true, env = "KILN_LANG", value_name = "LOCALE")]
    pub lang: Option<String>,

    /// Seconds a command the runner starts may take before it is killed.
    ///
    /// The deadline is enforced in Rust: the course must not depend on `timeout(1)`.
    #[arg(long, global = true, default_value_t = 300, value_name = "SECONDS")]
    pub timeout: u64,

    /// Command to run; a bare `kiln` continues at the first unfinished step.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// What `kiln` can be asked to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// List tracks and steps, with progress and the capability each step establishes.
    List {
        /// Track to list (default: all authored tracks).
        #[arg(value_name = "TRACK")]
        track: Option<String>,
    },

    /// Judge a step, then judge it again whenever the project changes.
    Watch {
        /// Step to watch (default: the first step that is not finished).
        #[arg(value_name = "STEP")]
        step: Option<String>,
    },

    /// Show the capability ledger: every depth, where it is established, and its status.
    Caps {
        /// Only capabilities that this track establishes.
        #[arg(long, value_name = "TRACK")]
        track: Option<String>,
    },

    /// Course self-check: the metadata rules R0-R6 over `capabilities.toml` and `track.toml`.
    ///
    /// Prose rules (R5) report themselves as skipped while step bodies are not authored yet.
    LintCaps,

    /// Run a step's spec tests (the fast path; default: the first unfinished step).
    Run {
        /// Step to run: `1.14`, `1.14-worker` or `mailer/1.14-worker`.
        #[arg(value_name = "STEP")]
        step: Option<String>,
    },

    /// Run a step's gates only, without its spec tests.
    Check {
        /// Step to check (default: the first unfinished step).
        #[arg(value_name = "STEP")]
        step: Option<String>,
    },

    /// Print a step's hint and where to read more.
    Hint {
        /// Step to hint at (default: the first unfinished step).
        #[arg(value_name = "STEP")]
        step: Option<String>,
    },

    /// Take a step's scaffold back out, keeping everything you wrote (default: the first
    /// unfinished step).
    Reset {
        /// Step to reset.
        #[arg(value_name = "STEP")]
        step: Option<String>,
    },

    /// Apply the reference solution of a step, and record that you did.
    Solution {
        /// Step whose solution to apply.
        #[arg(value_name = "STEP")]
        step: Option<String>,
    },

    /// Run one step's spec tests and gates against the reference solution, in a throwaway copy of
    /// the course.
    ///
    /// The course's own anti-rot check (plan §10): every step's spec must fail before its solution
    /// and pass after it. Steps other than the named ones are set up silently as prerequisites.
    Selftest {
        /// Steps to assert on (default: every authored step of the default track).
        #[arg(value_name = "STEP")]
        steps: Vec<String>,
    },

    /// Judge every authored step: its spec tests and its gates, plus what the result is worth.
    Verify {
        /// Track to verify (default: every authored track).
        #[arg(value_name = "TRACK")]
        track: Option<String>,

        /// Machine-readable report on stdout, for scripts and CI.
        #[arg(long)]
        json: bool,

        /// Also run the gates marked `slow` (mutants, coverage, soak, …).
        #[arg(long)]
        slow: bool,
    },

    /// The read-only spec files: restore them, or re-pin their hashes.
    Spec {
        /// What to do with them.
        #[command(subcommand)]
        command: SpecCommand,
    },

    /// Content locales: coverage, structure and translation drift.
    I18n {
        /// What to report (only `status` exists so far).
        #[command(subcommand)]
        command: I18nCommand,
    },

    /// Container images the authored steps declare (plan §9.3).
    Images {
        /// `pull` fetches what is missing; nothing just reports.
        #[arg(value_name = "ACTION")]
        action: Option<ImageAction>,
    },

    /// Diagnose the environment: toolchain, container runtime, cargo tools, registry mirror.
    Doctor,
}

/// What `kiln images` was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ImageAction {
    /// Fetch the images that are not in the local store.
    Pull,
}

/// Subcommands of `kiln spec`.
#[derive(Debug, Subcommand)]
pub enum SpecCommand {
    /// Write the course copy of a step's spec files back into the project.
    ///
    /// This is the command the `spec-integrity` gate points at: it refuses to restore from a
    /// course copy that no longer matches `spec/manifest.toml`.
    Restore {
        /// Step to restore (default: every authored step).
        #[arg(value_name = "STEP")]
        step: Option<String>,
    },

    /// Regenerate the hash manifest of the spec files (a course-maintenance command).
    Manifest {
        /// Track to re-pin (default: every track).
        #[arg(long, value_name = "TRACK")]
        track: Option<String>,
    },
}

/// Subcommands of `kiln i18n`.
#[derive(Debug, Subcommand)]
pub enum I18nCommand {
    /// Coverage, structure problems and drift for the requested locale.
    Status,
}

/// Run the CLI and turn its result into a process exit code.
pub fn run(cli: &Cli) -> ExitCode {
    match dispatch(cli) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("kiln: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

/// Parse and run one command line, for callers that are not the `kiln` binary.
///
/// `cargo xtask ci` uses this to run the course's self-check through exactly the code a learner
/// runs: CI and a human must not have two implementations of the same check (plan §10).
pub fn main(args: impl IntoIterator<Item = String>) -> ExitCode {
    match Cli::try_parse_from(args) {
        Ok(cli) => run(&cli),
        Err(error) => {
            // `--help` and `--version` leave through clap's error channel and are not failures.
            let _ = error.print();
            if error.use_stderr() {
                return ExitCode::from(EXIT_ERROR);
            }
            ExitCode::SUCCESS
        }
    }
}

fn dispatch(cli: &Cli) -> Result<u8> {
    let mut session = Session::new(cli)?;
    match &cli.command {
        Some(Command::List { track }) => list::run(&session, track.as_deref()),
        Some(Command::Caps { track }) => caps::run(&session, track.as_deref()),
        Some(Command::LintCaps) => lint_caps::run(&session),
        Some(Command::Run { step }) => run::run(&mut session, step.as_deref()),
        Some(Command::Watch { step }) => watch::run(&mut session, step.as_deref()),
        Some(Command::Check { step }) => check::run(&mut session, step.as_deref()),
        Some(Command::Hint { step }) => hint::run(&session, step.as_deref()),
        Some(Command::Reset { step }) => reset::run(&mut session, step.as_deref()),
        Some(Command::Solution { step }) => solution::run(&mut session, step.as_deref()),
        Some(Command::Selftest { steps }) => selftest::run(cli, &session, steps),
        Some(Command::Verify { track, json, slow }) => {
            verify::run(&mut session, track.as_deref(), *json, *slow)
        }
        Some(Command::Spec { command }) => spec::run(&session, command),
        Some(Command::I18n { command }) => match command {
            I18nCommand::Status => i18n::status(&session),
        },
        Some(Command::Images { action }) => {
            images::run(&session, *action == Some(ImageAction::Pull))
        }
        Some(Command::Doctor) => doctor::run(&session),
        None => list::welcome(&session),
    }
}

/// Resolve the step a command should act on.
///
/// No reference means "continue where I left off": the first unfinished step of the default
/// track, which is the behaviour a learner expects from typing `kiln run` (plan §3 D3).
///
/// The step is returned by value — it is small, and the commands need to record progress while
/// the step they are judging is still in view.
pub(crate) fn resolve_step(session: &Session, reference: Option<&str>) -> Result<Step> {
    match reference {
        Some(reference) => session.course().step(reference).cloned(),
        None => {
            let track = session.course().default_track()?;
            list::first_unfinished(track, session.state())
                .cloned()
                .ok_or_else(|| {
                    KilnError::Invalid(format!(
                        "every step of `{}` is done — name a step explicitly, or start the next \
                         track",
                        track.id
                    ))
                })
        }
    }
}

/// Everything a command needs: the course, the learner's locale and their progress.
pub struct Session {
    course: Course,
    locale: Locale,
    source: Locale,
    ui: Ui,
    state: State,
    timeout: Duration,
    /// Gate results already produced in this run, keyed by the tree they were produced on.
    ///
    /// `verify` judges **one tree** with nineteen steps' gate lists, and most of those are the same
    /// gate on the same tree (`fmt`, `clippy`, `docs:public`, …): the answer cannot differ, and asking
    /// nineteen times is most of what that command spends. `selftest` mutates its sandbox between
    /// steps, which is why the key is the *tree* rather than just the gate — a different tree is a
    /// different question. [`CACHEABLE`] holds the list of gates this may apply to and why the
    /// others are left out.
    cache: GateCache,
}

/// The gates whose answer is a property of the *tree*, and which a run may therefore answer once.
///
/// Everything else is left out on purpose, and for one of three reasons:
///
/// * **the step is part of the question** — `caps` and `i18n` judge that step's metadata and prose,
///   which live in the course rather than in the project the fingerprint describes;
/// * **the detail line is per step** — `images` names the images *this* step declared, and two steps
///   of one track declare different ones;
/// * **the gate is slow or not implemented** — `mutants`, `bench` and friends are run on purpose,
///   and `container-repro`/`links` say what they need before they can answer anything.
const CACHEABLE: &[&str] = &[
    "spec-integrity",
    "fmt",
    "clippy",
    "exit-code",
    "no-todo",
    "docs:public",
];

/// Gate results already produced in this run, by gate and by the tree they were produced on.
#[derive(Debug, Default)]
struct GateCache {
    /// Key: the gate as written, plus [`tree_fingerprint`] of the project.
    results: std::cell::RefCell<std::collections::HashMap<(String, String), GateResult>>,
    /// How many questions were answered from the cache, for tests and for the curious.
    hits: std::cell::Cell<usize>,
}

impl GateCache {
    /// `true` when this gate's answer is a property of the tree alone.
    fn covers(&self, spec: &crate::gates::GateSpec) -> bool {
        CACHEABLE.contains(&spec.name.as_str())
    }

    /// A result this run has already produced for this gate on this tree.
    fn get(&self, spec: &crate::gates::GateSpec, fingerprint: Option<&str>) -> Option<GateResult> {
        let fingerprint = fingerprint?;
        if !self.covers(spec) {
            return None;
        }
        let hit = self
            .results
            .borrow()
            .get(&(key_of(spec), fingerprint.to_owned()))
            .cloned();
        if hit.is_some() {
            self.hits.set(self.hits.get() + 1);
        }
        hit
    }

    /// Remember what this gate answered on this tree.
    fn put(&self, spec: &crate::gates::GateSpec, fingerprint: &str, result: &GateResult) {
        if !self.covers(spec) {
            return;
        }
        self.results
            .borrow_mut()
            .insert((key_of(spec), fingerprint.to_owned()), result.clone());
    }

    /// How many questions were answered without running a gate.
    #[cfg(test)]
    fn hits(&self) -> usize {
        self.hits.get()
    }
}

/// How one gate is named in the cache: the gate as the step wrote it, which is what decides the
/// command that runs and the parameter it is given. The tier is not part of it (whether a *failing*
/// gate blocks a step is decided after it has run, not by running it differently).
fn key_of(spec: &crate::gates::GateSpec) -> String {
    format!("{}@{:?}:{}", spec.name, spec.param, spec.slow)
}

/// Judge one step once — what `kiln run` does, in the shape a loop can call.
///
/// `watch` is the only caller that needs this, and it is why the run's work lives here rather than in
/// its own command file: a watcher and a one-shot run must not disagree about what "judging a step"
/// means.
pub(crate) fn run_step_once(session: &mut Session, step: &Step) -> Result<u8> {
    crate::cmd::run::run(session, Some(step.qualified_id().as_str()))
}

/// A fingerprint of a project: the *contents* of every file in it, in one number.
///
/// It exists so that a cached gate result is never reused for a different tree — `selftest` applies a
/// scaffold, a solution or a reset between steps, and all three change files. Contents rather than
/// timestamps, because the other direction matters too: a spec sync that rewrites a file with the
/// same bytes has not changed the tree, and a fingerprint that said it had would make every step's
/// gates run again for nothing.
pub(crate) fn tree_fingerprint(project_dir: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_fingerprint(project_dir, project_dir, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (path, bytes) in files {
        path.hash(&mut hasher);
        bytes.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// Walk one directory, skipping the two trees that are not the project: build output and the VCS.
fn collect_fingerprint(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == ".git" {
                continue;
            }
            collect_fingerprint(root, &path, files);
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        files.push((relative, bytes));
    }
}

impl Session {
    /// Load the course, resolve the locale and read progress.
    pub fn new(cli: &Cli) -> Result<Self> {
        let cwd = std::env::current_dir().map_err(|source| KilnError::Read {
            path: PathBuf::from("."),
            source,
        })?;
        Self::at(&cwd, cli)
    }

    /// Load the course rooted at `root` (walking upwards from it), with the locale and progress
    /// that belong to it.
    ///
    /// `kiln selftest` is why this exists: it judges a throwaway copy of the course, and that copy
    /// must be read with its own metadata, its own project and its own (empty) progress file.
    pub fn at(root: &Path, cli: &Cli) -> Result<Self> {
        let course = Course::discover(root)?;
        let source = Locale::parse(&course.manifest().default_locale)?;
        let locale = match &cli.lang {
            Some(requested) => Locale::parse(requested)?,
            None => source.clone(),
        };
        let ui = Ui::load(&course.dir(), &locale, &source)?;
        let state = State::load(course.root())?;
        Ok(Self {
            course,
            locale,
            source,
            ui,
            state,
            timeout: Duration::from_secs(cli.timeout),
            cache: GateCache::default(),
        })
    }

    /// Deadline applied to every command the runner starts.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The loaded course.
    #[must_use]
    pub fn course(&self) -> &Course {
        &self.course
    }

    /// Locale the learner asked for.
    #[must_use]
    pub fn locale(&self) -> &Locale {
        &self.locale
    }

    /// Locale holding the source text of the course.
    #[must_use]
    pub fn source(&self) -> &Locale {
        &self.source
    }

    /// CLI text for the requested locale.
    #[must_use]
    pub fn ui(&self) -> &Ui {
        &self.ui
    }

    /// Progress recorded so far.
    #[must_use]
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Progress, mutably, for commands that judge a step.
    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    /// Write progress back to `.kiln/state.toml`.
    pub fn save_state(&self) -> Result<()> {
        self.state.save(self.course.root())
    }

    /// Content-locale summary for the requested locale, or `None` when it is the source.
    ///
    /// This is what makes a bare `kiln --lang ja` visibly honest: the runner says which locale
    /// it actually read, instead of quietly printing English.
    pub fn locale_note(&self) -> Result<Option<String>> {
        if self.locale == self.source {
            return Ok(None);
        }
        let report = status::report(&self.course, &self.locale)?;
        let coverage = report
            .for_locale(&self.locale)
            .map_or((0, 0), |locale| (locale.covered(), locale.total()));
        let chain = report
            .chain
            .iter()
            .map(Locale::as_str)
            .collect::<Vec<_>>()
            .join(" -> ");
        Ok(Some(format!(
            "content locale: {} — {}/{} step bodies, falling back through {chain}",
            self.locale, coverage.0, coverage.1
        )))
    }

    /// The learner's project directory for a step's track.
    pub fn project_dir(&self, step: &Step) -> Result<PathBuf> {
        Ok(self
            .course
            .track(&step.track)?
            .project_dir(self.course.root()))
    }

    /// Run one step's gates without recording anything.
    ///
    /// The single place a [`GateContext`] is built, so `check`, `selftest` and `verify` all judge
    /// with the same locale, deadline and project directory.
    pub fn gates(&self, step: &Step, run_slow: bool) -> Result<Vec<GateResult>> {
        let track = self.course.track(&step.track)?;
        let project_dir = track.project_dir(self.course.root());
        // One fingerprint per call, and only when the step has something cacheable to run: reading
        // the tree is cheap, and reading it for a step whose gates are all step-scoped would be work
        // nobody uses.
        let fingerprint = step
            .gates
            .iter()
            .any(|spec| self.cache.covers(spec))
            .then(|| tree_fingerprint(&project_dir));
        let context = GateContext {
            course: &self.course,
            track,
            step,
            project_dir,
            timeout: self.timeout,
            locale: &self.locale,
        };
        // What has already been answered on this tree, and what still has to run.
        let cached: Vec<Option<GateResult>> = step
            .gates
            .iter()
            .map(|spec| self.cache.get(spec, fingerprint.as_deref()))
            .collect();
        let to_run: Vec<crate::gates::GateSpec> = step
            .gates
            .iter()
            .zip(&cached)
            .filter(|(_, cached)| cached.is_none())
            .map(|(spec, _)| spec.clone())
            .collect();
        let mut fresh = run_all(&context, &to_run, run_slow).into_iter();
        let mut results: Vec<GateResult> = Vec::with_capacity(step.gates.len());
        for (spec, cached) in step.gates.iter().zip(cached) {
            let result = match cached {
                Some(result) => result,
                None => {
                    let result = fresh.next().ok_or_else(|| {
                        KilnError::Invalid(format!(
                            "the gate runner answered fewer gates than it was given for {}",
                            step.qualified_id()
                        ))
                    })?;
                    if let Some(fingerprint) = &fingerprint {
                        self.cache.put(spec, fingerprint, &result);
                    }
                    result
                }
            };
            results.push(result);
        }
        Ok(results)
    }

    /// Run one step's spec tests (`run.cmd`) in the project, without recording anything.
    pub fn spec(&self, step: &Step) -> Result<process::Ran> {
        let run = step.run.as_ref().ok_or_else(|| {
            KilnError::Invalid(format!(
                "{} declares no `run.cmd`, so its spec tests cannot be started",
                step.qualified_id()
            ))
        })?;
        let dir = self.project_dir(step)?;
        process::run_line(&run.cmd, &dir, self.timeout)
    }
}

#[cfg(test)]
mod gate_cache_tests {
    use super::{GateCache, key_of, tree_fingerprint};
    use crate::gates::{GateSpec, Tier};

    /// A gate as a step would declare it.
    fn gate(name: &str, param: Option<&str>) -> GateSpec {
        GateSpec {
            name: name.to_owned(),
            param: param.map(str::to_owned),
            tier: Tier::Must,
            slow: false,
        }
    }

    /// A result worth caching.
    fn result(detail: &str) -> crate::gates::GateResult {
        crate::gates::GateResult {
            spec: gate("fmt", None),
            outcome: crate::gates::Outcome::Passed,
            detail: detail.to_owned(),
            elapsed: std::time::Duration::from_millis(1),
        }
    }

    #[test]
    fn a_tree_that_does_not_change_keeps_its_fingerprint() {
        let dir = std::env::temp_dir().join(format!("kiln-fingerprint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("a scratch directory");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").expect("write");
        std::fs::write(dir.join("src/lib.rs"), "//! x\n").expect("write");

        let first = tree_fingerprint(&dir);
        assert_eq!(first, tree_fingerprint(&dir), "reading twice is one answer");
        assert_eq!(
            first.len(),
            16,
            "a fingerprint is one short hex number: {first}"
        );

        // A rewrite with the same bytes is the *same* tree (a spec sync does exactly that), and a
        // different byte is a different one.
        std::fs::write(dir.join("src/lib.rs"), "//! x\n").expect("write");
        assert_eq!(first, tree_fingerprint(&dir), "same bytes, same tree");
        std::fs::write(dir.join("src/lib.rs"), "//! y\n").expect("write");
        assert_ne!(
            first,
            tree_fingerprint(&dir),
            "a tree that changed is a different question"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gate_is_asked_once_per_tree_and_not_for_a_step_scoped_one() {
        let cache = GateCache::default();
        let fmt = gate("fmt", None);
        let caps = gate("caps", None);
        let tree = "fingerprint";

        assert!(
            cache.get(&fmt, Some(tree)).is_none(),
            "nothing is cached yet"
        );
        cache.put(&fmt, tree, &result("`cargo fmt --check` is clean"));
        assert_eq!(
            cache
                .get(&fmt, Some(tree))
                .expect("the second look hits")
                .detail,
            "`cargo fmt --check` is clean"
        );
        assert_eq!(cache.hits(), 1);

        // A different tree is a different question, and a step-scoped gate is never cached: `caps`
        // judges the step's own metadata, which no project fingerprint describes.
        assert!(cache.get(&fmt, Some("other")).is_none());
        cache.put(&caps, tree, &result("no rule R0-R6 issue"));
        assert!(
            cache.get(&caps, Some(tree)).is_none(),
            "caps is never cached"
        );

        // The parameter is part of the question: `clippy@2` and `clippy@3` are different gates.
        let rung_two = gate("clippy", Some("2"));
        let rung_three = gate("clippy", Some("3"));
        cache.put(&rung_two, tree, &result("rung 2"));
        assert!(
            cache.get(&rung_three, Some(tree)).is_none(),
            "rungs do not share"
        );
        assert_eq!(key_of(&rung_two), key_of(&gate("clippy", Some("2"))));
        assert_ne!(key_of(&rung_two), key_of(&rung_three));
    }
}
