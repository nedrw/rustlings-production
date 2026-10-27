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
use std::process::{Child, Command, Stdio};
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
    ///
    /// A line that names the *failure* wins over a line that merely reports it: a test binary writes
    /// `test name … FAILED` and the panic to stdout while the tool that started it writes one line
    /// to stderr, so stderr-first produces `exit code 101: error: test failed` — a report nobody can
    /// act on (this is what made one red `kiln selftest` run undiagnosable).
    #[must_use]
    pub fn first_message(&self) -> Option<&str> {
        if let Some(line) = self.stdout.lines().find(|line| names_a_failure(line)) {
            return Some(line.trim());
        }
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

/// Whether a line of output says *what* failed rather than *that* it did.
///
/// One rule for every report that quotes a failing command: `test name … FAILED`,
/// `---- name stdout ----` and `thread 'name' panicked at …` are written by the test binary to
/// stdout, while `error: test failed` is written by the tool that started it to stderr. Both are
/// true; only one tells a reader where to look.
#[must_use]
pub fn names_a_failure(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("panicked at")
        || line.starts_with("assertion")
        || line.starts_with("thread '")
        || line.starts_with("---- ")
        || line.contains("FAILED")
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
        // kiln's own build directory says nothing about where the *project* should build. Letting it
        // through is how two different trees end up sharing one cargo slot — cargo keys a package by
        // name and version, not by directory — and a binary built in one tree carries that tree's
        // `CARGO_MANIFEST_DIR`, which is what this course's specs read their project files through:
        // a stale binary does not merely run old code, it reads another project's files (measured:
        // `docs/budget.md` "cannot be read" from a binary compiled elsewhere, with the file present).
        .env_remove("CARGO_TARGET_DIR")
        .args(args)
        .envs(env.iter().map(|(name, value)| (name, value)))
        .current_dir(dir)
        .stdin(Stdio::null());
    // A caller that names its own build directory knows better than the sandbox default (the
    // `coverage` gate's instrumented build), and the sandbox default is applied only when nothing
    // named one: see `share_cargo_target_dir`.
    let named_by_caller = env.iter().any(|(name, _)| name == "CARGO_TARGET_DIR");
    if let Some(target_dir) = CARGO_TARGET_DIR
        .get()
        .filter(|_| program == "cargo" && !named_by_caller)
    {
        command.env("CARGO_TARGET_DIR", target_dir);
    }
    if program == "cargo" {
        // A slow network must not decide a step. Cargo's defaults give up on a transfer that
        // manages ten bytes in thirty seconds, three times over — a rule tuned for a laptop on
        // a café wifi, not for a machine whose index update stalls while a hundred builds ask
        // for it at once. Measured: a CI run failed at `mailer/1.4-config` with "spurious
        // network error … failed to transfer more than 10 bytes in 30s", and the step was
        // reported as failing when the course had nothing to do with it.
        //
        // The knobs are only *defaults*: a caller that names its own wins, so a learner who
        // wants cargo's own behaviour (or `CARGO_NET_OFFLINE=true`) still gets it.
        // The numbers are bounded by a step's own deadline, because patience that outlasts the step
        // is not patience: a first version used ten retries of two minutes, and a machine with no
        // route to the registry spent the whole 300-second deadline per attempt and reported
        // "timed out" instead of "the network is down" (measured in this repository's CI).
        // Thirty seconds is long enough for a slow transfer, and three of them fail well inside a
        // step's deadline.
        for (name, value) in [
            ("CARGO_NET_RETRY", "3"),
            ("CARGO_HTTP_TIMEOUT", "30"),
            ("CARGO_HTTP_LOW_SPEED_LIMIT", "1"),
        ] {
            if !env.iter().any(|(given, _)| given == name) && std::env::var_os(name).is_none() {
                command.env(name, value);
            }
        }
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
            stop(&mut child);
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

/// Stop the child and everything it started.
///
/// `Child::kill` reaches one process, and a `cargo test` is two: the binary doing the work is one
/// level below the command a gate started, and it is the one that goes on burning a core — and
/// holding the shared build directory's lock, which makes every later command slower — after the
/// command it belonged to has been reported as timed out. Measured in `docs/handoff-m2.md` §4.4: a
/// hung `cargo test` left its test binary alive for forty minutes, and the runs after it looked ten
/// times slower than they were.
///
/// The list is taken *before* the child is signalled: a descendant of a dead process is adopted by
/// `init`, and the link that says whose it was goes with it. `ps` and `kill` are the only way to ask
/// that question and to send that signal from the standard library, and both are POSIX — the same
/// two tools the project's own process fixture already uses for `SIGTERM`.
fn stop(child: &mut Child) {
    for pid in descendants(child.id()) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// The pids running under `root`, in the order they were found.
///
/// An empty answer when `ps` is not there is the honest one: the child is still killed, and a
/// grandchild that survives is the old behaviour rather than a new failure.
fn descendants(root: u32) -> Vec<u32> {
    let Ok(listing) = Command::new("ps").args(["-A", "-o", "pid=,ppid="]).output() else {
        return Vec::new();
    };
    let mut children: Vec<(u32, u32)> = Vec::new();
    for line in String::from_utf8_lossy(&listing.stdout).lines() {
        let mut fields = line.split_whitespace();
        if let (Some(pid), Some(parent)) = (fields.next(), fields.next())
            && let (Ok(pid), Ok(parent)) = (pid.parse(), parent.parse())
        {
            children.push((pid, parent));
        }
    }
    let mut found = Vec::new();
    let mut level = vec![root];
    while let Some(parent) = level.pop() {
        for (pid, its_parent) in &children {
            if *its_parent == parent {
                found.push(*pid);
                level.push(*pid);
            }
        }
    }
    found
}

/// Run a whitespace-separated command line — the form `run.cmd` uses in `track.toml`.
pub fn run_line(line: &str, dir: &Path, timeout: Duration) -> Result<Ran> {
    let (program, args) = split_command(line)?;
    run(program, &args, dir, timeout)
}

/// The same, with environment variables set for the child.
///
/// A gate that re-runs a step's own command with a knob of its own needs this: `conformance` runs the
/// same spec once per backend (`KILN_BACKEND=…`), and `coverage` runs it under `RUSTFLAGS`. Reusing
/// the step's command is the point — the gate judges the spec the course shipped, not a paraphrase of
/// it.
pub fn run_line_with_env(
    line: &str,
    dir: &Path,
    timeout: Duration,
    env: &[(String, String)],
) -> Result<Ran> {
    let (program, args) = split_command(line)?;
    run_with_env(program, &args, dir, timeout, env)
}

/// `true` when a command's output says it could not reach the crate registry.
///
/// This is the one failure a step must never be blamed for. The strings are cargo's own: a
/// connection that never opened, a transfer that stalled, and the resolution error that follows
/// them. A verdict that says "the spec failed" when the registry is unreachable sends the reader to
/// the wrong file — the step is fine, the machine has no route to `index.crates.io`.
#[must_use]
pub fn network_trouble(ran: &Ran) -> bool {
    let said = format!("{}\n{}", ran.stdout, ran.stderr);
    [
        "spurious network error",
        "failed to get `",
        "failed to download",
        "unable to update registry",
        "Connection timed out",
        "Couldn't connect to server",
    ]
    .iter()
    .any(|marker| said.contains(marker))
}

/// Split a whitespace-separated command line into program and arguments, without a shell.
pub fn split_command(line: &str) -> Result<(&str, Vec<&str>)> {
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

/// Where cargo will build for a command started in `project`.
///
/// The same answer the child gets, from the same one place: [`share_cargo_target_dir`] when a run has
/// asked for a shared directory, and the project's own `target/` otherwise — never the environment
/// kiln itself was started in, because [`prepared`] clears that variable out of every child (a gate
/// that looked there would look where the build did not go). A gate that has to find what cargo
/// produced (`coverage` needs the binaries a profile was written from) asks here instead of
/// guessing, because a wrong answer would report a missing measurement rather than a wrong one.
#[must_use]
pub fn target_dir(project: &Path) -> PathBuf {
    CARGO_TARGET_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| project.join("target"))
}

/// `true` when `path` is a file this machine would let a program run.
#[must_use]
pub fn is_executable(path: &Path) -> bool {
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
    #[test]
    fn a_registry_failure_is_recognised_as_the_network() {
        let said = |text: &str| Ran {
            code: Some(101),
            stdout: String::new(),
            stderr: text.to_owned(),
            timed_out: false,
            elapsed: Duration::from_secs(1),
        };
        assert!(network_trouble(&said(
            "warning: spurious network error (3 tries remaining): [28] Timeout was reached"
        )));
        assert!(network_trouble(&said(
            "error: failed to get `serde` as a dependency of package `mailer v0.1.0`"
        )));
        assert!(
            !network_trouble(&said("test result: FAILED. 3 passed; 1 failed; 0 ignored")),
            "a failing test is not a network problem"
        );
    }

    /// The network knobs a judged cargo command gets.
    #[test]
    fn cargo_is_told_to_be_patient_with_a_slow_network() {
        let command = prepared("cargo", &["test"], Path::new("."), &[]);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for (name, value) in [
            ("CARGO_NET_RETRY", "3"),
            ("CARGO_HTTP_TIMEOUT", "30"),
            ("CARGO_HTTP_LOW_SPEED_LIMIT", "1"),
        ] {
            let given = envs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone());
            assert_eq!(
                given.flatten().as_deref(),
                Some(value),
                "{name} is the runner's default"
            );
        }
    }

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
    fn a_timed_out_command_takes_its_grandchildren_with_it() {
        // The case this exists for is `cargo test`, whose test binary is one level below the
        // command a gate started: killing only the direct child leaves the process that is actually
        // running behind (and holding the shared build directory's lock). `sleep 37` is a duration
        // nothing else on this machine is likely to be using.
        let temp = TempDir::new("process-descendants");
        let ran = run(
            "sh",
            &["-c", "sleep 37"],
            temp.path(),
            Duration::from_millis(300),
        )
        .unwrap();
        assert!(ran.timed_out, "{ran:?}");
        // The signal is asynchronous, so the answer is polled rather than assumed.
        let mut survivors = String::from("not looked at");
        for _ in 0..40 {
            let listing = Command::new("ps")
                .args(["-A", "-o", "command="])
                .output()
                .expect("ps is POSIX");
            survivors = String::from_utf8_lossy(&listing.stdout)
                .lines()
                .filter(|line| line.contains("sleep 37"))
                .collect::<Vec<_>>()
                .join("; ");
            if survivors.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            survivors.is_empty(),
            "a grandchild outlived the deadline: {survivors}"
        );
    }

    #[test]
    fn a_caller_named_target_directory_beats_the_sandbox_one() {
        // Two builds that differ only in `RUSTFLAGS` are two builds, so a gate that instruments the
        // project has to be able to keep its objects somewhere of its own — otherwise it invalidates
        // every ordinary build that follows it (measured: a cold rebuild of this project is 22 s).
        // The caller's name wins, and the sandbox default only ever reaches `cargo`: the other
        // programs a gate starts are not build tools.
        let temp = TempDir::new("process-target-dir");
        let sandbox = temp.path().join("sandbox-target");
        share_cargo_target_dir(sandbox);
        let mine = temp.path().join("instrumented-target");
        let named = [("CARGO_TARGET_DIR".to_owned(), mine.display().to_string())];
        let asked = |command: &Command| -> Option<String> {
            command
                .get_envs()
                .find(|(name, _)| *name == "CARGO_TARGET_DIR")
                .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
        };

        let instrumented = prepared("cargo", &["test"], temp.path(), &named);
        assert_eq!(
            asked(&instrumented).as_deref(),
            Some(mine.to_string_lossy().as_ref())
        );

        // The variable kiln itself was started with is *cleared*, not inherited: a caller's
        // `CARGO_TARGET_DIR` (say, one used to keep kiln's own build out of a repository) must not
        // decide where the project builds, or two trees can share one cargo slot. A program that is
        // not cargo gets no value at all — the removal is what makes that explicit.
        let shell = prepared("sh", &["-c", "true"], temp.path(), &[]);
        assert_eq!(asked(&shell), None);
        assert!(
            shell
                .get_envs()
                .any(|(name, value)| name == "CARGO_TARGET_DIR" && value.is_none()),
            "the inherited value has to be removed explicitly"
        );

        // And a cargo command with nothing named gets the sandbox's shared directory, which is the
        // one place that keeps `selftest` from compiling the dependency graph per step.
        let plain = prepared("cargo", &["test"], temp.path(), &[]);
        assert_eq!(
            asked(&plain),
            Some(target_dir(temp.path()).display().to_string())
        );
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
