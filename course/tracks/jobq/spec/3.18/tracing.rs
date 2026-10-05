//! Spec for `jobq/3.18-trace-sampling`: the decision is made at the door, carried on the row, and
//! revisited once — at the exit, where the outcome is finally known.
//!
//! `kiln run` copies this file into the project as `tests/spec_tracing.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Seven cases, and every one of them is a behaviour of the engine
//! or of a worker rather than a shape of a file:
//!
//! * **the grammar** — what a `traceparent` is, and what it is not (`relay/2.19-trace-propagation`
//!   wrote the same matrix for a service; this file writes it for a queue);
//! * **the sampler** — the decision is a function of the id, so two hops that ask about one trace
//!   agree, and nothing about the moment they asked in changes the answer;
//! * **the door** — a caller that arrived with a `traceparent` keeps its trace *and its decision*,
//!   because a hop has no business overriding the sampler that started the trace, and a caller that
//!   arrived without one starts a trace here;
//! * **the row** — the carrier. What the door decided is written with the job and read back by
//!   whoever claims it, and a job nobody traced has no trace rather than an invented one;
//! * **the exit** — the case the step is named for: an attempt that *fails* is kept whatever the
//!   door said, and a hop that asks the sampler again instead of reading the row drops exactly the
//!   trace an operator needs;
//! * **the hop** — every attempt mints a span of its own, so a retry is visible as a retry, and the
//!   trace id is the same one every time;
//! * **the second life** — a replay (`jobq/3.16-dlq-tooling`) carries the trace through, because the
//!   incident that ends in a replay is one trace from the door to the attempt that finally worked.
//!
//! All of it runs in this process against the in-memory store: what is being judged is the decision
//! and what carries it, and a container would add a minute to every run of it. The durable backend
//! keeps the same two facts in two columns (`migrations/0007_traces.sql`), which is what
//! `KILN_BACKEND=postgres kiln run jobq/3.18-trace-sampling` proves by running this file against it.

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::util::SubscriberInitExt;

use jobq::engine::{
    BoxFuture, Context, Engine, Error, Hop, Job, JobState, MemoryStore, Queue, Sampler, SpanId,
    Store, SubmitRequest, TraceId, Traceparent, WorkerId,
};
use jobq::service::handler::LogHandler;
use jobq::service::worker::{Handler, Shutdown, Worker, WorkerSettings};
use support::store;

/// How long a case waits for a condition before it gives up and says what it saw.
const DEADLINE: Duration = Duration::from_secs(30);
/// How often a waiting case looks again.
const POLL: Duration = Duration::from_millis(20);
/// A trace id whose last eight bytes are `0`, which every share of the id space includes.
const STARTED: &str = "4bf92f3577b34da60000000000000000";
/// A trace id whose last eight bytes are `999`, which only a share of the whole id space includes.
const DROPPED: &str = "4bf92f3577b34da600000000000003e7";
/// A span id, for the headers below.
const SPAN: &str = "00f067aa0ba902b7";

/// The queue every case in this file uses.
fn queue() -> Queue {
    Queue::parse("emails").expect("`emails` is a name this file uses everywhere")
}

/// A `traceparent` carrying `trace`, with the sampled flag `sampled`.
fn header(trace: &str, sampled: bool) -> String {
    format!("00-{trace}-{SPAN}-{:02x}", u8::from(sampled))
}

/// One environment variable a case passes to the process it starts.
fn engine() -> Engine<Arc<dyn Store>> {
    Engine::new(Arc::new(MemoryStore::new()) as Arc<dyn Store>)
}

#[test]
fn a_traceparent_is_a_grammar_and_not_a_string_to_pass_along() {
    let good = Traceparent::parse(&header(STARTED, true)).expect("this header is well formed");
    assert_eq!(good.trace().as_hex(), STARTED);
    assert_eq!(good.parent().as_hex(), SPAN);
    assert!(good.sampled());
    assert_eq!(
        good.to_string(),
        header(STARTED, true),
        "a header is parsed and regenerated rather than forwarded: what this queue sends is a
         header it built, in the format's own spelling (lower case, four fields)"
    );
    let child = good.child(SpanId::generate());
    assert_eq!(
        child.trace(),
        good.trace(),
        "a child is inside the same trace"
    );
    assert_ne!(
        child.parent(),
        good.parent(),
        "and it is a hop of its own rather than the parent's span reused: two hops that share a
         span look like one step in the waterfall, and the queue between them disappears"
    );
    assert_eq!(
        child.sampled(),
        good.sampled(),
        "the child inherits the sampling decision: a hop with no sampler of its own does not get to
         overrule the one that started the trace"
    );

    for broken in [
        "",
        "00",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
        "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
        "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-zz",
        "not-a-traceparent-at-all",
    ] {
        assert!(
            Traceparent::parse(broken).is_err(),
            "`{broken}` is not a trace this queue may continue, and accepting it invents an id
             somebody else will not find: upper case is a different id to every index, `0000…` is
             the format's own word for \"there is no trace\""
        );
    }

    // And the response to one is not a refusal of the job: a caller with a malformed header gets a
    // trace of its own (`relay/2.19`: a lost parent link is a worse trace, a dropped job is a worse
    // queue).
    let context = Context::of(None, &Sampler::always());
    assert!(!context.trace().as_hex().is_empty());
}

