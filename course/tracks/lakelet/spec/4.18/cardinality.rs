//! `lakelet/4.18-cardinality-cost` — how many series a store carries, and what refusing one means.
//!
//! Sixteen cases, one claim each, and they come in four groups:
//!
//! **The cap's rule** — a policy is a number and two answers, so the boundary is the first thing
//! to pin down:
//!
//! 1. [`the_limit_th_series_fits_and_the_next_one_does_not`] — the count is of *tracked* series,
//!    the `limit`-th new one is admitted, the next is refused, and a series the store already
//!    tracks is admitted however full the ledger is;
//! 2. [`an_offer_is_counted_once_and_the_identity_holds`] — `offered == admitted + rejected`, over
//!    a run of offers that mixes both answers;
//! 3. [`a_refusal_names_both_numbers`] — what a log line or a client can read, and that it is a
//!    value a caller can branch on without parsing anything;
//! 4. [`the_cap_this_build_ships_is_the_number_the_page_states`] — the default is a decision
//!    somebody has to defend, so it is pinned where a change to it is a red case;
//! 5. [`a_frame_is_judged_whole`] — a frame is written whole or not at all, so a frame the cap
//!    refuses charges nothing at all.
//!
//! **The ledger's arithmetic** — what "cost" means, and what it is measured from:
//!
//! 6. [`a_series_costs_what_the_write_path_was_handed`] — the bytes add up to the payload's
//!    length exactly, and the window is the records' own;
//! 7. [`a_timestamp_that_is_not_a_time_is_charged_as_the_zero_the_store_states`] — the same rule
//!    the manifest and the zone map state;
//! 8. [`bytes_that_are_not_a_batch_are_not_the_caps_business`] — a payload that names no records
//!    is not judged, counted or charged.
//!
//! **The rebuild, which is 4.14's doctrine applied to cardinality** — the ledger is derived state:
//!
//! 9. [`a_series_the_store_still_holds_stays_and_one_it_forgot_leaves`] — the prune, in both
//!    directions;
//! 10. [`the_window_is_the_extremes_the_index_states`] — the store's own answer about when a
//!     series happened, rather than the log's;
//! 11. [`a_store_that_lost_its_indexes_loses_precision_and_no_answer`] — a missing index proves
//!     nothing and drops nothing, and the store's records are what they were;
//! 12. [`the_ledger_of_a_store_is_the_series_its_records_name`] — `Registry::of_store`, from a
//!     store the write path filled and the segment store sealed.
//!
//! **The two paths a deployment runs** — the daemon and the tool:
//!
//! 13. [`the_ingest_path_refuses_a_frame_for_a_new_series`] — through the real daemon, and the
//!     connection's own counters are where it shows;
//! 14. [`a_cap_survives_a_restart`] — the ledger is rebuilt from the store, so a process that
//!     restarts does not admit a fresh limit of names;
//! 15. [`the_series_table_is_one_line_per_series_and_the_same_objects_as_json`] — `lakeletq
//!     inspect series`, in both formats and at all four of its exits;
//! 16. [`the_cap_is_the_deployments_number_and_the_engine_reads_no_clock`] — the cap is a
//!     deployment's setting in the same layers as the queue's bound, the default the daemon prints
//!     is the one this build ships, and the engine's half reads no clock and reaches for no
//!     service.

mod support;

use std::path::Path;
use std::process::{Command, Stdio};

use lakelet::engine::Spool;
use lakelet::engine::cardinality::{self, Admission, Cap, Cost, Registry};
use lakelet::engine::compaction::Segments;
use lakelet::engine::index::{self, SegmentIndex};
use lakelet::engine::query::series::Series;
use lakelet::protocol::batch::{self, Record};
use lakelet::protocol::frame::Frame;
use support::StoreDir;
use support::Var;
use support::service::Service;

/// The first timestamp every case writes, on a whole second.
const EPOCH: u64 = 1_700_000_000_000;

/// How far apart two records of one payload are.
const STEP_MILLIS: u64 = 1_000;

/// A store directory of this case's own, removed when the case ends.
fn store_dir(label: &str) -> StoreDir {
    StoreDir::new(&format!("cardinality-{label}"))
}

/// The path a command line names, as the tool will read it.
fn argument(path: &Path) -> String {
    path.to_str()
        .unwrap_or_else(|| panic!("{} is not UTF-8", path.display()))
        .to_owned()
}

/// `count` records of one series, one second apart from `at`.
fn records(series: &str, at: u64, count: usize) -> Vec<Record> {
    (0..count)
        .map(|index| Record {
            series: series.to_owned(),
            ts_millis: i64::try_from(at.saturating_add(index as u64 * STEP_MILLIS)).unwrap_or(0),
            value: index as f64,
        })
        .collect()
}

/// `count` records spread over `series`, one series after another, so that a payload carries more
/// than one name.
fn spread(series: &[&str], at: u64, each: usize) -> Vec<Record> {
    let mut all: Vec<Record> = Vec::new();
    for (index, name) in series.iter().enumerate() {
        let start = at.saturating_add(index as u64 * STEP_MILLIS * each as u64);
        all.extend(records(name, start, each));
    }
    all
}

/// The payload the protocol carries these records in: what the write path is handed.
fn payload(records: &[Record]) -> Vec<u8> {
    batch::encode(records)
}

