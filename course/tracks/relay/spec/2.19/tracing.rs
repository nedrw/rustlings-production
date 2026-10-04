//! Spec for `relay/2.19-trace-propagation`: the trace id comes in with the request and leaves with
//! the delivery.
//!
//! `kiln run` copies this file into the project as `tests/spec_tracing.rs`; it is the course's file,
//! not the learner's (plan §3 D2). One property, judged from three places, because the property is
//! about a *boundary* and each place can only see one side of it:
//!
//! * **the wire in.** A `traceparent` on `POST /events` becomes the trace of the work that request
//!   created — its trace id, its sampling decision, and its vendor state;
//! * **the wire out.** What the subscriber receives is a header this service *generated*: the same
//!   trace id, a span id of its own, and a shape a stranger's collector can read. The last part is
//!   what makes "the trace id equals the caller's" a real assertion rather than a string comparison:
//!   the header is parsed back, field by field, out of the request the target recorded;
//! * **the queue in between.** The trace is not carried in memory across the request and the
//!   delivery: it is written down when the event is accepted, and the row is where the adapter reads
//!   it back from. A delivery is made by a process that never saw the request, which is the failure
//!   this step is really about.
//!
//! A second, smaller property, and it is the one a tracing rollout turns on: **a header this service
//! cannot read is not an error**. The delivery happens, the trace restarts, and the caller's broken
//! trace id does not appear anywhere downstream — a service that refused events because of a header
//! would be a service where one caller's bug is everybody's outage.
//!
//! One test drives the real binary (`tests/support/service.rs`): that the trace id is in the log line
//! of the request *and* on the wire is a claim about a process's own logs, and an in-process fixture
//! does not write them.

mod support;

#[path = "support/target.rs"]
mod target;

use std::sync::Arc;
use std::time::{Duration, Instant};

use relay::db::Db;
use relay::delivery::{self, Sender};
use relay::outbound::{HttpSender, Policy};
use relay::trace::{self, Context, DbTraces, TraceId, Traceparent};
use serde_json::json;
use sqlx::PgPool;
use support::app::{self, App};
use support::client::Request;
use support::postgres::Postgres;
use support::service::Service;
use support::var;
use target::{Reply, Target};

/// The tenant these tests act as.
const ACME: (&str, &str) = ("acme", "key-acme-5f3d");
/// The trace id of the W3C's own example, which is what a caller's tracing library would send.
const CALLER_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
/// The span id from the same example: the caller's hop, which this service must not reuse.
const CALLER_SPAN: &str = "00f067aa0ba902b7";
/// How long a test waits for a process to do something.
const DEADLINE: Duration = Duration::from_secs(30);
/// How often a waiting test looks again.
const POLL: Duration = Duration::from_millis(50);

/// One database, one in-process service and one scripted subscriber.
struct Fixture {
    app: App,
    postgres: Postgres,
    subscriber: Target,
}

