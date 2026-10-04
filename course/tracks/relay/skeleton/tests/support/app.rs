//! The service *in process*: a state, a router, a thread, and a port the kernel picked.
//!
//! This is what most specs want. A route whose input is a body and whose output is an answer does
//! not need a child process, and starting one costs more than the assertions do. What a process
//! adds — signals, a log, a second instance, a `kill -9` — is [`super::service`]'s job, and a spec
//! that needs any of those uses it instead.
//!
//! The three promises of this fixture, in the order they matter:
//!
//! * **one app per test.** The state is built before the socket is armed, the port is the kernel's
//!   choice, and dropping the value stops the service. Two specs running at the same time share
//!   nothing;
//! * **the same pieces the binary uses.** [`relay::http::serve_on`] serves on a listener this
//!   module bound, so a test cannot drift away from what is deployed — and the state is built by
//!   the same [`AppState::start`] the binary calls, migrations included;
//! * **a runtime of its own, in a thread.** A test stays an ordinary blocking `#[test]`; the
//!   service is a process-like thing that happens to live in this one.

#![allow(dead_code)] // a spec uses the part of this harness it needs, not all of it

use std::net::{SocketAddr, TcpListener as StdListener};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use relay::config::{Config, Flags};
use relay::http::{self, AppState};
use relay::logging::RunId;
use tokio::sync::oneshot;

use super::client::Client;

/// How long the fixture waits for the service to accept connections.
pub const READY_DEADLINE: Duration = Duration::from_secs(20);
/// How often it looks again.
const RETRY: Duration = Duration::from_millis(20);

/// A service running for the length of one test.
pub struct App {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl App {
    /// Start the service on a free port, with this configuration.
    ///
    /// The port in `config` is ignored on purpose: a test that could choose its port would choose
    /// one another test is using. The service announces the port it actually bound.
    ///
    /// Panics when the state cannot be built or the service never answers: inside a test, "the
    /// fixture could not start" is a failure with a sentence, not a `Result` every caller unwraps.
    pub fn start(config: Config) -> Self {
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
            .name("relay-test-app".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("the test app's runtime starts");
                runtime.block_on(async move {
                    // Build the state before arming the socket (the binary does the same, in the
                    // same order): a state that cannot be built leaves the port un-armed, so the
                    // test sees a refused connection and this sentence on stderr instead of
                    // waiting for an answer that is never coming.
                    let state = match AppState::start(config, &id).await {
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

    /// Where the app listens.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// A client that speaks to this app.
    #[must_use]
    pub fn client(&self) -> Client {
        Client::new(self.address)
    }

    /// Stop the app and wait for its thread.
    pub fn stop(mut self) {
        self.shutdown_now();
    }

    /// Drop the shutdown half without consuming the value, for `Drop`.
    fn shutdown_now(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Wait until the service accepts connections, so no test has to sleep and hope.
    fn wait_until_ready(&self) {
        let deadline = Instant::now() + READY_DEADLINE;
        while Instant::now() < deadline {
            if std::net::TcpStream::connect(self.address).is_ok() {
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
        self.shutdown_now();
    }
}

/// The configuration a test starts the service with: the defaults, plus what the test adds.
///
/// `Flags::default()` is the point: a fixture that read the process environment would let the
/// machine decide what the test runs against.
#[must_use]
pub fn config(env: &[super::Var]) -> Config {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|var| (var.name.clone(), var.value.clone()))
        .collect();
    Config::load(&Flags::default(), &env).expect("the fixture configuration is valid")
}

/// The setting a service reads its database from.
pub const DATABASE_URL: &str = "RELAY__DATABASE__URL";
/// The setting that seeds tenants, as `name=key,name=key`.
pub const BOOTSTRAP_TENANTS: &str = "RELAY__BOOTSTRAP__TENANTS";
