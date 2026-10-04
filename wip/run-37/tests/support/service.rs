//! The daemon as a *process*: its log, its exit code, and the two signals a deployment sends.
//!
//! A service is not a library call. It decides things before anyone can see it — which
//! configuration won, whether the schema was in place before the first claim, what a `SIGTERM` does
//! to a job that is already running — and those are exactly the promises the later steps make. So
//! this fixture starts the real binary with an environment the test spells out and reads the lines
//! it prints.
//!
//! Two decisions worth keeping:
//!
//! * **the readiness marker is a log line**, not a sleep: [`READY`] is the phrase the daemon writes
//!   once it can actually drain (`draining queues=…`), and [`Service::start`] waits for exactly it.
//!   A process that exits first is reported with the lines it printed;
//! * **`env_clear`**: a test that depends on a setting the machine happens to have is a test that
//!   passes here and fails there. What the daemon needs, the test spells out.

#![allow(dead_code)] // a spec uses the part of this harness it needs, not all of it

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// The phrase the daemon prints when it is ready to drain, in both log formats.
pub const READY: &str = "draining";
/// How long the daemon is given to say it is ready.
const READY_DEADLINE: Duration = Duration::from_secs(30);
/// How long a process is given to exit after `SIGTERM`.
const EXIT_DEADLINE: Duration = Duration::from_secs(15);
/// How often a waiting test looks again.
const RETRY: Duration = Duration::from_millis(20);

/// One `jobqd` process, and everything it has said so far.
pub struct Service {
    child: Child,
    lines: Receiver<String>,
    /// Lines read while waiting for the readiness marker. [`Service::log_lines`] hands these back
    /// before whatever is still in the channel, so a test sees every line in order.
    early: Mutex<Vec<String>>,
}

impl Service {
    /// Start the daemon with `env` as its whole environment and wait until it is draining.
    ///
    /// Panics when the process exits or goes quiet before it is ready: inside a test, a fixture that
    /// cannot start is a failure with a sentence, and the sentence is the log it printed.
    pub fn start(env: &[super::Var]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jobqd"));
        command
            .env_clear()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in env {
            command.env(&var.name, &var.value);
        }
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("the jobqd binary could not be started: {error}"));

        let stdout = child
            .stdout
            .take()
            .expect("the daemon was started with a pipe");
        let stderr = child
            .stderr
            .take()
            .expect("the daemon was started with a pipe");
        let (sender, lines) = mpsc::channel();
        for stream in [
            Box::new(stdout) as Box<dyn std::io::Read + Send>,
            Box::new(stderr),
        ] {
            let sender = sender.clone();
            thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut service = Self {
            child,
            lines,
            early: Mutex::new(Vec::new()),
        };
        service.wait_until_ready();
        service
    }

    /// Wait for the line that says the daemon is draining, keeping what it read on the way.
    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + READY_DEADLINE;
        loop {
            match self.lines.recv_timeout(RETRY) {
                Ok(line) => {
                    let ready = line.contains(READY);
                    self.early
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(line);
                    if ready {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "the daemon stopped before it said it was {}: {:?}",
                        READY,
                        self.log_lines()
                    );
                }
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "the daemon exited with {} before it said it was {}: {:?}",
                    status.code().unwrap_or(-1),
                    READY,
                    self.log_lines()
                );
            }
            assert!(
                Instant::now() < deadline,
                "the daemon was not {} within {READY_DEADLINE:?}: {:?}",
                READY,
                self.log_lines()
            );
        }
    }

    /// The lines the process has printed so far, stdout and stderr as they arrived.
    pub fn log_lines(&self) -> Vec<String> {
        let mut lines = self
            .early
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        while let Ok(line) = self.lines.recv_timeout(RETRY) {
            lines.push(line);
        }
        lines
    }

    /// Wait until a line containing `needle` has been printed, and answer it.
    ///
    /// This is the "wait for a condition, not for a clock" that the course asks of every test
    /// (authoring.md §6.4): the deadline is what makes a failure readable, the loop is what makes
    /// it fast.
    pub fn wait_for_line(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + READY_DEADLINE;
        let mut seen = self.log_lines();
        loop {
            if let Some(found) = seen.iter().find(|line| line.contains(needle)) {
                return found.clone();
            }
            match self.lines.recv_timeout(RETRY) {
                Ok(line) => seen.push(line),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {}
            }
            assert!(
                Instant::now() < deadline,
                "no line containing {needle:?} within {READY_DEADLINE:?}: {seen:?}"
            );
        }
    }

    /// `true` while the process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Ask the process to stop the way an orchestrator does: `SIGTERM`.
    ///
    /// `std::process::Child` can only send `SIGKILL`, which is the one signal that makes a graceful
    /// shutdown unobservable, so this harness starts a named tool: `kill` is in every POSIX
    /// environment and `-TERM` is the signal a deployment sends.
    #[cfg(unix)]
    pub fn stop(&self) {
        let pid = self.child.id().to_string();
        let status = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap_or_else(|error| panic!("`kill -TERM {pid}` could not be run: {error}"));
        assert!(status.success(), "`kill -TERM {pid}` answered {status}");
    }

    /// Kill the process the way a machine does: `SIGKILL`, no chance to answer.
    ///
    /// A step that promises recovery after a crash needs the one signal that cannot be handled.
    pub fn kill(&mut self) {
        self.child
            .kill()
            .unwrap_or_else(|error| panic!("the daemon could not be killed: {error}"));
        let _ = self.child.wait();
    }

    /// Wait for the process to exit and answer the code a shell would see (`-1`: killed by signal).
    pub fn wait_for_exit(&mut self) -> i32 {
        let deadline = Instant::now() + EXIT_DEADLINE;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.code().unwrap_or(-1),
                Ok(None) => {}
                Err(error) => panic!("the daemon could not be waited for: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not exit within {EXIT_DEADLINE:?} of `SIGTERM`: {:?}",
                self.log_lines()
            );
            thread::sleep(RETRY);
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
