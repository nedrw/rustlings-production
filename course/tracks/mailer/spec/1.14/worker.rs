//! Spec for `mailer/1.14-worker`: a signup is answered before the mail server is asked, a queue
//! that is full refuses a request before anything is written for it, and a process that is asked to
//! stop finishes what it has.
//!
//! `kiln run` copies this file into the project as `tests/spec_worker.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * through the route, with a sender that **never answers** ([`Hanging`]), because "the answer no
//!   longer waits for the mail server" is only observable against a mail server that would have made
//!   the answer wait;
//! * through `psql` (`tests/support/postgres.rs`), because "the queue refused before writing" is a
//!   claim about rows, not about an answer;
//! * through a **real process** and a real signal, because graceful shutdown is a property of a
//!   process: the binary is started, `SIGTERM` is what it is sent, and the exit code is what the
//!   shell would see.
//!
//! The fixture is the one of step `mailer/1.13-email-port` (`tests/support/app.rs`) — the state is
//! built with the sender a test chose — so this spec sees the queue (step `mailer/1.14-worker`)
//! behind the same seam. Nothing here sleeps to synchronise: everything is waited for, and every
//! wait has a deadline.
//!
//! One property of the design is visible in what this file *costs*: a queue is filled one accepted
//! signup at a time, and every signup hashes a password, so [`QUEUE_CAPACITY`] has to be a number a
//! test can reach — a bound nobody can fill is a bound nobody can test.

#[path = "support/app.rs"]
mod app;
#[path = "support/email.rs"]
mod email;
#[path = "support/postgres.rs"]
mod postgres;

use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use app::App;
use email::Recording;
use mailer::delivery::{MAX_ATTEMPTS, QUEUE_CAPACITY};
use mailer::email::{EmailSender, Message, SendError};
use postgres::Postgres;

/// How long a test waits for something that is supposed to happen.
const DEADLINE: Duration = Duration::from_secs(10);
/// How long a process is given to exit after it was asked to stop.
const EXIT_DEADLINE: Duration = Duration::from_secs(15);
/// How often a waiting test looks again.
const RETRY: Duration = Duration::from_millis(20);

/// The configuration a test starts the service with: the defaults, plus what the test adds.
fn config(env: &[(String, String)]) -> mailer::config::Config {
    app::config(env)
}

/// The setting a service reads its database from (step `mailer/1.4-config`).
fn database_setting(url: &str) -> (String, String) {
    app::database_setting(url)
}

/// One signup body, as a caller writes it.
fn signup_body(email: &str) -> String {
    app::signup_body(email)
}

/// An address that is its own subscriber: the filler signups of the capacity test need one each.
fn filler(index: usize) -> String {
    format!("subscriber{index}@example.com")
}

/// Wait until `check` answers something, or fail loudly with what the test was waiting for.
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {DEADLINE:?}"
        );
        std::thread::sleep(RETRY);
    }
}

/// A sender that never answers: the mail server that took the connection and then went quiet.
///
/// It is what makes the first test mean something. A service that sends inside the request cannot
/// answer while this sender is outstanding, so the `201` is the proof that the send is not on the
/// request's path any more.
#[derive(Debug)]
struct Hanging;

impl EmailSender for Hanging {
    fn send(
        &self,
        _message: Message,
    ) -> Pin<Box<dyn Future<Output = Result<(), SendError>> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}

/// A sender that refuses the first attempt at every message and takes every one after that.
///
/// It is what makes "a failed send is retried" an observation instead of a hope: this fake refuses
/// *on its own schedule*, so the test never has to let a message fail and then hurry to fix the mail
/// server before the next attempt. The message is delivered or it is not.
#[derive(Debug, Default)]
struct Flaky {
    /// How many attempts have been started, over every message.
    attempts: AtomicUsize,
    /// The messages that were actually taken.
    delivered: Mutex<Vec<Message>>,
}

impl Flaky {
    /// How many attempts this fake was handed, over every message.
    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    /// The messages this fake took.
    fn delivered(&self) -> Vec<Message> {
        self.delivered
            .lock()
            .expect("the fixture lock is not poisoned")
            .clone()
    }
}

impl EmailSender for Flaky {
    fn send(
        &self,
        message: Message,
    ) -> Pin<Box<dyn Future<Output = Result<(), SendError>> + Send + '_>> {
        Box::pin(async move {
            // The counter is incremented before the decision, so the first attempt of the first
            // message is the one that fails, whoever else is sending.
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err(SendError::new(std::io::Error::other(
                    "the mail server is away",
                )));
            }
            self.delivered
                .lock()
                .expect("the fixture lock is not poisoned")
                .push(message);
            Ok(())
        })
    }
}