/// The frames one client sends, in one connection.
fn wire(records: &[Record]) -> Vec<u8> {
    Frame::new(payload(records)).encode()
}

/// A store in a directory of its own, sealed from `batches` in order, with one index per segment.
fn sealed(label: &str, batches: &[Vec<Record>]) -> (StoreDir, Segments) {
    let dir = store_dir(label);
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    for batch in batches {
        store
            .seal(batch)
            .unwrap_or_else(|error| panic!("a segment of a case's own records is closed: {error}"));
    }
    (dir, store)
}

/// The sidecar indexes of the segments a store holds, in the manifest's order.
///
/// Written here rather than called from the library on purpose: a spec that asked the
/// implementation for its own reader would be agreeing with it about what it reads. The pieces are
/// `lakelet/4.14-index-pruning`'s, public since then.
fn live_indexes(directory: &Path) -> Vec<SegmentIndex> {
    let store = Segments::open(directory)
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    store
        .manifest()
        .segments()
        .iter()
        .map(|info| {
            index::load(directory, info.index)
                .into_index()
                .unwrap_or_else(SegmentIndex::unstated)
        })
        .collect()
}

/// The bytes one series has been charged, added up over a ledger.
fn charged_bytes(registry: &Registry) -> u64 {
    registry
        .tracked()
        .fold(0u64, |total, (_, cost)| total.saturating_add(cost.bytes()))
}

/// One `lakeletq` run: what a script would have seen.
#[derive(Debug)]
struct Ran {
    /// The exit code, or a panic's worth of explanation when the process was killed.
    code: i32,
    /// What standard output received.
    stdout: String,
    /// What standard error received.
    stderr: String,
}

