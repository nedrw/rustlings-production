//! Spec for `relay/2.20-load-testing`: the load generator, the percentiles, and the curve.
//!
//! `kiln run` copies this file into the project as `tests/spec_load.rs`. It is the course's file,
//! not the learner's (plan §3 D2).
//!
//! Four properties, and each one is judged where it can be observed rather than argued:
//!
//! * **the generator is a function.** `loadtest::keys(200)` answers the same 200 unique keys on
//!   every machine, so a run can be repeated; and the ledger of a run is *the list*: the events the
//!   service holds are exactly the keys that were offered, once each, however many producers
//!   offered them. A generator that emitted a key twice would show up here as 199 events, and a
//!   generator seeded by the clock as a second run that adds rows;
//! * **the arithmetic is exact.** Every percentile is a nearest-rank answer, so a sample of twenty
//!   has three numbers a reader can compute by hand — and the spec computes them. This is the one
//!   half of a load run that does not depend on the machine, which is exactly why it is checked
//!   here;
//! * **the curve is a shape, and a shape can be broken.** The real run records one point per
//!   concurrency level (`tests/support/workload.rs` offers its keys from that many producers at
//!   once) and the spec asserts what is actually true of it: at least three levels, ordered, each
//!   inside a bound, and a backlog that grows — a queue's depth is arithmetic, not a clock. The
//!   *p99 trend* is recorded and printed, **not** asserted: on a machine that is also compiling,
//!   one level's largest observation moves further than the load does, and a spec that failed for
//!   that would be measuring the machine. What the shape check (`is_monotone_within`) is for is
//!   the pair of synthetic curves below, where it is proved to pass a rising curve and to refuse a
//!   collapsing one — otherwise the check, and the page written from it, would be decoration;
//! * **the load is work the service finishes.** Everything the ingest path accepted is delivered:
//!   the deliveries reach `sent`, the count is the events times the subscribers, and the backlog
//!   is empty. A load test that measured what it could push in and never what came out would
//!   report a service that is fast at forgetting.

#[path = "support/workload.rs"]
mod workload;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use relay::backoff::Budget;
use relay::concurrency::{Gate, Limits, Worker};
use relay::delivery::Sender;
use relay::loadtest::{self, Curve, JITTER, Percentiles, Point, percentile};
use relay::ordering::Lanes;
use workload::{Counted, Workload};

/// How many subscribers the fixture registers: the fan-out of one event.
const ENDPOINTS: usize = 2;
/// How many events one concurrency level offers.
const PER_LEVEL: usize = 60;
/// The concurrency levels a curve is read from: the shape of a queue that is just keeping up, that
/// is contended, and that is waiting.
const LEVELS: [usize; 3] = [1, 2, 4];
/// The traffic a full run puts through the ingest path.
const RUN: usize = 200;
/// How slow a request may be before this service is not a service any more.
const SLOWEST: Duration = Duration::from_millis(5_000);

/// Milliseconds, because every number in this file is one.
fn ms(milliseconds: u64) -> Duration {
    Duration::from_millis(milliseconds)
}

/// A worker over `pool` whose subscriber takes everything, with `seats` seats.
fn worker(pool: &sqlx::PgPool, seats: u32, counted: &Arc<Counted>) -> Worker {
    let limits = Limits::new(seats, 10_000).expect("seats and room for the queue");
    let sender: Arc<dyn Sender> = Arc::clone(counted) as Arc<dyn Sender>;
    Worker::new(
        pool.clone(),
        sender,
        Arc::new(Gate::new(limits)),
        Arc::new(Lanes::new()),
        Budget::default(),
        limits,
        "worker-load",
    )
}

