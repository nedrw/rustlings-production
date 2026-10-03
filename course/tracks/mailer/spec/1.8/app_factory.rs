//! Spec for `mailer/1.8-test-harness`: one factory, one app per test, a port of its own, nothing
//! shared.
//!
//! `kiln run` copies this file into the project as `tests/spec_app_factory.rs`; it is the course's
//! file, not the learner's (plan §3 D2). It is written the way the step wants every later test
//! written — `TestApp::start`, a handful of requests, `stop` — because a harness nobody uses is a
//! harness that rots. `tests/support/`, which it pulls in with `mod support;`, is the project's own
//! file: the harness is the learner's, and this spec is what holds it to its promise.
//!
//! Nothing here touches a fixed port or a shared file: the three tests below run in parallel with
//! each other and with every other spec in this project.

mod support;

use std::collections::BTreeSet;
use std::net::TcpStream;

use mailer::config::{Config, Flags};
use support::TestApp;

/// The configuration the tests start an app with: the defaults, plus anything the caller adds.
fn config(env: &[(&str, &str)]) -> Config {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    Config::load(&Flags::default(), &env).expect("the fixture configuration is valid")
}

#[test]
fn every_test_gets_an_app_of_its_own() {
    // Three apps at once, the way a test binary runs its tests at once. They must not meet.
    let first = TestApp::start(config(&[]));
    let second = TestApp::start(config(&[]));
    let third = TestApp::start(config(&[]));

    let ports: BTreeSet<u16> = [
        first.address().port(),
        second.address().port(),
        third.address().port(),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        ports.len(),
        3,
        "three apps share one port: {ports:?} — the tests would fight over it, and the failure \
         would show up in whichever test ran second"
    );

    let (status, body) = first.get("/healthz");
    assert_eq!(status, 200, "{body}");

    // Stopping one app is not stopping the others, and it is not abandoning it either: nothing is
    // shared, so nothing is torn down twice and nothing keeps listening.
    let stopped = second.address();
    second.stop();
    let (status, _) = third.get("/healthz");
    assert_eq!(status, 200, "stopping one app stopped another");
    assert!(
        TcpStream::connect(stopped).is_err(),
        "a stopped app has to be gone, not merely forgotten"
    );
}

#[test]
fn the_app_the_factory_builds_is_the_app_the_binary_serves() {
    // A harness that builds its own router drifts away from what is deployed; that is why the
    // factory starts the service it ships.
    let app = TestApp::start(config(&[]));

    let (status, body) = app.get("/healthz");
    assert_eq!(status, 200, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).expect("the body is JSON");
    assert_eq!(body["status"], "ok", "{body}");

    let (status, body) = app.get("/nope");
    assert_eq!(
        status, 404,
        "the factory has to serve the same routes as the binary: {body}"
    );

    let (status, _) = app.post("/healthz", "");
    assert_eq!(
        status, 405,
        "the factory has to serve the same fallbacks as the binary, not a router of its own"
    );
}

#[test]
fn the_configuration_a_test_gives_is_the_configuration_the_app_gets() {
    // The port is the exception, and it is deliberate: a test that could choose its port would
    // choose one another test is using.
    let mut chosen = config(&[]);
    chosen.listen.port = 8080;
    let app = TestApp::start(chosen);

    assert_ne!(
        app.address().port(),
        8080,
        "the factory has to pick the port: a fixed one is a port the next test will want too"
    );
    let (status, _) = app.get("/healthz");
    assert_eq!(status, 200, "the app answers on the address it reports");
}
