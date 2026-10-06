//! Spec for `lakelet/4.5-batch-writes`: batching changes the calls, and nothing else.
//!
//! `kiln run` copies this file into the project as `tests/spec_batch_writes.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Twelve cases, and the shape of them is the point:
//!
//! * **a batch is the bytes N appends would have written.** The same payloads go into two stores —
//!   one record at a time, and one call — and the two files have to be identical: the format is
//!   `4.4`'s and the order is the caller's, so all that batching may change is how many times the
//!   write path asked the kernel to do something;
//! * **the calls are counted, and they do not grow with the batch.** One batch of one, two, eight
//!   and sixty-four records costs the same two calls (its open and its write) — an absolute count
//!   and never a duration, because a count is the same number on a busy machine as on an idle one
//!   (handoff M2 §1.1) and the defect this step exists for is a write path that opens the file once
//!   per record;
//! * **an acknowledgement never moves ahead of the write.** A batch is readable to a reader that
//!   never went through the store the moment the call answers, and the daemon is judged the same way
//!   one level up: by the time it says a burst of frames was accepted, every one of those records is
//!   on the file;
//! * **a batch is not a blob.** Four payloads in one call are four records of `[length][payload]
//!   [seal]`, so `4.4`'s recovery still holds for a log written in batches: every cut point of that
//!   file recovers to a prefix of it, with no error and no panic;
//! * **durability is `4.3`'s and did not move.** Under `per-record` a batch is durable when the call
//!   answers (`durable_offset == len`) and costs one device call; under `never` the bytes are written
//!   and not durable, and both are visible in the counters rather than in the prose;
//! * **the page and the counters are part of the answer.** `docs/write-amplification.md` names the
//!   counters and the metrics the course's baseline is written in, because a budget whose names live
//!   in three places is a budget that drifts.
//!
//! Nothing here sleeps to synchronise: the daemon is waited for through its own log lines and its own
//! port, and every wait has a deadline.

mod support;

use std::path::PathBuf;

use support::StoreDir;
use support::service::Service;

use lakelet::engine::Spool;
use lakelet::engine::flush::{Batcher, Stats};
use lakelet::engine::spool::FILE;
use lakelet::engine::wal::Policy;
use lakelet::engine::wal::segment;
use lakelet::protocol::batch::{self, Record};
use lakelet::protocol::frame::Frame;

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/write-amplification.md";

/// The length of every payload these cases write, in bytes.
const PAYLOAD: usize = 64;

/// Bytes a record of a [`PAYLOAD`]-byte payload occupies: its length, its payload and its seal.
const RECORD: usize = 4 + PAYLOAD + 4;

/// How many frames the daemon cases send in one burst.
const BURST: usize = 8;

/// The metric names `tests/budget.rs` prints and the course's baseline is written in.
const METRICS: [&str; 11] = [
    "records_64",
    "calls_64",
    "bytes_64",
    "records_128",
    "calls_128",
    "bytes_128",
    "bytes_per_payload",
    "calls_per_record_max",
    "calls_ratio_max",
    "fsyncs_per_batch",
    "appends_calls_max",
];

/// The counters `Spool::stats` answers, in the order the page explains them.
const COUNTERS: [&str; 5] = ["records", "bytes", "calls", "syncs", "batches"];

/// `count` payloads, each one its own bytes.
///
/// Derived from the index rather than zero-filled: a batcher that reordered two payloads, dropped
/// one or wrote one of them twice cannot pass by agreeing with a pattern.
fn payloads(count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|index| {
            (0..PAYLOAD)
                .map(|offset| (index as u8).wrapping_mul(31).wrapping_add(offset as u8))
                .collect()
        })
        .collect()
}

/// Write `payloads` through a fresh batcher: the one call the step is about.
fn write_batch(spool: &mut Spool, payloads: &[Vec<u8>]) {
    let mut batcher = Batcher::new();
    for payload in payloads {
        batcher.push(payload.as_slice());
    }
    batcher
        .flush(spool)
        .unwrap_or_else(|error| panic!("a batch of {} record(s): {error}", payloads.len()));
}

/// The store's log right now, read by a reader that is not the store.
fn log(store: &StoreDir) -> Vec<u8> {
    store
        .read(FILE)
        .unwrap_or_else(|| panic!("{} holds a log", store.path().display()))
}

