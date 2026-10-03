//! Spec for `mailer/1.21-container`: the image this service ships as, judged from outside it.
//!
//! `kiln run` copies this file into the project as `tests/spec_image.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Everything here is about the *image* (`Dockerfile`), not about the
//! code inside it — the code has twenty other specs.
//!
//! Four claims, and none of them can be made from inside the process:
//!
//! * **it runs this service, and it is reachable.** The container answers `/healthz`, answers
//!   `/readyz` honestly (no database configured is a configuration, not a failure — step
//!   `mailer/1.9-postgres`), and answers an unknown path the way the binary on the host does. A
//!   container that listens only on its own loopback passes none of this, which is the most common
//!   way a first image is wrong;
//! * **it is small enough to be an image and not a workspace.** No `cargo`, no `rustc`, no `cc`, no
//!   sources — and a size ceiling, because a multi-stage build is a number as well as a shape;
//! * **it does not run as root;**
//! * **it migrates before it serves.** With a database, readiness turns green only once the schema is
//!   there: the start-up order of step `mailer/1.9-postgres`, now true of a container.
//!
//! The image is built once per test run, with the engine's layer cache and the fixture's build-cache
//! volumes: the first build on a machine pays for the dependency tree (minutes), every build after it
//! does not.

#[path = "support/container.rs"]
mod container;
#[path = "support/postgres.rs"]
mod postgres;

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use container::Container;
use postgres::Postgres;

/// The tag every test in this file builds and runs.
const TAG: &str = "kiln-mailer-spec";
/// What a service image like this one should stay under, in bytes.
///
/// Twenty-five megabytes is generous for one musl binary and a trust store (the pair is around
/// twenty), and strict enough to fail the mistake this step is about: a `Dockerfile` whose runtime
/// stage is the *builder* image is several hundred megabytes of toolchain nobody ships on purpose.
const SIZE_CEILING: u64 = 25 * 1024 * 1024;

/// The project's directory: the build context, and where the image's Dockerfile lives.
fn context() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Build the image once for the whole file.
///
/// Every test calls this, and only the first call builds: the engine caches the layers and the
/// fixture keeps the build's work in volumes, so the second and third calls are seconds. A build per
/// test would be minutes per test.
fn image() -> &'static str {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let took = container::build(TAG, context());
        println!("built `{TAG}` in {took:?}");
    });
    TAG
}

/// A container running the image, waiting until it answers.
fn start(extra: &[(&str, &str)]) -> Container {
    let running = Container::start(image(), extra);
    running.wait_until_healthy();
    running
}

#[test]
fn the_image_runs_this_service_and_is_reachable() {
    let running = start(&[]);

    let (status, headers, body) = running.get_with_headers("/healthz");
    assert_eq!(
        status, 200,
        "the image has to answer its liveness probe, and a published port has to lead to it: {body}"
    );
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.contains("json")),
        "and answer it the way the host binary does: {headers:?}"
    );

    // No database configured: readiness is *supposed* to be red, and for the reason step
    // `mailer/1.9-postgres` chose. Two probes that always agree are two probes an orchestrator
    // cannot use.
    let (status, body) = running.get("/readyz");
    assert_eq!(
        status, 503,
        "with no database configured, readiness has to be honest rather than optimistic: {body}"
    );
    assert!(
        body.contains("database"),
        "and it has to name the dependency that is missing: {body}"
    );

    // The same route table as the host binary: an unknown path is a 404 with this service's body.
    let (status, body) = running.get("/wp-login.php");
    assert_eq!(status, 404, "an unknown path is a 404: {body}");
    assert!(
        body.contains("status"),
        "with the JSON body this service answers everywhere: {body}"
    );

    let logs = running.logs();
    assert!(
        logs.contains("listening"),
        "the container's log has to say what the binary says when it starts: {logs}"
    );
}

#[test]
fn the_image_is_small_because_it_is_not_a_build_environment() {
    let running = start(&[]);

    for tool in ["cargo", "rustc", "cc"] {
        let outcome = running.exec(&format!("command -v {tool}"));
        assert!(
            !outcome.status.success(),
            "`{tool}` is in the runtime image: a multi-stage build leaves the toolchain behind, and \
             an image with a compiler in it is an image with a compiler's vulnerabilities in it \
             ({})",
            outcome.said().trim()
        );
    }

    let sources = running.exec("test -e /src");
    assert!(
        !sources.status.success(),
        "the sources are in the runtime image: what ships is a binary, not a workspace"
    );

    // And the binary *is* there, so the checks above are not checks of an empty image.
    let binary = running.exec("test -x /usr/local/bin/mailer");
    assert!(
        binary.status.success(),
        "the runtime image has to carry the binary the builder produced"
    );

    let size = running.size_of(TAG);
    assert!(
        size <= SIZE_CEILING,
        "the image is {size} bytes, over the {SIZE_CEILING} a service image should need: a runtime \
         stage that is really the builder is the shape this catches"
    );
}

#[test]
fn the_image_does_not_run_as_root() {
    let running = start(&[]);
    let outcome = running.exec("id -u");
    let uid = outcome.said().trim().to_owned();
    assert_ne!(
        uid, "0",
        "the service runs as root: a process that needs no privileges should have none"
    );
    assert!(
        outcome.status.success() && !uid.is_empty(),
        "and the image has to have the user it claims: {}",
        outcome.said().trim()
    );
}

#[test]
fn the_container_migrates_before_it_serves() {
    let postgres = Postgres::start();
    // The database a container reaches is not `127.0.0.1` — that is the container itself. Podman
    // gives a container one name for the machine it runs on, and the fixture's URL is rewritten to
    // use it: a published port is reachable by that name, not by the loopback address of somebody
    // else's namespace.
    let url = postgres
        .url()
        .replace("127.0.0.1", "host.containers.internal");
    let running = start(&[("MAILER__DATABASE__URL", url.as_str())]);

    // Readiness is the promise of step `mailer/1.9-postgres` — the schema is current before traffic
    // is served — and it is the one thing about a container that has to be checked *outside* the
    // process: the probe answers, and the database behind it has the tables.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (status, body) = running.get("/readyz");
        if status == 200 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "readiness never went green against a database the container can reach: {status} {body}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        postgres.query("SELECT count(*) FROM subscribers"),
        "0",
        "the schema the container brought up is the one this build expects"
    );
}
