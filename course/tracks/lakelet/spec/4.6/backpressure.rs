//! Spec for `lakelet/4.6-ingest-backpressure`: overload is a decision, and every one of them is
//! counted.
//!
//! `kiln run` copies this file into the project as `tests/spec_backpressure.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Twelve cases, and the shape of them is the point:
//!
//! * **the conservation law is the spine.** `offered == accepted + dropped` and
//!   `accepted == written + queued`, checked per policy. The second one is what "a frame the queue
//!   dropped is never acknowledged" means as arithmetic: a frame an eviction took back leaves
//!   `accepted` with it, so `written + depth` still describes what the queue answers for;
//! * **the bound is a bound.** A producer that never stops cannot make the queue hold more than its
//!   capacity, for any policy that answers at all, and the queue neither grows without limit nor
//!   panics while he tries;
//! * **each policy has one rule, and the rule is visible.** `Block` waits and loses nothing;
//!   `DropNewest` refuses the arriving frame; `DropOldest` keeps the arrival and drops the frame that
//!   waited longest; `Sample` keeps one arrival in `SAMPLE_KEEP`. Every one of them is checked by
//!   *which payload* survived, not only by how many;
//! * **the tally is the caller's half of the promise.** A frame is charged to the caller that offered
//!   it, at the moment its fate is decided — the write, or the drop — so one caller can say what its
//!   own frames came to, including a frame that was queued for a while and evicted later;
//! * **nothing is acknowledged before the write.** `take` is not a write: the counters move in
//!   `wrote`, and `settled` does not resolve until the frames the queue handed over are on the file;
//! * **the capacity is a deployment's number.** The four layers `lakelet/4.1-ingest-protocol`
//!   established carry it and the policy beside it, the bottom layer is the queue's own stated
//!   default, and a deployment that asks for a queue of zero frames is refused by name;
//! * **the daemon is judged as a process.** A burst of frames is on the file before the connection
//!   says what it accepted, and an overload is *reported*: the daemon keeps serving, says what its
//!   queue dropped, and stops cleanly with numbers that add up.
//!
//! Nothing here sleeps to synchronise: the queue's own waits are polled by hand, and the daemon is
//! waited for through its own log lines and its own port.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use support::StoreDir;
use support::Var;
use support::service::Service;

use lakelet::engine::Spool;
use lakelet::engine::flush::Batcher;
use lakelet::engine::spool::FILE;
use lakelet::engine::wal::Policy as WalPolicy;
use lakelet::engine::wal::segment;
use lakelet::protocol::batch::{self, Record};
use lakelet::protocol::frame::Frame;
use lakelet::service::backpressure::{
    DEFAULT_CAPACITY, DEFAULT_POLICY, Decision, Payload, Policy, Queue, SAMPLE_KEEP, Tally,
};
use lakelet::service::config::{self, Config, Flags};

/// How long one payload is. Not a round number, so a length/payload mix-up cannot pass by accident.
const PAYLOAD: usize = 41;

/// How many frames the daemon cases send in one write.
const BURST: usize = 8;

/// One payload, named by its first byte so a spec can say which one survived.
fn payload(marker: u8) -> Payload {
    (0..PAYLOAD)
        .map(|offset| marker.wrapping_mul(31).wrapping_add(offset as u8))
        .collect()
}

/// `count` payloads with distinct bytes.
fn payloads(count: usize) -> Vec<Payload> {
    (0..count).map(|index| payload(index as u8)).collect()
}

/// Offer one frame on its own, and answer the one decision it got.
async fn offer_one(queue: &Queue, tally: &Arc<Tally>, payload: Payload) -> Decision {
    let mut decisions = queue.offer(vec![payload], Arc::clone(tally)).await;
    assert_eq!(
        decisions.len(),
        1,
        "one frame offered is one decision answered"
    );
    decisions
        .pop()
        .expect("the vector held the decision it just answered")
}

/// Poll a future once and answer what it did: `Pending` means it is still waiting.
fn poll_once<F: Future>(future: &mut Pin<&mut F>) -> Poll<F::Output> {
    future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
}

/// The records a third party reads out of the store's log.
fn records(store: &StoreDir) -> Vec<Vec<u8>> {
    let bytes = store
        .read(FILE)
        .unwrap_or_else(|| panic!("{} holds a log", store.path().display()));
    segment::scan(&bytes)
        .unwrap_or_else(|error| panic!("the log the writer wrote is a log: {error}"))
        .records()
        .to_vec()
}

