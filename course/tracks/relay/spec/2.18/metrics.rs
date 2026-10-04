//! Spec for `relay/2.18-delivery-metrics`: latency, backlog, and which tenant is slow.
//!
//! `kiln run` copies this file into the project as `tests/spec_metrics.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The step's three numbers are judged in the three places they can
//! be wrong, and none of those places can see the other two:
//!
//! * **arithmetic** — the histogram: the buckets are cumulative, the last one is `+Inf`, and its
//!   value is the count. A bucket list that is not monotonic is a p99 that a dashboard computes
//!   from numbers that contradict each other, and nothing else notices;
//! * **the queue** — the backlog gauge: it is read from `deliveries` when it is scraped, it is per
//!   tenant, and a tenant with nothing owed is a `0` rather than a missing series. What a *cache*
//!   of this number gets wrong is exactly what a test like this one catches: the number after the
//!   work finished;
//! * **the wire** — the endpoint a scraper actually reads: the three families with their `# HELP`
//!   and `# TYPE`, the fixed bucket bounds for every tenant label, the fixed outcome values, and
//!   the status a caller without the right role gets. `relay/2.3-contract-first` is a standing rule
//!   here too: a route that is not in the document is a route whose answers nobody agreed to.
//!
//! The label set is judged as *cardinality*: every `tenant` label in the output is either a name
//! this service could have configured or the `unknown` it folds everything else into. A metric with
//! an endpoint URL or an event id in a label passes every other test in this file and takes a
//! production scrape down.
//!
//! The fixture's app does not run a worker (`relay/2.15-shutdown-leases` left that to the
//! deployment), so the deliveries this file measures are driven by a worker of its own — with a
//! registry it hands in, which is the same registry the [`Metrics::render`] half is read from.

mod support;

#[path = "support/target.rs"]
mod target;

use std::sync::Arc;
use std::time::Duration;

use relay::backoff::Budget;
use relay::concurrency::{Gate, Limits, Worker};
use relay::db::Db;
use relay::delivery::Sender;
use relay::domain::TenantName;
use relay::metrics::{self, BUCKETS, Backlog, Metrics, OUTCOMES, Outcome};
use relay::ordering::Lanes;
use relay::outbound::{HttpSender, Policy};
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::Request;
use support::postgres::Postgres;
use support::var;
use target::{Reply, Target};

/// The tenant this file is about, and the slow one.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// A second tenant: the one that is *not* slow, which is what makes "per tenant" observable.
const GLOBEX: (&str, &str) = ("globex", "key-globex-91b7");
/// A tenant whose key may read its own rows and not the numbers about the service.
const READER: (&str, &str) = ("acme-reader", "key-acme-reader-2c4f");
/// The reader, as configuration spells it (`relay/2.16-authz-audit`).
const READER_IN_CONFIG: &str = "acme-reader:tenant-reader=key-acme-reader-2c4f";
/// The worker every turn claims as.
const WORKER: &str = "metrics";

/// One database, one in-process service and one fast subscriber.
struct Fixture {
    app: App,
    postgres: Postgres,
    subscriber: Target,
}