/// The records a third party reads out of the store's log right now.
fn records_at(store: &StoreDir) -> Vec<Vec<u8>> {
    segment::scan(&log(store))
        .unwrap_or_else(|error| panic!("the log the write path wrote is a log: {error}"))
        .records()
        .to_vec()
}

/// A store of this case's own, with a policy that never waits for the device.
fn store(label: &str) -> (StoreDir, Spool) {
    let directory = StoreDir::new(label);
    let spool = Spool::with_policy(directory.path(), Policy::Never)
        .unwrap_or_else(|error| panic!("{} is a store: {error}", directory.path().display()));
    (directory, spool)
}

/// One `key=<number>` field of a daemon log line.
fn field(line: &str, key: &str) -> u64 {
    let (_, rest) = line.split_once(&format!("{key}=")).unwrap_or_else(|| {
        panic!(
            "the line `{line}` carries no `{key}=` — the daemon reports the write path's counters \
             as fields, which is what a collector and this spec both read"
        )
    });
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("`{key}=` in `{line}` is not a number: {error}"))
}

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

/// One frame per record, concatenated: the bytes a client sends in one write.
fn wire(count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for index in 0..count {
        let record = Record {
            series: format!("cpu,host=h{index}"),
            ts_millis: 1_700_000_000_000 + index as i64,
            value: index as f64,
        };
        bytes.extend_from_slice(&Frame::new(batch::encode(std::slice::from_ref(&record))).encode());
    }
    bytes
}

#[test]
fn a_batch_writes_the_bytes_n_appends_would_have_written() {
    let payloads = payloads(3);

    // One record at a time: the shape `lakelet/4.1-ingest-protocol` left behind.
    let (separate, mut spool) = store("batch-separate");
    for payload in &payloads {
        spool
            .append(payload)
            .unwrap_or_else(|error| panic!("one append: {error}"));
    }

    // And the same payloads as one call.
    let (together, mut spool) = store("batch-together");
    write_batch(&mut spool, &payloads);

    assert_eq!(
        log(&together),
        log(&separate),
        "a batch is the bytes N appends write, down to the last one: the format is \
         `wal::segment`'s and this step may not change a byte of it, so the only thing that moved \
         is how many calls it took to put them there"
    );
    assert_eq!(
        spool.records().expect("the records the batch wrote"),
        payloads,
        "and the store reads back what was written, in the order it was given"
    );
}

#[test]
fn a_batch_keeps_the_order_it_was_given() {
    let payloads = payloads(8);
    let (directory, mut spool) = store("batch-order");
    write_batch(&mut spool, &payloads);

    assert_eq!(
        spool.records().expect("the records the batch wrote"),
        payloads,
        "the records come back in the order the payloads went in: a batcher that sorted, grouped \
         or deduplicated them would still write plausible bytes and hand a reader a different \
         series"
    );
    assert_eq!(
        records_at(&directory),
        payloads,
        "and a reader that is not this store sees the same order"
    );
}

#[test]
fn the_calls_do_not_grow_with_the_batch_size() {
    for count in [1usize, 2, 8, 64] {
        let (directory, mut spool) = store(&format!("batch-size-{count}"));
        let payloads = payloads(count);
        let before = spool.stats();
        write_batch(&mut spool, &payloads);
        let after = spool.stats();

        assert_eq!(
            after.batches() - before.batches(),
            1,
            "one flush with {count} payload(s) is one batch"
        );
        assert_eq!(
            after.records() - before.records(),
            count as u64,
            "and it wrote {count} record(s): a batch is not a quota, it is exactly what was given"
        );
        assert_eq!(
            after.calls() - before.calls(),
            2,
            "one batch of {count} record(s) costs its open and its write — an absolute count and \
             never a duration: a write path that opens the file once per record costs two calls per \
             record, which is the defect this case is written against"
        );
        assert_eq!(
            after.bytes() - before.bytes(),
            (segment::HEADER + count * RECORD) as u64,
            "and the bytes are the same ones N appends write: the log's header plus one record per \
             payload"
        );
        assert_eq!(
            log(&directory).len() as u64,
            after.bytes(),
            "what the counters say was written is what is on the file"
        );
    }
}

