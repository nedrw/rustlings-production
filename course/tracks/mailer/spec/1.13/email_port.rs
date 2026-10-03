//! Spec for `mailer/1.13-email-port`: a signup hands the confirmation email to the sender the service
//! was built with, and a sender that refuses does not turn a signup into a loss.
//!
//! `kiln run` copies this file into the project as `tests/spec_email_port.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * through the route, with a **fake sender** built into the app for the test (a constructor
//!   argument, not a mocking framework — `tests/support/email.rs`), so what the service tried to send
//!   is a value the test can read instead of a mail server it would have to run;
//! * through `psql` (`tests/support/postgres.rs`), because "the subscriber survived a failed send" is
//!   a claim about a row, not about an answer;
//! * the port itself is judged once, over *every* adapter, because a fake that does not behave like
//!   the thing it replaces is a fake that will lie to a test one day.
//!
//! The harness lives in this file on purpose: the factory of step `mailer/1.8-test-harness` builds
//! its state with `AppState::start`, and this step's seam is the other constructor. Nothing here
//! sleeps, and every wait is a connection that either succeeds or has a deadline.

#[path = "support/email.rs"]
mod email;
#[path = "support/postgres.rs"]
mod postgres;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener as StdListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use email::Recording;
use mailer::config::{Config, Flags};
use mailer::domain::Email;
use mailer::email::{EmailSender, Logging, Message};
use mailer::http::{self, AppState};
use mailer::logging::RunId;
use postgres::Postgres;
use serde_json::json;
use tokio::sync::oneshot;

/// How long the harness waits for the service to accept connections.
const READY_DEADLINE: Duration = Duration::from_secs(20);
/// How often it tries again.
const RETRY: Duration = Duration::from_millis(20);

/// A service built with the sender a test chose.
///
/// The same shape as `TestApp` (step `mailer/1.8-test-harness`): the state is built first, the port
/// is the operating system's choice, and the runtime lives in a thread of its own so a test can be an
/// ordinary `#[test]`. The one difference is the argument this step is about.
struct App {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl App {
    /// Start the service on a free port, with this configuration and this sender.
    fn start(config: Config, sender: Arc<dyn EmailSender>) -> Self {
        let listener = StdListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("no port could be bound for the test app: {error}"));
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        listener
            .set_nonblocking(true)
            .expect("the listener can be made non-blocking");

        let (shutdown, stopped) = oneshot::channel();
        let id = RunId::of_this_process();
        let thread = std::thread::Builder::new()
            .name("test-app".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("the test app's runtime starts");
                runtime.block_on(async move {
                    let state = match AppState::start_with_sender(config, &id, sender).await {
                        Ok(state) => state,
                        Err(error) => {
                            eprintln!("the test app could not start: {error}");
                            return;
                        }
                    };
                    let listener = tokio::net::TcpListener::from_std(listener)
                        .expect("the listener is handed to the runtime");
                    tokio::select! {
                        result = http::serve_on(listener, state, &id) => {
                            if let Err(error) = result {
                                eprintln!("the test app stopped: {error}");
                            }
                        }
                        _ = stopped => {}
                    }
                });
            })
            .expect("the test app's thread starts");