impl Fixture {
    /// Start the service with one tenant, and register one subscriber for it.
    fn start() -> Self {
        let postgres = Postgres::start();
        let subscriber = Target::start(vec![Reply::ok("{}")]);
        let app = App::start(app::config(&[
            var(app::DATABASE_URL, postgres.url()),
            var(app::BOOTSTRAP_TENANTS, format!("{}={}", ACME.0, ACME.1)),
            // The subscriber in this file is on `127.0.0.1` (`relay/2.12-ssrf-defense`).
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
        let fixture = Self {
            app,
            postgres,
            subscriber,
        };
        fixture.register(ACME.1, &fixture.subscriber.url("/hook"));
        fixture
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

    /// Accept one event, carrying the headers the caller sent.
    fn accept(&self, idempotency: &str, headers: &[(&str, &str)]) -> support::client::Response {
        let mut request = Request::post("/events", Vec::new())
            .bearer(ACME.1)
            .header("idempotency-key", idempotency);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let accepted = self
            .app
            .client()
            .send(request.json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })));
        assert_eq!(accepted.status, 201, "{}", accepted.body);
        accepted
    }

    /// The pool the worker runs on.
    async fn pool(&self) -> PgPool {
        Db::connect(&self.postgres.url())
            .await
            .expect("the fixture's pool connects")
            .pool()
            .clone()
    }

    /// A sender that reads its traces out of the database, as the deployment's does.
    fn sender(&self, pool: &PgPool) -> Arc<dyn Sender> {
        Arc::new(
            HttpSender::new(Policy::default()).with_traces(Arc::new(DbTraces::new(pool.clone()))),
        )
    }

    /// One turn of the worker: everything due goes out through `sender`.
    async fn tick(&self, pool: &PgPool, sender: &Arc<dyn Sender>) {
        delivery::tick(pool, sender.as_ref(), "trace")
            .await
            .expect("one turn of the worker runs");
    }

    /// The trace id on the delivery row, as the database has it.
    fn stored(&self) -> Option<String> {
        let answer = self
            .postgres
            .query("select coalesce(trace_id, '') from deliveries order by id limit 1");
        let answer = answer.trim();
        (!answer.is_empty()).then(|| answer.to_owned())
    }
}

/// The `traceparent` of the nth request the subscriber received, parsed back into its fields.
fn received(subscriber: &Target, index: usize) -> Traceparent {
    let requests = subscriber.requests();
    let recorded = requests.get(index).unwrap_or_else(|| {
        panic!(
            "the subscriber received {} request(s), and this test wanted the one at {index}",
            requests.len()
        )
    });
    let header = recorded.header(trace::TRACEPARENT).unwrap_or_else(|| {
        panic!(
            "the delivery carries no `traceparent`: {:?}",
            recorded.headers
        )
    });
    Traceparent::parse(header).unwrap_or_else(|error| {
        panic!("`{header}` is not a traceparent this service generated: {error}")
    })
}

/// A trace id the caller sent is the trace of the delivery, one hop later.
#[tokio::test]
async fn the_callers_trace_reaches_the_subscriber_by_the_queue() {
    let fixture = Fixture::start();
    let header = format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01");
    fixture.accept("traced", &[(trace::TRACEPARENT, &header)]);
    let pool = fixture.pool().await;
    let sender = fixture.sender(&pool);
    fixture.tick(&pool, &sender).await;

    let out = received(&fixture.subscriber, 0);
    assert_eq!(
        out.trace().as_str(),
        CALLER_TRACE,
        "the trace id is the caller's, end to end: a delivery with a trace id of its own is a \
         delivery nobody can join to the request that asked for it"
    );
    assert_ne!(
        out.span().as_str(),
        CALLER_SPAN,
        "and the span is this service's own: the caller's span is one hop, this delivery is another"
    );
    assert!(
        out.sampled(),
        "the caller's sampling decision is propagated"
    );

    // The queue is what carried it: the row is where the adapter read the trace from.
    assert_eq!(
        fixture.stored().as_deref(),
        Some(CALLER_TRACE),
        "the trace id is written down with the work, in the transaction that accepted the event — \
         no request is in memory when the delivery happens"
    );
    assert_eq!(
        fixture
            .postgres
            .query("select coalesce(tracestate, '') from deliveries limit 1")
            .trim(),
        "",
        "and no vendor state is invented when the caller sent none"
    );
}

/// A caller that sends no trace still gets a delivery, and it has a trace of its own.
#[tokio::test]
async fn a_request_without_a_trace_starts_one() {
    let fixture = Fixture::start();
    fixture.accept("untraced", &[]);
    let pool = fixture.pool().await;
    let sender = fixture.sender(&pool);
    fixture.tick(&pool, &sender).await;

    let out = received(&fixture.subscriber, 0);
    assert_eq!(
        out.trace().as_str().len(),
        32,
        "a trace that was generated here is a well-formed trace id: {}",
        out.trace()
    );
    assert!(
        out.trace()
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()),
        "and hex: {}",
        out.trace()
    );
    assert!(
        out.trace().as_str().bytes().any(|byte| byte != b'0'),
        "and not the reserved all-zero id: {}",
        out.trace()
    );
    assert_eq!(
        fixture.stored().as_deref(),
        Some(out.trace().as_str()),
        "the row and the wire agree, which is what makes the trace usable from either end"
    );
}

