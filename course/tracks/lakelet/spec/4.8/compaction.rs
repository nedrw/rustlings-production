//! Spec for `lakelet/4.8-compaction`: the policy is a value, a reader never sees a half-compacted
//! store, and every crash point of a merge leaves one of two worlds.
//!
//! `kiln run` copies this file into the project as `tests/spec_compaction.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Fourteen cases, and the shape of them is the point:
//!
//! * **the plan is data.** `Policy` and a list of segment sizes give a `Plan`, with no store and no
//!   side effect: the rule is the oldest run of `width` consecutive segments whose sizes are all
//!   within a factor of `ratio` of each other, a run that is too short or not comparable enough is
//!   left alone, and a plan never asks for one segment (that rewrites bytes and removes none);
//! * **a snapshot is what a reader holds.** The records a snapshot answers are the records the
//!   store accepted — once each, in order — before a merge, after a merge, and *across* one: a
//!   reader that took its snapshot before the swap goes on reading the segments it names, which are
//!   still on the disk until the last reader that named them lets go;
//! * **every crash point of a merge is one of two worlds.** Planned, written, durable, swapped and
//!   retired: at each of them the store, once reopened, holds either the set of segments it held
//!   before the merge or the set the merge would make — never a gap between them and never a record
//!   twice. The open is what turns a crash's leftovers into a store, and the store that comes back
//!   is the manifest's, not the directory's;
//! * **what is not named is not the store.** A segment whose file is not the length the manifest
//!   states is refused rather than read (answering without it would answer a hole), a file the
//!   manifest does not name is deleted, and a plan taken before another merge is refused instead of
//!   duplicating the records it names;
//! * **the boundary holds.** `src/engine/**` may not name the service layer, read out of the
//!   sources as text (`jobq/3.1-engine-boundary`), and the files this step adds are where a new
//!   violation would appear.
//!
//! Nothing here sleeps, and nothing here measures a duration.

use std::path::{Path, PathBuf};

use lakelet::Error;
use lakelet::engine::compaction::{
    DIRECTORY, MANIFEST, PENDING, Plan, Policy, SegmentInfo, Segments, Snapshot, file_of,
};
use lakelet::protocol::batch::Record;

/// Segments a case seals before it merges, unless it builds its own list.
const SEGMENTS: usize = 4;

/// Records one sealed segment holds.
const PER_SEGMENT: usize = 8;

/// The series names a case's records use.
const SERIES: [&str; 2] = ["cpu,host=h0", "cpu,host=h1"];

/// One record, as a client sends it.
fn record(series: &str, ts_millis: i64, value: f64) -> Record {
    Record {
        series: series.to_owned(),
        ts_millis,
        value,
    }
}

/// The records segment `index` holds: decided by the two numbers that name them, so a case can
/// rebuild the whole store's record list without reading the store it is judging.
fn batch(index: usize) -> Vec<Record> {
    (0..PER_SEGMENT)
        .map(|offset| {
            record(
                SERIES[(index + offset) % SERIES.len()],
                1_700_000_000_000 + (index * PER_SEGMENT + offset) as i64 * 1_000,
                (index * PER_SEGMENT + offset) as f64 / 4.0,
            )
        })
        .collect()
}

/// Every record a store of [`SEGMENTS`] sealed segments holds, in the order it accepted them.
fn sealed() -> Vec<Record> {
    (0..SEGMENTS).flat_map(batch).collect()
}

/// `true` when two record lists are the same record for record, the value compared by *bits*.
fn same(left: &[Record], right: &[Record]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.series == right.series
                && left.ts_millis == right.ts_millis
                && left.value.to_bits() == right.value.to_bits()
        })
}

/// One segment as a plan sees it: no file, just what the manifest would hold.
fn info(index: u64, bytes: u64, records: u64) -> SegmentInfo {
    SegmentInfo::new(index, bytes, records)
}

/// A store directory of this case's own, removed when it is dropped.
struct Dir {
    /// Where the store lives.
    path: PathBuf,
}