#[test]
fn the_sampler_decides_from_the_id_and_from_nothing_else() {
    let started = TraceId::parse(STARTED).expect("this id is well formed");
    let dropped = TraceId::parse(DROPPED).expect("this id is well formed");

    assert!(Sampler::always().decide(&started));
    assert!(Sampler::always().decide(&dropped));
    assert!(!Sampler::never().decide(&started));
    assert!(
        Sampler::per_mille(1).decide(&started),
        "this id's last bytes are 0, which is the first thousandth of the id space and therefore in
         every share — the arithmetic is the point: a decision that came out of a clock or a
         counter would make this case a coin toss"
    );
    assert!(
        !Sampler::per_mille(999).decide(&dropped),
        "and the last thousandth is the one a share of 999 leaves out: the decision is a property of
         the id, so the deployment's share and the id are the whole input"
    );
    assert!(Sampler::per_mille(1000).decide(&dropped));
    assert_eq!(
        Sampler::per_mille(500).decide(&dropped),
        Sampler::per_mille(500).decide(&dropped),
        "two hops asking one question about one trace get one answer"
    );
    assert_eq!(
        Sampler::per_mille(4000).share(),
        1000,
        "more than all of them is all of them"
    );
}

#[test]
fn the_door_keeps_the_callers_decision_and_starts_a_trace_when_there_is_none() {
    // A caller inside a trace it decided not to keep, arriving at a deployment that samples
    // everything: the caller wins, and its ids are the ones that continue.
    let parent = Traceparent::parse(&header(DROPPED, false)).expect("this header is well formed");
    let inherited = Context::of(Some(&parent), &Sampler::always());
    assert_eq!(inherited.trace(), parent.trace());
    assert!(
        !inherited.sampled(),
        "the parent's decision is kept rather than re-made: this deployment's sampler would have
         said yes, and a hop that overrules its caller drops a trace that is already being
         collected somewhere else"
    );

    // A caller with nothing: the trace starts here, and this deployment's share decides.
    let started = Context::of(None, &Sampler::always());
    assert!(started.sampled());
    assert_ne!(
        started.trace(),
        parent.trace(),
        "a trace starts where somebody first asked, and two callers that sent nothing do not end up
         in one trace together"
    );
}

#[tokio::test]
async fn what_the_door_decided_is_written_on_the_row_and_read_back() {
    // The port rather than the engine, and `KILN_BACKEND` rather than a store: this is the case about
    // what a row *keeps*, so it is the one case in this file that has to run against both backends
    // — the in-memory store keeps the context in a field and the durable one keeps it in two
    // columns (`migrations/0007_traces.sql`), and `KILN_BACKEND=postgres kiln run jobq/3.18-\
    // trace-sampling` is what proves the two agree.
    let backend = store::backend().await;
    let port = backend.store();
    let worker = WorkerId::new("w");
    let context = Context::of(None, &Sampler::always());
    let job = port
        .enqueue(
            SubmitRequest::new(queue(), serde_json::json!({ "to": "a@b" }))
                .with_trace(context.clone()),
        )
        .await
        .unwrap_or_else(|error| panic!("the {} backend refused a submit: {error}", backend.name()));

    let written = port
        .get(&job.id())
        .await
        .expect("a read cannot fail")
        .expect("the job was just written");
    assert_eq!(
        written.trace(),
        Some(&context),
        "the row is the carrier: a worker is another process, and the only context it can read is
         the one somebody wrote down ({})",
        backend.name()
    );

    let claimed = port
        .claim(&worker, &queue(), 1)
        .await
        .expect("claiming cannot fail");
    assert_eq!(
        claimed.first().and_then(|job| job.trace()),
        Some(&context),
        "and the claim hands it on: what a worker is given is what the door decided, not a fresh
         decision ({})",
        backend.name()
    );

    // A caller that sent nothing still gets a job: it simply has no trace, and the absence travels.
    let untraced = port
        .enqueue(SubmitRequest::new(
            queue(),
            serde_json::json!({ "to": "c@d" }),
        ))
        .await
        .expect("submitting cannot fail");
    assert_eq!(
        port.get(&untraced.id())
            .await
            .expect("the job is there")
            .expect("the job is there")
            .trace(),
        None,
        "no trace is a job nobody asked to trace: an id invented at the attempt is a trace that
         exists in one line and nowhere else"
    );
}