#[test]
fn a_send_that_fails_once_is_delivered_by_its_retry() {
    let postgres = Postgres::start();
    let sender = Arc::new(Flaky::default());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");

    // No second signup, no new request: the message that failed is the message that arrives, which
    // is the whole difference between a retry and a person trying again.
    let delivered = wait_for("ada's confirmation email", || {
        let delivered = sender.delivered();
        (!delivered.is_empty()).then_some(delivered)
    });
    assert!(
        delivered
            .iter()
            .any(|message| message.to.as_str() == "ada@example.com"),
        "the message that failed once has to be delivered by a retry: {delivered:?}"
    );
    assert_eq!(
        sender.attempts(),
        2,
        "one refusal and one delivery: a queue that answers a transient failure with a single \
         attempt lost this message"
    );
    assert!(
        postgres.query("SELECT status FROM subscribers WHERE email = 'ada@example.com'")
            == "pending",
        "the subscriber is still waiting to confirm — delivered is not confirmed"
    );
}

#[test]
fn a_signup_is_answered_before_it_is_sent_and_a_full_queue_refuses_the_rest() {
    let postgres = Postgres::start();
    let app = App::start(
        config(&[database_setting(&postgres.url())]),
        Arc::new(Hanging),
    );

    // The first signup is answered while the sender is still hanging on its message. On a tree that
    // sends inside the request this is where the test stops: there is no answer to read, and the
    // fixture's own deadline turns that into a failure with a sentence.
    let (status, body) = app.post("/signup", &signup_body(&filler(0)));
    assert_eq!(
        status, 201,
        "a signup has to be answered without waiting for the mail server: it is the queue's \
         message now, not the request's ({body})"
    );

    // One message is in the worker's hands — it will never get a `send` back — and the queue holds
    // `QUEUE_CAPACITY` more behind it. Everything past that has to be refused, and refused *before*
    // it writes anything.
    let mut accepted = 1;
    let mut refused = 0;
    for index in 1..=QUEUE_CAPACITY + 1 {
        let (status, body) = app.post("/signup", &signup_body(&filler(index)));
        match status {
            201 => accepted += 1,
            503 => refused += 1,
            other => panic!(
                "a signup is either accepted or refused because the service is at capacity, \
                 not answered with {other}: {body}"
            ),
        }
    }
    assert!(
        refused >= 1,
        "a queue that cannot fill up is not a bound: {accepted} signups were accepted and none \
         was refused"
    );
    assert!(
        accepted >= QUEUE_CAPACITY,
        "the queue has to absorb the work the answer no longer waits for: only {accepted} of \
         {QUEUE_CAPACITY} messages fit"
    );

    // The row count is the half that makes the refusal honest: a subscriber without a confirmation
    // email is a subscriber nobody can confirm, so a request that is refused because the queue is
    // full must not have inserted anything.
    let rows: usize = postgres
        .query("SELECT count(*) FROM subscribers")
        .trim()
        .parse()
        .expect("`count(*)` is a number");
    assert_eq!(
        rows, accepted,
        "{accepted} signups were accepted and {rows} rows were written: a refused signup must \
         leave the database alone, or the client that retries gets a `409` for a subscriber it \
         cannot confirm"
    );
}

#[test]
fn a_message_that_keeps_failing_is_given_up_on_before_the_next_one() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    sender.fail_with("the mail server is away");
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    let (status, body) = app.post("/signup", &signup_body("grace@example.com"));
    assert_eq!(status, 201, "{body}");

    // One worker, one message at a time: the first attempt at grace's email is the first moment at
    // which ada's message is *finished* — delivered, or given up on. A worker that kept retrying it
    // would never reach grace at all, and this wait would run out of time and say so.
    let attempted = wait_for("grace's email to be handed to the sender", || {
        let attempted = sender.attempted();
        attempted
            .iter()
            .any(|message| message.to.as_str() == "grace@example.com")
            .then_some(attempted)
    });
    let ada = attempted
        .iter()
        .filter(|message| message.to.as_str() == "ada@example.com")
        .count();
    assert_eq!(
        ada, MAX_ATTEMPTS as usize,
        "a mail server that keeps refusing is retried `MAX_ATTEMPTS` times and not once more: \
         {attempted:?}"
    );
    assert_eq!(
        sender.delivered().len(),
        0,
        "the fixture sender was told to fail, so nothing was delivered"
    );

    // And the queue is not stuck on the message it gave up on: the moment the mail server answers
    // again, the next signup goes out.
    sender.clear_failure();
    let (status, body) = app.post("/signup", &signup_body("alan@example.com"));
    assert_eq!(status, 201, "{body}");
    let delivered = wait_for("alan's confirmation email", || {
        let delivered = sender.delivered();
        delivered
            .iter()
            .any(|message| message.to.as_str() == "alan@example.com")
            .then_some(delivered)
    });
    assert!(
        delivered
            .iter()
            .any(|message| message.to.as_str() == "alan@example.com"),
        "the message that could go out did: {delivered:?}"
    );
}