/// Run the real binary, with no environment and `/dev/null` on standard input.
fn lakeletq(args: &[&str]) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletq"));
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("`lakeletq {args:?}` could not be started: {error}"));
    Ran {
        code: output.status.code().unwrap_or_else(|| {
            panic!("`lakeletq {args:?}` was killed by a signal: it must exit with a code")
        }),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The failure rule, asserted once: nothing on stdout, a sentence on stderr, and this code.
fn refused(ran: &Ran, code: i32, what: &str) {
    assert_eq!(
        ran.code, code,
        "{what} must exit {code} — a script branches on this number, and {ran:?}"
    );
    assert_eq!(
        ran.stdout, "",
        "{what} must print nothing on stdout: a script that parses the answer must not be handed \
         half an answer, and it got `{}`",
        ran.stdout
    );
    assert!(
        !ran.stderr.trim().is_empty(),
        "{what} must say what went wrong on stderr: an exit code on its own sends the reader to \
         the source"
    );
}

#[test]
fn the_limit_th_series_fits_and_the_next_one_does_not() {
    let cap = Cap::new(3);
    let mut ledger = Registry::new(cap);
    assert_eq!(
        ledger.cap(),
        cap,
        "a ledger carries the policy it is judged by"
    );
    assert_eq!(ledger.tracked_count(), 0, "and it starts empty");

    for name in ["cpu,host=h0", "cpu,host=h1", "cpu,host=h2"] {
        let admission = ledger.admit(&Series::of(name), 4, 64, EPOCH);
        assert!(
            admission.is_admitted(),
            "`{name}` is one of the three this cap allows: {admission}"
        );
    }
    assert_eq!(
        ledger.tracked_count(),
        3,
        "three distinct names are three tracked series: {:?}",
        ledger
            .tracked()
            .map(|(name, _)| name)
            .collect::<Vec<&str>>()
    );

    // The next *new* name is refused, and the refusal says what the decision was made from.
    let refused = ledger.admit(&Series::of("cpu,host=h3"), 4, 64, EPOCH);
    assert_eq!(
        refused,
        Admission::Rejected {
            tracked: 3,
            limit: 3
        },
        "the fourth name does not fit under a cap of three: {refused}"
    );
    assert_eq!(
        ledger.cost_of("cpu,host=h3"),
        None,
        "and a refused series is not in the ledger — this is the line a cap that leaks crosses"
    );
    assert_eq!(
        ledger.tracked_count(),
        3,
        "the refusal moved no name into the live set"
    );

    // A series the store already tracks is always admitted: a cap must not break a series that is
    // being reported, or a deployment at its limit would lose the data it already holds.
    let again = ledger.admit(&Series::of("cpu,host=h0"), 6, 96, EPOCH + 10 * STEP_MILLIS);
    assert!(
        again.is_admitted(),
        "a tracked series is admitted at the limit, which is the whole of the rule: {again}"
    );
    let cost = ledger
        .cost_of("cpu,host=h0")
        .unwrap_or_else(|| panic!("a series the store took is in the ledger"));
    assert_eq!(cost.records(), 10, "and what it carried is charged to it");
    assert_eq!(cost.bytes(), 160, "bytes and all");
    assert_eq!(
        ledger.tracked_count(),
        3,
        "with no new name to make room for"
    );

    // A cap of one is the boundary the case above walks past: the first name fits, the second does
    // not, and the two answers are the two variants and not a third.
    let mut tight = Registry::new(Cap::new(1));
    assert!(
        tight
            .admit(&Series::of("cpu,host=h0"), 1, 8, EPOCH)
            .is_admitted()
    );
    assert!(
        !tight
            .admit(&Series::of("cpu,host=h1"), 1, 8, EPOCH)
            .is_admitted(),
        "under a cap of one there is no room for a name the store has never seen"
    );
}

#[test]
fn an_offer_is_counted_once_and_the_identity_holds() {
    let mut ledger = Registry::new(Cap::new(2));
    let offers = [
        ("cpu,host=h0", true),
        ("cpu,host=h0", true),
        ("cpu,host=h1", true),
        ("cpu,host=h2", false),
        ("cpu,host=h2", false),
        ("cpu,host=h1", true),
    ];
    for (name, expected) in offers {
        let admission = ledger.admit(&Series::of(name), 1, 16, EPOCH);
        assert_eq!(
            admission.is_admitted(),
            expected,
            "`{name}` is admitted only when it fits: {admission}"
        );
    }
    assert_eq!(ledger.series_admitted(), 4, "four offers were taken");
    assert_eq!(ledger.series_rejected(), 2, "and two were not");
    assert_eq!(
        ledger.offered(),
        ledger.series_admitted() + ledger.series_rejected(),
        "every offer is counted exactly once, on one side of the cap — the identity a leak breaks"
    );
    assert_eq!(
        ledger.tracked_count(),
        2,
        "and the live set is what survived the cap, not what was offered to it"
    );

    // A frame is a batch of offers, and it is counted the same way: a frame the cap refuses is a
    // frame whose every series the store did not take.
    let batch = payload(&spread(&["cpu,host=h4", "cpu,host=h5"], EPOCH, 2));
    let refused = ledger.charge(&batch);
    assert_eq!(
        refused,
        Some(Admission::Rejected {
            tracked: 2,
            limit: 2
        }),
        "a frame naming two series the store has never seen does not fit under a cap of two either"
    );
    assert_eq!(
        ledger.series_rejected(),
        4,
        "both of its series are counted as rejected, because neither of them was written"
    );
    assert_eq!(ledger.tracked_count(), 2, "and nothing was charged");
    assert_eq!(
        ledger.offered(),
        ledger.series_admitted() + ledger.series_rejected(),
        "the identity still holds after a frame, which is what makes it a claim about every path"
    );
}

#[test]
fn a_refusal_names_both_numbers() {
    let refusal = Admission::Rejected {
        tracked: 1024,
        limit: 1024,
    };
    let shown = refusal.to_string();
    assert!(
        shown.contains("1024") && shown.matches("1024").count() >= 2,
        "a refusal names how many series are tracked and what the cap allows, so that a log line \
         or a client can say what happened without parsing anything: {shown}"
    );
    assert!(
        !refusal.is_admitted(),
        "and it is a value a caller branches on, not a string it has to read: {refusal:?}"
    );
    let taken = Admission::Admitted;
    assert!(
        taken.is_admitted(),
        "the other answer is `true` here: {taken}"
    );
    assert!(
        !taken.to_string().is_empty(),
        "both answers are printable: a log line about a taken series is a line somebody writes too"
    );
    assert_eq!(
        Admission::Rejected {
            tracked: 1,
            limit: 2
        }
        .to_string(),
        Admission::Rejected {
            tracked: 1,
            limit: 2
        }
        .to_string(),
        "and the same decision reads the same twice"
    );
}

#[test]
fn the_cap_this_build_ships_is_the_number_the_page_states() {
    assert_eq!(
        Cap::DEFAULT.limit(),
        1024,
        "the cap this build ships is the number `docs/cardinality.md` prices and defends: changing \
         it is changing a documented budget, and this case is where that is noticed"
    );
    assert_eq!(
        Cap::new(7).limit(),
        7,
        "a cap is a value, and the value it answers is the value it was given"
    );
    assert_eq!(
        Cap::new(7).to_string(),
        "at most 7 series",
        "and it says what it is, for the line that prints the deployment's policy"
    );
}

#[test]
fn a_frame_is_judged_whole() {
    let mut ledger = Registry::new(Cap::new(2));
    // One name tracked and room for exactly one more, and a frame that names two the store has
    // never seen: the frame does not fit, and a frame the store refuses is a frame that is *not
    // written* — so nothing in it is charged, not even the series it names that the store has.
    assert!(
        ledger
            .admit(&Series::of("cpu,host=h0"), 1, 8, EPOCH)
            .is_admitted()
    );
    let batch = payload(&spread(
        &["cpu,host=h0", "cpu,host=h1", "cpu,host=h2"],
        EPOCH,
        2,
    ));
    assert!(
        !ledger
            .charge(&batch)
            .unwrap_or_else(|| panic!("a batch of a case's own records is readable"))
            .is_admitted(),
        "one new name would fit and two do not, and the frame is written whole or not at all"
    );
    assert_eq!(
        ledger.tracked_count(),
        1,
        "so the frame left the ledger exactly where it was"
    );
    let cost = ledger
        .cost_of("cpu,host=h0")
        .unwrap_or_else(|| panic!("the series the store took is still there"));
    assert_eq!(
        cost.records(),
        1,
        "including the series it names that the store already tracks: a refused frame is not a \
         half-written one"
    );

    // The same frame with room for both names is admitted, and then each series is charged its
    // share.
    let mut roomy = Registry::new(Cap::new(3));
    let taken = roomy.charge(&batch);
    assert!(
        taken.is_some_and(|admission| admission.is_admitted()),
        "with room for two new names the frame fits: {taken:?}"
    );
    assert_eq!(
        roomy.tracked_count(),
        3,
        "and all three of its series are tracked: {:?}",
        roomy.tracked().map(|(name, _)| name).collect::<Vec<&str>>()
    );
    assert_eq!(
        charged_bytes(&roomy),
        batch.len() as u64,
        "the payload's bytes are shared out over the series it named and add up to the payload"
    );
}

#[test]
fn a_series_costs_what_the_write_path_was_handed() {
    let mut ledger = Registry::new(Cap::new(8));
    // Three payloads: two of one series, then one carrying two series.
    let first = payload(&records("cpu,host=h0", EPOCH, 5));
    let second = payload(&records("cpu,host=h0", EPOCH + 60 * STEP_MILLIS, 3));
    let third = payload(&spread(
        &["cpu,host=h0", "mem,host=h1"],
        EPOCH + 120 * STEP_MILLIS,
        4,
    ));
    for batch in [&first, &second, &third] {
        assert!(
            ledger
                .charge(batch)
                .is_some_and(|admission| admission.is_admitted()),
            "a frame inside a cap of eight is taken"
        );
    }
    assert_eq!(
        charged_bytes(&ledger),
        (first.len() + second.len() + third.len()) as u64,
        "a store's per-series bytes add up to exactly the bytes it was handed: that is what makes \
         the arithmetic on the page a measurement rather than a claim"
    );
    let cpu = ledger
        .cost_of("cpu,host=h0")
        .unwrap_or_else(|| panic!("the series the run wrote is tracked"));
    assert_eq!(
        cpu.records(),
        5 + 3 + 4,
        "every record of it, over three payloads"
    );
    assert_eq!(
        cpu.first_millis(),
        EPOCH,
        "the window starts at the first record it carried"
    );
    assert_eq!(
        cpu.last_millis(),
        EPOCH + 123 * STEP_MILLIS,
        "and ends at the last record it carried, both inclusive — the third payload's fourth"
    );
    let mem = ledger
        .cost_of("mem,host=h1")
        .unwrap_or_else(|| panic!("the other series of the third payload is tracked"));
    assert_eq!(
        mem.records(),
        4,
        "and the second series is charged its own half"
    );
    assert!(
        mem.bytes() > 0 && mem.bytes() < third.len() as u64,
        "which is a share of the payload rather than the whole of it: {} of {}",
        mem.bytes(),
        third.len()
    );
    assert_eq!(
        cpu.bytes() + mem.bytes(),
        (first.len() + second.len() + third.len()) as u64,
        "and the two shares are the payloads' bytes and not a rounding of them"
    );
}

#[test]
fn a_timestamp_that_is_not_a_time_is_charged_as_the_zero_the_store_states() {
    let mut ledger = Registry::new(Cap::new(2));
    let mut early = records("cpu,host=h0", EPOCH, 2);
    early[0].ts_millis = -1;
    early[1].ts_millis = -9_000;
    let batch = payload(&early);
    assert!(
        ledger
            .charge(&batch)
            .is_some_and(|admission| admission.is_admitted()),
        "records with timestamps before the epoch are records like any others"
    );
    let cost = ledger
        .cost_of("cpu,host=h0")
        .unwrap_or_else(|| panic!("the series is tracked"));
    assert_eq!(
        cost.first_millis(),
        0,
        "a timestamp that is not a time counts as zero — the rule the manifest's own entry states \
         and the zone map repeats, because a ledger whose window disagreed with the store's would \
         make two documents out of one fact"
    );
    assert_eq!(cost.last_millis(), 0, "and so does the one after it");
}

#[test]
fn bytes_that_are_not_a_batch_are_not_the_caps_business() {
    let mut ledger = Registry::new(Cap::new(1));
    for bytes in [
        Vec::new(),
        vec![0u8; 7],
        b"not a batch at all, and not framed like one either".to_vec(),
    ] {
        assert_eq!(
            ledger.charge(&bytes),
            None,
            "a payload this build cannot read as a batch names no records, so the cap has nothing \
             to judge: `{}` bytes of the write path are the write path's business",
            bytes.len()
        );
    }
    assert_eq!(ledger.tracked_count(), 0, "and nothing was tracked");
    assert_eq!(
        (ledger.series_admitted(), ledger.series_rejected()),
        (0, 0),
        "and nothing was counted: an unreadable payload is not an offer, and counting it as one \
         would make the identity a claim about bytes rather than about series"
    );
    // A frame whose payload is a batch of zero records is the same kind of thing.
    let empty = payload(&[]);
    assert_eq!(
        ledger.charge(&empty),
        None,
        "a batch holding nothing names no series"
    );
}

#[test]
fn a_series_the_store_still_holds_stays_and_one_it_forgot_leaves() {
    let mut ledger = Registry::new(Cap::new(4));
    for name in [
        "cpu,host=h0",
        "mem,host=h0",
        "gone,host=h0",
        "other,host=h0",
    ] {
        assert!(ledger.admit(&Series::of(name), 2, 32, EPOCH).is_admitted());
    }
    assert_eq!(ledger.tracked_count(), 4, "four names are tracked");

    // The store's segments: two of the four names, and nothing else.
    let live = [
        SegmentIndex::of(&records("cpu,host=h0", EPOCH, 2)),
        SegmentIndex::of(&records("mem,host=h0", EPOCH + 10 * STEP_MILLIS, 2)),
    ];
    assert!(
        live[0].may_hold("cpu,host=h0"),
        "an index may hold the series whose records built it"
    );
    ledger.rebuild(&live);
    let mut kept: Vec<&str> = ledger.tracked().map(|(name, _)| name).collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        vec!["cpu,host=h0", "mem,host=h0"],
        "a series some live index may hold stays, and a series no index may hold is one the store \
         has forgotten — the prune is what makes room for a new name in a store that keeps its \
         window"
    );
    assert_eq!(
        ledger.cost_of("gone,host=h0"),
        None,
        "and a remembered cost is not a claim that the store still has the series"
    );

    // An unstated index — a segment whose vocabulary nobody could read — may hold anything, so it
    // drops nothing. That is the precision a lost index costs and the answer it does not.
    let mut cautious = Registry::new(Cap::new(2));
    for name in ["cpu,host=h0", "gone,host=h0"] {
        assert!(cautious.admit(&Series::of(name), 1, 8, EPOCH).is_admitted());
    }
    cautious.rebuild(&[SegmentIndex::unstated()]);
    assert_eq!(
        cautious.tracked_count(),
        2,
        "an index that cannot say what it holds is not evidence that the store forgot: {:?}",
        cautious
            .tracked()
            .map(|(name, _)| name)
            .collect::<Vec<&str>>()
    );

    // And a caller with no segments at all has made no claim either.
    let mut bare = Registry::new(Cap::new(2));
    assert!(
        bare.admit(&Series::of("cpu,host=h0"), 1, 8, EPOCH)
            .is_admitted()
    );
    bare.rebuild(&[]);
    assert_eq!(
        bare.tracked_count(),
        1,
        "a store that has closed no segment has nothing to check the ledger against, and a rebuild \
         that dropped everything for that reason would empty the cap on every fresh store"
    );
}

