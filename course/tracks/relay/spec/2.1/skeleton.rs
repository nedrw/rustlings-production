//! Spec for `relay/2.1-project-skeleton`: the service shape, judged from outside it.
//!
//! `kiln run` copies this file into the project as `tests/spec_skeleton.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Four claims, and every one of them is a behaviour:
//!
//! * **a configuration can be inspected without starting anything** — `--check-config` prints the
//!   effective settings, in the shape this file pins, and never a secret;
//! * **the health routes are public** — an orchestrator asks about a process that has no API key;
//! * **authentication is a layer, not a check inside a route** — a request that is refused still
//!   carries the correlation id it was given and still appears in the log with it, which is only
//!   observable if the id and the log run *outside* the refusal;
//! * **an endpoint belongs to exactly one tenant** — created with one key, visible to that key,
//!   invisible to another, and its signing secret is shown once and never listed again.
//!
//! Everything here is judged from outside a real process (`tests/support/service.rs`) when the
//! claim is about a log line, and from an in-process app (`tests/support/app.rs`) when the claim is
//! about an answer. Both need a database, and both get one of their own
//! (`tests/support/postgres.rs`).

mod support;

use std::process::Command;

use support::app::{self, App};
use support::postgres::Postgres;
use support::service::Service;
use support::var;

/// The tenant this file seeds, and the key it authenticates with.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// A second tenant, for the claims about one tenant not seeing another's data.
const GLOBEX: (&str, &str) = ("globex", "key-globex-91b7");