/// A handler that fails every attempt, so a case can watch an attempt that ended badly.
struct Failing;

impl Handler for Failing {
    fn handle<'a>(&'a self, _job: &'a Job) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            Err(Error::Invalid {
                detail: "the subscriber said 421".to_owned(),
            })
        })
    }
}

/// A buffer a subscriber writes into, so a case can read the lines a worker emitted.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    /// Everything written so far, as the lines a collector would have.
    fn lines(&self) -> Vec<Value> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("`json` mode prints one JSON object per line ({error}): `{line}`")
                })
            })
            .collect()
    }
}

impl std::io::Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for Sink {
    type Writer = Sink;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The fields of an event, where a collector finds them.
fn fields(event: &Value) -> &serde_json::Map<String, Value> {
    event["fields"]
        .as_object()
        .unwrap_or_else(|| panic!("this event has no `fields` object: {event}"))
}

/// One field of an event, as the collector reads it.
fn field<'a>(event: &'a Value, name: &str) -> &'a Value {
    fields(event)
        .get(name)
        .unwrap_or_else(|| panic!("this event has no field `{name}`: {event}"))
}

/// Drive one job through a worker in this process, under a subscriber that writes into `sink`.
///
/// The current-thread runtime and the *default* (thread-scoped) subscriber are what make this
/// possible at all: see the same fixture in `spec_alerts.rs`. The store is the in-memory one and
/// the context is whatever the caller passes, which is how a case puts a trace on a job that the
/// worker would otherwise decide about itself.
async fn drain<H: Handler + 'static>(
    handler: H,
    context: Option<Context>,
    sink: &Sink,
) -> Shutdown {
    let engine = engine();
    let queue = queue();
    let mut request = SubmitRequest::new(queue.clone(), serde_json::json!({ "to": "a@b" }));
    if let Some(context) = context {
        request = request.with_trace(context);
    }
    engine
        .submit(request)
        .await
        .expect("submitting cannot fail");
    sink.0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();

    let shutdown = Shutdown::new();
    let worker = Worker::new(
        Arc::clone(engine.store()),
        handler,
        WorkerSettings::new(WorkerId::new("tracer-1"), vec![queue])
            .with_poll_interval(Duration::from_millis(10)),
    );
    let signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = worker.run(signal).await;
    });
    shutdown
}

/// Wait for a line that satisfies `wanted`, and answer it.
async fn wait_for(sink: &Sink, wanted: impl Fn(&Value) -> bool, what: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(found) = sink.lines().into_iter().find(&wanted) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "no line {what} within {DEADLINE:?}: {:?}",
            sink.lines()
        );
        tokio::time::sleep(POLL).await;
    }
}

/// A subscriber that captures into `sink`, as a guard a case drops when it is done.
fn capture(sink: &Sink) -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_max_level(LevelFilter::INFO)
        .with_writer(sink.clone())
        .set_default()
}

/// A trace the door decided *not* to keep, so the exit's decision is the only one left.
fn not_sampled() -> Context {
    Context::of(None, &Sampler::never())
}

#[tokio::test]
async fn a_failed_attempt_is_kept_even_though_the_door_said_no() {
    let context = not_sampled();
    let trace = context.trace().as_hex();
    let sink = Sink::default();
    let guard = capture(&sink);

    let shutdown = drain(Failing, Some(context), &sink).await;
    let event = wait_for(
        &sink,
        |event| fields(event).get("outcome").and_then(Value::as_str) == Some("failed"),
        "about a failed attempt",
    )
    .await;
    shutdown.trigger();
    drop(guard);

    assert_eq!(
        field(&event, "trace_id"),
        trace.as_str(),
        "the attempt line carries the trace the row carried: the id is the one thing that has to
         survive the queue, and regenerating it is how a trace becomes two"
    );
    assert_eq!(
        field(&event, "sampled"),
        true,
        "and this hop is kept although the door said no: the trace an operator needs is the one
         nobody asked for in advance, and the worker is the only place that learns the outcome"
    );
    assert!(
        field(&event, "span_id")
            .as_str()
            .is_some_and(|span| span.len() == 16 && SpanId::parse(span).is_ok()),
        "the line has a span of this hop's own: an attempt is a step of the waterfall, and it needs
         the format's ids rather than a counter ({event})"
    );
    assert_ne!(
        field(&event, "span_id").as_str(),
        Some(SPAN),
        "and not the caller's span: reusing it makes the queue disappear from the trace"
    );
}

