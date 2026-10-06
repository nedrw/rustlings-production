//! The daemon as a *process*: the port it bound, the lines it printed, and the signals it answers.
//!
//! A fixture that drove the ingest loop in-process would miss the half of this step that is about a
//! socket: partial reads, a peer that stops mid-frame, a client that sends a length it should not
//! believe. So [`Service`] starts the real binary, waits until it accepts connections, hands the test
//! a socket to write raw bytes into, and can stop it the way an orchestrator does (`SIGTERM`) or the
//! way a power cut does (`SIGKILL`). A store outlives the process that wrote it, so a daemon can
//! also be started again on the same directory ([`Service::restart`]): that pair — a process that
//! dies and a directory that does not — is what makes a crash something a spec can watch instead of
//! describe.
//!
//! Nothing here sleeps to synchronise, and nothing here knows the protocol: a spec cuts the bytes it
//! wants to send, and that is exactly the point — a fixture that built "a valid frame" would hide the
//! difference between a frame the course defines and a frame the fixture happens to agree with.
//!
//! The lines are kept in a buffer the reader threads append to, not in a channel the test drains.
//! That is a measured decision: with a channel, a spec that polled every 50 ms saw nothing for three
//! seconds while the same spec, sleeping instead, saw every line — reading a log must not depend on
//! when somebody looked. Both streams land in the same buffer, because a daemon that panics says so
//! on stderr and a failure report that only quoted stdout would show an empty log and no reason.

#![allow(dead_code)] // a spec uses the part of this harness it needs, not all of it

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
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
    /// Every line the daemon has printed so far, in order.
    lines: Arc<Mutex<Vec<String>>>,
    address: SocketAddr,
    store: StoreDir,
    /// The environment this daemon was started with, so a restart starts the same daemon.
    env: Vec<Var>,
}

impl Service {
    /// Start the daemon on a port the kernel picks, with `env` added to a cleaned environment.
    ///
    /// Port `0`, deliberately: picking a port by binding and closing a listener races every other
    /// spec that does the same. The kernel hands one to *this* child, and the child says which.
    /// `env_clear` is deliberate too: a spec that depends on a setting the machine happens to have is
    /// a spec that passes here and fails there.
    ///
    /// The log **format** is pinned to `text` here, because the specs read the counters out of the
    /// lines and a JSON line spells them `"accepted":3`. Pinning it in the fixture keeps that a
    /// property of how the tests are written rather than of what the daemon defaults to — and a spec
    /// that wants to check the JSON format passes its own `LAKELET_LOG_FORMAT` (the last value wins,
    /// since `env` is applied after this one).
    #[must_use]
    pub fn start(label: &str, env: &[Var]) -> Self {
        let store = StoreDir::new(label);
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let child = start_process(&store, env, &lines);
        let mut service = Self {
            child,
            lines,
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
            store,
            env: env.to_vec(),
        };
        service.wait_until_running();
        service
    }

    /// Stop the daemon the way a crash does, and start it again on the **same** store.
    ///
    /// A restart is one new process and one thing carried over: the store directory. Everything the
    /// old process held in memory is gone, so what answers afterwards is what the store's bytes
    /// hold — which is the difference `lakelet/4.4-wal-recovery` is about, and the reason this
    /// fixture can model a crash at all: the directory outlives the process that wrote it.
    ///
    /// The lines [`Service::log_lines`] answers are the new process's. Keeping the dead one's would
    /// let a spec that waits for a line — `store recovered …`, say — be answered by the run *before*
    /// the restart, which is the line a crash is supposed to make different.
    #[cfg(unix)]
    pub fn restart(&mut self) {
        if self.is_running() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.lines
            .lock()
            .expect("the fixture is not poisoned")
            .clear();
        self.child = start_process(&self.store, &self.env, &self.lines);
        self.wait_until_running();
    }

    /// Wait until the process just started has said where it listens and answers there.
    fn wait_until_running(&mut self) {
        self.address = self.wait_for_listening();
        self.wait_until_ready();
    }

    /// The address the daemon announced, read out of its log.
    fn wait_for_listening(&mut self) -> SocketAddr {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let lines = self.log_lines();
            if let Some(address) = lines.iter().find_map(|line| listening_address(line)) {
                return address;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "the daemon exited with {} before it said where it listens: {lines:?}",
                    status.code().unwrap_or(-1)
                );
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not say where it listens within {DEADLINE:?}: {lines:?}"
            );
            thread::sleep(RETRY);
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
            assert!(
                Instant::now() < deadline,
                "the daemon at {} never started accepting connections: {:?}",
                self.address,
                self.log_lines()
            );
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

    /// Open a connection and answer the socket, so a spec can hold one open or half-close it.
    pub fn connect(&self) -> TcpStream {
        TcpStream::connect(self.address)
            .unwrap_or_else(|error| panic!("the ingest port refused a connection: {error}"))
    }

    /// The lines the daemon has printed so far, in order.
    pub fn log_lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("the fixture is not poisoned")
            .clone()
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

/// Start the daemon on `store` with `env`, and pipe both of its streams into `lines`.
///
/// The one place a process is started, so [`Service::start`] and [`Service::restart`] cannot drift
/// into starting two subtly different daemons: what a restart changes is the process, not the
/// command line or the store.
fn start_process(store: &StoreDir, env: &[Var], lines: &Arc<Mutex<Vec<String>>>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletd"));
    command
        .args([
            "--listen-addr",
            "127.0.0.1:0",
            "--store-dir",
            &store.path().display().to_string(),
        ])
        .env_clear()
        .env("LAKELET_LOG_FORMAT", "text")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let stderr = child
        .stderr
        .take()
        .expect("the daemon was started with a pipe");
    for stream in [
        Box::new(stdout) as Box<dyn std::io::Read + Send>,
        Box::new(stderr),
    ] {
        let lines = Arc::clone(lines);
        thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stream)
                .lines()
                .map_while(Result::ok)
            {
                if let Ok(mut lines) = lines.lock() {
                    lines.push(line);
                } else {
                    break;
                }
            }
        });
    }
    child
}

/// The address in the daemon's `listening on <addr>` line, if this is that line.
///
/// Deliberately a parser of one shape: an unrecognised line answers `None` and is kept for the
/// report, which is what makes a daemon that prints the wrong thing fail with its own log quoted.
fn listening_address(line: &str) -> Option<SocketAddr> {
    let (_, address) = line.split_once("listening on ")?;
    address.split_whitespace().next()?.parse().ok()
}