impl Fixture {
    /// Start the service with two administrators and a subscriber that answers at once.
    fn start() -> Self {
        let postgres = Postgres::start();
        let subscriber = Target::start(vec![Reply::ok("{}")]);
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(
                app::BOOTSTRAP_TENANTS,
                format!(
                    "{}={},{}={},{READER_IN_CONFIG}",
                    ACME.0, ACME.1, GLOBEX.0, GLOBEX.1
                ),
            ),
            // The subscribers in this file are on `127.0.0.1` (`relay/2.12-ssrf-defense`).
            var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
            var("RELAY__RATE_LIMIT__BURST", "100000"),
            var("RELAY__RATE_LIMIT__PER_SECOND", "100000"),
        ]));
        // The fixture's readiness check is the *port*, and the kernel answers a connect before the
        // service has finished starting; this file reads the tables.
        let serving = app.client().get("/healthz");
        assert_eq!(
            serving.status, 200,
            "the service has to be serving before this file asks anything of it: {}",
            serving.body
        );
        Self {
            app,
            postgres,
            subscriber,
        }
    }

    /// Register one subscriber for one tenant.
    fn register(&self, key: &str, url: &str) {
        let answer = self
            .app
            .client()
            .post_json("/endpoints", key, &json!({ "url": url }));
        assert_eq!(
            answer.status, 201,
            "the fixture registers `{url}`: {}",
            answer.body
        );
    }

    /// Accept `count` events for one tenant: one delivery each, per endpoint it owns.
    fn accept(&self, key: &str, prefix: &str, count: usize) {
        for index in 0..count {
            let accepted = self.app.client().send(
                Request::post("/events", Vec::new())
                    .bearer(key)
                    .header("idempotency-key", &format!("{prefix}-{index}"))
                    .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
            );
            assert_eq!(accepted.status, 201, "{}", accepted.body);
        }
    }

    /// The pool the worker runs on: the service's own connection string.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }

    /// A sender that speaks to this file's subscribers.
    fn sender(&self) -> Arc<dyn Sender> {
        Arc::new(HttpSender::new(Policy::default()))
    }

    /// A worker that writes what each attempt cost into `registry`.
    fn worker(&self, pool: &PgPool, registry: &Arc<Metrics>, budget: Budget) -> Worker {
        let limits = Limits::new(4, 100).expect("seats, and room for the queue");
        Worker::new(
            pool.clone(),
            self.sender(),
            Arc::new(Gate::new(limits)),
            Arc::new(Lanes::new()),
            budget,
            limits,
            WORKER,
        )
        .metered(Arc::clone(registry))
    }

    /// One question, answered as text.
    fn text(&self, sql: &str) -> String {
        self.postgres.query(sql)
    }
}

