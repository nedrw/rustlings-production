//! `lakelet/4.17-soak` — what is still alive after a long run, and what counts it.
//!
//! Seventeen cases, one claim each, and every one of them about a **stock** rather than a flow:
//!
//! 1. [`a_report_carries_the_five_counters_and_no_clock`] — the shape of the one value a soak
//!    reads, and the names it answers;
//! 2. [`a_balanced_cycle_returns_the_handles_it_borrowed`] — one cycle opens files and closes them
//!    again, and the store's two halves report the same count;
//! 3. [`a_handle_that_is_held_is_a_handle_the_store_reports`] — what the counter is for;
//! 4. [`two_stores_do_not_share_a_counter`] — a count is a store's, not the process's;
//! 5. [`the_store_files_are_the_files_the_store_itself_put_there`] — the naming rule, and a
//!    stranger's file in the directory;
//! 6. [`a_store_that_is_opened_again_reports_what_the_one_that_closed_it_did`] — the store is the
//!    disk, so a run that reads it at the end is reading what another run wrote;
//! 7. [`an_empty_store_reports_nothing_at_all`] — the boundary: no files, no bytes, no records;
//! 8. [`a_short_soak_reaches_a_steady_state`] — the claim the step exists for;
//! 9. [`the_same_soak_with_retention_off_grows`] — its pair: a steady state is a property of the
//!    configuration and not of the code;
//! 10. [`a_question_about_the_store_moves_nothing`] — reads are not writes;
//! 11. [`a_batch_the_store_refuses_leaves_the_counters_where_they_were`] — a cycle that did not
//!     happen is a cycle the counters must not report;
//! 12. [`the_counted_files_are_the_ones_the_manifest_names`] — the store and not the disk, with the
//!     garbage `4.14` leaves behind as the case that says the difference;
//! 13. [`a_merge_changes_the_shape_the_counters_count`] — compaction is the other half of the
//!     steady state, and one change of the store's shape like any other;
//! 14. [`the_ledger_counts_what_a_service_spawned_and_not_what_it_reaped`] — the task half;
//! 15. [`the_daemons_tasks_are_counted_and_its_stop_path_reaps_them`] — the daemon's loop, charged;
//! 16. [`the_engine_does_not_reach_into_the_service`] — the boundary this project keeps;
//! 17. [`the_counters_are_counts_of_state_and_never_of_time`] — read out of the sources, because
//!     "there is no clock in this API" is a fact about the files.
//!
//! Nothing here sleeps, and nothing here asserts how long anything took: the whole point of the
//! step is that the judgement is counts (`docs/soak.md`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelet::engine::Spool;
use lakelet::engine::compaction::{self, Segments};
use lakelet::engine::index::{self, Lookup};
use lakelet::engine::query::{exec, parse};
use lakelet::engine::retention;
use lakelet::engine::soak::{Live, Open};
use lakelet::engine::wal::Policy as Durability;
use lakelet::protocol::batch::{self, Record};
use lakelet::service::backpressure::{Policy as Overflow, Queue};
use lakelet::service::ingest;
use lakelet::service::soak::Tasks;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// How many records one cycle of a case's soak writes.
const BATCH: usize = 32;

/// How wide one cycle's window of time is, in milliseconds.
const WINDOW_MILLIS: u64 = 60_000;

/// How far apart two records of one batch are.
const STEP_MILLIS: u64 = 500;

/// How many windows a case's store keeps: the depth of the steady state it is looking for.
const KEEP_WINDOWS: u64 = 4;

/// The first timestamp a case writes, on a bucket boundary of its retention policy.
const EPOCH: u64 = 1_700_000_040_000;

/// The metric every batch carries.
const METRIC: &str = "cpu";

/// How far the one counter that is not exactly flat may move between the halfway mark and the end.
///
/// The same allowance the harness makes and for the same reason (`tests/soak.rs`): two segments
/// hold the same records at two different times of day and the varints carrying those timestamps
/// are not always the same width, so a segment may come out a few bytes longer than its
/// predecessor. It is a measured slack with room over it, and far below one segment.
const BYTE_SLACK: u64 = 256;