/// Write `payloads` through the real write path, the way the daemon's writer task does it.
fn write_into(spool: &mut Spool, payloads: &[Payload]) {
    let mut batcher = Batcher::new();
    for payload in payloads {
        batcher.push(payload.as_slice());
    }
    batcher
        .flush(spool)
        .unwrap_or_else(|error| panic!("a batch of {} record(s): {error}", payloads.len()));
}

/// A store of this case's own, with a policy that never waits for the device.
fn store(label: &str) -> (StoreDir, Spool) {
    let directory = StoreDir::new(label);
    let spool = Spool::with_policy(directory.path(), WalPolicy::Never)
        .unwrap_or_else(|error| panic!("{} is a store: {error}", directory.path().display()));
    (directory, spool)
}

/// One record, named by its index so two frames cannot be confused.
fn record(index: usize) -> Record {
    Record {
        series: format!("sim,host=h{index}"),
        ts_millis: 1_700_000_000_000 + index as i64,
        value: index as f64,
    }
}

/// One frame per record, concatenated: the bytes a client sends in one write.
fn wire(count: usize) -> Vec<u8> {
    (0..count)
        .flat_map(|index| Frame::new(batch::encode(&[record(index)])).encode())
        .collect()
}

/// The payloads [`wire`] sends, in order: what the log holds once they are written.
fn sent(count: usize) -> Vec<Payload> {
    (0..count)
        .map(|index| batch::encode(&[record(index)]))
        .collect()
}

/// One `key=<number>` field of a daemon log line.
fn field(line: &str, key: &str) -> u64 {
    let (_, rest) = line.split_once(&format!("{key}=")).unwrap_or_else(|| {
        panic!(
            "the line `{line}` carries no `{key}=` — the daemon reports what its queue did as \
             fields, which is what a collector and this spec both read"
        )
    });
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("`{key}=` in `{line}` is not a number: {error}"))
}