#[test]
fn the_window_is_the_extremes_the_index_states() {
    let mut ledger = Registry::new(Cap::new(1));
    // The log's records say this series happened at `EPOCH`; the store's own segment says it
    // happened an hour later, and the store's answer is the one the index states.
    assert!(
        ledger
            .admit(&Series::of("cpu,host=h0"), 2, 32, EPOCH)
            .is_admitted()
    );
    let later = EPOCH + 3_600_000;
    let segment = SegmentIndex::of(&records("cpu,host=h0", later, 3));
    ledger.rebuild(&[segment]);
    let cost = ledger
        .cost_of("cpu,host=h0")
        .unwrap_or_else(|| panic!("the series the index holds is tracked"));
    assert_eq!(
        cost.first_millis(),
        later,
        "the first millisecond is the earliest the index states"
    );
    assert_eq!(
        cost.last_millis(),
        later + 2 * STEP_MILLIS,
        "and the last one is the latest, both out of the zone map rather than out of the log"
    );
    assert_eq!(
        (cost.records(), cost.bytes()),
        (2, 32),
        "and what the series cost is what it was charged: an index states a vocabulary and a \
         window, not a bill"
    );

    // Two segments that both may hold it: the union of their windows.
    let mut both = Registry::new(Cap::new(2));
    assert!(
        both.admit(&Series::of("cpu,host=h0"), 1, 8, EPOCH)
            .is_admitted()
    );
    let early = SegmentIndex::of(&records("cpu,host=h0", EPOCH - 20 * STEP_MILLIS, 2));
    let late = SegmentIndex::of(&records("cpu,host=h0", EPOCH + 20 * STEP_MILLIS, 2));
    both.rebuild(&[early, late]);
    let cost = both
        .cost_of("cpu,host=h0")
        .unwrap_or_else(|| panic!("the series both segments may hold is tracked"));
    assert_eq!(cost.first_millis(), EPOCH - 20 * STEP_MILLIS);
    assert_eq!(cost.last_millis(), EPOCH + 21 * STEP_MILLIS);
}

