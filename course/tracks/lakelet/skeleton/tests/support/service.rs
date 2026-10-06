//! The daemon as a *process*: the port it bound, the lines it printed, and the signals it answers.
//!
//! A fixture that drove the ingest loop in-process would miss the half of this step that is about a
//! socket: partial reads, a peer that stops mid-frame, a client that sends a length it should not
//! believe. So [`Service`] starts the real binary, waits until it accepts connections, hands the test
//! a socket to write raw bytes into, and can stop it the way an orchestrator does (`SIGTERM`) or the
//! way a power cut does (`SIGKILL`).
//!
//! Nothing here sleeps to synchronise, and nothing here knows the protocol: a spec cuts the bytes it
//! wants to send, and that is exactly the point — a fixture that built "a valid frame" would hide
//! the difference between a frame the course defines and a frame the fixture happens to agree with.

#![allow(dead_code)] // a spec uses the part of this harness it needs, not all of it

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use super::{StoreDir, Var};

/// How long the daemon is given to say where it listens, and any one connection to be accepted.
const DEADLINE: Duration = Duration::from_secs(20);
/// How long it is given to exit after it was asked to stop.
const EXIT_DEADLINE: Duration = Duration::from_secs(15);
/// How often a waiting test looks again.
const RETRY: Duration = Duration::from_millis(20);

/// The daemon under test: a process, its store, and the lines it printed.
pub struct Service {
    child: Child,
    lines: Receiver<String>,
    /// Lines read while waiting for the address announcement. [`Service::log_lines`] hands these back
    /// before whatever is still in the channel, so a test sees every line in order.
    early: Mutex<Vec<String>>,
    address: SocketAddr,
    store: StoreDir,
}

impl Service {
    /// Start the daemon on a port the kernel picks, with `env` added to a cleaned environment.
    ///
    /// Port `0`, deliberately: picking a port by binding and closing a listener races every other
    /// spec that does the same. The kernel hands one to *this* child, and the child says which.
    /// `env_clear` is deliberate too: a spec that depends on a setting the machine happens to have is
    /// a spec that passes here and fails there.
    #[must_use]
    pub fn start(label: &str, env: &[Var]) -> Self {
        let store = StoreDir::new(label);
        let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletd"));
        command
            .args([
                "--listen-addr",
                "127.0.0.1:0",
                "--store-dir",
                &store.path().display().to_string(),
            ])
            .env_clear()
            .stdout(Stdio::piped());
        for var in env {
            command.env(&var.name, &var.value);
        }
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("the lakeletd binary could not be started: {error}"));

        let stdout = child
            .stdout
            .take()
            .expect("the daemon was started with a pipe");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
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
            store,
        };
        service.address = service.wait_for_listening();
        service.wait_until_ready();
        service
    }

    /// The address the daemon announced, read out of its log.
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
                        "the daemon stopped before it said where it listens: {:?}",
                        self.log_lines()
                    );
                }
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "the daemon exited with {} before it said where it listens: {:?}",
                    status.code().unwrap_or(-1),
                    self.log_lines()
                );
            }
            if Instant::now() >= deadline {
                panic!(
                    "the daemon did not say where it listens within {DEADLINE:?}: {:?}",
                    self.log_lines()
                );
            }
        }
    }

    /// Wait until the port accepts a connection, so no spec has to sleep and hope.
    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "the daemon exited with {} after saying it listens on {}: {:?}",
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
                    "the daemon at {} never started accepting connections: {:?}",
                    self.address,
                    self.log_lines()
                );
            }
            thread::sleep(RETRY);
        }
    }

    /// Where the ingest port listens.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Where this daemon's store lives.
    #[must_use]
    pub fn store(&self) -> &StoreDir {
        &self.store
    }

    /// Send raw bytes to the ingest port and close the connection.
    ///
    /// Raw on purpose: a spec that wants to send half a frame, a hostile length or a corrupted
    /// checksum has to be able to write exactly those bytes.
    pub fn send(&self, bytes: &[u8]) {
        let mut connection = TcpStream::connect(self.address)
            .unwrap_or_else(|error| panic!("the ingest port refused a connection: {error}"));
        connection
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("the bytes could not be sent: {error}"));
        connection
            .flush()
            .unwrap_or_else(|error| panic!("the bytes could not be flushed: {error}"));
    }

    /// Send bytes, leave the connection open, and answer the socket so a spec can half-close it.
    pub fn connect(&self) -> TcpStream {
        TcpStream::connect(self.address)
            .unwrap_or_else(|error| panic!("the ingest port refused a connection: {error}"))
    }

    /// The lines the daemon has printed so far.
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

    /// Wait until a line containing `needle` was printed, and answer it.
    ///
    /// Waiting on the log instead of sleeping is what makes a spec about "the daemon noticed"
    /// reliable on a machine that is compiling something else at the same time.
    pub fn wait_for_line(&self, needle: &str) -> String {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(line) = self
                .log_lines()
                .into_iter()
                .find(|line| line.contains(needle))
            {
                return line;
            }
            assert!(
                Instant::now() < deadline,
                "no line containing {needle:?} within {DEADLINE:?}: {:?}",
                self.log_lines()
            );
            thread::sleep(RETRY);
        }
    }

    /// Ask the process to stop the way an orchestrator does: `SIGTERM`.
    #[cfg(unix)]
    pub fn stop(&self) {
        let pid = self.child.id().to_string();
        let status = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap_or_else(|error| panic!("`kill -TERM {pid}` could not be run: {error}"));
        assert!(status.success(), "`kill -TERM {pid}` answered {status}");
    }

    /// Kill the process the way a power cut does: `SIGKILL`, no chance to answer.
    ///
    /// The step that promises recovery (`4.4`) needs the one signal that cannot be handled: a
    /// graceful stop proves nothing about what a crash leaves behind.
    pub fn kill(&mut self) {
        self.child
            .kill()
            .unwrap_or_else(|error| panic!("the daemon could not be killed: {error}"));
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
        if self.is_running() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The address in the daemon's `listening on <addr>` line, if this is that line.
///
/// Deliberately a parser of one shape: an unrecognised line answers `None` and is kept for the
/// report, which is what makes a daemon that prints the wrong thing fail with its own log quoted.
fn listening_address(line: &str) -> Option<SocketAddr> {
    let (_, address) = line.split_once("listening on ")?;
    address.trim().split_whitespace().next()?.parse().ok()
}
