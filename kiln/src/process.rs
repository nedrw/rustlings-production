//! Running external commands, with the deadline implemented here.
//!
//! Two constraints shape this module (plan §9.4): the course must not depend on GNU userland —
//! this machine has no `timeout(1)` at all — so the runner kills its own children; and a runaway
//! build must not soak the runner's memory, so captured output is capped per stream.
//!
//! Commands are argv lists, never shell strings: no quoting rules, no pipelines, and nothing that
//! silently behaves differently under `/bin/sh` versus `bash`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{KilnError, Result};

/// Largest number of bytes kept from each stream.
const OUTPUT_CAP: usize = 256 * 1024;
/// How often a running child is checked for exit or deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Outcome of a command that finished or was killed by the deadline.
#[derive(Debug, Clone)]
pub struct Ran {
    /// Exit code, or `None` when the process died from a signal.
    pub code: Option<i32>,
    /// Captured standard output (capped).
    pub stdout: String,
    /// Captured standard error (capped).
    pub stderr: String,
    /// `true` when the runner killed the command at the deadline.
    pub timed_out: bool,
    /// Wall-clock time the command took.
    pub elapsed: Duration,
}

impl Ran {
    /// `true` when the command exited with status 0 and was not killed.
    #[must_use]
    pub fn success(&self) -> bool {
        !self.timed_out && self.code == Some(0)
    }

    /// First non-empty line of standard error, for one-line reports.
    #[must_use]
    pub fn first_stderr_line(&self) -> Option<&str> {
        self.stderr.lines().find(|line| !line.trim().is_empty())
    }

    /// First line that says something about the outcome.
    ///
    /// Cargo narrates as it goes (`Compiling …`, `Finished …`), and those lines are the first thing
    /// a report would otherwise quote at a learner. Skipping them is what makes an exit-code summary
    /// useful instead of noise.
    #[must_use]
    pub fn first_message(&self) -> Option<&str> {
        let text = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        text.lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !is_progress(line))
    }

    /// One-line description used when a command is only a probe.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.timed_out {
            return format!("timed out after {:?}", self.elapsed);
        }
        match self.first_stderr_line() {
            Some(line) => format!("{}: {}", exit_code_text(self.code), plain(line)),
            None => exit_code_text(self.code),
        }
    }
}

/// How an exit code reads in a report: `exit code 1`, never `Some(1)`.
#[must_use]
pub fn exit_code_text(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit code {code}"),
        None => "the process was killed before it exited".to_owned(),
    }
}

/// A line of a child's output with terminal control sequences removed.
///
/// kiln prints a child's line inside its own report, and a tool that colours its output (podman
/// does) leaves the styling on for every line after it. Parsing keeps the raw text; only what a
/// person reads goes through here.
#[must_use]
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            out.push(character);
            continue;
        }
        match characters.peek() {
            // CSI: `ESC [`, then parameter bytes, then one final byte in `@`..=`~`.
            Some('[') => {
                characters.next();
                for skipped in characters.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&skipped) {
                        break;
                    }
                }
            }
            // OSC: `ESC ]`, then the payload, then `BEL` or `ESC \`.
            Some(']') => {
                characters.next();
                while let Some(skipped) = characters.next() {
                    if skipped == '\u{7}' {
                        break;
                    }
                    if skipped == '\u{1b}' {
                        characters.next();
                        break;
                    }
                }
            }
            // Any other escape introduces one character.
            Some(_) => {
                characters.next();
            }
            None => {}
        }
    }
    out
}

/// A cargo build directory every command this process starts should share, once somebody asks for
/// one.
///
/// `kiln selftest` runs one project nineteen times in a throwaway copy, and without this each of
/// those runs would compile the same dependencies from scratch — minutes per step, spent on crates
/// that did not change. One directory (see [`crate::scratch::shared_target_dir`]) lets cargo keep
/// what it built between runs, which is the difference between a check that takes minutes and one
/// that takes tens of minutes.
static CARGO_TARGET_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Share one cargo build directory across every command this process starts.
///
/// Never the project's own: what asks for this is a *sandboxed* run, whose project is a copy that is
/// deleted when the command ends.
pub fn share_cargo_target_dir(dir: PathBuf) {
    let _ = CARGO_TARGET_DIR.set(dir);
}

/// Run `program` with `args` in `dir`, killing it when `timeout` elapses.
pub fn run(program: &str, args: &[&str], dir: &Path, timeout: Duration) -> Result<Ran> {
    run_with_env(program, args, dir, timeout, &[])
}

/// The same as [`run`], with the child's output going straight to kiln's streams.
pub fn run_streamed_line(line: &str, dir: &Path, timeout: Duration) -> Result<Ran> {
    let (program, args) = split_command(line)?;
    run_streamed(program, &args, dir, timeout)
}