/// The keys a run offers are the events it holds: once each, however many producers offered them.
#[test]
fn the_keys_of_a_run_are_the_events_of_its_ledger() {
    let workload = Workload::start(ENDPOINTS, &[]);
    let keys = loadtest::keys(RUN);
    assert_eq!(
        keys.len(),
        RUN,
        "the generator answers as many keys as it was asked for: {} of {RUN}",
        keys.len()
    );
    let unique: BTreeSet<&String> = keys.iter().collect();
    assert_eq!(
        unique.len(),
        RUN,
        "two keys that are the same string are one event, so a generator that repeats one is a load \
         run that offers less than it says: {} distinct of {}",
        unique.len(),
        keys.len()
    );
    assert_eq!(
        loadtest::keys(RUN),
        keys,
        "the generator is a function of its argument: a run nobody can repeat is a number nobody can \
         check"
    );
    assert_ne!(
        loadtest::keys_from(RUN, RUN),
        keys,
        "and two levels of one curve offer different events, or the second level is a replay of the \
         first and measures nothing"
    );

    let offer = workload.offer(&keys, 4);
    assert_eq!(
        offer.answered(),
        RUN,
        "every offer is answered: {} accepted, {} replayed, {} refused",
        offer.accepted,
        offer.replayed,
        offer.refused
    );
    assert_eq!(
        offer.accepted, RUN,
        "each key is new to this service, so each is a `201` — not a replay, and not a refusal"
    );
    assert_eq!(
        workload.count("select count(*) from events"),
        i64::try_from(RUN).expect("a small number"),
        "the ledger is the list: the service holds exactly the events that were offered, no fewer \
         (a dropped offer) and no more (one key accepted as two events)"
    );
    assert_eq!(
        workload.count("select count(distinct idempotency_key) from events"),
        i64::try_from(RUN).expect("a small number"),
        "and it holds each of them once: `relay/2.2-idempotent-ingest` is what makes the set of \
         keys and the set of events the same set"
    );

    // The same load, offered again: what a producer does after a timeout, and what the load
    // generator has to make repeatable.
    let again = workload.offer(&keys, 4);
    assert_eq!(
        again.replayed, RUN,
        "the second run of the same keys is answered with the events already held (`200`), and with \
         nothing else: {} accepted, {} refused",
        again.accepted, again.refused
    );
    assert_eq!(
        workload.count("select count(*) from events"),
        i64::try_from(RUN).expect("a small number"),
        "and the ledger has not moved: a load run that could not be repeated would add rows here"
    );
    assert_eq!(
        workload.count("select count(*) from deliveries"),
        i64::try_from(RUN * ENDPOINTS).expect("a small number"),
        "nor has the queue: {RUN} events and {ENDPOINTS} subscribers are {} deliveries",
        RUN * ENDPOINTS
    );
}

/// The percentiles, against a sample of twenty whose three answers are computable by hand.
#[test]
fn the_percentiles_are_the_ones_a_sample_of_twenty_gives() {
    // Twenty observations, 5 ms apart: 5, 10, … 100 ms. Nearest rank: `ceil(q * n)`-th of the
    // sorted sample, so the 10th is the median, the 19th is the p95, and the 20th is the p99.
    let samples: Vec<Duration> = (1..=20).map(|n| ms(n * 5)).collect();
    let measured = Percentiles::of(&samples);
    assert_eq!(
        (measured.p50, measured.p95, measured.p99),
        (ms(50), ms(95), ms(100)),
        "the 10th, 19th and 20th of twenty observations: {measured}"
    );

    // The order of the sample is the caller's business, not the function's.
    let mut shuffled = samples.clone();
    shuffled.rotate_left(7);
    assert_eq!(
        Percentiles::of(&shuffled),
        measured,
        "the same observations in another order are the same run: {measured}"
    );

    let one = Percentiles::of(&[ms(7)]);
    assert_eq!(
        (one.p50, one.p95, one.p99),
        (ms(7), ms(7), ms(7)),
        "one observation is every quantile of one observation"
    );
    assert_eq!(
        Percentiles::of(&[]),
        Percentiles::default(),
        "a percentile of nothing is nothing — not a panic in a report somebody reads at three in the \
         morning"
    );
    assert_eq!(
        percentile(&samples, 0.0),
        ms(5),
        "a quantile below the sample is the first observation, not an index under it"
    );
    assert_eq!(
        percentile(&samples, 2.0),
        ms(100),
        "and one above it is the last"
    );
    assert!(
        measured.p50 <= measured.p95 && measured.p95 <= measured.p99,
        "the quantiles of one sample are ordered by construction: {measured}"
    );
    assert!(
        measured.one_line().contains("p99"),
        "the report names the quantiles it is about: {}",
        measured.one_line()
    );
}