/// A header this service cannot read is ignored, and the delivery happens anyway.
#[tokio::test]
async fn a_header_that_cannot_be_read_is_ignored() {
    let broken = [
        // A version this service does not speak.
        format!("01-{CALLER_TRACE}-{CALLER_SPAN}-01"),
        // Upper case hex: the format is lower case, and a header nobody would generate is a header
        // nobody can compare.
        format!("00-{}-{CALLER_SPAN}-01", CALLER_TRACE.to_uppercase()),
        // Three fields, and five.
        format!("00-{CALLER_TRACE}-{CALLER_SPAN}"),
        format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01-extra"),
        // All-zero ids, which the format reserves.
        format!("00-{}-{CALLER_SPAN}-01", "0".repeat(32)),
        format!("00-{CALLER_TRACE}-{}-01", "0".repeat(16)),
        // The wrong lengths.
        format!("00-{}-{CALLER_SPAN}-01", &CALLER_TRACE[..31]),
        format!("00-{CALLER_TRACE}-{}-01", &CALLER_SPAN[..15]),
        // Not a header at all.
        CALLER_TRACE.to_owned(),
        String::new(),
    ];

    let fixture = Fixture::start();
    let pool = fixture.pool().await;
    let sender = fixture.sender(&pool);

    for (index, header) in broken.iter().enumerate() {
        let before = fixture.subscriber.requests().len();
        fixture.accept(
            // One event per iteration: the same key twice is one event (`relay/2.2-idempotent-ingest`).
            &format!("broken-{index}"),
            &[
                (trace::TRACEPARENT, header.as_str()),
                // A vendor state next to a trace the service cannot read: it goes too, because a
                // `tracestate` only means anything beside the trace it arrived with.
                (trace::TRACESTATE, "vendor=abc123"),
            ],
        );
        fixture.tick(&pool, &sender).await;

        let out = received(&fixture.subscriber, before);
        assert!(
            Traceparent::parse(&out.to_string()).is_ok(),
            "`{header}` is not a traceparent this service can read, and what it sends downstream is \
             still one that anybody can: {out}"
        );
        assert!(
            !out.trace().as_str().starts_with(CALLER_TRACE),
            "and it is a new trace rather than the caller's broken one: {out} came from `{header}`"
        );
        // Every delivery of every one of these requests is a well-formed trace of its own: nothing
        // was refused, and nothing was copied out of a header the service could not read.
        assert_eq!(
            fixture
                .postgres
                .query(
                    "select count(*) from deliveries \
                      where trace_id is null or trace_id !~ '^[0-9a-f]{32}$'"
                )
                .trim(),
            "0",
            "every accepted event is traceable ({index})"
        );
        assert_eq!(
            fixture
                .postgres
                .query("select count(*) from deliveries where tracestate is not null")
                .trim(),
            "0",
            "and the unreadable header's vendor state was dropped with it"
        );
    }
    assert_eq!(
        fixture.subscriber.requests().len(),
        broken.len(),
        "every one of these requests was delivered: a header this service cannot read is not a \
         reason to lose an event"
    );
}