/// The same, with environment variables set for the child.
///
/// The environment is part of what a command *is* for the few callers that compare two runs of the
/// same program (`container-repro` asks a binary inside an image and a binary on this host the same
/// question, and the image's own settings are what make the two answers comparable). Everything else
/// gets the runner's own environment, which is what [`run`] passes on.
pub fn run_with_env(
    program: &str,
    args: &[&str],
    dir: &Path,
    timeout: Duration,
    env: &[(String, String)],
) -> Result<Ran> {
    let mut command = prepared(program, args, dir, env);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    execute(command, program, args, dir, timeout, true)
}

/// Run a command with its output going straight to kiln's own streams.
///
/// This is for the commands a person watches (`kiln run`, and `kiln watch` through it): cargo writes
/// its progress lines to stderr and its test results to stdout, so capturing the two streams and
/// printing one after the other shows the build finishing *after* the tests it ran. Nothing here is
/// parsed — the caller needs the exit status, not the text — so the child owns the terminal's
/// streams and the transcript arrives in the order it was written. On a timeout there is nothing to
/// print after the child's own output, so the note lands in [`Ran::stderr`] for the caller to show.
pub fn run_streamed(program: &str, args: &[&str], dir: &Path, timeout: Duration) -> Result<Ran> {
    let mut command = prepared(program, args, dir, &[]);
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    execute(command, program, args, dir, timeout, false)
}

/// The command every runner here starts: same argv, directory, stdio defaults and cargo knobs.
fn prepared(program: &str, args: &[&str], dir: &Path, env: &[(String, String)]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env.iter().map(|(name, value)| (name, value)))
        .current_dir(dir)
        .stdin(Stdio::null());
    if let Some(target_dir) = CARGO_TARGET_DIR.get().filter(|_| program == "cargo") {
        command.env("CARGO_TARGET_DIR", target_dir);
    }
    command
}

/// Spawn, watch the deadline, and collect what the caller asked for.
fn execute(
    mut command: Command,
    program: &str,
    args: &[&str],
    dir: &Path,
    timeout: Duration,
    capture: bool,
) -> Result<Ran> {
    let rendered = command_line(program, args);
    let started = Instant::now();
    let spawn_error = |source| KilnError::Spawn {
        command: rendered.clone(),
        dir: dir.to_path_buf(),
        source,
    };
    let mut child = command.spawn().map_err(spawn_error)?;

    // Draining the pipes from threads keeps a chatty child from blocking on a full pipe while the
    // main thread waits for it to exit.
    let stdout = child
        .stdout
        .take()
        .map(|pipe| thread::spawn(move || drain(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || drain(pipe)));

    let deadline = started + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(source) => return Err(spawn_error(source)),
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break child.wait().ok();
        }
        thread::sleep(POLL_INTERVAL);
    };

    let collect = |handle: Option<thread::JoinHandle<String>>| {
        handle.map_or_else(String::new, |handle| handle.join().unwrap_or_default())
    };
    let stdout = collect(stdout);
    let mut stderr = collect(stderr);
    if timed_out {
        stderr = format!("{stderr}\n{rendered} was killed after {timeout:?}");
    }
    // In streamed mode the child wrote to the terminal itself, so there is nothing captured to
    // hand back — except the timeout note above, which nobody else can print.
    debug_assert!(capture || stdout.is_empty());

    Ok(Ran {
        code: status.and_then(|status| status.code()),
        stdout,
        stderr,
        timed_out,
        elapsed: started.elapsed(),
    })
}

/// Run a whitespace-separated command line — the form `run.cmd` uses in `track.toml`.
pub fn run_line(line: &str, dir: &Path, timeout: Duration) -> Result<Ran> {
    let (program, args) = split_command(line)?;
    run(program, &args, dir, timeout)
}

/// Split a whitespace-separated command line into program and arguments.
fn split_command(line: &str) -> Result<(&str, Vec<&str>)> {
    let mut parts = line.split_whitespace();
    let program = parts.next().ok_or_else(|| {
        KilnError::Invalid("the command is empty: expected something like `cargo test`".to_owned())
    })?;
    Ok((program, parts.collect()))
}