/// Run `lakeletd` to completion with `arguments` and `environment`, and answer its stdout.
fn run_daemon(arguments: &[&str], environment: &[Var]) -> String {
    let output = daemon(arguments, environment);
    assert!(
        output.status.success(),
        "`lakeletd {}` exited {}: {}",
        arguments.join(" "),
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Run `lakeletd` expecting it to *refuse*, and answer what it said about why.
fn run_refused(arguments: &[&str], environment: &[Var]) -> String {
    let output = daemon(arguments, environment);
    assert!(
        !output.status.success(),
        "`lakeletd {}` was accepted, and a configuration the daemon cannot use must be a refusal \
         rather than a default: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stdout)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Start `lakeletd` with `arguments` and `environment` and wait for it to finish.
fn daemon(arguments: &[&str], environment: &[Var]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletd"));
    command.args(arguments).env_clear();
    for var in environment {
        command.env(&var.name, &var.value);
    }
    command
        .output()
        .expect("the lakeletd binary is built by cargo test")
}

#[tokio::test]
async fn a_frame_reaches_the_log_once_and_the_counters_agree() {
    let (directory, mut spool) = store("bp-once");
    let queue = Queue::new(4, Policy::Block);
    let tally = Arc::new(Tally::new());

    let frames = payloads(3);
    queue.offer(frames.clone(), Arc::clone(&tally)).await;

    // A checkpoint: nothing is being written, so what the queue took in is what it still owes.
    let stats = queue.stats();
    assert_eq!(stats.offered(), 3, "three frames were offered");
    assert_eq!(stats.accepted(), 3, "`block` refuses nothing");
    assert_eq!(stats.dropped(), 0, "and drops nothing");
    assert_eq!(
        stats.written(),
        0,
        "nothing is written until the writer writes it"
    );
    assert_eq!(
        stats.accepted(),
        stats.written() + queue.depth() as u64,
        "`accepted == written + queued`: the queue owes a write for exactly what it holds"
    );

    let batch = queue
        .take()
        .await
        .expect("the queue holds the three frames");
    write_into(&mut spool, &batch);
    queue.wrote(batch.len());

    assert_eq!(
        batch, frames,
        "the writer got the frames in the order they were offered"
    );
    assert_eq!(
        records(&directory),
        frames,
        "three frames in, three records out, byte for byte"
    );
    let stats = queue.stats();
    assert_eq!(stats.written(), 3);
    assert_eq!(queue.depth(), 0);
    assert_eq!(stats.accepted(), stats.written());
    assert_eq!(tally.written(), 3, "and the caller is told what it stored");
    assert_eq!(tally.dropped(), 0, "nothing was dropped, so nothing to say");
    assert_eq!(
        tally.bytes(),
        frames.iter().map(Vec::len).sum::<usize>() as u64,
        "the payload bytes behind those three frames"
    );
}

#[tokio::test]
async fn a_producer_that_never_stops_cannot_make_the_queue_grow() {
    // One frame at a time, a thousand of them, and no writer at all: the queue has to hold its
    // bound, keep its books and stay alive. `block` is not in this list because it *waits* for room
    // rather than answering — the case below is where it is judged.
    for policy in [Policy::DropNewest, Policy::DropOldest, Policy::Sample] {
        let queue = Queue::new(3, policy);
        let tally = Arc::new(Tally::new());
        for round in 0..1_000u64 {
            offer_one(&queue, &tally, payload((round % 251) as u8)).await;
            assert!(
                queue.depth() <= 3,
                "`{policy:?}` let the queue hold {} frame(s), and its capacity is 3",
                queue.depth()
            );
        }
        let stats = queue.stats();
        assert_eq!(
            stats.offered(),
            1_000,
            "every frame was offered and counted"
        );
        assert_eq!(
            stats.offered(),
            stats.accepted() + stats.dropped(),
            "`{policy:?}`: every frame is either the queue's or dropped, and the two add up"
        );
        assert_eq!(
            queue.depth(),
            3,
            "`{policy:?}` filled the queue and no more than filled it"
        );
        assert_eq!(
            stats.accepted(),
            3,
            "`{policy:?}`: the bound is what it can hold, and it holds exactly that"
        );
        assert_eq!(tally.dropped(), 997);
        assert_eq!(
            tally.written(),
            0,
            "`{policy:?}`: nothing was written, because no writer ever ran"
        );
    }
}

#[tokio::test]
async fn block_waits_for_room_instead_of_dropping() {
    let queue = Queue::new(1, Policy::Block);
    let tally = Arc::new(Tally::new());
    let first = payload(1);
    let second = payload(2);
    assert_eq!(
        offer_one(&queue, &tally, first.clone()).await,
        Decision::Accepted,
        "the first frame fits"
    );

    let mut waiting = std::pin::pin!(queue.offer(vec![second.clone()], Arc::clone(&tally)));
    assert!(
        poll_once(&mut waiting).is_pending(),
        "a `block` queue with no room has to make the producer wait for room instead of refusing \
         the frame"
    );
    let stats = queue.stats();
    assert_eq!(stats.dropped(), 0, "waiting is not dropping");
    assert_eq!(
        queue.depth(),
        1,
        "and the frame that is waiting is not in the queue yet"
    );
    assert_eq!(
        stats.offered(),
        stats.accepted() + stats.dropped() + 1,
        "the frame a producer is still waiting to hand over is offered and not decided: it is in \
         neither column until the queue answers"
    );

    // The room the writer frees is the whole event the producer was waiting for.
    assert_eq!(
        queue.take().await.expect("the queue holds the first frame"),
        vec![first],
        "the writer takes what was queued"
    );
    assert_eq!(
        poll_once(&mut waiting),
        Poll::Ready(vec![Decision::Accepted]),
        "the frame the writer made room for is taken in, not refused"
    );
    queue.wrote(1);

    let stats = queue.stats();
    assert_eq!(stats.offered(), 2);
    assert_eq!(stats.accepted(), 2);
    assert_eq!(
        stats.dropped(),
        0,
        "`block` loses nothing, which is the whole of what it promises"
    );
    assert_eq!(stats.written(), 1);
    assert_eq!(
        stats.accepted(),
        stats.written() + queue.depth() as u64,
        "one frame written and one still queued"
    );
}

#[tokio::test]
async fn drop_newest_refuses_the_arriving_frame_and_counts_it() {
    let queue = Queue::new(2, Policy::DropNewest);
    let tally = Arc::new(Tally::new());
    let frames = payloads(5);
    let decisions = queue.offer(frames.clone(), Arc::clone(&tally)).await;
    assert_eq!(
        decisions,
        vec![
            Decision::Accepted,
            Decision::Accepted,
            Decision::Refused(frames[2].clone()),
            Decision::Refused(frames[3].clone()),
            Decision::Refused(frames[4].clone()),
        ],
        "the frame that is refused is the one that arrived, and it comes back so the caller can say \
         which one it lost"
    );
    let stats = queue.stats();
    assert_eq!(stats.offered(), 5);
    assert_eq!(stats.accepted(), 2);
    assert_eq!(stats.dropped(), 3);
    assert_eq!(queue.depth(), 2, "the two that fitted are still waiting");
    assert_eq!(
        queue.take().await.expect("the queue holds two frames"),
        frames[..2].to_vec(),
        "and they are the first two that arrived, not the last two"
    );
    queue.wrote(2);
    assert_eq!(tally.written(), 2);
    assert_eq!(tally.dropped(), 3, "the caller is told about all three");
}

#[tokio::test]
async fn drop_oldest_evicts_the_frame_that_waited_longest() {
    let queue = Queue::new(2, Policy::DropOldest);
    let tally = Arc::new(Tally::new());
    let frames = payloads(4);
    let decisions = queue.offer(frames.clone(), Arc::clone(&tally)).await;
    assert_eq!(
        decisions,
        vec![
            Decision::Accepted,
            Decision::Accepted,
            Decision::Evicted(frames[0].clone()),
            Decision::Evicted(frames[1].clone()),
        ],
        "`drop-oldest` takes every arrival in and pays with the frame that has been waiting longest"
    );
    assert_eq!(
        queue.depth(),
        2,
        "and the bound does not move while it does"
    );
    let stats = queue.stats();
    assert_eq!(stats.offered(), 4);
    assert_eq!(
        stats.accepted(),
        2,
        "two frames are the queue's: the last two that arrived"
    );
    assert_eq!(
        stats.dropped(),
        2,
        "and the two it took back are counted as dropped, not as accepted"
    );
    assert_eq!(
        stats.accepted(),
        stats.written() + queue.depth() as u64,
        "a frame an eviction took back leaves `accepted` with it"
    );
    assert_eq!(
        queue.take().await.expect("the queue holds two frames"),
        vec![frames[2].clone(), frames[3].clone()],
        "what is left is the newest two: the loss landed on what arrived earlier"
    );
    queue.wrote(2);
    assert_eq!(tally.dropped(), 2);
    assert_eq!(tally.written(), 2);
    assert_eq!(
        tally.written() + tally.dropped(),
        4,
        "the caller can account for every frame it offered"
    );
}

#[tokio::test]
async fn sample_keeps_one_arrival_in_the_stated_fraction() {
    // One slot, three rounds of the fraction: the kept arrivals are the fourth, the eighth and the
    // twelfth, and the two that find the queue full make room the way `drop-oldest` does.
    let queue = Queue::new(1, Policy::Sample);
    let tally = Arc::new(Tally::new());
    let frames = payloads(SAMPLE_KEEP as usize * 3);
    let decisions = queue.offer(frames.clone(), Arc::clone(&tally)).await;

    let kept: Vec<usize> = decisions
        .iter()
        .enumerate()
        .filter(|(_, decision)| !matches!(decision, Decision::Refused(_)))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        kept,
        vec![3, 7, 11],
        "one arrival in {SAMPLE_KEEP} is taken in: the fourth, the eighth and the twelfth"
    );
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| matches!(decision, Decision::Evicted(_)))
            .count(),
        2,
        "the two kept arrivals that found the queue full took the oldest queued frame back"
    );
    let stats = queue.stats();
    assert_eq!(stats.offered(), 12);
    assert_eq!(
        stats.accepted(),
        1,
        "one frame is the queue's at the end: the last arrival it kept"
    );
    assert_eq!(
        stats.dropped(),
        11,
        "nine arrivals were refused and two were taken back"
    );
    assert_eq!(stats.offered(), stats.accepted() + stats.dropped());
    assert_eq!(queue.depth(), 1);
    assert_eq!(
        queue.take().await.expect("the queue holds one frame"),
        vec![frames[11].clone()]
    );
    queue.wrote(1);
    assert_eq!(tally.written(), 1);
    assert_eq!(tally.dropped(), 11);
}

#[tokio::test]
async fn an_eviction_is_charged_to_the_caller_that_offered_the_frame() {
    // Two producers, one queue. The frame an eviction takes back is not always the frame of the
    // caller that is offering, and the tally is what makes that visible instead of a global number
    // nobody can act on.
    let queue = Queue::new(2, Policy::DropOldest);
    let early = Arc::new(Tally::new());
    let late = Arc::new(Tally::new());
    let first = payloads(2);
    let second = payloads(3);

    queue.offer(first.clone(), Arc::clone(&early)).await;
    let decisions = queue.offer(second.clone(), Arc::clone(&late)).await;
    assert_eq!(
        decisions,
        vec![
            Decision::Evicted(first[0].clone()),
            Decision::Evicted(first[1].clone()),
            Decision::Evicted(second[0].clone()),
        ],
        "the frame that pays is the one that has been waiting longest, whoever offered it"
    );
    assert_eq!(
        early.dropped(),
        2,
        "the caller whose frames were taken back is told, and told exactly how many"
    );
    assert_eq!(early.written(), 0, "and it has nothing on the file");
    assert_eq!(
        late.dropped(),
        1,
        "the caller that is offering loses one of its own to its own later arrival"
    );

    let batch = queue
        .take()
        .await
        .expect("the queue holds the last two frames");
    queue.wrote(batch.len());
    assert_eq!(
        batch,
        vec![second[1].clone(), second[2].clone()],
        "what is left is what the two producers most recently offered"
    );
    assert_eq!(late.written(), 2);
    assert_eq!(early.written() + early.dropped(), 2);
    assert_eq!(late.written() + late.dropped(), 3);
    let stats = queue.stats();
    assert_eq!(stats.written(), 2);
    assert_eq!(stats.dropped(), 3);
    assert_eq!(stats.offered(), stats.accepted() + stats.dropped());
}

#[tokio::test]
async fn no_policy_puts_a_dropped_frame_on_the_file() {
    // `block` needs room or a writer, and this case gives it room; the other three are the ones that
    // have to lose something, and a capacity of two is what makes them.
    let cases = [
        (Policy::Block, 6),
        (Policy::DropNewest, 2),
        (Policy::DropOldest, 2),
        (Policy::Sample, 2),
    ];
    for (policy, capacity) in cases {
        let (directory, mut spool) = store(&format!("bp-{}", policy.as_str()));
        let queue = Queue::new(capacity, policy);
        let tally = Arc::new(Tally::new());
        let frames = payloads(6);
        queue.offer(frames.clone(), Arc::clone(&tally)).await;

        // The writer does what the daemon's writer task does it: take what is waiting, write it
        // through the real write path, and say so — until nothing is waiting.
        let mut written = Vec::new();
        while queue.depth() > 0 {
            let batch = queue.take().await.expect("the queue holds frames");
            write_into(&mut spool, &batch);
            queue.wrote(batch.len());
            written.extend(batch);
        }
        queue.settled().await;

        assert_eq!(
            records(&directory),
            written,
            "`{policy:?}`: the file holds exactly the frames the writer was handed, in order"
        );
        assert_eq!(
            tally.written() as usize,
            written.len(),
            "`{policy:?}`: the caller is told it stored as many frames as are on the file — a queue \
             that acknowledged one it had dropped would report more here"
        );
        assert_eq!(
            tally.written() + tally.dropped(),
            6,
            "`{policy:?}`: every frame of the turn is one or the other"
        );
        let stats = queue.stats();
        assert_eq!(stats.offered(), 6);
        assert_eq!(stats.offered(), stats.accepted() + stats.dropped());
        assert_eq!(stats.written(), written.len() as u64);
        assert_eq!(stats.accepted(), stats.written());
        assert_eq!(queue.depth(), 0, "`{policy:?}`: the writer took everything");
    }
}

#[tokio::test]
async fn a_frame_is_written_when_the_writer_says_so_and_not_when_it_is_handed_over() {
    let queue = Queue::new(2, Policy::DropNewest);
    let tally = Arc::new(Tally::new());
    let frames = payloads(2);
    queue.offer(frames.clone(), Arc::clone(&tally)).await;

    let batch = queue.take().await.expect("the queue holds the two frames");
    assert_eq!(batch, frames);
    assert_eq!(
        queue.stats().written(),
        0,
        "handing frames to the writer is not writing them"
    );
    assert_eq!(
        tally.written(),
        0,
        "and it is not an acknowledgement either"
    );
    assert_eq!(queue.depth(), 0);
    assert_ne!(
        queue.stats().accepted(),
        queue.stats().written() + queue.depth() as u64,
        "`accepted == written + queued` is a statement about a queue at rest: in the window between \
         `take` and `wrote` the frames are in the writer's hands and on nothing else, and this is \
         what that looks like from the counters"
    );

    let mut settled = std::pin::pin!(queue.settled());
    assert!(
        poll_once(&mut settled).is_pending(),
        "a queue whose frames nobody has written yet is not settled"
    );

    queue.wrote(batch.len());
    assert_eq!(queue.stats().written(), 2);
    assert_eq!(
        tally.written(),
        2,
        "the write is what moves the caller's counter"
    );
    assert!(
        poll_once(&mut settled).is_ready(),
        "and the queue settles the moment the write is reported"
    );
}

#[tokio::test]
async fn a_queue_stops_taking_frames_when_it_is_closed() {
    let queue = Queue::new(4, Policy::Block);
    let tally = Arc::new(Tally::new());
    let frames = payloads(2);
    queue.offer(frames.clone(), Arc::clone(&tally)).await;
    queue.close();

    // Closing is not abandoning: what the queue holds is still written.
    let refused = offer_one(&queue, &tally, payload(9)).await;
    assert!(
        matches!(refused, Decision::Refused(_)),
        "a closed queue refuses the arrival rather than making the producer wait for room that will \
         never come"
    );
    let stats = queue.stats();
    assert_eq!(stats.offered(), 3);
    assert_eq!(stats.dropped(), 1);
    assert_eq!(
        queue
            .take()
            .await
            .expect("what was queued before the close is still there"),
        frames,
        "the frames that were accepted before the close are what the writer writes"
    );
    queue.wrote(2);
    queue.settled().await;
    assert!(
        queue.take().await.is_none(),
        "and once it is closed and empty, the writer's loop is over"
    );
    assert_eq!(tally.written(), 2);
    assert_eq!(tally.dropped(), 1);
}

#[test]
fn the_capacity_and_the_policy_come_from_the_configuration_layers() {
    // The bottom layer: what a deployment gets when nobody says anything, and the queue's own stated
    // defaults are what `Config::default` has to carry.
    let default = Config::default();
    assert_eq!(
        default.queue_capacity, DEFAULT_CAPACITY,
        "the default capacity is the queue's own stated one"
    );
    assert_eq!(
        default.queue_policy, DEFAULT_POLICY,
        "and the default policy is the one that loses nothing"
    );
    let report = default.report();
    assert!(
        report.contains(&format!("queue.capacity = {DEFAULT_CAPACITY}")),
        "the report carries the bound, because that is what a deployment diffs:\n{report}"
    );
    assert!(
        report.contains(&format!("queue.policy = {}", DEFAULT_POLICY.as_str())),
        "and the decision:\n{report}"
    );

    // The file layer, and the environment over it.
    let store = StoreDir::new("bp-config");
    let file = store.path().join("lakelet.toml");
    std::fs::write(&file, "[queue]\ncapacity = 8\npolicy = \"drop-oldest\"\n")
        .expect("the fixture writes its own configuration file");
    let from_file = config::resolve(
        &Flags {
            config: Some(file.clone()),
            ..Flags::default()
        },
        |_| None,
    )
    .expect("the file is a configuration this daemon can use");
    assert_eq!(from_file.queue_capacity, 8);
    assert_eq!(from_file.queue_policy, Policy::DropOldest);

    let environment = |name: &str| match name {
        "LAKELET_QUEUE_CAPACITY" => Some("16".to_owned()),
        "LAKELET_QUEUE_POLICY" => Some("sample".to_owned()),
        _ => None,
    };
    let from_environment = config::resolve(
        &Flags {
            config: Some(file.clone()),
            ..Flags::default()
        },
        environment,
    )
    .expect("the environment is a layer like any other");
    assert_eq!(
        from_environment.queue_capacity, 16,
        "the environment wins over the file"
    );
    assert_eq!(from_environment.queue_policy, Policy::Sample);

    // The flag layer, through the binary: this is the layer a deployment actually runs, and the
    // report is where it reads back what it asked for.
    let flags_store = StoreDir::new("bp-config-flags");
    let reported = run_daemon(
        &[
            "--check-config",
            "--store-dir",
            &flags_store.path().display().to_string(),
            "--queue-capacity",
            "32",
            "--queue-policy",
            "drop-newest",
        ],
        &[
            Var::new("LAKELET_QUEUE_CAPACITY", "16"),
            Var::new("LAKELET_QUEUE_POLICY", "sample"),
        ],
    );
    assert!(
        reported.contains("queue.capacity = 32"),
        "a flag wins over the environment:\n{reported}"
    );
    assert!(
        reported.contains("queue.policy = drop-newest"),
        "and so does the policy beside it:\n{reported}"
    );

    // Two refusals, because a bound nobody can use and a policy nobody speaks are both mistakes a
    // deployment has to hear about by name.
    let zero = run_refused(&["--check-config", "--queue-capacity", "0"], &[]);
    assert!(
        zero.contains("--queue-capacity"),
        "a queue of zero frames is refused, and the refusal names the key that asked for it: {zero}"
    );
    let typo = run_refused(
        &["--check-config"],
        &[Var::new("LAKELET_QUEUE_POLICY", "drop-some")],
    );
    assert!(
        typo.contains("drop-some") && typo.contains("block"),
        "an unknown policy is refused with what it was given and what this build speaks: {typo}"
    );
}

#[test]
fn a_burst_of_frames_is_on_the_file_before_the_connection_says_what_it_accepted() {
    // The default policy is `block`, so nothing is dropped and every frame ends up written — the
    // question this case asks is *when*, and the answer is `lakelet/4.1-ingest-protocol`'s: before
    // the line the client's connection leaves behind.
    let service = Service::start("bp-burst", &[Var::new("LAKELET_QUEUE_CAPACITY", "2")]);
    service.send(&wire(BURST));
    let closed = service.wait_for_line(&format!("accepted={BURST}"));
    assert!(
        closed.contains("connection closed"),
        "the daemon said what it accepted when the client went away: {closed}"
    );
    assert_eq!(
        records(service.store()),
        sent(BURST),
        "every frame of the burst is a readable record, in the order it was sent, by the time the \
         daemon says the connection is closed: the queue is drained before the answer, not after"
    );
}

#[test]
fn an_overload_is_reported_and_the_daemon_keeps_serving() {
    // `sample` keeps one arrival in `SAMPLE_KEEP`, so a burst of eight frames loses six of them
    // whatever the writer's speed is — which is what makes this case's numbers the same on every
    // machine, and the reason the policy exists at all.
    let mut service = Service::start(
        "bp-overload",
        &[
            Var::new("LAKELET_QUEUE_CAPACITY", "2"),
            Var::new("LAKELET_QUEUE_POLICY", "sample"),
        ],
    );
    service.send(&wire(BURST));
    let dropped = service.wait_for_line("frame dropped by the queue");
    assert!(
        dropped.contains("policy=sample"),
        "the line says which decision dropped the frame: {dropped}"
    );
    assert!(
        service.is_running(),
        "an overload is not a reason to die: a daemon that cannot keep up has to shed load and say \
         so, and one that dies of it has made the overload everybody else's problem"
    );

    service.stop();
    assert_eq!(
        service.wait_for_exit(),
        0,
        "a requested stop is not a failure"
    );
    let drained = service.wait_for_line("queue drained");
    let offered = field(&drained, "offered");
    let accepted = field(&drained, "accepted");
    let shed = field(&drained, "dropped");
    let written = field(&drained, "written");
    assert_eq!(
        offered, BURST as u64,
        "every frame the client sent was offered to the queue: {drained}"
    );
    assert_eq!(
        offered,
        accepted + shed,
        "and every one of them is either the queue's or dropped: {drained}"
    );
    assert_eq!(
        written, accepted,
        "the queue drained before the line was printed, so nothing it owed a write is left: {drained}"
    );
    assert!(
        shed > 0,
        "this deployment is dropping data on purpose, and this line is how it says so: {drained}"
    );
    assert_eq!(
        shed,
        BURST as u64 - accepted,
        "the frames that were shed are the ones that were not accepted: {drained}"
    );
    assert_eq!(
        records(service.store()).len() as u64,
        written,
        "and the file holds exactly what the line calls written"
    );
}