#[test]
fn the_batcher_holds_what_arrived_until_it_is_flushed() {
    let (directory, mut spool) = store("batch-holds");
    let payloads = payloads(4);
    let mut batcher = Batcher::new();
    assert!(batcher.is_empty(), "a new batcher holds nothing");

    for payload in &payloads {
        batcher.push(payload.as_slice());
    }
    assert_eq!(
        batcher.len(),
        payloads.len(),
        "the batcher holds what arrived"
    );
    assert_eq!(
        spool.stats().calls(),
        0,
        "and holding is not writing: no call was made, so none is counted — this is the half of \
         the promise that lets a turn's frames share one call"
    );
    assert_eq!(spool.len(), 0, "and nothing moved the store's own length");
    assert!(
        log(&directory).is_empty(),
        "nothing is on the file before the flush"
    );

    batcher
        .flush(&mut spool)
        .unwrap_or_else(|error| panic!("the flush of four held payloads: {error}"));
    assert!(batcher.is_empty(), "a flush leaves nothing behind");
    assert_eq!(
        spool.records().expect("the records the batch wrote"),
        payloads,
        "and every payload that was held is a record afterwards"
    );
}

#[test]
fn an_acknowledged_batch_is_readable_the_moment_the_call_answers() {
    let (directory, mut spool) = store("batch-readable");
    let payloads = payloads(5);
    write_batch(&mut spool, &payloads);

    // A reader that never went through the store: the promise is about the bytes on the file and
    // not about what the writer remembers, so it is checked with the wal's own reader rather than
    // with `Spool::records`.
    let bytes = log(&directory);
    let scanned = segment::scan(&bytes)
        .unwrap_or_else(|error| panic!("the log the write path wrote is a log: {error}"));
    assert_eq!(
        scanned.stop(),
        segment::Stop::End,
        "the file ends on a record boundary: what was written is whole"
    );
    assert_eq!(
        scanned.records(),
        payloads.as_slice(),
        "every record of the batch is readable the moment the call answered — a batcher that \
         answered first and wrote afterwards would answer with fewer bytes here, and `4.1`'s \
         promise is the reason that is the defect and not an optimisation"
    );
}

#[test]
fn an_empty_batch_writes_nothing_and_costs_nothing() {
    let (directory, mut spool) = store("batch-empty");
    let mut batcher = Batcher::new();
    batcher
        .flush(&mut spool)
        .unwrap_or_else(|error| panic!("a flush with nothing to write: {error}"));

    assert_eq!(
        spool.stats(),
        Stats::default(),
        "a flush with nothing to write is not a write: no open, no call counted, no batch counted"
    );
    assert_eq!(spool.len(), 0);
    assert!(log(&directory).is_empty());

    // And the store is still a store: the first record that exists is the one that declares the
    // log, whether or not an empty batch was flushed at it before.
    write_batch(&mut spool, &payloads(1));
    assert_eq!(
        spool.records().expect("the one record"),
        payloads(1),
        "the first real batch still declares the log and writes its record"
    );
    assert_eq!(
        spool.stats().calls(),
        2,
        "and it costs its open and its write"
    );
}

#[test]
fn the_device_calls_are_the_policys_and_a_batch_pays_for_one() {
    // `never`: the bytes are written and not durable, and no device call is made.
    let (never, mut spool) = store("batch-never");
    write_batch(&mut spool, &payloads(3));
    assert_eq!(
        spool.stats().syncs(),
        0,
        "a policy that never waits asks for no device call, whatever the batch size is"
    );
    assert!(
        spool.durable_offset() < spool.len(),
        "and the boundary the write path promises is behind what it wrote: written is readable, \
         durable is something a policy asks for"
    );
    assert_eq!(
        spool.records().expect("the records").len(),
        3,
        "written records are readable even when they are not durable"
    );
    assert!(!log(&never).is_empty());

    // `per-record`: one device call per batch, and the batch is durable when the call answers.
    let directory = StoreDir::new("batch-durable");
    let mut spool = Spool::with_policy(directory.path(), Policy::PerRecord)
        .unwrap_or_else(|error| panic!("the store: {error}"));
    write_batch(&mut spool, &payloads(3));
    assert_eq!(
        spool.stats().syncs(),
        1,
        "one batch of three records costs one device call under `per-record`, because the policy \
         is asked once per call and not once per record"
    );
    assert_eq!(
        spool.durable_offset(),
        spool.len(),
        "and the batch is durable when the call answers: the boundary `4.3` promises did not move, \
         the number of times it is reached did"
    );

    write_batch(&mut spool, &payloads(3));
    assert_eq!(
        spool.stats().syncs(),
        2,
        "two batches, two device calls — and not six, one per record"
    );
    assert_eq!(
        spool.records().expect("the records").len(),
        6,
        "both batches are readable"
    );
}