/// One event, two subscribers: one trace, two spans.
#[tokio::test]
async fn every_hop_gets_a_span_and_the_trace_stays_the_same() {
    let fixture = Fixture::start();
    let second = Target::start(vec![Reply::ok("{}")]);
    fixture.register(ACME.1, &second.url("/hook"));
    let header = format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01");
    fixture.accept("fan-out", &[(trace::TRACEPARENT, &header)]);
    let pool = fixture.pool().await;
    let sender = fixture.sender(&pool);
    fixture.tick(&pool, &sender).await;
    // Both endpoints answer at once, so the two deliveries may be attempted in either order: read
    // them as a set rather than as two positions.
    let mut spans: Vec<String> = Vec::new();
    for recorded in fixture
        .subscriber
        .requests()
        .iter()
        .chain(second.requests().iter())
    {
        let header = recorded
            .header(trace::TRACEPARENT)
            .unwrap_or_else(|| panic!("a delivery with no traceparent: {:?}", recorded.headers));
        let parent = Traceparent::parse(header).expect("a header this service generated parses");
        assert_eq!(
            parent.trace().as_str(),
            CALLER_TRACE,
            "every delivery of one event carries the trace of the request that accepted it"
        );
        spans.push(parent.span().as_str().to_owned());
    }
    assert_eq!(spans.len(), 2, "two endpoints, two deliveries");
    assert_ne!(
        spans[0], spans[1],
        "and two spans: a span is one hop, and these are two different hops — sharing one would \
         make two attempts look like one in whatever the subscriber is running"
    );
}

/// Vendor state travels with the trace, unchanged, in both directions of the claim.
#[tokio::test]
async fn the_vendor_state_travels_with_the_trace() {
    let fixture = Fixture::start();
    let header = format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01");
    fixture.accept(
        "with-state",
        &[
            (trace::TRACEPARENT, &header),
            (trace::TRACESTATE, "vendor=abc123,other=xyz"),
        ],
    );
    fixture.accept("without-state", &[(trace::TRACEPARENT, &header)]);
    let pool = fixture.pool().await;
    let sender = fixture.sender(&pool);
    fixture.tick(&pool, &sender).await;

    let forwarded: Vec<Option<String>> = fixture
        .subscriber
        .requests()
        .iter()
        .map(|recorded| recorded.header(trace::TRACESTATE).map(str::to_owned))
        .collect();
    assert!(
        forwarded.contains(&Some("vendor=abc123,other=xyz".to_owned())),
        "the caller's vendor state is handed on as it arrived: a service that rewrote it would be \
         breaking the agreement between two systems it does not know about: {forwarded:?}"
    );
    assert!(
        forwarded.contains(&None),
        "and a request that carried none produces no header: an empty `tracestate` is a header \
         somebody will parse: {forwarded:?}"
    );
}

/// The trace id is in the log line of the request and on the wire of the delivery.
#[test]
fn the_trace_id_is_in_the_log_and_on_the_wire() {
    let postgres = Postgres::start();
    let subscriber = Target::start(vec![Reply::ok("{}")]);
    let service = Service::start(&[
        var("RELAY__DATABASE__URL", postgres.url()),
        var(
            "RELAY__BOOTSTRAP__TENANTS",
            format!("{}={}", ACME.0, ACME.1),
        ),
        var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
        var("RELAY__RATE_LIMIT__BURST", "100000"),
        var("RELAY__RATE_LIMIT__PER_SECOND", "100000"),
    ]);
    let client = service.client();
    let registered = client.post_json(
        "/endpoints",
        ACME.1,
        &json!({ "url": subscriber.url("/hook") }),
    );
    assert_eq!(registered.status, 201, "{}", registered.body);

    let header = format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01");
    let accepted = client.send(
        Request::post("/events", Vec::new())
            .bearer(ACME.1)
            .header("idempotency-key", "process")
            .header(trace::TRACEPARENT, &header)
            .json(&json!({ "kind": "invoice.paid", "payload": { "n": 1 } })),
    );
    assert_eq!(accepted.status, 201, "{}", accepted.body);

    // The delivery is made by the worker of that process, and this is the one thing a test cannot
    // do for it: wait for it.
    let deadline = Instant::now() + DEADLINE;
    while subscriber.requests().is_empty() && Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
    let out = received(&subscriber, 0);
    assert_eq!(out.trace().as_str(), CALLER_TRACE);

    let lines = service.log_lines();
    assert!(
        lines.iter().any(|line| line.contains(CALLER_TRACE)),
        "the request's own log line carries the trace id, so an operator can go from a delivery \
         that never arrived to the request that asked for it in one search: {lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("trace_id") && line.contains(CALLER_TRACE)),
        "and it carries it as a field rather than somewhere in a message: {lines:#?}"
    );
    service.stop();
}