impl Dir {
    /// Claim a directory for `label`.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lakelet-compaction-spec-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("{} cannot be created: {error}", path.display()));
        Self { path }
    }

    /// Where the store lives.
    fn path(&self) -> &Path {
        &self.path
    }

    /// A file inside the store directory.
    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// The bytes of the store's manifest, as they are on the disk.
    fn manifest_bytes(&self) -> Vec<u8> {
        std::fs::read(self.file(MANIFEST)).unwrap_or_else(|error| {
            panic!(
                "{MANIFEST} cannot be read: {error} — a store that has sealed a segment has a \
                 manifest"
            )
        })
    }

    /// How many files the store's segment directory holds.
    fn segment_files(&self) -> usize {
        std::fs::read_dir(self.path.join(DIRECTORY))
            .unwrap_or_else(|error| panic!("{} cannot be listed: {error}", self.path.display()))
            .filter_map(std::result::Result::ok)
            .count()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A store in a directory of its own, with [`SEGMENTS`] sealed segments in it.
fn sealed_store(label: &str) -> (Dir, Segments) {
    let dir = Dir::new(label);
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    for index in 0..SEGMENTS {
        store.seal(&batch(index)).unwrap_or_else(|error| {
            panic!("a segment of {PER_SEGMENT} records is closed: {error}")
        });
    }
    (dir, store)
}

/// The records a store or a snapshot answers, or the sentence that says why it refused.
fn records_of(snapshot: &Snapshot) -> Vec<Record> {
    snapshot.records().unwrap_or_else(|error| {
        panic!("a snapshot of the segment set the manifest names is readable: {error}")
    })
}

/// The plan one policy makes over segment sizes, without a store.
fn plan_of(policy: Policy, sizes: &[u64]) -> Plan {
    let segments: Vec<SegmentInfo> = sizes
        .iter()
        .enumerate()
        .map(|(index, bytes)| info(index as u64, *bytes, 1))
        .collect();
    policy.plan(&segments)
}

#[test]
fn a_plan_takes_the_oldest_run_of_comparable_segments() {
    let equal = [4096u64; 8];
    let policy = Policy::new(4, 2);
    let plan = policy.plan(
        &equal
            .iter()
            .enumerate()
            .map(|(index, bytes)| info(index as u64, *bytes, 10))
            .collect::<Vec<SegmentInfo>>(),
    );
    assert_eq!(
        plan.segments(),
        [0, 1, 2, 3],
        "the plan takes the *oldest* `width` segments of the oldest run that qualifies: those are \
         the ones a reader has been opening longest, and a policy that took the youngest run would \
         leave the old ones to grow old forever"
    );
    assert_eq!(
        plan.len(),
        4,
        "the plan takes `width` segments, not a whole tier: a merge whose \
        cost followed the size of the tier would depend on how long the store had been running"
    );
    assert_eq!(
        plan.bytes(),
        4 * 4096,
        "the plan adds up what it would read"
    );
    assert_eq!(plan.records(), 40, "and what it would copy");
    // The same question asked twice is the same answer: a plan is data, not a decision with a
    // memory.
    assert_eq!(
        policy.plan(
            &equal
                .iter()
                .enumerate()
                .map(|(index, bytes)| info(index as u64, *bytes, 10))
                .collect::<Vec<SegmentInfo>>()
        ),
        plan,
        "asking a policy for a plan twice answers the same plan: what a plan is made from is the \
         sizes and counts in front of it, and nothing else"
    );

    // A run that starts later is the run that gets taken: a segment far larger than its neighbours
    // is not comparable with them, so the run begins after it.
    assert_eq!(
        plan_of(policy, &[40_960, 4096, 4096, 4096, 4096]).segments(),
        [1, 2, 3, 4],
        "a segment ten times its neighbours' size is not part of their run, so the plan begins at \
         the first segment that is"
    );
}

#[test]
fn a_run_that_is_too_short_or_not_comparable_ends_the_plan() {
    let policy = Policy::new(4, 2);
    assert!(
        plan_of(policy, &[4096, 4096, 4096]).is_empty(),
        "three comparable segments and a width of four is nothing to merge: `width` is how many \
         segments one merge takes, and a merge of fewer is the next call's business"
    );
    assert_eq!(
        plan_of(policy, &[4096, 4096, 4096, 4096]).len(),
        4,
        "exactly `width` comparable segments is a plan: the rule is `at least width`, and an \
         off-by-one here is a store that never merges its last run"
    );
    assert!(
        plan_of(policy, &[40_960, 40_960, 10]).is_empty(),
        "a run compares a newcomer with *both* ends of itself: 10 bytes is within a factor of two \
         of 40 960's largest but not of its smallest, and a rule that looked one way would merge a \
         tiny file into a large one — which is exactly the rewrite a size-tiered policy exists to \
         avoid"
    );
    assert!(
        plan_of(policy, &[4096, 4096, 10, 10]).is_empty(),
        "two comparable segments and a width of four is nothing to merge, however comparable the \
         four of them are one at a time"
    );
    assert_eq!(
        plan_of(Policy::new(2, 2), &[4096, 4096, 40_960, 40_960]).segments(),
        [0, 1],
        "a narrower policy takes the first run that qualifies, not the largest one"
    );
}

#[test]
fn a_plan_never_asks_for_one_segment_and_never_overflows() {
    for width in [0usize, 1, 2] {
        let policy = Policy::new(width, 1);
        let plan = plan_of(policy, &[4096, 4096, 4096]);
        assert!(
            plan.len() >= 2,
            "a policy with width {width} planned {} segment(s): merging one file into one file \
             costs bytes and removes none, so a merge of one is never a plan — whatever a caller \
             asked for",
            plan.len()
        );
    }
    let huge = [u64::MAX, u64::MAX - 1, u64::MAX, u64::MAX];
    let plan = plan_of(Policy::new(4, u64::MAX), &huge);
    assert_eq!(
        plan.len(),
        4,
        "sizes and ratios are numbers a caller can make large, and a comparison that multiplied \
         them in `u64` would overflow — a policy that panics on a large store is a policy a store \
         cannot run"
    );
}

#[test]
fn the_shipped_policy_merges_a_run_and_not_a_pair() {
    let policy = Policy::DEFAULT;
    let segments: Vec<SegmentInfo> = (0..8).map(|index| info(index, 4096, 10)).collect();
    let plan = policy.plan(&segments);
    assert!(
        plan.len() >= 3,
        "this build's policy merges {} segment(s) at a time (width {}): merging pairs as soon as \
         two vaguely similar segments exist makes the file count fall fast and rewrites the store \
         over and over to do it, which is the sentence \
         `bytes_rewritten_per_byte_removed_max` in the budget measures — the tempting value is not \
         the decision, and this is where the decision is a number",
        plan.len(),
        policy.width()
    );
    assert!(
        plan.len() <= segments.len(),
        "a plan cannot name more segments than the store has"
    );
    assert!(
        policy.ratio() >= 1,
        "a ratio of zero is a factor nothing can be within, including itself: two segments of the \
         same size are comparable, and this is the shortest way to say so"
    );
}

/// A refusal a caller's own argument deserves: an `Error::Invalid` that says what was wrong.
fn refused_as_invalid(result: Result<(), Error>, what: &str) -> String {
    match result {
        Ok(()) => panic!("{what} was accepted, and it is not something this store can do"),
        Err(Error::Invalid { detail }) => {
            assert!(
                !detail.trim().is_empty(),
                "{what} was refused with an empty sentence: the caller has to be able to read \
                 what it asked for that the store cannot do"
            );
            detail
        }
        Err(other) => panic!(
            "{what} was refused by `{other}`: a caller's own mistake is an `Error::Invalid` \
             (something the store was asked to do), and an `Error::Protocol` means the bytes on \
             the disk are not this store's — a caller branches on which of the two it got"
        ),
    }
}

/// A refusal a store's own bytes deserve: an `Error::Protocol` that says what disagreed.
fn refused_by(result: Result<(), Error>, what: &str) -> String {
    match result {
        Ok(()) => panic!(
            "{what} was accepted: a store reads what it wrote, and nothing else, so bytes that \
             are not its own are refused instead of answered"
        ),
        Err(Error::Protocol { detail }) => {
            assert!(
                !detail.trim().is_empty(),
                "{what} was refused with an empty sentence: the only thing an operator has is \
                 what the error says disagreed"
            );
            detail
        }
        Err(other) => panic!(
            "{what} was refused by `{other}`, and bytes that are not this store's are an \
             `Error::Protocol`: a caller branches on the variant to tell \"somebody changed my \
             files\" from \"the disk is gone\""
        ),
    }
}

#[test]
fn sealing_closes_a_segment_the_manifest_names() {
    let dir = Dir::new("seal");
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    assert!(
        store.manifest().is_empty(),
        "a store that has closed no segment holds none: the manifest is the store, and there is \
         nothing to name yet"
    );
    assert_eq!(
        store.manifest().epoch(),
        0,
        "and nothing has happened to it yet"
    );
    assert_eq!(
        records_of(&store.snapshot()),
        Vec::new(),
        "which a reader sees as an empty store"
    );

    let first = store
        .seal(&batch(0))
        .unwrap_or_else(|error| panic!("a segment of {PER_SEGMENT} records is closed: {error}"));
    let second = store
        .seal(&batch(1))
        .unwrap_or_else(|error| panic!("a second segment is closed: {error}"));
    assert_ne!(
        first, second,
        "every closed segment gets an ordinal of its own"
    );
    let named: Vec<u64> = store
        .manifest()
        .segments()
        .iter()
        .map(|info| info.index)
        .collect();
    assert_eq!(
        named,
        vec![first, second],
        "the manifest lists the segments in the order their records were accepted, which is the \
         order a reader gets them back in"
    );
    assert_eq!(
        store.manifest().segments()[0].records,
        PER_SEGMENT as u64,
        "the manifest states how many records each segment holds: that number is what a plan and a \
         reader work from without opening a file"
    );
    let path = file_of(dir.path(), first);
    assert_eq!(
        std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        store.manifest().segments()[0].bytes,
        "the length the manifest states is the length of the file it names: an index that \
         disagrees with its files is a store nobody can answer from"
    );
    assert!(
        same(
            &records_of(&store.snapshot()),
            &[batch(0), batch(1)].concat()
        ),
        "a reader of the store answers what its two segments hold, in the order the manifest \
         lists them"
    );
    assert_eq!(
        store.manifest().records(),
        2 * PER_SEGMENT as u64,
        "and the store knows how many records it holds without reading a single file"
    );
    assert_eq!(
        store.manifest().epoch(),
        2,
        "closing a segment changes what the store is, and the epoch is how a reader names that \
         change"
    );

    let refused = store.seal(&[]);
    assert!(
        refused.is_err(),
        "closing nothing is not a segment: a file holding no records is one a plan would carry for \
         ever, and a store that made it would pay for a name nobody asked for"
    );
    assert_eq!(
        store.manifest().epoch(),
        2,
        "and the refusal left the store exactly as it was"
    );
    assert_eq!(dir.segment_files(), 2, "with two files and no third");
}

#[test]
fn a_merge_keeps_every_record_once_in_the_same_order() {
    let (_dir, mut store) = sealed_store("order");
    let accepted = sealed();
    let before = store.snapshot();
    assert!(
        same(&records_of(&before), &accepted),
        "before any merge the store answers the records it accepted — every one, once, in order"
    );
    let plan = store.plan();
    assert_eq!(
        plan.len(),
        SEGMENTS,
        "the store's own policy plans over its own segments: {SEGMENTS} segments of the same size \
         are one run"
    );
    let report = store
        .compact()
        .unwrap_or_else(|error| panic!("a plan that is not empty is a merge: {error}"))
        .unwrap_or_else(|| panic!("a plan that is not empty is a merge, and this one was empty"));
    assert!(
        same(&records_of(&store.snapshot()), &accepted),
        "after the merge the store answers exactly the same records, once each, in the same order: \
         a merge that dropped one, or that wrote one twice, is not a merge of a store whose \
         records are its content"
    );
    assert_eq!(
        report.records(),
        accepted.len() as u64,
        "and the report accounts for every one of them"
    );
    assert_eq!(
        store.manifest().records(),
        accepted.len() as u64,
        "the manifest's count agrees with what the store hands back, which is the number a caller \
         trusts before it reads"
    );
    assert_eq!(
        store.manifest().segments().len(),
        1,
        "four segments became one file"
    );
    assert_eq!(
        store.manifest().epoch(),
        before.epoch() + 1,
        "the store changed shape exactly once, and a merge is visible in exactly one step: an \
         epoch per stage would be a reader watching a store that is neither of its two states"
    );
    assert!(
        same(&records_of(&before), &accepted),
        "and the reader that took its snapshot before the merge answers the same thing it would \
         have answered then"
    );
}

#[test]
fn the_file_count_strictly_drops_and_the_merge_writes_less_than_it_took() {
    let (dir, mut store) = sealed_store("cost");
    let before = store.manifest().segments().len();
    let report = store
        .compact()
        .unwrap_or_else(|error| panic!("a store of comparable segments has a plan: {error}"))
        .unwrap_or_else(|| panic!("a store of comparable segments has a plan"));
    assert_eq!(
        report.segments_before(),
        before,
        "the report says what the store was"
    );
    assert!(
        report.segments_after() < report.segments_before(),
        "a merge leaves fewer files than it found ({} became {}): the number of files a reader \
         opens is the cost this step exists to bring down, so a merge that did not drop it is work \
         nobody asked for",
        report.segments_before(),
        report.segments_after()
    );
    assert_eq!(
        report.files_removed(),
        report.segments_before() - report.segments_after(),
        "and how many it removed is a count, not an estimate"
    );
    assert_eq!(
        report.merged().len(),
        report.segments_before(),
        "the merge took every segment the store had, and left one file behind it"
    );
    assert!(
        report.bytes_out() < report.bytes_in(),
        "the file that replaces the run is smaller than the run ({} byte(s) became {}): the \
         columns of one block share a dictionary, a frame and a header that {} blocks each paid \
         for — and this is the relation the budget lane's bound is written from",
        report.bytes_in(),
        report.bytes_out(),
        report.merged().len()
    );
    assert_eq!(
        report.bytes_removed(),
        report.bytes_in() - report.bytes_out(),
        "bytes removed is what went in less what came out, and never a negative number wearing a \
         `u64`"
    );
    assert_eq!(
        store.manifest().bytes(),
        report.bytes_out(),
        "the store's own index agrees: what it holds is the file the merge wrote"
    );
    assert_eq!(
        dir.segment_files(),
        report.segments_after(),
        "and the files the merge replaced are gone with it: a store that kept them would be a \
         directory that grows with every merge"
    );
}

#[test]
fn a_reader_that_took_a_snapshot_before_the_swap_answers_the_same_after_it() {
    let (dir, mut store) = sealed_store("snapshot");
    let accepted = sealed();
    let reader = store.snapshot();
    let plan = store.plan();
    let ran = store
        .merge(&plan)
        .unwrap_or_else(|error| panic!("a store of comparable segments merges: {error}"));
    let retired: Vec<PathBuf> = ran
        .taken()
        .iter()
        .map(|index| file_of(dir.path(), *index))
        .collect();
    store
        .install(ran)
        .unwrap_or_else(|error| panic!("a merge this store wrote is installed: {error}"));
    assert!(
        reader.epoch() < store.manifest().epoch(),
        "the snapshot names the store it was taken from, and the store has moved on: the epoch is \
         how the two are told apart"
    );
    assert!(
        retired.iter().all(|path| path.exists()),
        "the segments a merge replaced are still on the disk while a reader names them: deleting \
         them at the swap would hand a reader that began before it whatever the operating system \
         does to a file nobody names"
    );
    assert!(
        same(&records_of(&reader), &accepted),
        "so a read that began before the swap answers exactly what a read that ran entirely before \
         it answered — the same records, once each, in the same order"
    );
    assert!(
        same(&records_of(&store.snapshot()), &accepted),
        "and a read that began after it answers the same thing, which is the other half of the \
         claim: the store is one store before the swap and after it"
    );
    drop(reader);
    assert!(
        retired.iter().all(|path| !path.exists()),
        "once the last reader that named the old set lets go, its files go: that is the one place \
         a merge deletes anything, and the reason a snapshot holds a set rather than a list"
    );
    assert!(
        same(&records_of(&store.snapshot()), &accepted),
        "and the store goes on answering after them"
    );
}

/// The stages of a merge this case kills a store at.
const STAGES: [&str; 5] = ["planned", "written", "durable", "swapped", "retired"];

#[test]
fn a_crash_at_each_stage_leaves_one_of_the_two_worlds() {
    let accepted = sealed();
    for stage in STAGES {
        let dir = Dir::new(&format!("crash-{stage}"));
        let mut store = Segments::open(dir.path()).unwrap_or_else(|error| {
            panic!("stage {stage}: a directory this process owns is a store: {error}")
        });
        for index in 0..SEGMENTS {
            store
                .seal(&batch(index))
                .unwrap_or_else(|error| panic!("stage {stage}: a segment is closed: {error}"));
        }
        let before_epoch = store.manifest().epoch();
        let before_files = store.manifest().segments().len();
        let plan = store.plan();
        let reader = store.snapshot();
        // A process death does not run destructors, so the two stages whose disk state depends on a
        // reader still holding the old set leak their snapshot instead of dropping it: that is what
        // leaves the replaced files for the next open to find.
        let lingering = matches!(stage, "planned" | "swapped");

        if stage != "planned" {
            let ran = store.merge(&plan).unwrap_or_else(|error| {
                panic!("stage {stage}: the plan's segments are there: {error}")
            });
            if stage == "written" {
                // The device committed part of the write and not the rest, which is the disk a kill
                // *inside* a write leaves. The file is an orphan either way, and the point of this
                // stage is that a store never reads a file its manifest does not name.
                let cut = std::fs::metadata(ran.file())
                    .unwrap_or_else(|error| {
                        panic!("stage {stage}: the new segment is a file: {error}")
                    })
                    .len()
                    / 2;
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(ran.file())
                    .unwrap_or_else(|error| {
                        panic!("stage {stage}: the new segment opens: {error}")
                    });
                file.set_len(cut)
                    .unwrap_or_else(|error| panic!("stage {stage}: the write is cut: {error}"));
                file.sync_all().unwrap_or_else(|error| {
                    panic!("stage {stage}: the cut is on the device: {error}")
                });
            }
            if matches!(stage, "swapped" | "retired") {
                store.install(ran).unwrap_or_else(|error| {
                    panic!("stage {stage}: a written merge installs: {error}")
                });
                assert!(
                    same(&records_of(&store.snapshot()), &accepted),
                    "stage {stage}: the world the swap installed is a whole store — the records it \
                     answers are the records that were accepted, once each"
                );
            }
            // A crash before the swap is *not* modelled by putting the manifest back: what a death
            // takes is the part of a write the device never committed, and the manifest is exactly
            // what these stages say the merge has not touched yet. A merge that made itself visible
            // while it wrote its segment therefore dies here rather than being tidied up by this
            // case — the world asserted below is the one the stage leaves, not the one the code
            // would have preferred.
        }
        if lingering {
            std::mem::forget(reader);
        } else {
            drop(reader);
        }
        drop(store);

        let reopened = Segments::open(dir.path()).unwrap_or_else(|error| {
            panic!(
                "stage {stage}: a store whose merge was interrupted has to open, because a store \
                 that refuses to start after a crash loses the data *and* the service: {error}"
            )
        });
        let answered = records_of(&reopened.snapshot());
        assert!(
            same(&answered, &accepted),
            "stage {stage}: every acknowledged record is in the store once, in order, and the store \
             answered {} record(s) of {}: a merge may not lose one, and may not hand one back twice",
            answered.len(),
            accepted.len()
        );
        assert_eq!(
            reopened.manifest().records(),
            accepted.len() as u64,
            "stage {stage}: the index counts what a reader gets back"
        );
        let new_world = matches!(stage, "swapped" | "retired");
        let files = if new_world {
            before_files - plan.len() + 1
        } else {
            before_files
        };
        let epoch = if new_world {
            before_epoch + 1
        } else {
            before_epoch
        };
        assert_eq!(
            reopened.manifest().segments().len(),
            files,
            "stage {stage}: a crash in the middle of a merge leaves the old set of segments or the \
             new one, and never a mixture of the two"
        );
        assert_eq!(
            reopened.manifest().epoch(),
            epoch,
            "stage {stage}: and the epoch says which of the two worlds this is"
        );
        assert_eq!(
            dir.segment_files(),
            files,
            "stage {stage}: opening a store is what finishes the merge a crash interrupted — the \
             segment nobody installed, the segments a merge replaced and the index a swap did not \
             finish are all files the manifest does not name, so the directory holds the store and \
             nothing else"
        );
    }
}

#[test]
fn a_store_whose_named_segment_is_damaged_refuses_rather_than_answer_a_shorter_one() {
    let (dir, store) = sealed_store("damaged");
    let accepted = sealed();
    let named = store.manifest().segments()[1];
    let path = file_of(dir.path(), named.index);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("a segment this store wrote is readable: {error}"));

    // A device that lost the tail of a block: the file is shorter than the index says.
    std::fs::write(&path, &bytes[..bytes.len() - 1])
        .unwrap_or_else(|error| panic!("the segment is cut: {error}"));
    let refused = refused_by(
        Segments::open(dir.path()).map(|_| ()),
        "a store whose named segment is shorter than its index",
    );
    assert!(
        refused.contains(&named.index.to_string()) || refused.contains(MANIFEST),
        "the refusal names what disagreed, and what disagreed here is segment {} or the index \
         that states its length: {refused}",
        named.index
    );
    std::fs::write(&path, &bytes)
        .unwrap_or_else(|error| panic!("the segment is restored: {error}"));
    let reopened = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("the store comes back when its bytes do: {error}"));
    assert!(
        same(&records_of(&reopened.snapshot()), &accepted),
        "and it is the store it was"
    );

    // A device that wrote something else: the length is right and one byte is not.
    let mut changed = bytes.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 0x01;
    std::fs::write(&path, &changed)
        .unwrap_or_else(|error| panic!("the segment is changed: {error}"));
    Segments::open(dir.path()).unwrap_or_else(|error| {
        panic!(
            "a segment of the right length and the wrong bytes opens: the index states a length, \
             and the bytes themselves are proved by the segment's own seal when they are read: {error}"
        )
    });
    let answer = reopened.snapshot().records();
    refused_by(
        answer.map(|_| ()),
        "a read of a segment whose bytes changed after it was written",
    );
    std::fs::write(&path, &bytes)
        .unwrap_or_else(|error| panic!("the segment is restored: {error}"));

    // And the index itself: a manifest cut in half is not a store with fewer segments.
    let manifest = dir.manifest_bytes();
    std::fs::write(dir.file(MANIFEST), &manifest[..manifest.len() / 2])
        .unwrap_or_else(|error| panic!("the index is cut: {error}"));
    refused_by(
        Segments::open(dir.path()).map(|_| ()),
        "a store whose index was cut in half",
    );
    std::fs::write(dir.file(MANIFEST), &manifest)
        .unwrap_or_else(|error| panic!("the index is restored: {error}"));
    let reopened = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("the store comes back when its index does: {error}"));
    assert!(
        same(&records_of(&reopened.snapshot()), &accepted),
        "a cut index is refused rather than read as a store with fewer segments, which is what \
         would answer a hole in every query from then on"
    );
}

