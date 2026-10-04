//! What the daemon does with a job: the two handlers it can run, and the rule that picks one.
//!
//! A queue engine cannot know what a job *does*. That is the application's business, and this crate
//! is the engine — so the worker takes a [`Handler`] (`crate::service::worker`) and this module is
//! the daemon's answer to "which one": the program the configuration names, or the one that records
//! the job and takes it as done.
//!
//! Two decisions are worth the paragraphs:
//!
//! * **no handler is a recording handler, not a failing one.** A daemon that has been started
//!   without a `worker.handler` still drains its queue: it logs each job and settles it as
//!   succeeded. The alternative — failing every job — would spend a queue's attempt budgets on a
//!   misconfiguration, and the first thing a deployment does with a wrong setting is start it
//!   again. What is *not* offered is silence: the reference handler says what it took, and the
//!   step body says so where an operator will read it.
//! * **a handler is one program, its payload on standard input.** The program and its arguments are
//!   whitespace-separated in one setting and are never interpreted by a shell: a payload that
//!   contains `; rm -rf /` is a payload, not a command line. The exit status is the verdict —
//!   zero means the job is done, anything else is an attempt that failed, with the status as the
//!   reason the row keeps.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use crate::engine::domain::Job;
use crate::engine::error::Error;
use crate::engine::store::BoxFuture;
use crate::service::config::Config;
use crate::service::worker::Handler;

/// How often the daemon looks at a handler's child process.
///
/// The daemon polls rather than waits for a signal, because the wait has to be *interruptible*: the
/// drain deadline has to be able to give up on a program that will not finish, and a future that is
/// blocked inside a wait cannot be. The interval is short enough that a handler's real work dominates
/// it by orders of magnitude, and long enough that an idle worker with one job is not a busy loop.
const POLL: Duration = Duration::from_millis(20);

/// The handler the configuration asks for.
///
/// # Errors
///
/// [`Error::Invalid`] when `worker.handler` is not a program the daemon can name (an empty setting,
/// or one whose program is not there): this is a start-up decision, so it is refused before the
/// first claim rather than discovered by the first job.
pub fn for_config(config: &Config) -> Result<Box<dyn Handler>, Error> {
    match config.worker.handler.as_deref() {
        Some(command) => Ok(Box::new(CommandHandler::parse(command)?)),
        None => Ok(Box::new(LogHandler)),
    }
}

/// The handler a deployment gets when it names no program: it records the job and takes it as done.
#[derive(Debug, Default)]
pub struct LogHandler;

impl Handler for LogHandler {
    fn handle<'a>(&'a self, job: &'a Job) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            tracing::info!(
                job = %job.id(),
                queue = %job.queue(),
                attempt = job.attempts(),
                payload = %job.payload(),
                "handled"
            );
            Ok(())
        })
    }
}

/// The handler that runs a program: one process per attempt, the payload on its standard input.
///
/// The program's own output is *inherited* rather than captured — it lands in the daemon's streams,
/// next to the log lines that name the job it belongs to, which is where an operator looks for it.
/// That is also what keeps this handler from deadlocking: nothing here reads a pipe after the child,
/// so a program that prints more than a pipe buffer of output is a program whose output goes
/// straight out rather than a daemon that stops.
#[derive(Debug, Clone)]
pub struct CommandHandler {
    /// The program to run.
    program: PathBuf,
    /// What to run it with.
    arguments: Vec<String>,
}

impl CommandHandler {
    /// Parse a `worker.handler` setting: a program, then its arguments, separated by whitespace.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when the setting names no program, or names one that cannot be found: a
    /// handler that does not exist is a deployment mistake, and finding out at the first job means
    /// finding out after the job has been claimed.
    pub fn parse(command: &str) -> Result<Self, Error> {
        let mut words = command.split_whitespace();
        let Some(program) = words.next() else {
            return Err(Error::Invalid {
                detail:
                    "`worker.handler` is empty: name the program to run for each job, or leave \
                         the setting out and let the daemon record the jobs it takes"
                        .to_owned(),
            });
        };
        let program = PathBuf::from(program);
        // `which` is not a dependency and not a shell builtin: the check a daemon can honestly make
        // without one is that the *path* is a file, which covers the common mistake (a name that is
        // not on the PATH) with a sentence the operator can act on.
        if program.components().count() > 1 && !program.is_file() {
            return Err(Error::Invalid {
                detail: format!(
                    "`worker.handler` names `{}`, which is not a file: a program without a path is \
                     looked up in the daemon's PATH by the operating system, and a path has to be \
                     right",
                    program.display()
                ),
            });
        }
        Ok(Self {
            program,
            arguments: words.map(str::to_owned).collect(),
        })
    }
}

impl Handler for CommandHandler {
    fn handle<'a>(&'a self, job: &'a Job) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let payload = serde_json::to_vec(job.payload()).map_err(|error| Error::Invalid {
                detail: format!("this job's payload cannot be written out: {error}"),
            })?;
            let mut child = Started::spawn(self, &payload)?;
            let status = child.wait().await?;
            if status.success() {
                return Ok(());
            }
            Err(Error::Io {
                source: std::io::Error::other(format!(
                    "`{}` exited {}",
                    self.program.display(),
                    describe(status)
                )),
            })
        })
    }
}

/// How a child process ended, for the reason a failed attempt keeps.
fn describe(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("with status {code}"),
        None => "before it could exit: the signal it was sent ended it".to_owned(),
    }
}

/// A child process that is ended when this value goes away.
///
/// The drain deadline aborts the task of a job that is taking too long, and dropping a future runs no
/// code of the future's own — so the kill has to be in a `Drop`, or the daemon leaves a program
/// running after it has decided to stop waiting for it. (`tokio::process::Command` has the same
/// feature under the name `kill_on_drop`; this crate does not have that feature turned on, and a
/// `Drop` is the whole of what it does.)
struct Started {
    /// The child, reaped by `Drop`.
    child: Child,
}

impl Started {
    /// Start the handler's program and offer it the payload.
    fn spawn(handler: &CommandHandler, payload: &[u8]) -> Result<Self, Error> {
        let mut child = Command::new(&handler.program)
            .args(&handler.arguments)
            .stdin(Stdio::piped())
            // Inherited, so the program's own output goes where the daemon's log goes rather than
            // into a pipe nobody is reading.
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|source| Error::Io { source })?;
        if let Some(mut stdin) = child.stdin.take() {
            // A handler that answers before it reads its input has decided not to read it, and the
            // broken pipe that follows is not a job failure: the payload was offered.
            let _ = stdin.write_all(payload);
        }
        Ok(Self { child })
    }

    /// Wait for the program to end, without blocking the thread it is running on.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the operating system cannot be asked about the child at all.
    async fn wait(&mut self) -> Result<ExitStatus, Error> {
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|source| Error::Io { source })?
            {
                return Ok(status);
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        // The `wait` is not politeness: a killed child that is never reaped stays a zombie until the
        // daemon exits, and a worker that hands back a job every few seconds would leave one behind
        // every time.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