/// The header's grammar, as a matrix: what is read, what is not, and what a hop does with it.
#[test]
fn parsing_the_header_is_a_matrix() {
    let accepted = [
        (format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01"), true),
        (format!("00-{CALLER_TRACE}-{CALLER_SPAN}-00"), false),
        // A trailing newline is what a header sent by a script has, and trimming it is the
        // difference between "traced" and "not traced" for that caller.
        (format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01\n"), true),
    ];
    for (header, sampled) in accepted {
        let parsed = Traceparent::parse(&header).unwrap_or_else(|error| {
            panic!("`{header}` is a traceparent this service reads: {error}")
        });
        assert_eq!(parsed.trace().as_str(), CALLER_TRACE, "{header}");
        assert_eq!(parsed.span().as_str(), CALLER_SPAN, "{header}");
        assert_eq!(
            parsed.sampled(),
            sampled,
            "the sampling decision is the flag: {header}"
        );
    }

    let refused = [
        "".to_owned(),
        "-".to_owned(),
        "00".to_owned(),
        format!("00-{CALLER_TRACE}-{CALLER_SPAN}"),
        format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01-1"),
        format!("ff-{CALLER_TRACE}-{CALLER_SPAN}-01"),
        format!("00-{}-{CALLER_SPAN}-01", "0".repeat(32)),
        format!("00-{CALLER_TRACE}-{}-01", "0".repeat(16)),
        format!("00-{}-{CALLER_SPAN}-01", CALLER_TRACE.to_uppercase()),
        format!("00-{CALLER_TRACE}-{CALLER_SPAN}-0102"),
        format!("00-{CALLER_TRACE}-{CALLER_SPAN}-zz"),
    ];
    for header in refused {
        assert!(
            Traceparent::parse(&header).is_err(),
            "`{header}` is not a header this service generated, and reading it would mean trusting \
             whatever a caller sent"
        );
    }

    // A hop: the same trace, a span of its own, and the decision that was already made.
    let parent = Traceparent::parse(&format!("00-{CALLER_TRACE}-{CALLER_SPAN}-00"))
        .expect("the caller's header");
    let child = parent.child();
    assert_eq!(child.trace(), parent.trace());
    assert_ne!(child.span(), parent.span());
    assert!(
        !child.sampled(),
        "a hop does not decide what the trace's sampler decided"
    );
    assert_eq!(
        Traceparent::parse(&child.to_string()).expect("what this service generates parses"),
        child,
        "and what this service writes is what this service reads: the format round-trips"
    );

    let context = Context::of(&parent, Some("vendor=1"));
    assert_eq!(context.trace, *parent.trace());
    assert_eq!(context.state.as_deref(), Some("vendor=1"));
    let hop = context.child();
    assert_eq!(hop.trace(), parent.trace(), "the trace survives the queue");
    assert_ne!(hop.span(), parent.span(), "the span does not");
    assert!(!hop.sampled());

    // Two generated traces are two traces: a generator that repeated itself would join unrelated
    // requests into one trace, which is worse than no trace at all.
    let one = TraceId::generate();
    let two = TraceId::generate();
    assert_ne!(one, two);
    assert_eq!(one.as_str().len(), 32);
    assert!(Traceparent::parse(&Traceparent::generate().to_string()).is_ok());
}