#[test]
fn a_store_ignores_what_its_manifest_does_not_name() {
    let (dir, store) = sealed_store("orphans");
    let accepted = sealed();
    // A whole, valid segment a merge wrote and never installed.
    let orphan = file_of(dir.path(), 4242);
    std::fs::copy(file_of(dir.path(), 0), &orphan)
        .unwrap_or_else(|error| panic!("a copy of a segment is written: {error}"));
    // The manifest an interrupted swap left under the name it is written to.
    std::fs::write(dir.file(PENDING), b"half a manifest")
        .unwrap_or_else(|error| panic!("a pending index is written: {error}"));
    // And a file that is not a segment at all, in the segment directory.
    let stranger = dir.path().join(DIRECTORY).join("notes.txt");
    std::fs::write(&stranger, b"not a segment")
        .unwrap_or_else(|error| panic!("a stranger file is written: {error}"));
    drop(store);

    let reopened = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("an open deals with what a crash left: {error}"));
    assert!(
        !orphan.exists(),
        "a segment the manifest does not name is not the store: deleting it is what makes the \
         leftover of every merge stage the same kind of thing, and a store that kept it would pay \
         for a file nobody can read"
    );
    assert!(
        !dir.file(PENDING).exists(),
        "and neither is the index a swap did not finish"
    );
    assert!(
        !stranger.exists(),
        "and neither is a file that is not this store's at all"
    );
    assert_eq!(
        dir.segment_files(),
        reopened.manifest().segments().len(),
        "so the segment directory holds exactly the segments the manifest names"
    );
    assert!(
        same(&records_of(&reopened.snapshot()), &accepted),
        "and the store is the store it was"
    );
}