/// The value of one sample of the exposition text, by family name and labels.
///
/// Labels are matched in the order they are given, and the exposition format's convention — which
/// the renderer follows — is that they are sorted by name.
fn sample(text: &str, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    let wanted = format!(
        "{name}{{{}}}",
        labels
            .iter()
            .map(|(label, value)| format!("{label}=\"{value}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    text.lines()
        .find_map(|line| line.strip_prefix(&wanted))
        .and_then(|rest| rest.trim().parse().ok())
}

/// One sample, or a panic that shows the whole exposition text.
fn value(text: &str, name: &str, labels: &[(&str, &str)]) -> f64 {
    sample(text, name, labels)
        .unwrap_or_else(|| panic!("no sample `{name}` with labels {labels:?} in:\n{text}"))
}

/// Every value of one label, in the order they appear.
fn labels_of(text: &str, label: &str) -> Vec<String> {
    let needle = format!("{label}=\"");
    text.lines()
        .filter_map(|line| {
            let start = line.find(&needle)? + needle.len();
            let rest = line.get(start..)?;
            let end = rest.find('"')?;
            rest.get(..end).map(str::to_owned)
        })
        .collect()
}

/// The exposition text of a registry that has recorded `count` deliveries for one tenant, forty
/// milliseconds apart.
fn recorded(tenant: &str, count: usize) -> String {
    let registry = Metrics::new();
    for index in 0..count {
        registry.record(
            tenant,
            Outcome::Sent,
            Duration::from_millis(u64::try_from(index).unwrap_or_default() * 40),
        );
    }
    registry.render(&[])
}

/// The endpoint a scraper reads: the three families, their types, and the buckets that are fixed.
#[test]
fn the_endpoint_exports_the_named_metrics() {
    let fixture = Fixture::start();
    let answer = fixture.app.client().get_with_key("/metrics", ACME.1);
    assert_eq!(answer.status, 200, "{}", answer.body);
    let content_type = answer
        .header("content-type")
        .unwrap_or_else(|| panic!("a scrape is not JSON: no content-type in {}", answer.body));
    assert!(
        content_type.starts_with("text/plain"),
        "the exposition format is text, and a scraper reads the type before the body: `{content_type}`"
    );
    let text = &answer.body;

    for (family, kind) in [
        ("relay_delivery_duration_seconds", "histogram"),
        ("relay_deliveries_total", "counter"),
        ("relay_backlog_depth", "gauge"),
    ] {
        assert!(
            text.contains(&format!("# TYPE {family} {kind}\n")),
            "`{family}` has to be declared as a `{kind}` before its samples:\n{text}"
        );
        assert!(
            text.contains(&format!("# HELP {family} ")),
            "and described, because the name is the only documentation a dashboard gets:\n{text}"
        );
    }

    // The buckets are the fixed set, for every tenant label — a family that appears only after the
    // first delivery is a family a dashboard has to guess at.
    for tenant in [ACME.0, GLOBEX.0, READER.0] {
        for bound in BUCKETS {
            let bound = bound.to_string();
            value(
                text,
                "relay_delivery_duration_seconds_bucket",
                &[("tenant", tenant), ("le", &bound)],
            );
        }
        value(
            text,
            "relay_delivery_duration_seconds_bucket",
            &[("tenant", tenant), ("le", "+Inf")],
        );
        value(
            text,
            "relay_delivery_duration_seconds_sum",
            &[("tenant", tenant)],
        );
        value(
            text,
            "relay_delivery_duration_seconds_count",
            &[("tenant", tenant)],
        );
        for outcome in OUTCOMES {
            value(
                text,
                "relay_deliveries_total",
                &[("tenant", tenant), ("outcome", outcome.as_str())],
            );
        }
        value(text, "relay_backlog_depth", &[("tenant", tenant)]);
    }
    assert!(
        !text.contains("outcome=\"\""),
        "every counter sample names an outcome:\n{text}"
    );
}

/// A scrape is a tenant's business, not the whole world's.
#[test]
fn a_scraper_reads_the_metrics_only_with_a_role_that_may() {
    let fixture = Fixture::start();
    assert_eq!(
        fixture.app.client().get("/metrics").status,
        401,
        "a caller with no key is refused before the matrix is even asked about rows"
    );
    // The reader of `relay/2.16-authz-audit`: it may read its own rows and not the numbers about
    // the whole service. The label set is every tenant's name, which is not a reader's business.
    let refused = fixture.app.client().get_with_key("/metrics", READER.1);
    assert_eq!(refused.status, 403, "{}", refused.body);

    let answer = fixture.app.client().get_with_key("/metrics", ACME.1);
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(
        answer.body.contains(&format!("tenant=\"{}\"", GLOBEX.0)),
        "and the numbers a scraper gets are every tenant's, which is why they are not for \
         everybody: {}",
        answer.body
    );
}

/// Two tenants, one slow subscriber: the histogram tells them apart.
#[tokio::test]
async fn a_slow_tenant_can_be_told_from_a_fast_one() {
    let fixture = Fixture::start();
    let slow = Target::start(vec![Reply::ok("{}").slow(300)]);
    fixture.register(ACME.1, &slow.url("/hook"));
    fixture.register(GLOBEX.1, &fixture.subscriber.url("/hook"));
    fixture.accept(ACME.1, "slow", 1);
    fixture.accept(GLOBEX.1, "fast", 1);

    let pool = fixture.pool().await;
    let registry = Arc::new(Metrics::new());
    let worker = fixture.worker(&pool, &registry, Budget::default());
    worker.work(2).await.expect("the worker runs two turns");

    let text = registry.render(&[]);
    for (tenant, key) in [(ACME.0, ACME.1), (GLOBEX.0, GLOBEX.1)] {
        let _ = key;
        assert_eq!(
            value(
                &text,
                "relay_deliveries_total",
                &[("tenant", tenant), ("outcome", "sent")],
            ),
            1.0,
            "each tenant's delivery was taken: {tenant}\n{text}"
        );
    }
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_count",
            &[("tenant", GLOBEX.0)]
        ),
        1.0,
        "{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_bucket",
            &[("tenant", GLOBEX.0), ("le", "0.25")]
        ),
        1.0,
        "the fast subscriber's attempt is inside the quarter-second bucket:\n{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_bucket",
            &[("tenant", ACME.0), ("le", "0.25")]
        ),
        0.0,
        "and the slow one's is not — this is the whole reason the histogram has a tenant label:\n\
         {text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_bucket",
            &[("tenant", ACME.0), ("le", "+Inf")]
        ),
        1.0,
        "{text}"
    );
}