/// The five counter names, spelled here rather than read from [`Live::NAMES`]: a case that asked
/// the implementation what to check would agree with it about anything.
const NAMES: [&str; 5] = [
    "open_tasks",
    "open_handles",
    "store_files",
    "store_bytes",
    "records",
];

#[test]
fn a_report_carries_the_five_counters_and_no_clock() {
    let dir = fresh("shape");
    let (_, store) = open(dir.path());
    let live = Live::of(dir.path(), store.manifest(), 7);

    assert_eq!(
        Live::NAMES,
        NAMES,
        "the report carries these five counters, in this order: a transcript is read beside this \
         list and not against a second copy of it (`docs/soak.md`)"
    );
    for name in NAMES {
        assert!(
            live.get(name).is_some(),
            "`{name}` is one of the five, and a report that answered `None` for it would be a \
             soak counting nothing: {live:?}"
        );
    }
    assert_eq!(live.open_tasks(), 7, "the tasks are the caller's to count");
    assert_eq!(
        live.get("cycles"),
        None,
        "how many cycles a run did is the harness's total and not a counter of what is alive: a \
         report that carried it would be reporting the run's length where its state belongs"
    );
    assert_eq!(
        live.get("elapsed"),
        None,
        "and there is no clock in this API"
    );

    // The report is a value: copied, compared, and printed. A type that borrowed a live store
    // could not be held across the flush that follows it, which is exactly what a soak does.
    let copied = live;
    let again = Live::of(dir.path(), store.manifest(), 7);
    assert_eq!(
        copied, again,
        "two questions about one unchanged store are one answer"
    );
}

#[test]
fn a_balanced_cycle_returns_the_handles_it_borrowed() {
    let dir = fresh("balanced");
    let (mut spool, mut store) = open(dir.path());
    let policy = retention::Policy::new(KEEP_WINDOWS * WINDOW_MILLIS, WINDOW_MILLIS);

    assert_eq!(
        (spool.open_handles(), store.open_handles()),
        (0, 0),
        "a store that has opened nothing holds nothing open"
    );

    let records = batch(0);
    spool
        .append(&batch::encode(&records))
        .unwrap_or_else(|error| panic!("the write path accepts a batch: {error}"));
    assert_eq!(
        spool.open_handles(),
        0,
        "the append's own handle is let go before it answers: a write path that answered with the \
         log still open would be a file held for the life of the process"
    );

    store
        .seal(&records)
        .unwrap_or_else(|error| panic!("a flush closes a batch into a segment: {error}"));
    store
        .expire(&policy, now(0))
        .unwrap_or_else(|error| panic!("a retention pass runs on a store this small: {error}"));
    ask(&mut store, &records);
    assert_eq!(
        store.open_handles(),
        0,
        "and the segment set's are let go too: a cycle that returns the store to where it started \
         is the whole steady-state claim"
    );
    assert_eq!(
        spool.open_handles(),
        store.open_handles(),
        "the log and the segments of one store are one store, so one count: a soak that summed two \
         numbers would be two half-answers"
    );
}

#[test]
fn a_handle_that_is_held_is_a_handle_the_store_reports() {
    let dir = fresh("held");
    std::fs::create_dir_all(dir.path().join(compaction::DIRECTORY))
        .unwrap_or_else(|error| panic!("the store directory can be made: {error}"));
    let file = dir
        .path()
        .join(compaction::DIRECTORY)
        .join("0000000000.lks");
    std::fs::write(&file, b"not a segment, and not read here")
        .unwrap_or_else(|error| panic!("the fixture's file can be written: {error}"));

    let open = Open::at(dir.path());
    assert_eq!(open.get(), 0, "nothing is open yet");
    let held = open
        .read_handle(&file)
        .unwrap_or_else(|error| panic!("a file this case wrote can be opened: {error}"));
    assert_eq!(
        open.get(),
        1,
        "a handle the store holds is a handle the store reports: this is the counter a long run \
         reads, and a leak is nothing more than a handle that never leaves this state"
    );
    let another = open
        .append(&file)
        .unwrap_or_else(|error| panic!("the same file can be opened again: {error}"));
    assert_eq!(open.get(), 2, "two handles are two");
    drop(held);
    drop(another);
    assert_eq!(
        open.get(),
        0,
        "and dropping them is what the counter is for: a store that grew by one handle per cycle \
         would read 4000 here rather than nothing"
    );
    assert_eq!(
        file.metadata().map(|data| data.len()).unwrap_or(0) as usize,
        b"not a segment, and not read here".len(),
        "releasing the count is not a read or a write: the file is what it was"
    );
}