#[test]
fn a_store_that_lost_its_indexes_loses_precision_and_no_answer() {
    let batches = [
        records("cpu,host=h0", EPOCH, 4),
        records("mem,host=h1", EPOCH + 60 * STEP_MILLIS, 4),
    ];
    let (dir, store) = sealed("lost-indexes", &batches);
    let before = live_indexes(dir.path());
    assert_eq!(before.len(), 2, "one index per segment the manifest names");

    // The ledger of a store whose segments hold these two series, pruned against the real indexes.
    let mut ledger = Registry::new(Cap::new(4));
    for name in ["cpu,host=h0", "mem,host=h1", "gone,host=h2"] {
        assert!(ledger.admit(&Series::of(name), 2, 16, EPOCH).is_admitted());
    }
    ledger.rebuild(&before);
    assert_eq!(
        ledger.tracked_count(),
        2,
        "the segments hold two of the three names, and the third is the store's to have forgotten"
    );

    // Now the loss 4.14 describes: deleting the index directory costs reads and nothing else.
    std::fs::remove_dir_all(dir.path().join(index::DIRECTORY)).unwrap_or_else(|_| {
        panic!(
            "{} cannot be removed — this case is about a store that lost its index",
            dir.path().join(index::DIRECTORY).display()
        )
    });
    let after = live_indexes(dir.path());
    assert_eq!(
        after.len(),
        2,
        "the manifest still names both segments: the store is what the manifest says it is, and an \
         index is a hint beside it"
    );
    assert!(
        after.iter().all(|index| index.may_hold("gone,host=h2")),
        "and each of them may hold anything now, which is what `unstated` means: the vocabulary \
         nobody can read proves nothing"
    );
    let mut cautious = Registry::new(Cap::new(4));
    for name in ["cpu,host=h0", "mem,host=h1", "gone,host=h2"] {
        assert!(
            cautious
                .admit(&Series::of(name), 2, 16, EPOCH)
                .is_admitted()
        );
    }
    cautious.rebuild(&after);
    assert_eq!(
        cautious.tracked_count(),
        3,
        "so the ledger keeps every name it cannot rule out: a lost index costs tracking precision \
         and never an answer"
    );

    // The answers themselves, which is the half "never an answer" is about: the store's records are
    // what they were, with and without an index beside them.
    let records = store
        .snapshot()
        .records()
        .unwrap_or_else(|error| panic!("the store's own records: {error}"));
    let expected: Vec<Record> = batches.concat();
    assert_eq!(
        records, expected,
        "a store that lost its index answers with the same records it answered with before: the \
         index is what saves a read, and the manifest and the segment files are the store"
    );
}

