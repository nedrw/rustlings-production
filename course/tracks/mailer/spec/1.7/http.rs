//! Spec for `mailer/1.7-http`: the service answers what it promises, answers honestly when it
//! cannot, and tells a caller where it listens.
//!
//! `kiln run` copies this file into the project as `tests/spec_http.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The service is started as a real process and judged over a real
//! socket — the step is about the HTTP surface, and a surface that is never spoken to is not one.
//! The HTTP client below is deliberately hand-written on `std::net`: the course's specs stay free
//! of dependencies, and a test that speaks the protocol itself sees exactly what a client sees.
//!
//! Nothing here sleeps its way out of a race: the service is *waited for* ([`wait_until`]).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long the service is given to answer its first request.
const READY_DEADLINE: Duration = Duration::from_secs(20);
/// How often a waiting test tries again.
const RETRY: Duration = Duration::from_millis(25);

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

/// The service under test: a process, and the lines it prints while it runs.
struct Service {
    child: Child,
    lines: Receiver<String>,
}

impl Service {
    /// Start the service with `extra` in its environment, on a port the kernel picks for it.
    ///
    /// Port 0, deliberately: a port chosen by binding `127.0.0.1:0` and closing the listener is a
    /// port two spec files can be handed at once, and then one of them talks to the other's
    /// service. The child says which port it got, and the spec reads that line.
    fn start(extra: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mailer"));
        command
            .args(["--listen-port", "0"])
            .env_clear()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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

        Self { child, lines }
    }

    /// The next line the service printed, waiting at most `timeout` for it.
    fn next_line(&self, timeout: Duration) -> Option<String> {
        // A timeout and a closed pipe mean the same thing here: no more lines for now.
        self.lines.recv_timeout(timeout).ok()
    }

    /// The address of the first line that says where the service listens.
    fn bound_address(&self) -> SocketAddr {
        let deadline = Instant::now() + READY_DEADLINE;
        while Instant::now() < deadline {
            let Some(line) = self.next_line(RETRY) else {
                continue;
            };
            if let Some(address) = listening_address(&line) {
                return address;
            }
        }
        panic!(
            "the service has to log the address it bound (port 0 means only the operating system \
             knows it), and it never did"
        );
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One answer, as a client sees it.
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    /// A header, case-insensitively (the protocol is).
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body, parsed: every answer of this service is JSON.
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("the body is JSON ({error}): `{}`", self.body))
    }
}

/// Send one request and read the whole answer.
///
/// `Connection: close` makes the answer finite, which is what lets the test read to the end.
fn request(address: SocketAddr, method: &str, path: &str) -> Response {
    let mut stream = TcpStream::connect(address)
        .unwrap_or_else(|error| panic!("{} is not accepting connections: {error}", address));
    stream
        .set_read_timeout(Some(READY_DEADLINE))
        .expect("a read deadline can be set");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {address}\r\nconnection: close\r\n\
         content-length: 0\r\n\r\n"
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
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("the status line has a code: `{status_line}`"));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();

    Response {
        status,
        headers,
        body: body.to_owned(),
    }
}

/// Wait until `check` says yes, or give up after [`READY_DEADLINE`].
fn wait_until(mut check: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + READY_DEADLINE;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(RETRY);
    }
    panic!("{what} did not happen within {READY_DEADLINE:?}");
}

#[test]
fn the_service_answers_the_matrix_it_promises() {
    let service = Service::start(&[]);
    let address = service.bound_address();
    wait_until(
        || TcpStream::connect(address).is_ok(),
        "the service to accept connections",
    );

    // Liveness: the process is up.
    let health = request(address, "GET", "/healthz");
    assert_eq!(health.status, 200, "{}", health.body);
    assert_eq!(health.json()["status"], "ok", "{}", health.body);

    // An unknown path is a 404, and the answer says so in a body a client can read.
    let missing = request(address, "GET", "/nope");
    assert_eq!(
        missing.status, 404,
        "an unknown path is not something else: {}",
        missing.body
    );
    assert!(
        missing.json()["status"]
            .as_str()
            .is_some_and(|it| !it.is_empty()),
        "a 404 with an empty body tells a caller nothing: `{}`",
        missing.body
    );

    // A known path with the wrong method is a 405, and `Allow` says what would work: the router
    // knows, a catch-all route would have to guess.
    let wrong_method = request(address, "POST", "/healthz");
    assert_eq!(wrong_method.status, 405, "{}", wrong_method.body);
    let allow = wrong_method
        .header("allow")
        .unwrap_or_else(|| panic!("a 405 carries `Allow`: {:?}", wrong_method.headers));
    assert!(
        allow.contains("GET"),
        "`Allow` has to name the method that works, it said `{allow}`"
    );

    // HEAD is the same route without a body: the framework answers it for a `GET` route.
    let head = request(address, "HEAD", "/healthz");
    assert_eq!(head.status, 200, "{}", head.body);
    assert!(head.body.is_empty(), "HEAD has no body: `{}`", head.body);
}

#[test]
fn the_service_reports_the_address_it_actually_bound() {
    // Port 0 is "whatever is free": the configuration cannot know the port, so the service has to
    // say which one it got — otherwise nobody can send it a request.
    let service = Service::start(&[("MAILER__LOG__FORMAT", "json")]);
    let address = service.bound_address();
    assert_ne!(address.port(), 0, "the bound port is a real one");

    let health = request(address, "GET", "/healthz");
    assert_eq!(
        health.status, 200,
        "the reported address has to be the one that answers: {}",
        health.body
    );
}