        let app = Self {
            address,
            shutdown: Some(shutdown),
            thread: Some(thread),
        };
        app.wait_until_ready();
        app
    }

    /// One `POST` with a JSON body, answered as `(status, body)`.
    fn post(&self, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(self.address).unwrap_or_else(|error| {
            panic!(
                "the test app at {} is not accepting connections: {error}",
                self.address
            )
        });
        stream
            .set_read_timeout(Some(READY_DEADLINE))
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

    /// Wait until the service accepts connections, so no test has to sleep and hope.
    fn wait_until_ready(&self) {
        let deadline = Instant::now() + READY_DEADLINE;
        while Instant::now() < deadline {
            if TcpStream::connect(self.address).is_ok() {
                return;
            }
            std::thread::sleep(RETRY);
        }
        panic!(
            "the test app at {} never started accepting connections",
            self.address
        );
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The configuration a test starts the service with: the defaults, plus what the test adds.
fn config(env: &[(String, String)]) -> Config {
    Config::load(&Flags::default(), env).expect("the fixture configuration is valid")
}

/// The setting a service reads its database from (step `mailer/1.4-config`).
fn database_setting(url: &str) -> (String, String) {
    ("MAILER__DATABASE__URL".to_owned(), url.to_owned())
}

/// One signup body, as a caller writes it.
fn signup_body(email: &str) -> String {
    json!({ "email": email, "name": "Ada Lovelace", "password": "correct horse battery staple" })
        .to_string()
}

/// The token the service stored for an address: the only thing its confirmation link can carry.
fn token_of(postgres: &Postgres, address: &str) -> String {
    postgres.query(&format!(
        "SELECT confirmation_token FROM subscribers WHERE email = '{address}'"
    ))
}

#[test]
fn a_signup_hands_one_confirmation_email_to_the_sender() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body(" Ada@Example.COM "));
    assert_eq!(status, 201, "the fixture signup failed: {body}");

    let delivered = sender.delivered();
    assert_eq!(
        delivered.len(),
        1,
        "a signup sends exactly one email, and this one sent {}: {delivered:?}",
        delivered.len()
    );
    let message = &delivered[0];
    assert_eq!(
        message.to.as_str(),
        "ada@example.com",
        "the email goes to the address as it was stored, not as it was typed"
    );
    assert!(
        !message.subject.is_empty(),
        "an email without a subject is an email that gets reported as spam"
    );
    assert!(
        message.body.contains(&format!(
            "/confirm?token={}",
            token_of(&postgres, "ada@example.com")
        )),
        "the body has to carry the link with the token that is in the row — the only thing \
         `/confirm` accepts: {}",
        message.body
    );
    assert!(
        !sender.attempted().is_empty(),
        "the fake has to be what the route called, not a second path through the service"
    );
}

#[test]
fn an_address_that_is_already_subscribed_gets_no_second_email() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 409, "{body}");

    assert_eq!(
        sender.delivered().len(),
        1,
        "the second signup must not email anybody: the address already has a subscriber, and a \
         second confirmation link is a link that confirms nothing new"
    );
}

#[test]
fn a_sender_that_refuses_does_not_lose_the_subscriber() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    sender.fail_with("the mail server is away");
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(
        status, 201,
        "a mail server that is away is not a signup that failed: the subscriber exists, and the \
         message is a delivery problem ({body})"
    );
    assert_eq!(
        sender.attempted().len(),
        1,
        "the service has to try: a sender that is never called is a message nobody missed"
    );
    assert_eq!(
        sender.delivered().len(),
        0,
        "the fixture sender was told to fail"
    );
    assert_eq!(
        postgres.query("SELECT status FROM subscribers WHERE email = 'ada@example.com'"),
        "pending",
        "the subscriber has to be there, waiting: a signup that is undone by a failed send loses an \
         address this service already has"
    );
}

#[test]
fn a_send_is_attempted_again_after_the_sender_recovers() {
    let postgres = Postgres::start();
    let sender = Arc::new(Recording::new());
    sender.fail_with("the mail server is away");
    let app = App::start(config(&[database_setting(&postgres.url())]), sender.clone());

    let (status, body) = app.post("/signup", &signup_body("ada@example.com"));
    assert_eq!(status, 201, "{body}");
    sender.clear_failure();
    let (status, body) = app.post("/signup", &signup_body("grace@example.com"));
    assert_eq!(status, 201, "{body}");

    let delivered = sender.delivered();
    assert_eq!(sender.attempted().len(), 2, "two signups, two attempts");
    assert_eq!(
        delivered.len(),
        1,
        "one message actually went out, and a failure does not stick to the sender"
    );
    assert_eq!(delivered[0].to.as_str(), "grace@example.com");
}

#[tokio::test]
async fn every_sender_keeps_the_same_contract() {
    // The port is only worth having if an adapter can be swapped for another one: every
    // implementation has to take a message it can deliver, and the fake is the only one that has to
    // be able to refuse — which is the whole reason it exists.
    let message = Message {
        to: Email::parse("ada@example.com").expect("the fixture address is valid"),
        subject: "Confirm your subscription".to_owned(),
        body: "Confirm your subscription by opening the link.".to_owned(),
    };
    let senders: Vec<Box<dyn EmailSender>> = vec![Box::new(Logging), Box::new(Recording::new())];
    for sender in &senders {
        assert!(
            sender.send(message.clone()).await.is_ok(),
            "a message an adapter can deliver has to be taken by every adapter: {sender:?}"
        );
    }

    let refusing = Recording::new();
    refusing.fail_with("the mail server is away");
    assert!(
        refusing.send(message).await.is_err(),
        "a fake that cannot fail cannot be used to test what happens when sending fails"
    );
}