#[test]
fn two_stores_do_not_share_a_counter() {
    let first = fresh("two-first");
    let second = fresh("two-second");
    let file = first.path().join("ingest.log");
    std::fs::write(&file, b"a log this case made")
        .unwrap_or_else(|error| panic!("the fixture's log can be written: {error}"));

    let held = Open::at(first.path())
        .read_handle(&file)
        .unwrap_or_else(|error| panic!("the fixture's log can be opened: {error}"));
    assert_eq!(
        (Open::at(first.path()).get(), Open::at(second.path()).get()),
        (1, 0),
        "a handle in one store is not a handle in another: a second deployment on the same machine \
         must not be able to move this one's counters"
    );
    drop(held);
}

#[test]
fn the_store_files_are_the_files_the_store_itself_put_there() {
    let dir = fresh("files");
    let (_, mut store) = open(dir.path());
    let policy = retention::Policy::new(KEEP_WINDOWS * WINDOW_MILLIS, WINDOW_MILLIS);
    let empty = Live::of(dir.path(), store.manifest(), 0);
    assert_eq!(
        (empty.store_files(), empty.store_bytes(), empty.records()),
        (0, 0, 0),
        "a store that has closed nothing holds no file and no record"
    );

    for cycle in 0..3 {
        let records = batch(cycle);
        store
            .seal(&records)
            .unwrap_or_else(|error| panic!("flush {cycle} closes a batch: {error}"));
    }
    let sealed = Live::of(dir.path(), store.manifest(), 0);
    assert_eq!(
        sealed.store_files(),
        7,
        "three segments, the index beside each of them, and the manifest that names them: {:?}",
        listing(dir.path())
    );
    assert_eq!(
        sealed.records(),
        3 * BATCH as u64,
        "and the manifest is where the record count comes from, without a file being read"
    );
    let measured: u64 = listing(dir.path())
        .iter()
        .map(|path| path.metadata().map(|data| data.len()).unwrap_or(0))
        .sum();
    assert_eq!(
        sealed.store_bytes(),
        measured,
        "the bytes the store holds are the bytes its own files are"
    );

    // A file somebody else left in the directory. A soak that counted it would report a number that
    // moved when the machine changed rather than when the store did — and on a machine that writes
    // one of these into every directory it touches, that is every run.
    std::fs::write(dir.path().join(".DS_Store"), b"a stranger")
        .unwrap_or_else(|error| panic!("a stranger's file can be written: {error}"));
    std::fs::write(
        dir.path().join(compaction::DIRECTORY).join("notes.txt"),
        b"mine",
    )
    .unwrap_or_else(|error| panic!("a note can be written: {error}"));
    let stranger = Live::of(dir.path(), store.manifest(), 0);
    assert_eq!(
        stranger, sealed,
        "a file the store did not write is not a file the store names, and the report does not move"
    );

    store.expire(&policy, now(3)).unwrap_or_else(|error| {
        panic!("a retention pass that expires nothing still answers: {error}")
    });
}