#[test]
fn the_ledger_of_a_store_is_the_series_its_records_name() {
    let dir = store_dir("of-store");
    let batches = [
        records("cpu,host=h0", EPOCH, 3),
        records("cpu,host=h0", EPOCH + 60 * STEP_MILLIS, 3),
        records("mem,host=h1", EPOCH + 120 * STEP_MILLIS, 2),
    ];
    // The write path took them (that is the log), and the segment store closed two of the three
    // into segments — which is exactly what a deployment that flushes does.
    {
        let mut spool = Spool::open(dir.path())
            .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
        for batch in &batches {
            spool.append(&payload(batch)).unwrap_or_else(|error| {
                panic!("the write path takes a payload of its own: {error}")
            });
        }
    }
    {
        let mut store = Segments::open(dir.path())
            .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
        for batch in &batches {
            store
                .seal(batch)
                .unwrap_or_else(|error| panic!("a segment of a case's own records: {error}"));
        }
    }

    let ledger = Registry::of_store(dir.path(), Cap::new(4))
        .unwrap_or_else(|error| panic!("the store this case built has a ledger: {error}"));
    let mut names: Vec<&str> = ledger.tracked().map(|(name, _)| name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["cpu,host=h0", "mem,host=h1"],
        "the ledger is the series this store's records name, and it is rebuilt rather than kept"
    );
    assert_eq!(
        ledger.cost_of("cpu,host=h0").map(Cost::records),
        Some(6),
        "with the records it holds of each one"
    );
    assert_eq!(
        ledger.cost_of("mem,host=h1").map(Cost::records),
        Some(2),
        "and of the other, whose segment it closed as well"
    );
    assert_eq!(
        ledger.offered(),
        0,
        "and none of it is an offer: what the cap decides is what a client may *add* to a store, and \
         a store that judged its own records would have a cap that depended on which way round it \
         was opened"
    );
    assert_eq!(
        ledger.tracked_count(),
        2,
        "two series are what this store carries"
    );

    // And the tool prints the same ledger: one rule, so the number a cap is applied to is the
    // number a person reads before they change it.
    let printed = lakeletq(&["inspect", "series", "--store", &argument(dir.path())]);
    assert_eq!(printed.code, 0, "the tool reads this store: {printed:?}");
    assert_eq!(
        printed.stdout.lines().count(),
        ledger.tracked_count() as usize,
        "the table has one line per tracked series: `{}`",
        printed.stdout
    );
    for (name, cost) in ledger.tracked() {
        let line = format!("{name} {} {}", cost.records(), cost.bytes());
        assert!(
            printed
                .stdout
                .lines()
                .any(|printed| printed.starts_with(&line)),
            "`{line}` is one of the lines the store's ledger rebuilt: `{}`",
            printed.stdout
        );
    }
}