/// The backlog is what the queue owes, per tenant, read when it is asked for.
#[tokio::test]
async fn the_backlog_is_per_tenant_and_counts_what_is_still_owed() {
    let fixture = Fixture::start();
    fixture.register(ACME.1, &fixture.subscriber.url("/hook"));
    fixture.register(GLOBEX.1, &fixture.subscriber.url("/hook"));
    fixture.accept(ACME.1, "acme", 3);
    fixture.accept(GLOBEX.1, "globex", 1);

    let pool = fixture.pool().await;
    let before = metrics::backlog(&pool)
        .await
        .expect("the queue is readable");
    assert_eq!(
        before,
        vec![
            Backlog {
                tenant: ACME.0.to_owned(),
                depth: 3
            },
            Backlog {
                tenant: READER.0.to_owned(),
                depth: 0
            },
            Backlog {
                tenant: GLOBEX.0.to_owned(),
                depth: 1
            },
        ],
        "one row per tenant in a stable order, counting what is neither delivered nor buried — \
         including the tenant that owes nothing, because a gauge that disappears at zero is a \
         gauge a dashboard shows as a gap"
    );
    let registry = Arc::new(Metrics::new());
    assert!(
        registry
            .render(&before)
            .contains(&format!("relay_backlog_depth{{tenant=\"{}\"}} 3", ACME.0)),
        "and the gauge is that number:\n{}",
        registry.render(&before)
    );

    // One turn delivers both tenants' work; the gauge follows the queue, not the clock.
    let worker = fixture.worker(&pool, &registry, Budget::default());
    worker.work(2).await.expect("the worker runs two turns");
    let after = metrics::backlog(&pool)
        .await
        .expect("the queue is readable");
    assert_eq!(
        after
            .iter()
            .find(|backlog| backlog.tenant == GLOBEX.0)
            .map(|backlog| backlog.depth),
        Some(0),
        "the tenant whose work was delivered owes nothing: {after:?}"
    );
    assert_eq!(
        after
            .iter()
            .find(|backlog| backlog.tenant == ACME.0)
            .map(|backlog| backlog.depth),
        Some(0),
        "and neither does the one that was behind it: {after:?}"
    );
    assert_eq!(
        fixture.text("select count(*) from deliveries where state = 'sent'"),
        "4"
    );

    // A delivery that is *owed* is not the same as a row that exists: a `sent` one is not a backlog.
    fixture.accept(ACME.1, "later", 1);
    let again = metrics::backlog(&pool)
        .await
        .expect("the queue is readable");
    assert_eq!(
        again
            .iter()
            .find(|backlog| backlog.tenant == ACME.0)
            .map(|backlog| backlog.depth),
        Some(1),
        "only the new one is outstanding: {again:?}"
    );
}

/// The histogram's buckets are cumulative, and the last one is the count.
#[test]
fn the_histogram_is_cumulative_and_the_last_bucket_is_the_count() {
    let text = recorded(ACME.0, 7);

    let mut previous = 0.0;
    for bound in BUCKETS {
        let bound = bound.to_string();
        let bucket = value(
            &text,
            "relay_delivery_duration_seconds_bucket",
            &[("tenant", ACME.0), ("le", &bound)],
        );
        assert!(
            bucket >= previous,
            "the buckets are cumulative and they have to rise with their bound: `le={bound}` is \
             {bucket} after {previous}\n{text}"
        );
        previous = bucket;
    }
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_bucket",
            &[("tenant", ACME.0), ("le", "+Inf")]
        ),
        7.0,
        "every observation is inside `+Inf`:\n{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_count",
            &[("tenant", ACME.0)]
        ),
        7.0,
        "and `_count` is the same number: a p99 computed from a count that disagrees with the \
         buckets is a number nobody can reproduce\n{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_bucket",
            &[("tenant", ACME.0), ("le", "0.005")]
        ),
        1.0,
        "one of the seven is inside five milliseconds:\n{text}"
    );
    assert!(
        value(
            &text,
            "relay_delivery_duration_seconds_sum",
            &[("tenant", ACME.0)]
        ) > 0.0,
        "and the sum is what an average is computed from:\n{text}"
    );
    assert_eq!(
        BUCKETS.windows(2).filter(|pair| pair[1] <= pair[0]).count(),
        0,
        "the bucket bounds are strictly rising: {BUCKETS:?}"
    );
}