#[test]
fn a_store_that_is_opened_again_reports_what_the_one_that_closed_it_did() {
    let dir = fresh("reopened");
    let (_, mut store) = open(dir.path());
    for cycle in 0..3 {
        let records = batch(cycle);
        store
            .seal(&records)
            .unwrap_or_else(|error| panic!("flush {cycle} closes a batch: {error}"));
    }
    let closed = Live::of(dir.path(), store.manifest(), 0);
    let epoch = store.manifest().epoch();
    assert_eq!(
        epoch, 3,
        "one seal is one change of the store's shape, and the store counts them: a run that read \
         the epoch as anything else would be reporting a store nobody wrote ({closed:?})"
    );
    drop(store);

    let (_, mut reopened) = open(dir.path());
    let again = Live::of(dir.path(), reopened.manifest(), 0);
    assert_eq!(
        again, closed,
        "a store is what is on its disk, so the counters of a store that was opened again are the \
         counters of the store that closed it: this is the whole of what a soak's numbers are \
         about, because the run that reads them at the end did not make the store it is reading"
    );
    assert_eq!(
        reopened.manifest().epoch(),
        epoch,
        "including the epoch, which is the number a snapshot carries and the one thing a store \
         cannot answer twice differently"
    );
    for ordinal in 0..3u64 {
        assert!(
            matches!(index::load(dir.path(), ordinal), Lookup::Found(_)),
            "and the index beside each segment reads back as the index of that segment: \
             `store_files` counts those files, so a file the run counts and nobody can read is a \
             counter measuring a directory rather than a store"
        );
    }
    assert!(
        ask(&mut reopened, &batch(2)) > 0,
        "and the store answers a question about what it was holding after the reopen"
    );
}

#[test]
fn an_empty_store_reports_nothing_at_all() {
    let dir = fresh("empty");
    let (spool, store) = open(dir.path());
    let live = Live::of(dir.path(), store.manifest(), 0);

    assert_eq!(
        (
            live.open_tasks(),
            live.open_handles(),
            live.store_files(),
            live.store_bytes(),
            live.records()
        ),
        (0, 0, 0, 0, 0),
        "a store that has closed no segment is five zeroes and not a missing answer: the boundary \
         case of every counter in this report"
    );
    assert_eq!(
        spool.open_handles(),
        0,
        "the write path touched its log when it opened and holds nothing now"
    );
    assert!(
        spool.path().exists(),
        "the log itself is not one of the five: it is the write path's buffer of records no flush \
         has closed, and it lives at {}, which is not a file this report counts",
        spool.path().display()
    );
}

#[test]
fn a_short_soak_reaches_a_steady_state() {
    let dir = fresh("steady");
    let policy = retention::Policy::new(KEEP_WINDOWS * WINDOW_MILLIS, WINDOW_MILLIS);
    let reports = soak(dir.path(), policy, 40);

    let halfway = &reports[reports.len() / 2];
    let end = reports.last().expect("the soak ran cycles");
    for name in NAMES {
        let moved = end
            .get(name)
            .unwrap_or(0)
            .abs_diff(halfway.get(name).unwrap_or(0));
        let allowed = if name == "store_bytes" { BYTE_SLACK } else { 0 };
        assert!(
            moved <= allowed,
            "`{name}` was {} at the halfway mark and is {} at the end, which is {moved} more than \
             the {allowed} a steady state allows it: every cycle this store writes is one its \
             retention rule forgets, so a counter that moves is something the run opened and did \
             not close (docs/soak.md)",
            halfway.get(name).unwrap_or(0),
            end.get(name).unwrap_or(0)
        );
    }
    assert!(
        end.records() > 0 && end.store_files() > 0,
        "and the store did real work while staying still: {end:?}"
    );
    assert_eq!(
        end.records(),
        KEEP_WINDOWS * BATCH as u64,
        "the store holds exactly the windows its policy keeps: {end:?}"
    );
    assert_eq!(
        end.store_files(),
        2 * KEEP_WINDOWS + 1,
        "which is this many files: a segment and its index per window, and the manifest"
    );
}