#[test]
fn the_ingest_path_refuses_a_frame_for_a_new_series() {
    // A cap of one, through the real daemon: the deployment's number arrives as configuration and
    // the refusal is the store's, not the test's.
    let mut service = Service::start(
        "cardinality-refuse",
        &[Var::new("LAKELET_SERIES_LIMIT", "1")],
    );

    // The first series the store has never seen is taken.
    service.send(&wire(&records("cpu,host=h0", EPOCH, 4)));
    let taken = service.wait_for_line("connection closed");
    assert!(
        taken.contains("accepted=1") && taken.contains("rejected=0"),
        "a frame inside the cap is written and the connection says so: {taken}"
    );

    // The second is not, and the connection's own counters are where a client's operator sees it.
    service.send(&wire(&records("cpu,host=h9", EPOCH, 4)));
    let refusal = service.wait_for_line("frame refused by the series cap");
    assert!(
        refusal.contains("cpu,host=h9"),
        "the refusal names the series the client sent, which is the one thing an operator cannot \
         work out from a count: {refusal}"
    );
    assert!(
        refusal.contains("1 series") && refusal.contains("allows 1"),
        "and it names both numbers the decision was made from: {refusal}"
    );
    let dropped = service.wait_for_line("connection closed accepted=0 rejected=1");
    assert!(
        dropped.contains("bytes=0"),
        "the connection counted the frame as rejected and took nothing: {dropped}"
    );

    service.stop();
    assert_eq!(service.wait_for_exit(), 0, "the daemon stops cleanly");

    // What the store holds is the four records of the series it took, and nothing else.
    let spool = Spool::open(service.store().path())
        .unwrap_or_else(|error| panic!("the store the daemon wrote: {error}"));
    let held = spool
        .records()
        .unwrap_or_else(|error| panic!("the store's log is readable: {error}"));
    assert_eq!(
        held.len(),
        1,
        "one frame was written and the refused one was not"
    );
    let written = batch::decode(&held[0])
        .unwrap_or_else(|error| panic!("what the store holds is a batch: {error}"));
    assert!(
        written.iter().all(|record| record.series == "cpu,host=h0"),
        "and it holds the series the store took: {:?}",
        written
            .iter()
            .map(|record| record.series.as_str())
            .collect::<Vec<&str>>()
    );

    // The four numbers an operator reads out of the stop path.
    let ledger = service.wait_for_line("series ledger");
    for expected in [
        "limit=1",
        "tracked=1",
        "admitted=1",
        "rejected=1",
        "offered=2",
    ] {
        assert!(
            ledger.contains(expected),
            "the stop path reports `{expected}`: {ledger}"
        );
    }
}

#[test]
fn a_cap_survives_a_restart() {
    // The leak this step exists to prevent: a cap that counted only what *this process* admitted
    // would let a fresh limit of new names in on every restart, and a deployment whose process
    // manager restarts it would grow without bound — slowly, and with no counter saying so.
    let mut service = Service::start(
        "cardinality-restart",
        &[Var::new("LAKELET_SERIES_LIMIT", "1")],
    );
    service.send(&wire(&records("cpu,host=h0", EPOCH, 4)));
    service.wait_for_line("connection closed accepted=1");
    // A process that died and a directory that did not: everything the first daemon held in memory
    // is gone, so what answers afterwards is what the store's bytes hold.
    service.restart();

    service.send(&wire(&records("cpu,host=h9", EPOCH, 4)));
    let refusal = service.wait_for_line("frame refused by the series cap");
    assert!(
        refusal.contains("cpu,host=h9"),
        "after a restart the store still refuses the name it never took, because the ledger was \
         rebuilt from the records it holds: {refusal}"
    );
    service.stop();
    assert_eq!(service.wait_for_exit(), 0, "the daemon stops cleanly");
    let ledger = service.wait_for_line("series ledger");
    assert!(
        ledger.contains("tracked=1"),
        "and the series it took before the restart is still what it tracks: {ledger}"
    );
    assert!(
        ledger.contains("admitted=0"),
        "which is the point of the case: this process admitted nothing at all, so the series it \
         tracks is one it rebuilt from the store rather than one it counted from its own stdin: \
         {ledger}"
    );
    assert!(
        ledger.contains("rejected=1") && ledger.contains("offered=1"),
        "the one offer that reached it was refused, and the ledger says so: {ledger}"
    );
}