/// Every label value is a tenant this service could have, or the `unknown` everything else folds
/// into.
#[test]
fn the_label_set_is_bounded() {
    let registry = Metrics::new();
    registry.record(ACME.0, Outcome::Sent, Duration::from_millis(5));
    // A tenant name this service would not have written, an id, and a name with an injection in it:
    // each one is a series a scraper would have to keep for ever.
    registry.record(
        "<script>alert(1)</script>",
        Outcome::Sent,
        Duration::from_millis(5),
    );
    registry.record(
        "9f1d1f4e-6f7e-4a2f-9b0a-2c6f4b1d3e77",
        Outcome::Dead,
        Duration::from_millis(5),
    );
    registry.record(
        "https://hooks.example.test/a/b/c?x=1",
        Outcome::Sent,
        Duration::from_millis(5),
    );
    let text = registry.render(&[]);

    let found = labels_of(&text, "tenant");
    assert!(
        found.iter().any(|value| value == ACME.0),
        "the tenant that delivered is labelled by its name:\n{text}"
    );
    for value in &found {
        assert!(
            value == "unknown" || TenantName::parse(value).is_ok(),
            "`{value}` is not a label value this service chose: a label that carries what the \
             caller sent is a metric whose size is decided by traffic\n{text}"
        );
    }
    assert!(
        !text.contains("9f1d1f4e") && !text.contains("script") && !text.contains("hooks.example"),
        "and none of them appears anywhere in the output:\n{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_deliveries_total",
            &[("tenant", "unknown"), ("outcome", "dead")]
        ),
        1.0,
        "they are not dropped either: what cannot be named is counted under `unknown`:\n{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_deliveries_total",
            &[("tenant", "unknown"), ("outcome", "sent")]
        ),
        2.0,
        "{text}"
    );
}

/// A delivery that is buried is counted as buried, which is the outcome a queue's health needs.
#[tokio::test]
async fn a_delivery_that_dies_is_counted_as_dead() {
    let fixture = Fixture::start();
    let refusing = Target::start(vec![Reply::status(500)]);
    fixture.register(ACME.1, &refusing.url("/hook"));
    fixture.accept(ACME.1, "doomed", 1);

    let pool = fixture.pool().await;
    let registry = Arc::new(Metrics::new());
    // One attempt is all this delivery gets: what `relay/2.8-retry-budget` decides when the budget
    // runs out is `dead`, and that is the value this test is about.
    let worker = fixture.worker(
        &pool,
        &registry,
        Budget {
            max_attempts: 1,
            ..Budget::default()
        },
    );
    worker.work(2).await.expect("the worker runs two turns");

    assert_eq!(
        fixture.text("select state from deliveries"),
        "dead",
        "a 5xx is a retryable failure, and the budget was one attempt"
    );
    let text = registry.render(&[]);
    assert_eq!(
        value(
            &text,
            "relay_deliveries_total",
            &[("tenant", ACME.0), ("outcome", "dead")]
        ),
        1.0,
        "the delivery is counted by what happened to it:\n{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_deliveries_total",
            &[("tenant", ACME.0), ("outcome", "sent")]
        ),
        0.0,
        "{text}"
    );
    assert_eq!(
        value(
            &text,
            "relay_delivery_duration_seconds_count",
            &[("tenant", ACME.0)]
        ),
        1.0,
        "a failed attempt took time and the histogram says how much:\n{text}"
    );
}

/// The route is in the contract, with every answer it can give.
#[test]
fn the_contract_declares_the_metrics_route() {
    let fixture = Fixture::start();
    let document = fixture.app.client().get("/openapi.json").json();
    let route = document
        .pointer("/paths/~1metrics/get")
        .unwrap_or_else(|| panic!("the document declares no `GET /metrics`: {document}"));
    for status in ["200", "401", "403", "429"] {
        assert!(
            route.pointer(&format!("/responses/{status}")).is_some(),
            "the service answers `{status}` on this route, so the document has to describe it: \
             {route}"
        );
    }
    assert!(
        route
            .pointer("/responses/200/content/text~1plain")
            .is_some(),
        "and the media type of the answer is part of the contract, not a detail: {route}"
    );
}
