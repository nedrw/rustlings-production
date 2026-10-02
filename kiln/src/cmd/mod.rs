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

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::error::{KilnError, Result};
use crate::i18n::{Locale, Ui, status};
use crate::info::{Course, Step};
use crate::state::State;

pub mod caps;
pub mod check;
pub mod lint_caps;
pub mod list;
pub mod run;

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

fn dispatch(cli: &Cli) -> Result<u8> {
    let mut session = Session::new(cli)?;
    match &cli.command {
        Some(Command::List { track }) => list::run(&session, track.as_deref()),
        Some(Command::Caps { track }) => caps::run(&session, track.as_deref()),
        Some(Command::LintCaps) => lint_caps::run(&session),
        Some(Command::Run { step }) => run::run(&mut session, step.as_deref()),
        Some(Command::Check { step }) => check::run(&mut session, step.as_deref()),
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
}

impl Session {
    /// Load the course, resolve the locale and read progress.
    pub fn new(cli: &Cli) -> Result<Self> {
        let cwd = std::env::current_dir().map_err(|source| KilnError::Read {
            path: PathBuf::from("."),
            source,
        })?;
        let course = Course::discover(&cwd)?;
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
}