#[tokio::test]
async fn a_hop_that_succeeded_minds_the_doors_decision() {
    let context = not_sampled();
    let trace = context.trace().as_hex();
    let sink = Sink::default();
    let guard = capture(&sink);

    let shutdown = drain(LogHandler, Some(context), &sink).await;
    let event = wait_for(
        &sink,
        |event| fields(event).get("outcome").and_then(Value::as_str) == Some("succeeded"),
        "about a succeeded attempt",
    )
    .await;
    shutdown.trigger();
    drop(guard);

    assert_eq!(field(&event, "trace_id"), trace.as_str());
    assert_eq!(
        field(&event, "sampled"),
        false,
        "nothing happened that anybody needs to look at, so the door's answer stands — keeping
         every trace is the same as sampling none of them ({event})"
    );
}

#[tokio::test]
async fn every_attempt_is_a_hop_of_its_own() {
    let context = Context::of(None, &Sampler::always());
    let trace = context.trace().as_hex();
    let sink = Sink::default();
    let guard = capture(&sink);

    // A failing handler spends the job's budget one attempt at a time, so this case sees a retry
    // without waiting for anything: three attempts, three spans, one trace.
    let shutdown = drain(Failing, Some(context), &sink).await;
    let _ = wait_for(
        &sink,
        |event| fields(event).get("attempt").and_then(Value::as_u64) == Some(3),
        "about a third attempt",
    )
    .await;
    let lines = sink.lines();
    shutdown.trigger();
    drop(guard);

    let attempts: Vec<&Value> = lines
        .iter()
        .filter(|event| fields(event).contains_key("outcome"))
        .collect();
    let spans: std::collections::BTreeSet<&str> = attempts
        .iter()
        .filter_map(|event| field(event, "span_id").as_str())
        .collect();
    assert_eq!(
        spans.len(),
        attempts.len(),
        "every attempt mints a span of its own ({} attempts, {} spans): a retry that shares the
         first attempt's span is invisible, and the second attempt is the one worth looking at",
        attempts.len(),
        spans.len()
    );
    for event in &attempts {
        assert_eq!(
            field(event, "trace_id"),
            trace.as_str(),
            "and the trace id is the same on every one of them"
        );
        assert_eq!(field(event, "sampled"), true);
    }
}

#[tokio::test]
async fn a_dead_letter_keeps_its_trace_through_the_replay() {
    let engine = engine();
    let context = Context::of(None, &Sampler::never());
    let id = engine
        .submit(
            SubmitRequest::new(queue(), serde_json::json!({ "to": "a@b" }))
                .with_trace(context.clone())
                .with_max_attempts(1),
        )
        .await
        .expect("submitting cannot fail");

    let worker = WorkerId::new("tracer-1");
    engine
        .claim(&worker, &queue(), 1)
        .await
        .expect("claiming cannot fail");
    let state = engine
        .fail(&worker, &id, "the subscriber said 421")
        .await
        .expect("recording a failure cannot fail");
    assert_eq!(state, JobState::Failed, "one attempt of one");

    let replayed = engine
        .store()
        .replay(&id)
        .await
        .expect("a dead letter has a second life");
    assert_eq!(
        replayed.trace(),
        Some(&context),
        "a second life keeps who it was: the incident an operator is reconstructing is one trace
         from the request that submitted the job to the attempt that finally worked, and a replay
         that dropped the trace would break it exactly where somebody is looking"
    );
    assert_eq!(
        replayed.state(),
        JobState::Queued,
        "and it is the same row: the trace is not a field this step re-decides"
    );

    // The trace is a fact about the job rather than about a process, so it is also what the store
    // answers for a job it never handed out: this is the same read the daemon makes.
    let fresh = engine.store().get(&id).await.expect("a read cannot fail");
    assert_eq!(
        fresh.and_then(|job| job.trace().cloned()),
        Some(context),
        "what is on the row is what every reader sees"
    );
}

/// `Hop` is the hop-shaped half of the same rule, and the reason it is a type rather than two
/// arguments at each call site: the context is the job's and the span is the hop's, and mixing them
/// up is how a trace gets two ids or one hop reuses another's span.
#[test]
fn a_hop_is_a_span_of_its_own_over_the_job_s_context() {
    let context = Context::of(None, &Sampler::never());
    let first = Hop::of(&context);
    let second = Hop::of(&context);
    assert_eq!(first.trace(), context.trace());
    assert_eq!(second.trace(), context.trace());
    assert_ne!(first.span(), second.span());
    assert!(
        !first.exported(false),
        "the door decided against this trace and nothing failed, so this hop is not kept"
    );
    assert!(
        first.exported(true),
        "and a failure is kept: the decision is made from the door's answer and the outcome, never
         from a fresh roll"
    );
}