#[test]
fn a_plan_taken_before_another_merge_is_refused() {
    let (dir, mut store) = sealed_store("stale");
    let accepted = sealed();
    let plan = store.plan();
    assert!(
        !plan.is_empty(),
        "a store of comparable segments has a plan"
    );
    let ran = store
        .merge(&plan)
        .unwrap_or_else(|error| panic!("the plan's segments are all there: {error}"));
    refused_as_invalid(
        store.merge(&plan).map(|_| ()),
        "a second merge while one is written and not installed",
    );
    store
        .install(ran.clone())
        .unwrap_or_else(|error| panic!("a written merge installs: {error}"));
    refused_as_invalid(
        store.install(ran).map(|_| ()),
        "installing the same merge twice",
    );

    let stale = store.merge(&plan);
    let detail = refused_as_invalid(
        stale.map(|_| ()),
        "a plan whose segments another merge already took",
    );
    assert!(
        !detail.is_empty(),
        "and the refusal says which plan it was: a planner has to ask again, and a caller that \
         cannot see why would merge twice"
    );
    assert!(
        same(&records_of(&store.snapshot()), &accepted),
        "which is the point: a store that carried out a stale plan would write a segment holding \
         records it had already merged, and every record in it would be answered twice"
    );
    assert_eq!(
        dir.segment_files(),
        store.manifest().segments().len(),
        "with no extra file left behind by either refusal"
    );
}