#[test]
fn the_same_soak_with_retention_off_grows() {
    let dir = fresh("growing");
    let reports = soak(dir.path(), retention::Policy::DEFAULT, 40);
    let halfway = &reports[reports.len() / 2];
    let end = reports.last().expect("the soak ran cycles");

    for name in ["store_files", "store_bytes", "records"] {
        assert!(
            end.get(name).unwrap_or(0) > halfway.get(name).unwrap_or(0),
            "`{name}` is {} at the halfway mark and {} at the end: a store that keeps everything \
             grows for ever, and the pair of cases is the claim — a steady state is a property of \
             the configuration this store was given and not of the code that writes it",
            halfway.get(name).unwrap_or(0),
            end.get(name).unwrap_or(0)
        );
    }
    assert_eq!(
        end.records(),
        40 * BATCH as u64,
        "nothing was forgotten, so every record this soak wrote is still there"
    );
}

#[test]
fn a_question_about_the_store_moves_nothing() {
    let dir = fresh("question");
    let (_, mut store) = open(dir.path());
    for cycle in 0..3 {
        let records = batch(cycle);
        store
            .seal(&records)
            .unwrap_or_else(|error| panic!("flush {cycle} closes a batch: {error}"));
    }
    let before = Live::of(dir.path(), store.manifest(), 0);
    let rows = ask(&mut store, &batch(2));
    let after = Live::of(dir.path(), store.manifest(), 0);

    assert!(
        rows > 0,
        "the question is one the store answers: {rows} row(s)"
    );
    assert_eq!(
        (after.store_files(), after.store_bytes(), after.records()),
        (before.store_files(), before.store_bytes(), before.records()),
        "asking is not writing: a store whose reads moved its counters would make every soak \
         report the questions it asked rather than the data it holds"
    );
    assert_eq!(
        after.open_handles(),
        0,
        "and the handles a query opens are closed by the time it answers"
    );
}

#[test]
fn a_batch_the_store_refuses_leaves_the_counters_where_they_were() {
    let dir = fresh("refused");
    let (mut spool, mut store) = open(dir.path());
    let records = batch(0);
    store
        .seal(&records)
        .unwrap_or_else(|error| panic!("the first flush closes a batch: {error}"));
    let before = Live::of(dir.path(), store.manifest(), 0);

    assert!(
        store.seal(&[]).is_err(),
        "a flush with nothing in it is refused: a segment that holds no records is one a plan \
         would carry for ever"
    );
    assert!(
        spool.append(&[]).is_ok(),
        "the write path takes an empty payload without writing anything — the two refusals are \
         different, and this case is about the one that is a refusal"
    );
    let refusal = store.expire(&retention::Policy::DEFAULT, now(9));
    assert!(
        refusal.is_ok(),
        "a pass that has nothing to expire is an answer and not a failure: {refusal:?}"
    );

    let after = Live::of(dir.path(), store.manifest(), 0);
    assert_eq!(
        after, before,
        "an interrupted cycle is a cycle the counters must not report: a refusal that had already \
         moved a counter would make a soak report work the store did not do"
    );
}

#[test]
fn the_counted_files_are_the_ones_the_manifest_names() {
    let dir = fresh("expired");
    let (_, mut store) = open(dir.path());
    let policy = retention::Policy::new(KEEP_WINDOWS * WINDOW_MILLIS, WINDOW_MILLIS);
    for cycle in 0..KEEP_WINDOWS + 2 {
        let records = batch(cycle);
        store
            .seal(&records)
            .unwrap_or_else(|error| panic!("flush {cycle} closes a batch: {error}"));
        store
            .expire(&policy, now(cycle))
            .unwrap_or_else(|error| panic!("the pass at cycle {cycle} runs: {error}"));
    }

    let kept: Vec<u64> = store
        .manifest()
        .segments()
        .iter()
        .map(|info| info.index)
        .collect();
    assert_eq!(
        kept,
        (2..KEEP_WINDOWS + 2).collect::<Vec<u64>>(),
        "the two oldest segments left the store and the four the policy keeps stayed: {:?}",
        listing(dir.path())
    );
    let live = Live::of(dir.path(), store.manifest(), 0);
    assert_eq!(
        live.store_files(),
        2 * KEEP_WINDOWS + 1,
        "a segment and the index beside it per ordinal the manifest names, and the manifest"
    );
    assert_eq!(
        live.records(),
        KEEP_WINDOWS * BATCH as u64,
        "and the records of the ones that left are not the store's any more"
    );

    // The garbage `4.14-index-pruning` deliberately leaves: an expired ordinal's index file stays
    // behind, because nothing names that ordinal any more and nothing can consult it. A counter
    // that counted it would report that garbage as a leak, one file per expired segment, and a
    // soak would spend its life blaming the store for a later step's capacity number.
    for ordinal in 0..2u64 {
        let orphan = index::file_of(dir.path(), ordinal);
        assert!(
            orphan.exists(),
            "{} is the index of an ordinal nothing names any more: this build keeps it on purpose",
            orphan.display()
        );
        assert!(
            !compaction::file_of(dir.path(), ordinal).exists(),
            "and the segment it belonged to left the store with the whole file"
        );
    }
    let named = listing(dir.path());
    assert_eq!(
        named.len() as u64,
        live.store_files() + 3,
        "so the directory holds three files the store does not name — the two orphans and the write \
         path's log — and the count is a count of the store rather than of the disk, which is what \
         makes it flat over a long run: {named:?}"
    );
    for ordinal in 0..2u64 {
        assert!(
            named.contains(&index::file_of(dir.path(), ordinal)),
            "and the orphans are two of the three: {named:?}"
        );
    }
}