/// Find a program on `PATH`, without `which` (which does not exist everywhere) or a shell.
#[must_use]
pub fn locate(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

/// Everything the runner prints about a command it started.
#[must_use]
pub fn command_line(program: &str, args: &[&str]) -> String {
    if args.is_empty() {
        return program.to_owned();
    }
    format!("{program} {}", args.join(" "))
}

/// `true` for the lines a command prints about *its own* progress rather than about what it found.
///
/// Cargo narrates what it is doing before it says anything useful — and on a cold or contended run
/// it narrates a lot: a dozen `Compiling …` lines, a `Blocking waiting for file lock on package
/// cache`, an index update. A report that quotes those is a report that says nothing, which is why
/// both the failure summary and every gate's clipped detail filter them out.
#[must_use]
pub(crate) fn is_progress(line: &str) -> bool {
    // Cargo indents its narration, and the gate reports keep the indentation: without this the
    // predicate only ever matched the lines that happened to start in column one.
    let line = line.trim_start();
    [
        "Compiling",
        "Checking",
        "Finished",
        "Running",
        "Blocking",
        "Downloading",
        "Updating",
        // Cargo narrates its dependency resolution before it builds anything. It is the first
        // line of a cold run, so a report that quotes it says "Locking 17 packages" where the
        // learner needs to read the failure.
        "Locking",
        "Adding",
        "Removing",
        "Documenting",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

/// Read a stream to the end, keeping at most [`OUTPUT_CAP`] bytes.
fn drain(mut reader: impl Read) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    while let Ok(read) = reader.read(&mut chunk) {
        if read == 0 {
            break;
        }
        if let Some(room) = OUTPUT_CAP.checked_sub(buffer.len())
            && room > 0
            && let Some(slice) = chunk.get(..read.min(room))
        {
            buffer.extend_from_slice(slice);
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Unix only: the test needs a command that blocks, and `sleep` is the one thing every Unix
    /// has (the course itself avoids depending on it).
    #[cfg(unix)]
    #[test]
    fn a_command_that_outlives_its_deadline_is_killed() {
        let temp = TempDir::new("process-timeout");
        let ran = run("sleep", &["10"], temp.path(), Duration::from_millis(300)).unwrap();
        assert!(ran.timed_out, "{ran:?}");
        assert!(!ran.success());
        assert!(ran.stderr.contains("was killed after"), "{}", ran.stderr);
        assert!(ran.elapsed < Duration::from_secs(5), "{:?}", ran.elapsed);
    }

    #[cfg(unix)]
    #[test]
    fn output_is_captured_and_the_exit_code_is_kept() {
        let temp = TempDir::new("process-output");
        let ran = run(
            "sh",
            &["-c", "printf 'out'; printf 'err' >&2; exit 3"],
            temp.path(),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(ran.stdout, "out");
        assert_eq!(ran.stderr, "err");
        assert_eq!(ran.code, Some(3));
        assert!(!ran.success());
        assert_eq!(ran.first_stderr_line(), Some("err"));
    }

    #[test]
    fn a_missing_program_is_a_spawn_error_naming_the_command() {
        let temp = TempDir::new("process-missing");
        let error = run(
            "kiln-no-such-tool",
            &[],
            temp.path(),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(matches!(error, KilnError::Spawn { .. }), "{error}");
        assert!(error.to_string().contains("kiln-no-such-tool"), "{error}");
    }

    #[test]
    fn a_program_is_located_on_the_path_or_not_at_all() {
        assert!(locate("cargo").is_some(), "cargo is running this test");
        assert!(locate("kiln-definitely-not-a-program").is_none());
        let found = locate("cargo").expect("cargo is on PATH");
        assert!(found.is_absolute(), "{}", found.display());
    }

    #[test]
    fn a_command_line_is_split_on_whitespace_without_a_shell() {
        let temp = TempDir::new("process-line");
        let ran = run_line("printf hello", temp.path(), Duration::from_secs(30)).unwrap();
        assert_eq!(ran.stdout, "hello");

        let error = run_line("   ", temp.path(), Duration::from_secs(30)).unwrap_err();
        assert!(error.to_string().contains("empty"), "{error}");
    }

    #[test]
    fn progress_lines_are_skipped_when_summarising_output() {
        let temp = TempDir::new("process-summary");
        let ran = run(
            "sh",
            &[
                "-c",
                "printf '   Compiling mailer v0.1.0\\n    Finished dev\\nerror: it broke\\n' \
                 >&2; exit 101",
            ],
            temp.path(),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(ran.first_stderr_line(), Some("   Compiling mailer v0.1.0"));
        assert_eq!(ran.first_message(), Some("error: it broke"));
    }

    #[test]
    fn dependency_resolution_is_progress_too() {
        // The first line of a cold `cargo test` in a project with dependencies: quoting it as the
        // reason a step failed tells a course author nothing.
        let temp = TempDir::new("process-locking");
        let ran = run(
            "sh",
            &[
                "-c",
                "printf '    Locking 17 packages to latest Rust 1.98 compatible versions\\n\
                 error: test failed, to rerun pass `-p mailer --test spec_config`\\n' >&2; exit 101",
            ],
            temp.path(),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(
            ran.first_message(),
            Some("error: test failed, to rerun pass `-p mailer --test spec_config`")
        );
    }
}