#[test]
fn the_module_documents_the_stages_of_a_merge() {
    let source = std::fs::read_to_string("src/engine/compaction/mod.rs").unwrap_or_else(|error| {
        panic!(
            "src/engine/compaction/mod.rs cannot be read: {error} — this is a structural check, \
             and it is here because the stages of a merge are what an operator reads before a \
             repair: the code, the step's page and the crash map have to spell them the same way, \
             and this is the half a test can hold"
        )
    });
    let mut at = 0usize;
    for stage in STAGES {
        let found = source[at..].find(stage).unwrap_or_else(|| {
            panic!(
                "the module never names the `{stage}` stage of a merge, in order: the five stages \
                 are `planned`, `written`, `durable`, `swapped` and `retired`, and a store whose \
                 code stopped saying which one it is at has stopped saying what a crash there \
                 leaves"
            )
        });
        at += found + stage.len();
    }
    for name in ["Policy", "Plan", "Snapshot", "Manifest"] {
        assert!(
            source.contains(name),
            "the module does not name `{name}`: the plan, the store's index and what a reader \
             holds are what this step is made of, and a reader of the code has to find them"
        );
    }
    assert!(
        !source.contains("crate::service"),
        "the module names the service layer, which the next case and `lakelet/4.1-ingest-protocol`'s \
         own check both refuse"
    );
}

#[test]
fn the_compaction_sources_do_not_reach_into_the_service() {
    let engine = PathBuf::from("src/engine/compaction");
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&engine, &mut files).expect("the compaction module is part of the project");
    assert!(
        files.len() >= 3,
        "{} holds {} Rust file(s): the plan, the store and the snapshot are what this step \
         delivers, and a store whose merge lives in one file the size of a chapter is a store \
         nobody can review",
        engine.display(),
        files.len()
    );
    for file in &files {
        let source = std::fs::read_to_string(file).expect("a source file the project keeps");
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !source.contains(forbidden),
                "{} names `{forbidden}` — the daemon uses the engine, never the other way, and this \
                 module is new code the rule applies to (`lakelet/4.1-ingest-protocol` reads the \
                 same sources for it)",
                file.display()
            );
        }
    }
}

/// Every `.rs` file below `directory`, for the checks that read the sources as text.
fn collect_rust(directory: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    Ok(())
}