#[test]
fn a_merge_changes_the_shape_the_counters_count() {
    let dir = fresh("merge");
    let (_, mut store) = open_with(dir.path(), compaction::Policy::new(2, u64::MAX));
    for cycle in 0..2 {
        let records = batch(cycle);
        store
            .seal(&records)
            .unwrap_or_else(|error| panic!("flush {cycle} closes a batch: {error}"));
    }
    let before = Live::of(dir.path(), store.manifest(), 0);
    let epoch = store.manifest().epoch();
    assert_eq!(
        (before.store_files(), before.records()),
        (5, 2 * BATCH as u64),
        "two segments, the index beside each, and the manifest: {before:?}"
    );

    let plan = store.plan();
    let ran = store
        .merge(&plan)
        .unwrap_or_else(|error| panic!("two comparable segments merge: {error}"));
    store
        .install(ran)
        .unwrap_or_else(|error| panic!("a merge this store wrote is installed: {error}"));
    let after = Live::of(dir.path(), store.manifest(), 0);

    assert_eq!(
        (after.store_files(), after.records()),
        (3, 2 * BATCH as u64),
        "a merge takes two files into one without losing a record, and this is what a soak reads \
         the shape from: a store that compacts never grows a file per cycle for ever"
    );
    assert_eq!(
        store.manifest().epoch(),
        epoch + 1,
        "and it is one change of the store's shape, like every other change: the epoch is the \
         number a snapshot carries, so a pass that moved it by anything else would answer two \
         different stores with the same number: {after:?}"
    );
    assert_eq!(
        after.open_handles(),
        0,
        "and the merge let go of everything it opened"
    );
}

#[test]
fn the_ledger_counts_what_a_service_spawned_and_not_what_it_reaped() {
    let tasks = Tasks::new();
    let other = Tasks::new();
    assert_eq!(
        (tasks.open(), other.open()),
        (0, 0),
        "a ledger that spawned nothing counts nothing"
    );

    runtime().block_on(async {
        let held = tasks.spawn(async { 7u8 });
        assert_eq!(
            tasks.open(),
            1,
            "a spawned task is a task the service is holding: the ledger is charged where the \
             spawning happens and not by a reader walking a list afterwards"
        );
        let joined = held.join().await;
        assert_eq!(joined.ok(), Some(7u8), "the task ran");
        assert_eq!(
            tasks.open(),
            0,
            "and awaiting it is the reap: the count is released by the handle that owned it"
        );

        let dropped = tasks.spawn(async {});
        assert_eq!(
            tasks.open(),
            1,
            "a task that was spawned and not awaited is held"
        );
        drop(dropped);
        assert_eq!(
            tasks.open(),
            0,
            "letting the handle go is a reap too, and it has to be: a service that keeps a handle \
             per cycle is a service that grows by one per cycle, which is the leak this step is \
             about"
        );

        let held = tasks.spawn(async {});
        assert_eq!(
            (tasks.open(), other.open()),
            (1, 0),
            "two services in one process do not share a ledger, exactly as two stores do not \
             share a counter"
        );
        drop(held);
    });
}