/// A curve is read as a shape, and the shape check is not vacuous: a rising curve passes it and a
/// collapsing one does not.
#[test]
fn a_curve_carries_a_shape_that_can_be_broken() {
    let mut rising = Curve::new();
    for (concurrency, p99, backlog) in [(1, 4, 10), (2, 9, 24), (4, 31, 60)] {
        rising.record(Point {
            concurrency,
            p99: ms(p99),
            backlog,
        });
    }
    assert_eq!(
        rising.points().len(),
        3,
        "one level per point, in the order they were run: {:?}",
        rising.points()
    );
    assert!(
        rising.is_monotone_within(JITTER),
        "a curve that rises at every level is a curve: {}",
        rising.render()
    );
    let (second, third) = (rising.points()[1], rising.points()[2]);
    assert!(
        (third.growth_from(&second) - 31.0 / 9.0).abs() < 1e-9,
        "the growth of a level over the one below it is what a slope is computed from: {}",
        third.growth_from(&second)
    );

    let mut collapsing = Curve::new();
    for (concurrency, p99, backlog) in [(1, 40, 10), (2, 6, 24), (4, 4, 60)] {
        collapsing.record(Point {
            concurrency,
            p99: ms(p99),
            backlog,
        });
    }
    assert!(
        !collapsing.is_monotone_within(JITTER),
        "a curve whose levels fall to a tenth of the one below them is not flat — it is a run that \
         stopped measuring what it thought it was measuring, and the check has to say so: {}",
        collapsing.render()
    );
    assert!(
        Curve::new().is_monotone_within(JITTER),
        "a curve with no levels has no level that breaks the shape"
    );

    let table = rising.render();
    assert!(
        table.contains("concurrency") && table.contains("p99") && table.contains("backlog"),
        "the rendered curve is a table a capacity page can quote: a level, what it cost, and what \
         it left owing: {table}"
    );
}

/// The real run: three concurrency levels, inside the bound, with a queue that grows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bounded_load_run_stays_inside_its_bound_and_records_a_curve() {
    let workload = Workload::start(ENDPOINTS, &[]);
    let pool = workload.pool().await;
    let mut curve = Curve::new();
    let mut offered = 0;

    for concurrency in LEVELS {
        let keys = loadtest::keys_from(offered, PER_LEVEL);
        offered += PER_LEVEL;
        let offer = workload.offer(&keys, concurrency);
        assert_eq!(
            offer.accepted, PER_LEVEL,
            "every event of level {concurrency} is accepted: {} replayed, {} refused",
            offer.replayed, offer.refused
        );

        // One turn of the worker, with as many seats as this level had producers: the level's
        // backlog is what the level itself left behind, so the depth is a measurement of the load
        // rather than of how long the test waited.
        let counted = Arc::new(Counted::new());
        worker(
            &pool,
            u32::try_from(concurrency).expect("a small level"),
            &counted,
        )
        .turn()
        .await
        .expect("the worker runs");

        let measured = Percentiles::of(&offer.samples);
        let backlog = workload.outstanding();
        curve.record(Point {
            concurrency,
            p99: measured.p99,
            backlog,
        });
        eprintln!(
            "level {concurrency}: {} · backlog {backlog}",
            measured.one_line()
        );
    }

    let points = curve.points();
    assert!(
        points.len() >= 3,
        "a curve of fewer than three levels is a measurement, not a shape: {:?}",
        points
    );
    assert!(
        points
            .windows(2)
            .all(|pair| pair[0].concurrency < pair[1].concurrency),
        "the levels are in the order they were run: {:?}",
        points
    );
    assert!(
        points.iter().all(|point| point.p99 < SLOWEST),
        "no level's p99 is past {SLOWEST:?}: a service that takes longer than that to accept an \
         event is not keeping up, whatever its backlog says\n{}",
        curve.render()
    );

    // The queue is arithmetic rather than a clock: every level offers `PER_LEVEL * ENDPOINTS` more
    // deliveries and drains one turn of its own seats, so a level can only leave more owing than
    // the one below it. This is the half of a load run that does not depend on the machine.
    assert!(
        points
            .windows(2)
            .all(|pair| pair[1].backlog > pair[0].backlog),
        "every level leaves the queue deeper than the one below it — {} deliveries per level, and \
         one turn of the level's own seats:\n{}",
        PER_LEVEL * ENDPOINTS,
        curve.render()
    );

    eprintln!("\n{}", curve.render());
}