/// `--check-config` answers with the effective settings, and never with the secret inside them.
#[test]
fn the_effective_configuration_is_reported_without_starting_the_service() {
    let dir = std::env::temp_dir().join(format!("relay-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = dir.join("relay.toml");
    std::fs::write(
        &file,
        "[listen]\nport = 4321\n\n[database]\nurl = \"postgres://relay:secret-not-printed@localhost/relay\"\n",
    )
    .expect("the config file is writable");

    let ran = Command::new(env!("CARGO_BIN_EXE_relay"))
        .args([
            "--check-config",
            "--config",
            file.to_str().expect("a utf-8 path"),
            // The environment is the next layer up: it wins over the file, and the flag wins over it.
            "--listen-port",
            "4322",
        ])
        .env_clear()
        .env(
            "RELAY__DATABASE__URL",
            "postgres://relay:secret-not-printed@localhost/relay",
        )
        .output()
        .expect("the relay binary runs");
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        ran.status.success(),
        "`--check-config` has to succeed without a database: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    assert!(
        stdout.contains("listen.port = 4322"),
        "the flag wins over the file, which wins over the default: {stdout}"
    );
    assert!(
        stdout.contains("database.url = set"),
        "a configured secret is reported as `set`, and the URL itself never appears: {stdout}"
    );
    assert!(
        !stdout.contains("secret-not-printed"),
        "the report must not print the connection string: {stdout}"
    );
    assert!(
        stdout.contains("log.level = info"),
        "the default still shows up in the report: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `/healthz` and `/readyz` are what an orchestrator asks, and it has no API key.
#[test]
fn the_health_routes_answer_without_an_api_key() {
    let postgres = Postgres::start();
    let app = App::start(app::config(&[
        var(app::DATABASE_URL, postgres.url()),
        var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
    ]));

    let client = app.client();
    let health = client.get("/healthz");
    assert_eq!(
        health.status, 200,
        "liveness is not allowed to depend on identity: {}",
        health.body
    );

    let ready = client.get("/readyz");
    assert_eq!(
        ready.status, 200,
        "the database this app was started with is up, so readiness is true: {}",
        ready.body
    );

    // A route that does not exist is a 404 with a body a caller can read, not an empty answer.
    let missing = client.get("/nothing-here");
    assert_eq!(
        missing.status, 404,
        "unknown routes are honest: {}",
        missing.body
    );
    assert!(
        missing.body.contains("error"),
        "the 404 body says what happened: {}",
        missing.body
    );
}

/// A refused request is still logged, with the id it was given: the layers run outside the refusal.
#[test]
fn a_rejected_request_is_logged_with_the_id_it_carries() {
    let postgres = Postgres::start();
    let service = Service::start(&[
        var("RELAY__DATABASE__URL", postgres.url()),
        var(
            "RELAY__BOOTSTRAP__TENANTS",
            format!("{}={}", ACME.0, ACME.1),
        ),
    ]);
    let client = service.client();

    let given = "4f2a1c7e-9b6d-4a2e-8f10-1c2d3e4f5a6b";
    let refused = client.send(
        support::client::Request::post("/endpoints", Vec::new())
            .header("x-request-id", given)
            .json(&serde_json::json!({ "url": "http://example.test/hook" })),
    );
    assert_eq!(refused.status, 401, "no key, no route: {}", refused.body);
    assert_eq!(
        refused.header("x-request-id"),
        Some(given),
        "the correlation id is assigned outermost, so a refusal echoes it too"
    );

    // The log line for the refusal has to carry the same id: that is what makes the two layers
    // (request id, then logging) observable as layers rather than as one handler that does both.
    let lines = service.log_lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains(given) && line.contains("401")),
        "the refusal is in the log with its id: {lines:#?}"
    );

    // One a caller invents badly is replaced, not trusted: the id ends up in a header and a filter.
    let invented = client
        .send(support::client::Request::get("/healthz").header("x-request-id", "../../etc/passwd"));
    let assigned = invented
        .header("x-request-id")
        .expect("the service always answers with an id");
    assert_ne!(assigned, "../../etc/passwd");
    assert!(
        assigned.len() >= 16 && assigned.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "an id the caller invented badly is replaced by one this service made: {assigned}"
    );

    let service = service;
    service.stop();
}

/// An endpoint is one tenant's data; the secret is shown once and never listed.
#[test]
fn an_endpoint_belongs_to_the_tenant_that_created_it() {
    let postgres = Postgres::start();
    let app = App::start(app::config(&[
        var(app::DATABASE_URL, postgres.url()),
        var(
            app::BOOTSTRAP_TENANTS,
            format!("{}={},{}={}", ACME.0, ACME.1, GLOBEX.0, GLOBEX.1),
        ),
    ]));
    let client = app.client();

    let created = client.post_json(
        "/endpoints",
        ACME.1,
        &serde_json::json!({ "url": "http://hooks.example.test/acme" }),
    );
    assert_eq!(
        created.status, 201,
        "a tenant with a valid key may register a target: {}",
        created.body
    );
    let id = created.field("id");
    let secret = created.field("secret");
    assert!(
        secret.as_str().is_some_and(|secret| secret.len() >= 32),
        "the signing secret is generated by the service and answered once: {secret}"
    );

    let mine = client.get_with_key("/endpoints", ACME.1);
    assert_eq!(mine.status, 200, "{}", mine.body);
    let body = mine.json();
    let listed = body
        .get("endpoints")
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| panic!("the listing has an `endpoints` array: {body}"));
    assert_eq!(listed.len(), 1, "acme sees its own endpoint: {body}");
    assert_eq!(listed[0].get("id"), Some(&id), "{body}");
    assert!(
        listed[0].get("secret").is_none(),
        "the secret is shown once, at creation, and never listed again: {body}"
    );

    let theirs = client.get_with_key("/endpoints", GLOBEX.1);
    assert_eq!(theirs.status, 200, "{}", theirs.body);
    let their_body = theirs.json();
    assert_eq!(
        their_body
            .get("endpoints")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(0),
        "globex sees none of acme's endpoints: {their_body}"
    );

    // A target that is not an absolute http(s) URL is refused where it enters the system.
    let refused = client.post_json(
        "/endpoints",
        ACME.1,
        &serde_json::json!({ "url": "file:///etc/passwd" }),
    );
    assert_eq!(
        refused.status, 400,
        "an endpoint URL is parsed, not trusted: {}",
        refused.body
    );

    // A key that belongs to nobody is refused the same way a missing one is.
    let stranger = client.get_with_key("/endpoints", "key-that-was-never-issued");
    assert_eq!(stranger.status, 401, "{}", stranger.body);
}