#[test]
fn the_daemons_tasks_are_counted_and_its_stop_path_reaps_them() {
    let dir = fresh("daemon");
    let (spool, _) = open(dir.path());
    let queue = Queue::new(8, Overflow::Block);
    let ledger = queue.tasks();
    assert_eq!(
        ledger.open(),
        0,
        "the queue's ledger is empty before anything is spawned through it"
    );

    runtime().block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port is free");
        let address: SocketAddr = listener
            .local_addr()
            .expect("the kernel answers the address");
        let stop = Arc::new(Notify::new());
        let waiting = Arc::clone(&stop);
        let store = Arc::new(tokio::sync::Mutex::new(spool));
        let serving = ledger.spawn({
            let queue = Arc::clone(&queue);
            async move {
                ingest::serve(listener, store, queue, async move {
                    waiting.notified().await;
                })
                .await
            }
        });
        // A client that says nothing and leaves. Waiting for the daemon to finish with it is what
        // makes the counts below deterministic: the loop spawns its writer before it accepts
        // anything, so a connection that was *served* proves the writer is there and the loop has
        // run at least once.
        for _ in 0..2 {
            let mut client = tokio::net::TcpStream::connect(address)
                .await
                .expect("the daemon this case started accepts a connection");
            client
                .shutdown()
                .await
                .expect("the client's half of the connection can be closed");
            let mut end = [0u8; 1];
            assert_eq!(
                client
                    .read(&mut end)
                    .await
                    .expect("the daemon closes the connection"),
                0,
                "this protocol is one-way, so the daemon's answer to a client is end-of-file"
            );
            assert_eq!(
                ledger.open(),
                2,
                "the daemon's loop is a task of this ledger and the writer it spawned is another \
                 one — and the connection's own task is *not* a third, because the loop lets its \
                 handle go: a loop that kept it would grow by one per client, which is the leak \
                 this step's `open_tasks` names"
            );
        }
        stop.notify_one();
        let served = serving.join().await.expect("the daemon's task finished");
        assert!(served.is_ok(), "the daemon stopped cleanly: {served:?}");
        assert_eq!(
            ledger.open(),
            0,
            "and a stopped daemon is a service that holds nothing: the writer is awaited by the \
             stop path rather than abandoned"
        );
    });
}

#[test]
fn the_engine_does_not_reach_into_the_service() {
    let source = std::fs::read_to_string("src/engine/soak.rs")
        .expect("the module this step adds is part of the project");
    for forbidden in ["crate::service", "tokio"] {
        assert!(
            !source.contains(forbidden),
            "src/engine/soak.rs names `{forbidden}` — the counters of the store belong to the \
             engine, which runs in a test, in a tool and in the daemon alike, and the task half of \
             the report is handed in as a count"
        );
    }
}

#[test]
fn the_counters_are_counts_of_state_and_never_of_time() {
    for file in ["src/engine/soak.rs", "src/service/soak.rs"] {
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{file} is part of the project: {error}"));
        for forbidden in ["Instant", "SystemTime", "Duration", "elapsed"] {
            assert!(
                !source.contains(forbidden),
                "{file} names `{forbidden}`: a counter that is a duration is a counter that says \
                 something different on a loaded machine, and the judgement this step makes is \
                 counts (handoff M2 §1.1, docs/soak.md)"
            );
        }
    }
}

/// One case's store, in a directory of its own, removed when the case ends.
struct Dir {
    path: PathBuf,
}

impl Dir {
    /// Where it lives.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A runtime for the cases that spawn tasks: two threads, because a daemon and its client are two
/// tasks that have to make progress one another's work.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime can be built")
}

