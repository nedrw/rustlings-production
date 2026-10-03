//! Spec for `mailer/1.9-postgres`: an empty database reaches the schema the later steps build on,
//! and a database the service cannot use stops it instead of leaving it half-serving.
//!
//! `kiln run` copies this file into the project as `tests/spec_migrations.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Everything is judged from outside the service:
//!
//! * every claim about the schema is asked of Postgres itself (`psql`, through the container harness
//!   in `tests/support/postgres.rs`), so the migrations are judged by what they did to a database
//!   that started empty;
//! * the service is started the way a deployment starts it — the `MAILER__DATABASE__URL` setting,
//!   `TestApp` (step `mailer/1.8-test-harness`) and, for the failure case, the binary itself —
//!   because "migrations run before the service serves" is a statement about a process.
//!
//! Nothing here sleeps its way out of a race, and nothing waits without a deadline: a service that
//! is *wrong* about its database has to fail this file loudly, not hang it (the deadline is
//! enforced in Rust, since the machine has no `timeout(1)`).

// Each spec uses the part of the shared harness it needs; what it does not use is not dead code, it
// is the rest of the harness (this is the one spec that needs two of them at once).
#[path = "support/postgres.rs"]
mod postgres;
#[allow(dead_code)]
mod support;

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use mailer::config::{Config, Flags};
use postgres::Postgres;
use support::TestApp;

/// How long the service is given to start — or to give up and stop.
const STARTUP_DEADLINE: Duration = Duration::from_secs(10);
/// How often a test looks again while it waits.
const RETRY: Duration = Duration::from_millis(25);

/// The configuration a test starts the service with: the defaults, plus what the test adds.
fn config(env: &[(String, String)]) -> Config {
    Config::load(&Flags::default(), env).expect("the fixture configuration is valid")
}

/// The setting a service reads its database from (step `mailer/1.4-config`).
fn database_setting(url: &str) -> (String, String) {
    ("MAILER__DATABASE__URL".to_owned(), url.to_owned())
}

/// A port nothing is listening on, asked of the operating system.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port can be found");
    listener
        .local_addr()
        .expect("a bound listener has an address")
        .port()
}

#[test]
fn an_empty_database_reaches_the_current_schema_before_the_service_serves() {
    let postgres = Postgres::start();
    // The premise of the test is a database with *nothing* in it, and the harness is what has to
    // hand one over: if that ever stops being true, this test would pass for the wrong reason.
    assert_eq!(
        postgres.query("SELECT to_regclass('subscribers')::text"),
        ""
    );
    assert_eq!(
        postgres.query("SELECT to_regclass('_sqlx_migrations')::text"),
        ""
    );

    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    // Readiness means "the schema this service needs is there", so the answer is the same fact as
    // "the migrations ran": the service cannot be ready and unmigrated at once.
    let (status, body) = app.get("/readyz");
    assert_eq!(status, 200, "{body}");

    assert_eq!(
        postgres.query("SELECT count(*) FROM subscribers"),
        "0",
        "migrating an empty database must not invent rows"
    );
    assert_eq!(
        postgres.query("SELECT version::text || ':' || success::text FROM _sqlx_migrations"),
        "1:true",
        "the migration that ran has to be recorded as applied, or the next start runs it again"
    );
    app.stop();
}

#[test]
fn the_schema_is_the_one_the_later_steps_build_on() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));
    assert_eq!(
        app.get("/readyz").0,
        200,
        "the fixture database is not ready"
    );

    // The columns, by name: steps 1.10-1.18 read and write exactly these, and a rename here is a
    // rename in four later steps.
    assert_eq!(
        postgres.query(
            "SELECT string_agg(column_name, ', ' ORDER BY column_name) FROM information_schema.\
             columns WHERE table_name = 'subscribers'"
        ),
        "confirmation_expires_at, confirmation_token, confirmed_at, email, id, name, \
         password_hash, status, subscribed_at",
        "the table the migrations create is not the table the later steps need"
    );

    // `status` says where a subscriber is, and only in the words this service knows.
    postgres.execute(
        "INSERT INTO subscribers (email, name, password_hash) VALUES ('first@example.com', \
         'First', 'not-a-real-hash')",
    );
    assert_eq!(
        postgres.query("SELECT status FROM subscribers WHERE email = 'first@example.com'"),
        "pending",
        "a subscriber who has not confirmed yet is pending, without anyone having to say so"
    );
    assert!(
        postgres
            .try_execute(
                "INSERT INTO subscribers (email, name, password_hash, status) VALUES \
                 ('second@example.com', 'Second', 'not-a-real-hash', 'confirmed')"
            )
            .is_err(),
        "a status outside the set the code knows has to be refused by the database, not by review"
    );

    // `email` is unique, and the database is what enforces it: step 1.11 lets two concurrent
    // signups race on exactly this constraint instead of reading and then writing.
    assert!(
        postgres
            .try_execute(
                "INSERT INTO subscribers (email, name, password_hash) VALUES \
                 ('first@example.com', 'Somebody Else', 'not-a-real-hash')"
            )
            .is_err(),
        "one email, one subscriber"
    );

    // `active` means confirmed — and the converse is deliberately *not* asserted, because
    // unsubscribing a subscriber who was confirmed has to stay possible (step
    // `mailer/1.18-authz-boundary`). This is the shape of that constraint, in three statements.
    assert!(
        postgres
            .try_execute(
                "UPDATE subscribers SET status = 'active' WHERE email = 'first@example.com'"
            )
            .is_err(),
        "a subscriber cannot be active without a confirmation"
    );
    postgres.execute(
        "UPDATE subscribers SET status = 'active', confirmed_at = now() WHERE email = \
         'first@example.com'",
    );
    postgres.execute(
        "UPDATE subscribers SET status = 'unsubscribed' WHERE email = 'first@example.com'",
    );
    assert_eq!(
        postgres.query(
            "SELECT status || ':' || (confirmed_at IS NOT NULL)::text FROM subscribers WHERE \
             email = 'first@example.com'"
        ),
        "unsubscribed:true",
        "leaving was never the same thing as never having arrived"
    );

    // `subscribed_at` is the service's own business: whoever inserts a subscriber does not have to
    // remember it, and cannot forget it either.
    assert_eq!(
        postgres.query(
            "SELECT (subscribed_at IS NOT NULL)::text FROM subscribers WHERE email = \
             'first@example.com'"
        ),
        "true"
    );
    app.stop();
}