#[test]
fn the_counters_start_where_this_process_started() {
    let directory = StoreDir::new("batch-reopen");
    let payloads = payloads(2);
    {
        let mut spool = Spool::with_policy(directory.path(), Policy::Never)
            .unwrap_or_else(|error| panic!("the first process's store: {error}"));
        write_batch(&mut spool, &payloads);
    }

    let mut spool = Spool::with_policy(directory.path(), Policy::Never)
        .unwrap_or_else(|error| panic!("the second process's store: {error}"));
    assert_eq!(
        spool.stats(),
        Stats::default(),
        "the counters belong to this write path: the bytes a previous process left are readable \
         and are not this process's work, or two runs of one workload would report two numbers"
    );
    assert!(
        !spool.is_empty(),
        "the log's length is recovered from the file, and the counters are not"
    );
    assert_eq!(
        spool.records().expect("the recovered records"),
        payloads,
        "and what the last process wrote is still there"
    );

    write_batch(&mut spool, &payloads);
    assert_eq!(
        spool.stats().records(),
        2,
        "only the records this process wrote are counted"
    );
    assert_eq!(
        spool.records().expect("the records").len(),
        4,
        "while the log holds both processes' work"
    );
}

#[test]
fn a_batch_is_n_records_and_every_cut_inside_it_recovers_to_a_prefix() {
    let directory = StoreDir::new("batch-cut");
    let payloads = payloads(4);
    let written = {
        let mut spool = Spool::with_policy(directory.path(), Policy::PerRecord)
            .unwrap_or_else(|error| panic!("the store: {error}"));
        write_batch(&mut spool, &payloads);
        log(&directory)
    };

    let scanned =
        segment::scan(&written).unwrap_or_else(|error| panic!("the log the batch wrote: {error}"));
    assert_eq!(
        scanned.records().len(),
        payloads.len(),
        "four payloads in one call are four records and not one blob: a reader written for one \
         record per append reads a batch as what it is"
    );
    assert_eq!(
        scanned.verified(),
        written.len(),
        "and every byte of the file is part of one of them"
    );

    // Every cut point of that batch-written file, from nothing to the whole log: what a crash
    // leaves has to be a prefix of the batch, with no error and no panic. This is `4.4`'s sweep
    // held against a log this step wrote, and the case is cheap for the same reason that one is.
    let mut cut_inside_a_record = 0;
    for cut in 0..=written.len() {
        std::fs::write(
            directory.path().join(FILE),
            written.get(..cut).expect("the cut is inside the log"),
        )
        .unwrap_or_else(|error| panic!("a log cut at {cut} byte(s): {error}"));

        let opened = Spool::open(directory.path()).unwrap_or_else(|error| {
            panic!("a store cut at {cut} byte(s) is still a store: {error}")
        });
        let recovered = opened.recovered();
        let kept = recovered.records().len();
        assert_eq!(
            recovered.records(),
            &payloads[..kept],
            "cut at {cut} byte(s), recovery keeps a prefix of the batch's records, each one intact"
        );
        assert!(
            recovered.bytes() <= cut as u64,
            "recovery proves no more bytes than the file holds (cut at {cut})"
        );
        assert_eq!(
            recovered.discarded(),
            cut as u64 - recovered.bytes(),
            "and the bytes it threw away are exactly the ones no record proves (cut at {cut})"
        );
        assert_eq!(
            std::fs::metadata(directory.path().join(FILE))
                .map(|file| file.len())
                .unwrap_or_else(|error| panic!("the store's file: {error}")),
            recovered.bytes(),
            "opening the store cut the tail no append finished (cut at {cut}), so the next append \
             continues where the log ended"
        );
        if kept < payloads.len() {
            cut_inside_a_record += 1;
        }
    }
    assert!(
        cut_inside_a_record > 0,
        "some of those cuts landed inside a record rather than between two of them, so the sweep \
         tested the tail a half-written batch leaves and not only its record boundaries"
    );
}

