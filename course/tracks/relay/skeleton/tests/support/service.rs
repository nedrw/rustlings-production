//! The service as a *process*: its log, its port, and the three things a process can do that a
//! thread cannot (step `relay/2.15-shutdown-leases` is where it earns its keep).
//!
//! The fixtures before this one drive the service *in process* (`tests/support/app.rs` builds a
//! state and a router and runs them in a thread). That is the right shape for a route whose only
//! input is a body. It is the wrong shape for authentication, where the input is a *cookie* or an
//! `Authorization` header and the output is a `Set-Cookie` — a client that cannot send or read
//! headers cannot test any of it, and a process that speaks HTTP is what a browser and a program
//! both talk to.
//!
//! So this file is the other fixture: [`Service`] starts the real binary on a port of the
//! operating system's choosing, keeps its stdout for the test to read, and hands out a [`Client`]
//! with the status line, the headers and the body of whatever was sent. Nothing here sleeps to
//! synchronise: the service is *waited for*, and every wait has a deadline.
//!
//! The port is the one thing this harness reads out of the log, and it reads it for a reason: a
//! test that picks a port by binding `127.0.0.1:0` and closing the listener is racing every other
//! test that does the same — two of them can be handed the same port, and then one service exits
//! with `Address already in use` while the other test's requests may reach the wrong process. Port
//! 0 makes the kernel hand a port to *this* child, and the child says which one it got.
//!
//! What it deliberately does not do is know anything about the course: no route names, no cookie
//! parsing, no JSON helpers. A test that wants one of those writes it, so that what a test asserts
//! stays visible in the test.

#![allow(dead_code)] // a spec uses the part of this harness it needs, not all of it

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};

use super::client::Client;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// How long the service is given to accept connections, and any one request to answer.
const DEADLINE: Duration = Duration::from_secs(20);
/// How long a process is given to exit after it was asked to stop.
const EXIT_DEADLINE: Duration = Duration::from_secs(15);
/// How often a waiting test looks again.
const RETRY: Duration = Duration::from_millis(20);

/// The service under test: a process, the lines it prints, and where it listens.
pub struct Service {
    child: Child,
    lines: Receiver<String>,
    /// Lines read while waiting for the service to say where it listens. [`Service::log_lines`]
    /// hands these back before whatever is still in the channel, so a test sees every line.
    early: Mutex<Vec<String>>,
    address: SocketAddr,
}

impl Service {
    /// Start the service with `env` as its whole environment, on a port the kernel picks for it.
    ///
    /// Port 0, deliberately: choosing a port by binding `127.0.0.1:0` and closing the listener is a
    /// race against every other test that does the same. The kernel hands a port to *this* child,
    /// and the child says which — [`Service::start`] reads that line out of its log.
    ///
    /// `env_clear` is deliberate: a test that depends on a setting the machine happens to have set
    /// is a test that passes here and fails there. What the service needs, the test spells out.
    pub fn start(env: &[super::Var]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_relay"));
        command
            .args(["--listen-port", "0"])
            .env_clear()
            .stdout(Stdio::piped());
        for var in env {
            command.env(&var.name, &var.value);
        }
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("the relay binary could not be started: {error}"));

        let stdout = child
            .stdout
            .take()
            .expect("the service was started with a pipe");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        let mut service = Self {
            child,
            lines,
            early: Mutex::new(Vec::new()),
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
        };
        service.address = service.wait_for_listening();
        service.wait_until_ready();
        service
    }

    /// The address the service says it bound, read from the line that announces it.
    ///
    /// Port 0 means only the child knows the port, so this is the handshake: wait for the line, take
    /// the address out of it, and keep what was read on the way for [`Service::log_lines`]. A
    /// service that never says it — or one that exits first — is reported with the lines it printed.
    fn wait_for_listening(&mut self) -> SocketAddr {
        let deadline = Instant::now() + DEADLINE;
        loop {
            match self.lines.recv_timeout(RETRY) {
                Ok(line) => {
                    let address = listening_address(&line);
                    self.early
                        .lock()
                        .expect("the fixture is not poisoned")
                        .push(line);
                    if let Some(address) = address {
                        return address;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!(
                        "the service stopped before it said where it listens: {:?}",
                        self.log_lines()
                    );
                }
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "the service exited with {} before it said where it listens: {:?}",
                    status.code().unwrap_or(-1),
                    self.log_lines()
                );
            }
            if Instant::now() >= deadline {
                panic!(
                    "the service did not say where it listens within {DEADLINE:?}: {:?}",
                    self.log_lines()
                );
            }
        }
    }

    /// Where the service listens.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Wait until the service accepts connections, so no test has to sleep and hope.
    ///
    /// The child is asked first: a connect can succeed against *any* listener on that address, so a
    /// service that has already died must not be mistaken for a ready one.
    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "the service exited with {} after saying it listens on {}: {:?}",
                    status.code().unwrap_or(-1),
                    self.address,
                    self.log_lines()
                );
            }
            if TcpStream::connect(self.address).is_ok() {
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "the service at {} never started accepting connections: {:?}",
                    self.address,
                    self.log_lines()
                );
            }
            thread::sleep(RETRY);
        }
    }

    /// A client that speaks to this process, with the same header and body helpers every other
    /// fixture uses (`tests/support/client.rs`).
    #[must_use]
    pub fn client(&self) -> Client {
        Client::new(self.address)
    }

    /// The lines the service has printed so far.
    pub fn log_lines(&self) -> Vec<String> {
        let mut lines = self
            .early
            .lock()
            .expect("the fixture is not poisoned")
            .clone();
        while let Ok(line) = self.lines.recv_timeout(RETRY) {
            lines.push(line);
        }
        lines
    }

    /// Ask the process to stop the way an orchestrator does: `SIGTERM`.
    ///
    /// `std::process::Child` can only send `SIGKILL`, which is the one signal that makes a graceful
    /// shutdown unobservable, so this harness starts a named tool: `kill` is in every POSIX
    /// environment and `-TERM` is the signal a deployment sends. A Windows process has no `SIGTERM`
    /// to answer, which is why this half of the harness is `#[cfg(unix)]`.
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
    /// A step that promises recovery after a crash needs the one signal that cannot be handled: a
    /// graceful stop proves nothing about what a power cut leaves behind.
    pub fn kill(&mut self) {
        self.child
            .kill()
            .unwrap_or_else(|error| panic!("the service could not be killed: {error}"));
        let _ = self.child.wait();
    }

    /// `true` while the process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait for the process to exit and answer the code a shell would see (`-1`: killed by a signal).
    pub fn wait_for_exit(&mut self) -> i32 {
        let deadline = Instant::now() + EXIT_DEADLINE;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.code().unwrap_or(-1),
                Ok(None) => {}
                Err(error) => panic!("the service could not be waited for: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "the service did not exit within {EXIT_DEADLINE:?} of `SIGTERM`: {:?}",
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

/// The address in a line that says the service is listening, if this line is one.
///
/// The two log formats differ around it (`address=…` in the pretty one, `"address":"…"` in JSON),
/// and nothing else in a startup line looks like a socket address, so the scanner is: find
/// `address`, skip what separates it from its value, read the address.
fn listening_address(line: &str) -> Option<SocketAddr> {
    let rest = line.get(line.find("address")? + "address".len()..)?;
    let rest = rest.trim_start_matches(['=', ':', '"', ' ']);
    let end = rest
        .find(|character: char| !(character.is_ascii_alphanumeric() || ".:-[]".contains(character)))
        .unwrap_or(rest.len());
    rest.get(..end)?.parse().ok()
}