#[test]
fn a_second_start_against_the_same_database_changes_nothing() {
    let postgres = Postgres::start();
    let setting = database_setting(&postgres.url());

    let first = TestApp::start(config(std::slice::from_ref(&setting)));
    assert_eq!(first.get("/readyz").0, 200, "the first start is not ready");
    first.stop();

    // Starting twice happens on every deploy and every restart: the second start has a schema that
    // is already current, and that is not a failure.
    let second = TestApp::start(config(&[setting]));
    let (status, body) = second.get("/readyz");
    assert_eq!(status, 200, "{body}");
    second.stop();

    assert_eq!(
        postgres.query("SELECT count(*) FROM _sqlx_migrations"),
        "1",
        "a migration is applied once, not once per start"
    );
}

#[test]
fn readiness_follows_the_database_and_liveness_does_not() {
    let postgres = Postgres::start();
    let app = TestApp::start(config(&[database_setting(&postgres.url())]));

    assert_eq!(app.get("/healthz").0, 200, "the process is up");
    assert_eq!(app.get("/readyz").0, 200, "a migrated database is ready");

    // The new route is a route like any other: the wrong method is answered by the router with the
    // `Allow` header (step `mailer/1.7-http`), not by a catch-all.
    let (status, body) = app.post("/readyz", "");
    assert_eq!(status, 405, "{body}");

    // Take the database away while the service runs: liveness is about the process and stays green,
    // readiness is about what a request needs and goes red — and the answer is computed per
    // request, not remembered from startup.
    postgres.stop();
    let (status, body) = app.get("/readyz");
    assert_eq!(
        status, 503,
        "a service that lost its database is not ready: {body}"
    );
    assert!(
        !body.trim().is_empty(),
        "a probe answer with an empty body tells a client nothing"
    );
    assert_eq!(
        app.get("/healthz").0,
        200,
        "losing a dependency must not fail liveness: that would restart a healthy process"
    );
    app.stop();
}

#[test]
fn a_service_without_a_database_starts_and_says_what_it_is() {
    // Steps 1.4-1.8 are still true here: a service with nothing configured runs, because a database
    // is something a deployment adds. What it must not do is claim to be ready.
    let app = TestApp::start(config(&[]));
    assert_eq!(app.get("/healthz").0, 200, "the process is up");
    let (status, body) = app.get("/readyz");
    assert_eq!(status, 503, "{body}");
    assert!(
        !body.trim().is_empty(),
        "a 503 that says nothing is a 503 nobody can act on"
    );
    app.stop();
}

#[test]
fn a_database_the_service_cannot_reach_stops_it_before_it_serves() {
    // A port nothing listens on. `free_port` binds and releases, so the connection is *refused*
    // rather than dropped into a firewall: either way this test has a deadline.
    let database_port = free_port();
    let listen_port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mailer"))
        .args(["--listen-port", &listen_port.to_string()])
        .env_clear()
        .env(
            "MAILER__DATABASE__URL",
            format!("postgres://postgres:postgres@127.0.0.1:{database_port}/mailer"),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("the mailer binary could not be started: {error}"));

    let status = wait_for_exit(&mut child);
    let stderr = read_stderr(child);

    // A database that cannot be used is the deployment's problem, and step 1.5's table says that is
    // exit code 1 — not 0 ("started"), not a panic.
    assert_eq!(
        status.code(),
        Some(1),
        "a service that cannot reach its database has to exit 1, it said: {stderr}"
    );
    assert!(
        stderr.contains("database"),
        "the process has to say what it could not do: {stderr}"
    );
    // And it never served: the port it was told to listen on is still free, so a client that was
    // sent there found nothing.
    assert!(
        TcpStream::connect(("127.0.0.1", listen_port)).is_err(),
        "the service bound a listener it could not serve from"
    );
}

/// Wait for a process to end on its own, killing it when it does not.
///
/// `Command::output()` would hang here: a service that ignores the database serves happily and
/// never exits, and a test with no deadline of its own would sit there until the whole run is
/// killed. A wrong answer has to be loud *and* quick.
fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + STARTUP_DEADLINE;
    loop {
        match child
            .try_wait()
            .unwrap_or_else(|error| panic!("the child can be polled: {error}"))
        {
            Some(status) => return status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!(
                    "the service was still running after {STARTUP_DEADLINE:?}: a database it cannot \
                     use has to stop it, not leave it serving"
                );
            }
            None => thread::sleep(RETRY),
        }
    }
}

/// Everything a finished process complained about.
fn read_stderr(child: Child) -> String {
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("the output of a finished process can be read: {error}"));
    String::from_utf8_lossy(&output.stderr).into_owned()
}