#[test]
fn a_burst_of_frames_is_answered_only_after_its_records_are_written() {
    let service = Service::start("batch-burst", &[]);
    // One write, one connection: this is what one turn of the daemon's loop sees — the frames that
    // arrived together.
    service.send(&wire(BURST));
    let closed = service.wait_for_line(&format!("accepted={BURST}"));
    assert!(
        closed.contains("connection closed"),
        "the daemon said what it accepted when the client went away: {closed}"
    );

    // That line is the client's answer, and this is the moment the promise is about: a write path
    // that acknowledged a batch and wrote it afterwards would be visible here and nowhere else,
    // which is why the case is at the daemon rather than at the store.
    let bytes = std::fs::read(service.store().path().join(FILE))
        .unwrap_or_else(|error| panic!("the store the daemon writes: {error}"));
    let scanned =
        segment::scan(&bytes).unwrap_or_else(|error| panic!("the log the daemon wrote: {error}"));
    assert_eq!(
        scanned.records().len(),
        BURST,
        "every frame of the burst is a readable record by the time the daemon says the connection \
         is closed: the batch was written before it was answered, not after"
    );
}

#[test]
fn the_daemon_writes_a_turns_frames_in_fewer_calls_than_frames_and_says_so() {
    let mut service = Service::start("batch-cost", &[]);
    service.send(&wire(BURST));
    service.wait_for_line(&format!("accepted={BURST}"));
    service.stop();
    assert_eq!(
        service.wait_for_exit(),
        0,
        "a requested stop is not a failure"
    );

    let cost = service.wait_for_line("store write cost");
    let records = field(&cost, "records");
    let calls = field(&cost, "calls");
    assert_eq!(
        records, BURST as u64,
        "the daemon reports how many records its write path wrote: {cost}"
    );
    assert!(
        field(&cost, "batches") >= 1,
        "one turn of frames is at least one batch: {cost}"
    );
    assert!(
        calls < records,
        "a burst of {BURST} frames written as one turn's batch costs fewer calls than it wrote \
         records — that is the whole of this step, and a loop that opened the file once per frame \
         reports twice the records here: {cost}"
    );

    let flushed = service.wait_for_line("flushed the store");
    assert_eq!(
        field(&flushed, "written"),
        field(&flushed, "durable"),
        "and the stop path `4.4` made true is untouched: a clean stop still closes the window the \
         policy left open: {flushed}"
    );
}

#[test]
fn the_page_states_the_counters_and_the_budget_the_gate_compares() {
    let page = page();
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a page that still has a blank in it is the page \
             nobody wrote"
        );
    }
    for metric in METRICS {
        assert!(
            page.contains(metric),
            "{PAGE} does not state `{metric}`, which the harness prints and the course's baseline \
             is written in — a metric a page does not explain is a number a reviewer has to guess at"
        );
    }
    for counter in COUNTERS {
        assert!(
            low.contains(counter),
            "{PAGE} does not define the `{counter}` counter: the counters are the learner's \
             contract and the page is where they are written down"
        );
    }
    assert!(
        page.contains("cargo test -p lakelet --test budget"),
        "{PAGE} does not name the command that prints the numbers"
    );
    assert!(
        page.contains("bench-budget"),
        "{PAGE} does not name the gate that compares them with the baseline"
    );
    assert!(
        low.contains("upper bound"),
        "{PAGE} does not say that a `_max` metric is an upper bound rather than a target, which \
         is the difference between a gate that rewards getting cheaper and one that punishes it"
    );
    assert!(
        page.contains("64") && page.contains("128"),
        "{PAGE} does not state the two batch sizes the counts are taken at, and a cost that \
         follows the batch size is invisible at one of them"
    );
    assert!(
        page.contains("store write cost"),
        "{PAGE} does not name the line the daemon prints, which is where an operator reads what \
         the write path cost"
    );
}
