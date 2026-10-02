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

    /// One-line description used when a command is only a probe.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.timed_out {
            return format!("timed out after {:?}", self.elapsed);
        }
        match self.first_stderr_line() {
            Some(line) => format!("exit code {:?}: {line}", self.code),
            None => format!("exit code {:?}", self.code),
        }
    }
}

/// Run `program` with `args` in `dir`, killing it when `timeout` elapses.
pub fn run(program: &str, args: &[&str], dir: &Path, timeout: Duration) -> Result<Ran> {
    let command = command_line(program, args);
    let started = Instant::now();
    let spawn_error = |source| KilnError::Spawn {
        command: command.clone(),
        dir: dir.to_path_buf(),
        source,
    };

    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_error)?;

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
    let stderr = if timed_out {
        let collected = collect(stderr);
        format!("{collected}\n{command} was killed after {timeout:?}")
    } else {
        collect(stderr)
    };

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
    let mut parts = line.split_whitespace();
    let program = parts.next().ok_or_else(|| {
        KilnError::Invalid("the command is empty: expected something like `cargo test`".to_owned())
    })?;
    let args: Vec<&str> = parts.collect();
    run(program, &args, dir, timeout)
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
}