/// A Windows process has no `SIGTERM` to answer, and the step's shutdown path is `#[cfg(unix)]`
/// for the same reason: this test is the Unix half of the course, not a platform-neutral one.
#[cfg(unix)]
#[test]
fn a_process_that_is_asked_to_stop_finishes_what_it_has() {
    let postgres = Postgres::start();
    let url = postgres.url();
    let mut service = Service::start(&[("MAILER__DATABASE__URL", &url)]);

    let (status, body) = service.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");

    // `SIGTERM` is what an orchestrator sends when it takes a container out of service, and it is
    // the signal this step has to answer: stop accepting, finish what was accepted, and exit `0`.
    service.stop();
    let code = service.wait_for_exit();
    assert_eq!(
        code,
        0,
        "a process that drained its queue has to exit `0`: SIGTERM means stop, not abandon \
         (exit code {code}, -1 for a process that was killed): {:?}",
        service.log_lines()
    );

    // The log is the other half of the claim, and it comes from two places: the worker says what it
    // finished, and the adapter says what it was given. A process that exits without either of them
    // exited before its work was done.
    let lines = wait_for("the service to report a drained queue", || {
        let lines = service.log_lines();
        lines
            .iter()
            .any(|line| line.contains("the delivery queue drained"))
            .then_some(lines)
    });
    assert!(
        lines
            .iter()
            .any(|line| line.contains("an email would be sent")),
        "the queued confirmation email has to be delivered before the process exits: {lines:?}"
    );
}

/// A port nothing is listening on right now, asked of the operating system.
#[cfg(unix)]
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port can be found");
    listener
        .local_addr()
        .expect("a bound listener has an address")
        .port()
}

/// The service under test: a process, and the lines it prints while it runs.
#[cfg(unix)]
struct Service {
    child: Child,
    lines: Receiver<String>,
    address: SocketAddr,
}

#[cfg(unix)]
impl Service {
    /// Start the service with `extra` in its environment, on a port of this process's choosing.
    fn start(extra: &[(&str, &str)]) -> Self {
        let port = free_port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_mailer"));
        command
            .args(["--listen-port", &port.to_string()])
            .env_clear()
            .stdout(Stdio::piped());
        for (name, value) in extra {
            command.env(name, value);
        }
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("the mailer binary could not be started: {error}"));

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

        let service = Self {
            child,
            lines,
            address: SocketAddr::from(([127, 0, 0, 1], port)),
        };
        service.wait_until_ready();
        service
    }

    /// Wait until the service accepts connections, so no test has to sleep and hope.
    fn wait_until_ready(&self) {
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            if TcpStream::connect(self.address).is_ok() {
                return;
            }
            std::thread::sleep(RETRY);
        }
        panic!(
            "the service at {} never started accepting connections: {:?}",
            self.address,
            self.log_lines()
        );
    }

    /// One `POST` with a JSON body, answered as `(status, body)`.
    fn post(&self, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(self.address).unwrap_or_else(|error| {
            panic!(
                "the service at {} is not accepting connections: {error}",
                self.address
            )
        });
        stream
            .set_read_timeout(Some(DEADLINE))
            .expect("a read deadline can be set");
        let request = format!(
            "POST {path} HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            self.address,
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .unwrap_or_else(|error| panic!("the request could not be sent: {error}"));
        let mut raw = String::new();
        stream
            .read_to_string(&mut raw)
            .unwrap_or_else(|error| panic!("the answer could not be read: {error}"));

        let (head, body) = raw
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("an HTTP answer has a head and a body: `{raw}`"));
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("the status line has a code: `{head}`"));
        (status, body.to_owned())
    }

    /// Ask the process to stop the way an orchestrator does: `SIGTERM`.
    ///
    /// `std::process::Child` can only send `SIGKILL` ([`Child::kill`]), which is the one signal that
    /// makes a graceful shutdown unobservable, so this is the spec that starts a named tool: `kill`
    /// is in every POSIX environment and `-TERM` is the signal a deployment sends. A Windows
    /// process has no `SIGTERM` to answer, which is why the whole of this file's subprocess
    /// half is `#[cfg(unix)]`.
    fn stop(&self) {
        let pid = self.child.id().to_string();
        let status = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap_or_else(|error| panic!("`kill -TERM {pid}` could not be run: {error}"));
        assert!(status.success(), "`kill -TERM {pid}` answered {status}");
    }

    /// Wait for the process to exit and answer the code a shell would see (`-1`: killed by a signal).
    fn wait_for_exit(&mut self) -> i32 {
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
            std::thread::sleep(RETRY);
        }
    }

    /// The lines the service has printed so far.
    fn log_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(RETRY) {
            lines.push(line);
        }
        lines
    }
}

#[cfg(unix)]
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