/// A directory under the system's temporary directory that belongs to this case.
fn fresh(label: &str) -> Dir {
    let path = std::env::temp_dir().join(format!("lakelet-soak-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap_or_else(|error| {
        panic!("{} could not be created: {error}", path.display());
    });
    Dir { path }
}

/// The two halves of an empty store rooted at `directory`.
///
/// The compaction policy is deliberately idle: most of these cases are about a store that keeps one
/// shape, and `budget/4.8-compaction.toml` is where the cost of merging is judged. The cases that
/// do want a merge ask for one by name ([`open_with`]).
fn open(directory: &Path) -> (Spool, Segments) {
    open_with(directory, compaction::Policy::new(usize::MAX, u64::MAX))
}

/// The same, with the compaction policy this case wants.
fn open_with(directory: &Path, policy: compaction::Policy) -> (Spool, Segments) {
    let spool = Spool::with_policy(directory, Durability::Never)
        .unwrap_or_else(|error| panic!("the write path opens a store: {error}"));
    let store = Segments::with_policy(directory, policy)
        .unwrap_or_else(|error| panic!("the segment set opens a store: {error}"));
    (spool, store)
}

/// A soak of `cycles` cycles over one store, answering the report after every one of them.
fn soak(directory: &Path, policy: retention::Policy, cycles: u64) -> Vec<Live> {
    let (mut spool, mut store) = open(directory);
    let mut reports = Vec::with_capacity(cycles as usize);
    for cycle in 0..cycles {
        let records = batch(cycle);
        spool
            .append(&batch::encode(&records))
            .unwrap_or_else(|error| panic!("cycle {cycle} is accepted: {error}"));
        store
            .seal(&records)
            .unwrap_or_else(|error| panic!("cycle {cycle} is closed: {error}"));
        store
            .expire(&policy, now(cycle))
            .unwrap_or_else(|error| panic!("cycle {cycle} forgets what is old: {error}"));
        ask(&mut store, &records);
        reports.push(Live::of(directory, store.manifest(), 0));
    }
    reports
}

/// One batch: the same records every cycle, moved on to the window this cycle writes.
fn batch(cycle: u64) -> Vec<Record> {
    let start = EPOCH.saturating_add(cycle.saturating_mul(WINDOW_MILLIS));
    (0..BATCH)
        .map(|index| {
            let offset = u64::try_from(index)
                .unwrap_or(0)
                .saturating_mul(STEP_MILLIS);
            Record {
                series: format!("{METRIC},host=h{}", index % 4),
                ts_millis: i64::try_from(start.saturating_add(offset)).unwrap_or(i64::MAX),
                value: f64::from(u32::try_from(index % 13).unwrap_or(0)) / 4.0,
            }
        })
        .collect()
}

/// When a cycle's retention pass runs: the end of the window it just wrote.
fn now(cycle: u64) -> u64 {
    EPOCH.saturating_add(cycle.saturating_add(1).saturating_mul(WINDOW_MILLIS))
}

/// One question about the store, and the rows it answered with.
fn ask(store: &mut Segments, records: &[Record]) -> u64 {
    let mut first = i64::MAX;
    let mut last = i64::MIN;
    for record in records {
        first = first.min(record.ts_millis);
        last = last.max(record.ts_millis);
    }
    let to = last.saturating_add(i64::try_from(STEP_MILLIS).unwrap_or(i64::MAX));
    let text = format!(
        "SELECT count(value) FROM {METRIC} WHERE ts BETWEEN {first} AND {to} \
         GROUP BY time({WINDOW_MILLIS}ms)"
    );
    let query = match parse::parse(&text) {
        Ok(parse::Statement::Query(query) | parse::Statement::Explain(query)) => query,
        Err(refusal) => panic!("this case's own question parses: {refusal}"),
    };
    let outcome = exec::execute(&store.snapshot(), &query).unwrap_or_else(|refusal| {
        panic!("the store answers a question about what it holds: {refusal}")
    });
    outcome.rows_answered()
}

/// Every file under the store directory, in path order: what a reader would find with `ls -R`.
fn listing(directory: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}