#[test]
fn the_series_table_is_one_line_per_series_and_the_same_objects_as_json() {
    let dir = store_dir("series-table");
    {
        let mut spool = Spool::open(dir.path())
            .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
        spool
            .append(&payload(&records(
                "mem,host=h1",
                EPOCH + 60 * STEP_MILLIS,
                2,
            )))
            .unwrap_or_else(|error| panic!("the write path takes a payload of its own: {error}"));
        spool
            .append(&payload(&records("cpu,host=h0", EPOCH, 3)))
            .unwrap_or_else(|error| panic!("the write path takes a payload of its own: {error}"));
    }
    let store = argument(dir.path());

    let lines = lakeletq(&["inspect", "series", "--store", &store]);
    assert_eq!(lines.code, 0, "the table is an answer: {lines:?}");
    assert_eq!(lines.stderr, "", "and not a diagnosis: {lines:?}");
    let rows: Vec<&str> = lines.stdout.lines().collect();
    assert_eq!(
        rows.len(),
        2,
        "one line per tracked series and no summary: `{}`",
        lines.stdout
    );
    assert!(
        rows[0].starts_with("cpu,host=h0 ") && rows[1].starts_with("mem,host=h1 "),
        "in name order, so that two runs of the tool can be diffed: {rows:?}"
    );
    let fields: Vec<&str> = rows[0].split(' ').collect();
    assert_eq!(
        fields.len(),
        5,
        "a line is the series, its records, its bytes, and the two ends of its window: {}",
        rows[0]
    );
    for field in &fields[1..] {
        assert!(
            field.parse::<u64>().is_ok(),
            "`{field}` is one of the four numbers and not a label: {}",
            rows[0]
        );
    }
    assert_eq!(fields[1], "3", "the records the store holds of it");
    assert_eq!(
        fields[3],
        EPOCH.to_string(),
        "and the window starts at the first record"
    );
    assert_eq!(
        fields[4],
        (EPOCH + 2 * STEP_MILLIS).to_string(),
        "and ends at the last one"
    );

    let document = lakeletq(&["inspect", "series", "--store", &store, "--json"]);
    assert_eq!(
        document.code, 0,
        "`--json` is a format and not a second command: {document:?}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&document.stdout)
        .unwrap_or_else(|error| panic!("the shape is JSON: {error}"));
    let objects = parsed
        .as_array()
        .unwrap_or_else(|| panic!("the answer is one array: {parsed}"));
    assert_eq!(
        objects.len(),
        2,
        "with the same objects the text form has lines: {parsed}"
    );
    assert_eq!(
        objects[0]["series"], "cpu,host=h0",
        "each one naming its series: {parsed}"
    );
    for key in ["records", "bytes", "first_millis", "last_millis"] {
        assert_eq!(
            objects[0][key].as_u64().map(|value| value.to_string()),
            Some(
                fields[if key == "records" {
                    1
                } else if key == "bytes" {
                    2
                } else if key == "first_millis" {
                    3
                } else {
                    4
                }]
                .to_owned()
            ),
            "`{key}` is the same number in both forms, which is the point of having two forms: {parsed}"
        );
    }
    assert_eq!(
        document.stdout.trim_end().matches('{').count(),
        2,
        "and one object per series: {}",
        document.stdout
    );

    // An empty store is an empty table in both forms — the boundary, and not a missing answer.
    let empty = store_dir("series-empty");
    let nothing = lakeletq(&["inspect", "series", "--store", &argument(empty.path())]);
    assert_eq!(
        (nothing.code, nothing.stdout.as_str()),
        (0, ""),
        "an empty table is no lines"
    );
    let empty_json = lakeletq(&[
        "inspect",
        "series",
        "--store",
        &argument(empty.path()),
        "--json",
    ]);
    assert_eq!(
        (empty_json.code, empty_json.stdout.trim_end()),
        (0, "[]"),
        "and `[]` in JSON, which is what a script iterating over it needs: {empty_json:?}"
    );

    // The four exits, from outside the library.
    refused(
        &lakeletq(&["inspect", "series"]),
        2,
        "a call that never says which store",
    );
    refused(
        &lakeletq(&["inspect", "series", "--store"]),
        2,
        "a flag with nothing after it",
    );
    refused(
        &lakeletq(&["inspect", "series", "--store", "/there/is/no/store/here"]),
        3,
        "a store that is not there",
    );
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file, b"nothing").unwrap_or_else(|error| panic!("{error}"));
    refused(
        &lakeletq(&["inspect", "series", "--store", &argument(&file)]),
        1,
        "a path that is not a directory",
    );
}

#[test]
fn the_cap_is_the_deployments_number_and_the_engine_reads_no_clock() {
    // The number a deployment reads back before it starts: `series.limit` is a setting like the
    // queue's, with the three layers `lakelet/4.6-ingest-backpressure` established.
    let dir = store_dir("cap-config");
    let reported = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletd"));
        command
            .args([
                "--check-config",
                "--store-dir",
                &argument(dir.path()),
                "--series-limit",
                "32",
            ])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().unwrap_or_else(|error| {
            panic!("`lakeletd --check-config` could not be started: {error}")
        });
        assert!(
            output.status.success(),
            "`--check-config` prints the settings and exits: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert!(
        reported.contains("series.limit = 32"),
        "the report carries the cap, because that is what a deployment diffs between two \
         machines:\n{reported}"
    );
    let default = {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletd"));
        command
            .args(["--check-config"])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().unwrap_or_else(|error| {
            panic!("`lakeletd --check-config` could not be started: {error}")
        });
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert!(
        default.contains(&format!("series.limit = {}", Cap::DEFAULT.limit())),
        "and the default it prints is the cap this build ships, so the number in the page and the \
         number in the daemon are one number:\n{default}"
    );

    // The engine's half: a ledger that read a clock or reached for the service layer would be a
    // ledger two runs of one workload disagree about, and the boundary this project keeps.
    for file in [
        "src/engine/cardinality/mod.rs",
        "src/engine/cardinality/cap.rs",
    ] {
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{file} is part of the project: {error}"));
        for forbidden in [
            "crate::service",
            "crate::cli",
            "Instant",
            "SystemTime",
            "tokio",
        ] {
            assert!(
                !source.contains(forbidden),
                "{file} names `{forbidden}`: the cap is a decision about records, the engine runs \
                 in a test and in a tool as well as in the daemon, and `now` is handed in by the \
                 caller so that a case can charge any moment it likes"
            );
        }
    }
    // And the module is reachable where the engine's own map says it is: one declaration, in the
    // block later steps add theirs to.
    let engine = std::fs::read_to_string("src/engine/mod.rs")
        .unwrap_or_else(|error| panic!("src/engine/mod.rs is part of the project: {error}"));
    assert!(
        engine.contains("pub mod cardinality;"),
        "the ledger is declared by the engine's own module map:\n{engine}"
    );
    assert!(
        cardinality::series_of(&payload(&records("cpu,host=h0", EPOCH, 1))).is_some(),
        "and the one helper the ingest path names a refused frame with reads the same payload the \
         cap judged"
    );
}