/// Everything the load was allowed to be is work the service finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_load_the_service_accepted_is_work_it_finishes() {
    let workload = Workload::start(ENDPOINTS, &[]);
    let keys = loadtest::keys(RUN);
    let offer = workload.offer(&keys, 4);
    assert_eq!(
        offer.accepted, RUN,
        "the run is accepted before anything is measured about draining it: {} refused",
        offer.refused
    );
    let pool = workload.pool().await;
    let counted = Arc::new(Counted::new());

    let started = std::time::Instant::now();
    let report = worker(&pool, 8, &counted)
        .work(60)
        .await
        .expect("the worker runs");
    let elapsed = started.elapsed();

    assert_eq!(
        workload.outstanding(),
        0,
        "the queue is empty: nothing is left pending, claimed or retrying"
    );
    assert_eq!(
        report.claimed,
        RUN * ENDPOINTS,
        "the worker claimed every delivery the fan-out wrote, once each: {report:?}"
    );
    assert_eq!(
        workload.count("select count(*) from deliveries where state = 'sent'"),
        i64::try_from(RUN * ENDPOINTS).expect("a small number"),
        "and every one of them was taken: {RUN} events times {ENDPOINTS} subscribers"
    );
    assert_eq!(
        counted.finished(),
        RUN * ENDPOINTS,
        "the subscriber was asked exactly as many times as there were deliveries: {}",
        counted.describe()
    );
    let measured = Percentiles::of(&offer.samples);
    assert!(
        measured.p99 < SLOWEST,
        "the ingest path stays inside its bound while it is being drained: {measured}"
    );
    eprintln!(
        "ingest {} · {} deliveries drained in {elapsed:?} by 8 seats",
        measured.one_line(),
        RUN * ENDPOINTS
    );
}

/// The page: a capacity number a reader can trace back to the run that produced it.
///
/// The `links` gate already checks that every target in this file resolves, and that no target is a
/// placeholder. What that gate cannot see is whether the page is *about a measurement*: the last
/// test reads it and asks for the sections a capacity question has, the command that repeats the
/// run, and a table with the levels in it. It measures a structure rather than a behaviour, and it
/// does so because a page is not something a running service can be asked about (authoring.md §6.6)
/// — the alternative is a page nobody judges at all.
#[test]
fn the_capacity_page_names_its_method_its_numbers_and_its_slope() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/capacity.md");
    let page = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is the page this step delivers, and it cannot be read: {error}",
            path.display()
        )
    });
    for section in [
        "## Method",
        "## Measurements",
        "## The slope",
        "## Workers and throughput",
    ] {
        assert!(
            page.contains(section),
            "the capacity page has no `{section}` section: a page that skips one of these questions \
             leaves it to be invented by whoever is on call"
        );
    }
    assert!(
        page.contains("spec_load"),
        "the page names the run a reader can repeat — otherwise the numbers and the evidence are two \
         documents that never meet"
    );
    assert!(
        !page.contains("TODO"),
        "every placeholder is filled: a capacity page with a hole in it is one nobody trusts on the \
         day it matters"
    );
    let measured = page
        .lines()
        .skip_while(|line| !line.starts_with("## Measurements"))
        .take_while(|line| !line.starts_with("## ") || line.starts_with("## Measurements"))
        .filter(|line| {
            line.starts_with("| ") && !line.contains("---") && !line.contains("concurrency")
        })
        .count();
    assert!(
        measured >= 3,
        "the measurements are a table of at least three levels (this run has three), and it has \
         {measured} row(s): a curve of one point is not a curve"
    );
}
