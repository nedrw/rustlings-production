//! Spec for `mailer/1.20-metrics`: what this service counts, and the promise a label makes.
//!
//! `kiln run` copies this file into the project as `tests/spec_metrics.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Everything is judged from outside a **real process**
//! (`tests/support/service.rs`): `/metrics` is an endpoint like any other, and what a scraper reads
//! is exactly what these tests read.
//!
//! Three claims are worth more than the rest, and they are why this step has a spec at all:
//!
//! * **the vocabulary is fixed.** A dashboard is written against `http_requests_total{route,status}`;
//!   the snapshot below fails when a name or an outcome drifts, which is the only moment a rename is
//!   cheap to notice;
//! * **cardinality is bounded.** A request for a path nobody routed is *one* series called
//!   `/{*rest}` — not one series per string a stranger sends;
//! * **a delivery's three endings are three numbers.** A message that was retried and then delivered
//!   must not look like a message nobody will ever receive.

#[path = "support/postgres.rs"]
mod postgres;
#[path = "support/service.rs"]
mod service;

use std::collections::BTreeSet;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use postgres::Postgres;
use service::Service;

/// The password the fixture subscribes with.
const PASSWORD: &str = "correct horse battery staple";

/// The body of `POST /signup`.
fn signup_body(email: &str) -> String {
    format!(r#"{{"email":"{email}","name":"Ada Lovelace","password":"{PASSWORD}"}}"#)
}

/// A service with a database and a log a test can read.
fn service(postgres: &Postgres) -> Service {
    Service::start(&[
        ("MAILER__DATABASE__URL", &postgres.url()),
        ("MAILER__LOG__FORMAT", "json"),
    ])
}

/// The whole exposition, waited for until `check` accepts it.
fn exposition(service: &Service, what: &str, mut check: impl FnMut(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    // The last exposition read, so that a timeout can show what *was* there: "it did not show up"
    // is a complaint, and what the endpoint answered is the evidence.
    let mut last;
    loop {
        let answer = service.get("/metrics");
        assert_eq!(
            answer.status, 200,
            "`GET /metrics` has to answer with the exposition: {} {}",
            answer.status, answer.body
        );
        if check(&answer.body) {
            return answer.body;
        }
        last = answer.body;
        assert!(
            Instant::now() < deadline,
            "{what} did not show up within 10s. The exposition was:\n{last}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One sample's value, by the exact line that carries it.
fn value(exposition: &str, sample: &str) -> Option<u64> {
    exposition
        .lines()
        .find_map(|line| line.strip_prefix(sample)?.trim().parse().ok())
}

/// The metric *families* an exposition declares, read from its `# HELP` lines.
///
/// The declared vocabulary rather than the samples that happen to exist: a family with nothing
/// counted yet is still part of the API (`deliveries_total` before any message went out), and the
/// `_bucket`/`_sum`/`_count` suffixes of a histogram belong to the format, not to this service.
fn families(exposition: &str) -> BTreeSet<String> {
    exposition
        .lines()
        .filter_map(|line| line.strip_prefix("# HELP "))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_request_is_counted_by_its_matched_route_and_status() {
    let postgres = Postgres::start();
    let service = service(&postgres);

    assert_eq!(service.get("/healthz").status, 200);
    assert_eq!(
        service
            .post("/signup", &[], r#"{"email":"not-a-signup"}"#)
            .status,
        400
    );

    let exposition = exposition(&service, "the counted requests", |exposition| {
        value(
            exposition,
            r#"http_requests_total{route="/healthz",status="200"}"#,
        ) == Some(1)
            && value(
                exposition,
                r#"http_requests_total{route="/signup",status="400"}"#,
            ) == Some(1)
    });
    assert_eq!(
        value(
            &exposition,
            r#"http_requests_total{route="/healthz",status="200"}"#
        ),
        Some(1),
        "one request is one count: {exposition}"
    );
    assert_eq!(
        value(
            &exposition,
            r#"http_requests_total{route="/signup",status="400"}"#
        ),
        Some(1),
        "and the status is the label a dashboard groups errors by: {exposition}"
    );
}

#[test]
fn a_path_nobody_routed_is_one_series() {
    let postgres = Postgres::start();
    let service = service(&postgres);

    // Two paths that a stranger could have chosen, and neither of them may become a series: a label
    // whose values come from the request is how one metric becomes ten thousand.
    assert_eq!(service.get("/wp-login.php").status, 404);
    assert_eq!(service.get("/.env").status, 404);

    // The label is the *pattern* the router matched — one series for every path nobody routed —
    // rather than the path that was asked for.
    let exposition = exposition(&service, "the unrouted requests", |exposition| {
        value(
            exposition,
            r#"http_requests_total{route="/{*rest}",status="404"}"#,
        ) == Some(2)
    });
    assert!(
        !exposition.contains("wp-login"),
        "the path a client asked for must not be a label value: {exposition}"
    );
    assert!(
        !exposition.contains(".env"),
        "nor a scan's favourite file: {exposition}"
    );
}

#[test]
fn the_metric_names_are_a_snapshot() {
    let postgres = Postgres::start();
    let service = service(&postgres);
    assert_eq!(service.get("/healthz").status, 200);

    let exposition = exposition(&service, "the request to be counted", |exposition| {
        value(
            exposition,
            r#"http_requests_total{route="/healthz",status="200"}"#,
        ) == Some(1)
    });

    // A snapshot, and the point of it is the *failure*: this test is what turns a rename into a red
    // build rather than into a dashboard that quietly stopped moving. The suffixes of a histogram
    // (`_bucket`, `_sum`, `_count`) belong to the format, so the families are what is pinned.
    let expected: BTreeSet<String> = [
        "http_requests_total",
        "http_request_duration_seconds",
        "deliveries_total",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        families(&exposition),
        expected,
        "the metric vocabulary is part of this service's API: {exposition}"
    );

    // The histogram is a histogram: buckets that grow, a `+Inf` that equals the count, and a sum.
    let count = value(
        &exposition,
        r#"http_request_duration_seconds_count{route="/healthz"}"#,
    )
    .expect("a route that was requested has a count");
    let infinite = value(
        &exposition,
        r#"http_request_duration_seconds_bucket{route="/healthz",le="+Inf"}"#,
    )
    .expect("and a `+Inf` bucket");
    assert_eq!(
        infinite, count,
        "every observation is below infinity, or the histogram is lying: {exposition}"
    );
    assert!(
        exposition
            .lines()
            .any(|line| line.starts_with(r#"http_request_duration_seconds_sum{route="/healthz"}"#)),
        "a histogram without a sum cannot answer an average: {exposition}"
    );
}

#[test]
fn a_delivery_is_counted_by_how_it_ended() {
    let postgres = Postgres::start();
    let service = service(&postgres);

    // The logging adapter never fails (step `mailer/1.19-test-layering` keeps it as the default), so
    // this signup is delivered.
    assert_eq!(
        service
            .post("/signup", &[], &signup_body("ada@example.com"))
            .status,
        201
    );
    let exposition = exposition(&service, "the delivery to be counted", |exposition| {
        value(exposition, r#"deliveries_total{outcome="sent"}"#) == Some(1)
    });
    assert_eq!(
        value(&exposition, r#"deliveries_total{outcome="sent"}"#),
        Some(1),
        "a message the sender took is `sent`: {exposition}"
    );
    assert_eq!(
        value(&exposition, r#"deliveries_total{outcome="failed"}"#),
        None,
        "and nothing failed on the way: {exposition}"
    );
}

#[test]
fn a_mail_server_that_is_away_ends_as_attempts_and_a_give_up() {
    let postgres = Postgres::start();
    // A port nothing listens on: the mail server that is not there (step
    // `mailer/1.19-test-layering` made this observable at all).
    let closed = TcpListener::bind("127.0.0.1:0")
        .expect("a free port can be found")
        .local_addr()
        .expect("a bound listener has an address");
    let url = postgres.url();
    let port = closed.port().to_string();
    let service = Service::start(&[
        ("MAILER__DATABASE__URL", &url),
        ("MAILER__EMAIL__SMTP_HOST", "127.0.0.1"),
        ("MAILER__EMAIL__SMTP_PORT", &port),
        ("MAILER__LOG__FORMAT", "json"),
    ]);

    assert_eq!(
        service
            .post("/signup", &[], &signup_body("ada@example.com"))
            .status,
        201
    );

    let exposition = exposition(&service, "the give-up to be counted", |exposition| {
        value(exposition, r#"deliveries_total{outcome="given_up"}"#) == Some(1)
    });
    assert_eq!(
        value(&exposition, r#"deliveries_total{outcome="failed"}"#),
        Some(3),
        "every attempt that failed is one `failed`, retries included — it is the number that says \
         the mail server is unwell: {exposition}"
    );
    assert_eq!(
        value(&exposition, r#"deliveries_total{outcome="given_up"}"#),
        Some(1),
        "and the third ended the story, which is a different number for a different alert: \
         {exposition}"
    );
}
