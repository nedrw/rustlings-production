//! Spec for `lakelet/4.9-retention`: old data leaves in whole buckets, nothing newer than the
//! cutoff is touched, and a store that has forgotten something keeps saying so.
//!
//! `kiln run` copies this file into the project as `tests/spec_retention.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Fifteen cases, and the shape of them is the point:
//!
//! * **the bucket arithmetic is pure.** A `Partition` is made from a timestamp and a width and
//!   answers with the bucket it belongs to — half-open, `[start, end)`, a boundary timestamp in the
//!   bucket that starts on it — and `Policy::cutoff` is `now - keep` snapped down through it, so a
//!   pass removes whole buckets and the boundary is the same number on every machine. A policy
//!   that keeps everything answers `0` whatever the clock says;
//! * **a plan is data.** A list of segments and a cutoff give expired, retained and straddling,
//!   with the manifest's order and the manifest's values preserved: `last < cutoff` expires,
//!   `last == cutoff` is kept, and a retained segment whose `first < cutoff` straddles. A segment
//!   whose age the index does not state — a value built by hand for a decision about size — is
//!   kept and is not straddling: `None` is an unstated age, not an old one;
//! * **a pass that expires nothing does not move the store.** Empty plan, same epoch, byte for byte
//!   the same index, no file touched — a background task that ran every minute would otherwise
//!   rewrite the index every minute to say what it already said;
//! * **a pass that expires something accounts for it.** `segments_before - expired ==
//!   segments_after`, the bytes the report claims are the bytes the disk gave back, the retained
//!   segments are byte-identical files with the manifest's own values, and the expired ones are
//!   gone;
//! * **what the index stopped naming is not an answer.** The records a read returns are the
//!   records at or after the cutoff and nothing older — while a straddler's old records stay on
//!   the disk, are not rewritten, and are still counted by `len()`;
//! * **a reader that began before the pass keeps its view.** The snapshot it took answers what it
//!   would have answered, and the files the pass expired are on the disk for exactly as long as
//!   that reader names them;
//! * **the floor is durable and one-way.** It survives a reopen (with the orphan a crash leaves
//!   deleted rather than resurrected), and a later pass with an earlier cutoff writes nothing at
//!   all — a promise that data is gone cannot be taken back;
//! * **a merge keeps the other half of the index true.** The segment a merge writes states the
//!   union of its inputs' ranges, from the manifest alone, and an expiry that follows drops what
//!   no longer straddles;
//! * **the boundary holds.** `src/engine/**` may not name the service layer, read out of the
//!   sources as text (`jobq/3.1-engine-boundary`), and the files this step adds are where a new
//!   violation would appear. The index's version byte is what refuses an older index instead of
//!   misreading it.
//!
//! Nothing here sleeps, nothing here reads a clock, and nothing here measures a duration: `now` is
//! a number every case passes in.

use std::path::{Path, PathBuf};

use lakelet::Error;
use lakelet::engine::compaction::{DIRECTORY, MANIFEST, SegmentInfo, Segments, file_of};
use lakelet::engine::retention::partition::Partition;
use lakelet::engine::retention::{ExpireReport, Policy};
use lakelet::protocol::batch::Record;

/// The bucket width every case counts in, unless it builds a policy of its own.
const HOUR: u64 = 3_600_000;

/// Milliseconds between two records of one segment: a sample a minute.
const STEP: i64 = 60_000;

/// The first timestamp of the first segment, on a bucket boundary so the arithmetic reads.
///
/// `1_699_999_200_000` is `472_222 * HOUR`: every segment therefore *starts* on a boundary and runs
/// ninety minutes past it, which is what makes a segment straddle a cutoff that is one.
const EPOCH: i64 = 472_222 * HOUR as i64;

/// Segments a case seals, unless it builds its own list.
const SEGMENTS: usize = 8;

/// Records one sealed segment holds: ninety minutes of one series, a sample a minute.
const PER_SEGMENT: usize = 90;

/// What the cases' policy keeps: six and a half hours.
///
/// Deliberately not a whole number of buckets — the cutoff is snapped *down* to a boundary, and a
/// window that was already on one would not exercise the snap at all.
const KEEP: u64 = 6 * HOUR + 30 * 60_000;

/// The `now` every case passes in: ten hours after [`EPOCH`], so the whole store is in the past.
const NOW: u64 = 472_222 * HOUR + 10 * HOUR;

/// The cutoff [`KEEP`] has at [`NOW`]: three and a half hours after the epoch, snapped down to the
/// third hour. Two of the eight segments are below it and the third straddles it.
const CUTOFF: u64 = 472_222 * HOUR + 3 * HOUR;

/// The policy the cases use: keep [`KEEP`], count in buckets of an hour.
fn policy() -> Policy {
    Policy::new(KEEP, HOUR)
}

/// One record, as a client sends it.
fn record(index: usize, offset: usize) -> Record {
    Record {
        series: "cpu,host=h0".to_owned(),
        ts_millis: EPOCH + index as i64 * HOUR as i64 + offset as i64 * STEP,
        value: offset as f64 / 4.0,
    }
}

/// The records segment `index` holds: decided by the two numbers that name it, so a case can
/// rebuild the whole store's record list without reading the store it is judging.
fn batch(index: usize) -> Vec<Record> {
    (0..PER_SEGMENT)
        .map(|offset| record(index, offset))
        .collect()
}

/// Every record a store of [`SEGMENTS`] sealed segments holds, in the order it accepted them.
fn sealed() -> Vec<Record> {
    (0..SEGMENTS).flat_map(batch).collect()
}

/// The records a read of that store answers once [`policy`] has run at [`NOW`]: every record at or
/// after [`CUTOFF`], in the order the store kept them.
fn still_an_answer() -> Vec<Record> {
    sealed()
        .into_iter()
        .filter(|record| record.ts_millis >= CUTOFF as i64)
        .collect()
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

/// One segment as a plan sees it, with the range of timestamps the store stated for it.
fn spanned(
    index: u64,
    bytes: u64,
    records: u64,
    first_millis: u64,
    last_millis: u64,
) -> SegmentInfo {
    SegmentInfo::spanning(index, bytes, records, first_millis, last_millis)
}

/// One segment as a plan sees it whose age nobody stated: what a caller builds by hand for a
/// decision about *size*, which is what `lakelet/4.8-compaction`'s plans are made from.
fn unstated(index: u64, bytes: u64, records: u64) -> SegmentInfo {
    SegmentInfo::new(index, bytes, records)
}

/// The indices of a segment list, in order.
fn indices(segments: &[SegmentInfo]) -> Vec<u64> {
    segments.iter().map(|info| info.index).collect()
}

/// What a segment list costs together, as the manifest states it.
fn bytes_of(segments: &[SegmentInfo]) -> u64 {
    segments.iter().map(|info| info.bytes).sum()
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
            "lakelet-retention-spec-{label}-{}",
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

    /// What one segment's file holds, or nothing when it is not on the disk.
    fn segment_bytes(&self, index: u64) -> Option<Vec<u8>> {
        std::fs::read(file_of(&self.path, index)).ok()
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
fn records_of(snapshot: &lakelet::engine::compaction::Snapshot) -> Vec<Record> {
    snapshot.records().unwrap_or_else(|error| {
        panic!("a snapshot of the segment set the manifest names is readable: {error}")
    })
}

/// A pass this store was asked to carry out and refused, with the sentence it refused in.
fn refused_by(result: Result<(), Error>, what: &str) -> String {
    match result {
        Ok(()) => panic!(
            "{what} was accepted: this store reads what it wrote, and nothing else, so an index \
             that is not its own is refused instead of acted on"
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

/// The pass a case carries out, or the panic that says why a store of this shape must do it.
fn expire(store: &mut Segments, policy: &Policy, now_millis: u64) -> ExpireReport {
    store.expire(policy, now_millis).unwrap_or_else(|error| {
        panic!("a store whose segments all have a time range carries a pass out: {error}")
    })
}

#[test]
fn a_timestamp_belongs_to_the_bucket_that_starts_at_it() {
    let hour = Partition::of(0, HOUR);
    assert_eq!(hour.start(), 0, "the first bucket starts at the epoch");
    assert_eq!(hour.end(), HOUR, "and ends one width later");
    assert_eq!(hour.width(), HOUR, "which is the width it was made with");
    assert_eq!(hour.index(), 0, "counted from the epoch: `start / width`");
    assert!(
        hour.contains(0) && hour.contains(HOUR - 1),
        "a bucket is half-open `[start, end)`: its first millisecond is in it and its last \
         millisecond before `end` is too"
    );
    assert!(
        !hour.contains(HOUR),
        "and the boundary is not: a timestamp exactly on a boundary belongs to the bucket that \
         starts there, which is what keeps two neighbours from both holding it"
    );

    let third = Partition::of(CUTOFF, HOUR);
    assert_eq!(
        third.start(),
        CUTOFF,
        "a timestamp that is already on a boundary is its own bucket's start"
    );
    assert_eq!(
        Partition::of(CUTOFF + 1, HOUR).start(),
        CUTOFF,
        "and one millisecond past a boundary is in the bucket below it: a cutoff is snapped *down*, \
         never up, so a pass never deletes a bucket the policy did not name"
    );
    assert_eq!(
        third.index(),
        472_225,
        "which bucket that is, counted from the epoch"
    );

    let odd = Partition::of(1_000, 7 * 60_000);
    assert_eq!(
        odd.start(),
        0,
        "a width that does not divide the timestamp leaves the remainder in the bucket above"
    );
    assert_eq!(odd.end(), 7 * 60_000, "and the bucket is `[0, 7 minutes)`");
    assert!(
        odd.contains(1_000) && odd.contains(419_999) && !odd.contains(420_000),
        "which is half-open at both ends of the remainder"
    );

    let single = Partition::of(86_399_999, 1);
    assert_eq!(
        single.width(),
        1,
        "a width of one millisecond is a bucket per millisecond"
    );
    assert_eq!(
        single.start(),
        86_399_999,
        "which holds exactly the timestamp it was made from"
    );
    assert_eq!(single.end(), 86_400_000, "and ends one millisecond later");
    assert!(single.contains(86_399_999) && !single.contains(86_400_000));

    let zero = Partition::of(41, 0);
    assert_eq!(
        zero.width(),
        1,
        "a width of zero is treated as one: a bucket that covered no time would hold no timestamp \
         at all, including the one it was made from"
    );
    assert_eq!(
        zero.start(),
        41,
        "so the timestamp is in a bucket of its own"
    );
    assert!(
        zero.contains(41) && !zero.contains(42),
        "and that bucket holds exactly one millisecond"
    );

    assert_eq!(
        Partition::of(CUTOFF, HOUR).to_string(),
        format!("[{CUTOFF}, {})", CUTOFF + HOUR),
        "a partition prints as the half-open range it is: an operator reading a cutoff in a log \
         line has to see which bucket it is, and `[start, end)` says it without a note"
    );
}

#[test]
fn a_cutoff_is_a_bucket_boundary_and_zero_keeps_everything() {
    let policy = policy();
    assert_eq!(
        policy.keep_millis(),
        KEEP,
        "a policy states how much time it keeps"
    );
    assert_eq!(
        policy.width_millis(),
        HOUR,
        "and how wide a bucket of it is"
    );
    assert!(
        !policy.keeps_everything(),
        "a policy that keeps six and a half hours does not keep everything"
    );
    assert_eq!(
        policy.cutoff(NOW),
        CUTOFF,
        "the cutoff is `now - keep` snapped down to the hour below it: three and a half hours past \
         the epoch is the third hour"
    );
    assert_eq!(
        policy.cutoff(NOW) % HOUR,
        0,
        "so the cutoff is a multiple of the width — the same number on every machine and in every \
         timezone, whatever the clock said a millisecond ago"
    );
    assert_eq!(
        Policy::new(3 * HOUR, HOUR).cutoff(NOW),
        472_222 * HOUR + 7 * HOUR,
        "a window that is already a whole number of buckets lands on a boundary and is not snapped \
         any further"
    );

    for now in [0u64, 1, NOW, u64::MAX] {
        assert_eq!(
            Policy::DEFAULT.cutoff(now),
            0,
            "a policy that keeps everything answers a cutoff of zero at `now = {now}`, which is \
             below every timestamp a store can hold — an unconfigured store deletes nothing"
        );
        assert!(
            Policy::DEFAULT.keeps_everything(),
            "which is what `keeps_everything` says"
        );
    }
    assert_eq!(
        Policy::DEFAULT.width_millis(),
        HOUR,
        "while the bucket width it counts in is the one an operator reads telemetry in"
    );
    assert_eq!(
        Policy::new(u64::MAX, HOUR).cutoff(1_000),
        0,
        "and a window that reaches back past the epoch saturates: there is no millisecond below \
         zero to expire"
    );
    assert_eq!(
        Policy::new(HOUR, 0).cutoff(NOW),
        NOW - HOUR,
        "a width of zero is a millisecond bucket, so nothing is snapped away — a cutoff is a \
         boundary of the width it was asked for"
    );
}

#[test]
fn a_plan_names_what_expires_what_stays_and_what_straddles() {
    let segments = vec![
        spanned(0, 100, 10, 0, CUTOFF - 1),
        spanned(1, 200, 20, 0, CUTOFF),
        spanned(2, 300, 30, CUTOFF - 1, CUTOFF + 1),
        spanned(3, 400, 40, CUTOFF, CUTOFF + 5),
        spanned(4, 500, 50, CUTOFF + 1, CUTOFF + 9),
        spanned(5, 600, 60, CUTOFF + 10, CUTOFF + 10),
    ];
    let plan = policy().plan(&segments, CUTOFF);
    assert_eq!(
        plan.cutoff(),
        CUTOFF,
        "the plan says which cutoff it was made at"
    );
    assert_eq!(
        indices(plan.expired()),
        [0],
        "a segment expires exactly when its *last* timestamp is below the cutoff: one whose last \
         record is one millisecond older than the cutoff is old data"
    );
    assert_eq!(
        indices(plan.retained()),
        [1, 2, 3, 4, 5],
        "and the segments that stay keep the manifest's order, oldest first — including the one \
         whose last timestamp is *exactly* the cutoff, which is the `<=`-in-place-of-`<` trap this \
         whole rule turns on"
    );
    assert_eq!(
        plan.retained(),
        &segments[1..],
        "with the manifest's own values: same index, same bytes, same records, same timestamps. \
         `nothing newer than the cutoff was touched` is an equality claim about the index, not a \
         count of what survived"
    );
    assert_eq!(
        plan.straddling(),
        [1, 2],
        "a retained segment whose *first* timestamp is below the cutoff straddles it: it holds \
         records on both sides, which is what makes it neither deletable nor rewritable"
    );
    assert!(
        !plan.straddling().contains(&3),
        "while a segment whose first timestamp is exactly the cutoff does not: it has nothing below \
         it, so a pass that rewrote it would spend bytes to remove nothing"
    );
    assert!(
        !plan.is_empty(),
        "a plan with something in `expired` is not empty — that is the test a caller makes before \
         touching the store"
    );
    assert_eq!(
        plan.expired_bytes(),
        100,
        "a plan adds up what it would delete"
    );
    assert_eq!(plan.expired_records(), 10, "and how many records that is");
    assert_eq!(
        plan.retained_bytes(),
        2000,
        "and what the segments that stay cost"
    );
    assert_eq!(
        plan.retained().len() + plan.expired().len(),
        segments.len(),
        "every segment is on exactly one side of the plan: an expiry that dropped one silently, or \
         listed one twice, would show up here before it touched a disk"
    );

    assert!(
        policy().plan(&segments, 0).is_empty(),
        "a cutoff of zero expires nothing: every timestamp is at or after it, so a store that keeps \
         everything does not move"
    );
    assert_eq!(
        policy().plan(&segments, 0).retained().len(),
        segments.len(),
        "and every segment is retained"
    );
    assert!(
        policy().plan(&segments, 0).straddling().is_empty(),
        "with nothing straddling a cutoff of zero: a partition is half-open, so zero is not below \
         a first timestamp of zero"
    );
    assert!(
        policy().plan(&[], CUTOFF).is_empty(),
        "and a store with no segments has nothing to expire"
    );
    assert!(
        policy().plan(&segments, u64::MAX).expired().len() == segments.len(),
        "while a cutoff past every timestamp expires the whole store: the rule is a comparison, \
         and it does not care how large the numbers are"
    );

    // A segment whose age nobody wrote down is kept, and it is not straddling either. That is the
    // conservative default, and it is where the rule is pinned: `SegmentInfo::new` is what a caller
    // builds when the question is about size, so `None` reaches a plan in practice.
    let silent = [unstated(0, 100, 10), unstated(1, 200, 20)];
    let plan = policy().plan(&silent, CUTOFF);
    assert!(
        plan.is_empty(),
        "a plan made from segments whose ranges nobody stated expires nothing: `None` is not an old \
         timestamp, it is an unstated one, and deleting a file is the one decision this store \
         cannot take back"
    );
    assert_eq!(
        plan.retained(),
        &silent[..],
        "so every one of them stays, with the index's own values"
    );
    assert!(
        plan.straddling().is_empty(),
        "and none of them straddles: nobody compared them with the cutoff, so calling one of them \
         straddling would claim it reaches below a boundary it was never read against"
    );
    let mixed = policy().plan(
        &[unstated(0, 100, 10), spanned(1, 200, 20, 0, CUTOFF - 1)],
        CUTOFF,
    );
    assert_eq!(
        indices(mixed.expired()),
        [1],
        "while a segment whose range *is* stated expires by the ordinary rule, in the same plan: the \
         default is about the segment nobody wrote down, not about the plan"
    );
    assert_eq!(
        indices(mixed.retained()),
        [0],
        "and the silent one stays, which is the whole of the rule"
    );
}

#[test]
fn a_pass_that_expires_nothing_writes_nothing() {
    let (dir, mut store) = sealed_store("nothing");
    let epoch = store.manifest().epoch();
    let manifest = dir.manifest_bytes();
    let files = dir.segment_files();
    let report = expire(&mut store, &Policy::DEFAULT, NOW);

    assert!(
        report.is_empty(),
        "the default policy keeps everything, so nothing expired"
    );
    assert_eq!(
        report.segments_before(),
        SEGMENTS,
        "the report says what the store was, and it was the whole store"
    );
    assert_eq!(
        report.segments_after(),
        SEGMENTS,
        "and what it still is: a pass that expired nothing removed nothing"
    );
    assert_eq!(
        report.epoch(),
        epoch,
        "a store that did not change shape did not move its epoch — a reader's snapshot and the \
         store are still the same store"
    );
    assert_eq!(
        store.manifest().epoch(),
        epoch,
        "which the store agrees with"
    );
    assert_eq!(
        store.floor(),
        0,
        "and its floor is still the one it started with"
    );
    assert_eq!(
        dir.manifest_bytes(),
        manifest,
        "the index on the disk is byte for byte the index it was: a pass with an empty plan writes \
         nothing, because a task that runs every minute would otherwise rewrite it every minute"
    );
    assert_eq!(dir.segment_files(), files, "and no file was removed");
    for index in 0..SEGMENTS as u64 {
        assert!(
            dir.segment_bytes(index).is_some(),
            "segment {index} is still on the disk"
        );
    }

    let long = Policy::new(10 * HOUR, HOUR);
    let report = expire(&mut store, &long, NOW);
    assert!(
        report.is_empty(),
        "a window that reaches back to the epoch keeps every segment: the cutoff is the epoch and \
         the oldest record is after it"
    );
    assert!(
        report.straddling().is_empty(),
        "with nothing below that cutoff at all"
    );
    assert_eq!(
        store.manifest().epoch(),
        epoch,
        "so the store still has not moved"
    );
    assert_eq!(dir.manifest_bytes(), manifest, "not by a byte");
}

#[test]
fn a_pass_deletes_whole_segments_and_reports_the_conservation() {
    let (dir, mut store) = sealed_store("pass");
    let before = store.manifest().segments().to_vec();
    let bytes_before: u64 = before.iter().map(|info| info.bytes).sum();
    let kept: Vec<(u64, Vec<u8>)> = before
        .iter()
        .map(|info| {
            let bytes = dir
                .segment_bytes(info.index)
                .unwrap_or_else(|| panic!("segment {} is on the disk before the pass", info.index));
            (info.index, bytes)
        })
        .collect();
    let epoch = store.manifest().epoch();

    let report = expire(&mut store, &policy(), NOW);
    assert_eq!(
        indices(report.expired()),
        [0, 1],
        "the two segments whose last record is below the cutoff are the ones that expire, and a \
         whole segment is the unit: never a record inside one"
    );
    assert_eq!(
        report.segments_before(),
        SEGMENTS,
        "the report says what the store was made of"
    );
    assert_eq!(
        report.segments_before() - report.expired().len(),
        report.segments_after(),
        "and `segments_before - expired == segments_after` is the identity that says a pass \
         accounted for every segment it took"
    );
    assert_eq!(
        report.segments_after(),
        SEGMENTS - 2,
        "which is what the store is made of now"
    );
    assert_eq!(
        store.manifest().segments(),
        &before[2..],
        "the index names the retained segments with the values it had: same order, same index, \
         same byte length, same record count, same timestamps"
    );
    assert_eq!(
        store.manifest().epoch(),
        epoch + 1,
        "the store changed shape exactly once: one pass is one epoch, however many segments it \
         expired"
    );
    assert_eq!(
        store.manifest().floor(),
        CUTOFF,
        "and the floor is the cutoff the pass acted at: the store has written down how far it has \
         forgotten"
    );
    assert_eq!(
        report.bytes_reclaimed(),
        before[..2].iter().map(|info| info.bytes).sum::<u64>(),
        "the bytes the report claims are the expired segments' bytes"
    );
    assert_eq!(
        report.records_expired(),
        (2 * PER_SEGMENT) as u64,
        "and the records that left with them"
    );

    let mut bytes_after = 0u64;
    for (index, was) in &kept {
        match dir.segment_bytes(*index) {
            None => assert!(
                *index < 2,
                "segment {index} is gone from the disk and the plan did not expire it: a pass may \
                 only remove what it named"
            ),
            Some(now) => {
                assert!(
                    *index >= 2,
                    "segment {index} was expired and is still on the disk with no reader holding \
                     it: the file leaves when the last reader that named the set lets go"
                );
                assert_eq!(
                    &now, was,
                    "the retained segment {index} is byte-identical: a pass deletes files and \
                     writes an index, and rewriting a segment that stayed would spend bytes this \
                     step exists to stop spending"
                );
                bytes_after += now.len() as u64;
            }
        }
    }
    assert_eq!(
        dir.segment_files(),
        report.segments_after(),
        "so the segment directory holds exactly the segments the index names"
    );
    assert_eq!(
        bytes_before - bytes_after,
        report.bytes_reclaimed(),
        "and the disk gave back exactly what the report says was reclaimed: the two are measured \
         from different places, and a store that reported bytes it did not free would fail here"
    );
    assert_eq!(
        bytes_of(report.retained()),
        bytes_before - report.bytes_reclaimed(),
        "`retained bytes == before - reclaimed` closes the other half of the identity, and it is          computed from the report's own list rather than from a number it states twice"
    );
    assert_eq!(
        report.retained().len(),
        SEGMENTS - 2,
        "and the retained list is what the store is made of now"
    );
}

#[test]
fn what_the_index_stopped_naming_is_not_an_answer() {
    let (_dir, mut store) = sealed_store("answer");
    let report = expire(&mut store, &policy(), NOW);
    let answered = records_of(&store.snapshot());

    assert!(
        !answered.is_empty(),
        "a store whose cutoff is inside its data still answers what is at or after the cutoff"
    );
    assert!(
        same(&answered, &still_an_answer()),
        "the records a read returns are the records at or after the cutoff, once each, in the \
         store's order — and nothing older, whoever can still find it on the disk"
    );
    assert_eq!(
        answered.len(),
        480,
        "which is the six retained segments, less the sixty records the straddler holds below the \
         cutoff"
    );
    assert!(
        answered
            .iter()
            .all(|record| record.ts_millis >= CUTOFF as i64),
        "nothing below the cutoff is an answer, and this is the half of the claim a reader can \
         check for itself"
    );
    assert!(
        answered
            .iter()
            .any(|record| record.ts_millis == CUTOFF as i64),
        "while a record whose timestamp is *exactly* the cutoff is still there: the comparison is \
         `last < cutoff`, and `<=` would delete a millisecond the policy promised to keep"
    );
    assert!(
        answered
            .iter()
            .any(|record| record.ts_millis == CUTOFF as i64 + STEP),
        "and so is the next sample after it"
    );
    assert_eq!(
        store.floor(),
        CUTOFF,
        "the store's floor is the cutoff it acted at"
    );
    assert_eq!(
        store.snapshot().floor(),
        CUTOFF,
        "which a read that begins now sees: `Snapshot::floor` is the number the answer was filtered \
         by"
    );
    assert!(
        report
            .expired()
            .iter()
            .all(|info| info.last_millis().is_some_and(|last| last < CUTOFF)),
        "and every segment the pass deleted stated a range that ends below the cutoff: a segment \
         whose age nobody wrote down is never one of them"
    );
}

#[test]
fn a_straddling_segment_is_kept_and_not_rewritten() {
    let (dir, mut store) = sealed_store("straddle");
    let straddler = store.manifest().segments()[2];
    let was = dir
        .segment_bytes(straddler.index)
        .unwrap_or_else(|| panic!("the segment that will straddle is on the disk"));
    let report = expire(&mut store, &policy(), NOW);

    assert_eq!(
        report.straddling(),
        [straddler.index],
        "the third segment reaches below the cutoff and past it, so the plan names it as the one \
         that straddles"
    );
    assert_eq!(
        dir.segment_bytes(straddler.index).as_ref(),
        Some(&was),
        "and it is not rewritten: deleting it would take records newer than the cutoff with it, and \
         rewriting it would pay the write amplification `lakelet/4.8-compaction` bounds — for a \
         pass whose whole job is to stop paying for bytes"
    );
    assert!(
        store.manifest().segments().contains(&straddler),
        "the index states what it always stated about it: same bytes, same records, and the same \
         two timestamps that say it straddles"
    );
    assert_eq!(
        report.expired().len(),
        2,
        "so it is the two segments entirely below the cutoff that leave, and no more"
    );

    let snapshot = store.snapshot();
    assert_eq!(
        snapshot.len(),
        540,
        "`Snapshot::len` still counts every record the retained segments hold, the straddler's old \
         ones included: it is the number a caller sizes memory with, and redefining it as `what a \
         read would hand back` would make it a number nobody can compute before reading"
    );
    let answered = records_of(&snapshot);
    assert_eq!(
        snapshot.len() - answered.len() as u64,
        60,
        "while the difference between the count and the answer is exactly the straddler's records \
         below the cutoff: the store holds more than it answers, and both numbers are honest"
    );
    assert!(
        answered
            .iter()
            .all(|record| record.ts_millis >= CUTOFF as i64),
        "which the answer says the same way"
    );
    assert_eq!(
        bytes_of(report.retained()),
        store.manifest().bytes(),
        "and the bytes the report says it retained are the bytes the index says the store holds"
    );
}

#[test]
fn a_reader_that_took_its_snapshot_before_the_pass_keeps_its_view() {
    let (dir, mut store) = sealed_store("reader");
    let accepted = sealed();
    let reader = store.snapshot();
    let expired: Vec<PathBuf> = (0..2).map(|index| file_of(dir.path(), index)).collect();

    let report = expire(&mut store, &policy(), NOW);
    assert_eq!(
        report.expired().len(),
        2,
        "the pass expires the two segments below the cutoff"
    );
    assert!(
        reader.epoch() < store.manifest().epoch(),
        "the reader names the store it was taken from, and the store has moved on"
    );
    assert!(
        expired.iter().all(|path| path.exists()),
        "the segments a pass expired are still on the disk while a reader names them: deleting them \
         at the pass would hand a reader that began before it whatever the operating system does to \
         a file nobody names"
    );
    assert_eq!(
        reader.floor(),
        0,
        "and the floor *of the set this reader named* is the one it was taken at: the pass moved \
         the store's floor, not this reader's"
    );
    assert!(
        same(&records_of(&reader), &accepted),
        "so a read that began before the pass answers exactly what it would have answered then — \
         every record it named, the expired ones included"
    );

    let after = store.snapshot();
    assert_eq!(
        after.floor(),
        CUTOFF,
        "a read that begins after the pass sees the new floor"
    );
    assert!(
        same(&records_of(&after), &still_an_answer()),
        "and is confined to what the store still keeps: the two reads answer different things, \
         which is the whole of what a retention pass does"
    );

    drop(reader);
    assert!(
        expired.iter().all(|path| !path.exists()),
        "once the last reader that named the old set lets go, its files go: that is the one place a \
         pass deletes anything, and the reason a snapshot holds a set rather than a list of paths"
    );
    assert_eq!(
        dir.segment_files(),
        store.manifest().segments().len(),
        "and what is left on the disk is what the index names"
    );
    assert!(
        same(&records_of(&store.snapshot()), &still_an_answer()),
        "while the store goes on answering what it keeps"
    );
}

#[test]
fn the_floor_survives_the_open() {
    let dir = Dir::new("reopen");
    let epoch;
    let files;
    {
        let mut store = Segments::open(dir.path())
            .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
        for index in 0..SEGMENTS {
            store
                .seal(&batch(index))
                .unwrap_or_else(|error| panic!("a segment is closed: {error}"));
        }
        let report = expire(&mut store, &policy(), NOW);
        assert_eq!(report.expired().len(), 2, "the pass expires two segments");
        epoch = store.manifest().epoch();
        files = store.manifest().segments().len();
        // A pass that died between the swap and the deletion leaves the file it expired behind:
        // the next open is what makes "the index is the store" true of a retention pass as well.
        let leftover = dir
            .segment_bytes(2)
            .unwrap_or_else(|| panic!("a retained segment is on the disk"));
        std::fs::write(file_of(dir.path(), 0), &leftover)
            .unwrap_or_else(|error| panic!("a leftover file is written: {error}"));
    }

    let reopened = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a store with a floor opens: {error}"));
    assert_eq!(
        reopened.manifest().floor(),
        CUTOFF,
        "the floor is in the index, so a store that has forgotten something comes back knowing it: \
         a floor that reset on reopen would answer data the store promised was gone"
    );
    assert_eq!(
        reopened.manifest().epoch(),
        epoch,
        "and the store is the store it was"
    );
    assert_eq!(
        reopened.manifest().segments().len(),
        files,
        "with the segments it had, not the ones the pass expired"
    );
    assert!(
        !file_of(dir.path(), 0).exists(),
        "the file the pass expired does not come back as an orphan: what the index does not name is \
         not the store, and an open deletes it"
    );
    assert_eq!(
        dir.segment_files(),
        files,
        "so the segment directory holds exactly what the index names"
    );
    assert!(
        same(&records_of(&reopened.snapshot()), &still_an_answer()),
        "and the expired records are still not answers after a restart"
    );
    assert_eq!(
        Segments::open(dir.path())
            .unwrap_or_else(|error| panic!("a store opens twice: {error}"))
            .floor(),
        CUTOFF,
        "opening it again does not move the floor either"
    );
}

#[test]
fn the_floor_never_moves_backwards() {
    let (dir, mut store) = sealed_store("floor");
    let first = expire(&mut store, &policy(), NOW);
    assert_eq!(
        first.expired().len(),
        2,
        "the first pass expires two segments"
    );
    let epoch = store.manifest().epoch();
    let manifest = dir.manifest_bytes();
    let files = dir.segment_files();

    // A later pass whose window grew: `now - keep` is now *earlier* than the cutoff the first pass
    // acted at. The store has already said that data is gone, and a policy cannot unsay it.
    let bigger = expire(&mut store, &Policy::new(10 * HOUR, HOUR), NOW);
    assert!(
        bigger.is_empty(),
        "a policy whose window reaches back to the epoch has nothing to expire — everything below \
         its cutoff was already deleted"
    );
    assert_eq!(
        store.floor(),
        CUTOFF,
        "so the floor does not move backwards: a promise that data is gone cannot be taken back, \
         and a store that reset it would answer records it has already deleted"
    );
    assert_eq!(
        store.manifest().epoch(),
        epoch,
        "and a pass with nothing to expire does not move the store"
    );
    assert_eq!(dir.manifest_bytes(), manifest, "not by a byte of its index");
    assert_eq!(dir.segment_files(), files, "nor by a file");

    // A clock that moved backwards is the same rule from the other side.
    let earlier = expire(&mut store, &policy(), NOW - 4 * HOUR);
    assert!(
        earlier.is_empty(),
        "a pass whose `now` is earlier than the last one's cutoff expires nothing: the cutoff \
         saturates to zero and the floor holds it where it was"
    );
    assert_eq!(
        store.floor(),
        CUTOFF,
        "and the floor is still the cutoff of the first pass"
    );

    // While a *later* cutoff expires more: the store goes on deleting as the clock moves.
    let later = expire(&mut store, &policy(), NOW + 4 * HOUR);
    assert_eq!(
        later.expired().len(),
        4,
        "four hours later, four segments are entirely below the cutoff: the second pass is the same \
         rule at a later number, not a different one"
    );
    assert_eq!(
        store.floor(),
        472_222 * HOUR + 7 * HOUR,
        "and the floor moved to the new cutoff, which is a boundary seven hours past the epoch"
    );
    assert_eq!(
        store.manifest().floor() % HOUR,
        0,
        "still a bucket boundary: every cutoff this store acts at is one"
    );
    assert!(
        records_of(&store.snapshot())
            .iter()
            .all(|record| record.ts_millis >= store.floor() as i64),
        "and the store answers nothing below the floor it moved to"
    );
}

#[test]
fn a_merge_gives_the_new_segment_the_union_of_its_inputs() {
    let (_dir, mut store) = sealed_store("merge");
    let before = store.manifest().segments().to_vec();
    let plan = store.plan();
    assert_eq!(
        plan.len(),
        4,
        "the store's own policy plans over its own segments: {SEGMENTS} segments of the same size \
         are one run of four"
    );
    let report = store
        .compact()
        .unwrap_or_else(|error| panic!("a store of comparable segments has a plan: {error}"))
        .unwrap_or_else(|| panic!("a store of comparable segments has a plan"));
    let merged = store.manifest().segments()[0];
    assert_eq!(
        merged.index,
        report.wrote(),
        "the segment the merge wrote sits where its inputs did"
    );
    assert_eq!(
        merged.first_millis(),
        before[..4]
            .iter()
            .filter_map(|info| info.first_millis())
            .min(),
        "and it states the *union* of its inputs' ranges: its first timestamp is the smallest of \
         theirs"
    );
    assert_eq!(
        merged.last_millis(),
        before[..4]
            .iter()
            .filter_map(|info| info.last_millis())
            .max(),
        "and its last is the largest"
    );
    assert_eq!(
        merged.first_millis(),
        Some(EPOCH as u64),
        "which for the first four segments is the epoch to the ninety-minute mark of the fourth"
    );
    assert_eq!(
        merged.last_millis(),
        Some(EPOCH as u64 + 269 * 60_000),
        "exactly"
    );
    assert_eq!(
        merged.records,
        (4 * PER_SEGMENT) as u64,
        "and the index still says how many records it holds, without a file being read to find out \
         — by the merge, or by this case"
    );

    // The union is what makes a merged segment expire *as a whole* later on: it no longer straddles
    // anything once a cutoff is past its last record, so the pass that follows drops one file.
    for index in SEGMENTS..SEGMENTS + 4 {
        store
            .seal(&batch(index))
            .unwrap_or_else(|error| panic!("a later segment is closed: {error}"));
    }
    let later = expire(&mut store, &policy(), NOW + 8 * HOUR);
    assert!(
        later
            .expired()
            .iter()
            .any(|info| info.index == merged.index),
        "a cutoff eight hours later is past the merged segment's last record, so the whole merged \
         segment expires — one file, four segments' worth of data"
    );
    let cutoff = later.cutoff();
    let spanning: Vec<u64> = store
        .manifest()
        .segments()
        .iter()
        .filter(|info| {
            info.first_millis().is_some_and(|first| first < cutoff)
                && info.last_millis().is_some_and(|last| last >= cutoff)
        })
        .map(|info| info.index)
        .collect();
    assert_eq!(
        later.straddling(),
        spanning,
        "and the segments that reach below the cutoff now are exactly the ones the index says span \
         it: the pass reports what its own plan found, not a count it keeps of its own"
    );
    assert_eq!(
        spanning.len(),
        1,
        "which is one segment — the only file in the store that holds records on both sides of the \
         cutoff"
    );
    assert_eq!(
        store.floor(),
        472_222 * HOUR + 11 * HOUR,
        "with the floor at the cutoff the pass acted at"
    );
}

#[test]
fn a_reopened_store_adds_segments_without_reusing_an_ordinal() {
    let dir = Dir::new("ordinals");
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    for index in 0..4 {
        store
            .seal(&batch(index))
            .unwrap_or_else(|error| panic!("a segment is closed: {error}"));
    }
    let report = expire(&mut store, &policy(), NOW);
    assert_eq!(
        report.expired().len(),
        2,
        "the pass expires the two oldest segments"
    );
    let epoch = store.manifest().epoch();
    let named: Vec<u64> = store
        .manifest()
        .segments()
        .iter()
        .map(|info| info.index)
        .collect();
    let kept: Vec<Vec<u8>> = named
        .iter()
        .map(|index| {
            dir.segment_bytes(*index)
                .unwrap_or_else(|| panic!("segment {index} is named and on the disk"))
        })
        .collect();
    drop(store);

    let mut reopened = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a store with a floor opens: {error}"));
    assert_eq!(
        reopened.manifest().floor(),
        CUTOFF,
        "a store that comes back knows how far it has forgotten"
    );
    let index = reopened
        .seal(&batch(4))
        .unwrap_or_else(|error| panic!("a segment is closed into a store that has some: {error}"));
    assert!(
        index > *named.iter().max().unwrap_or(&0),
        "segment {index} is closed into a store whose segments are {named:?}: an ordinal a store \
         has handed out is never handed out again, because a segment written over another one is \
         two entries naming one file — and one of the two is not what its bytes say"
    );
    assert_eq!(index, 4, "and they go up by one rather than sideways");
    assert_eq!(
        reopened.manifest().segments().len(),
        named.len() + 1,
        "so the store grew by exactly one segment"
    );
    assert_eq!(
        reopened.manifest().epoch(),
        epoch + 1,
        "with one change of shape, as every other write of a segment is"
    );
    assert_eq!(
        reopened.manifest().floor(),
        CUTOFF,
        "and a segment closed after a pass does not move the floor: the promise is the store's, not \
         the pass's"
    );
    for (position, index) in named.iter().enumerate() {
        assert_eq!(
            dir.segment_bytes(*index).as_ref(),
            kept.get(position),
            "segment {index} is byte-identical after all of it: closing a segment writes one file \
             and replaces the index, and touches nothing else"
        );
    }
    assert_eq!(
        dir.segment_files(),
        named.len() + 1,
        "so the segment directory holds exactly the segments the index names"
    );

    let mut wanted: Vec<Record> = (0..4)
        .flat_map(batch)
        .filter(|record| record.ts_millis >= CUTOFF as i64)
        .collect();
    wanted.extend(batch(4));
    assert!(
        same(&records_of(&reopened.snapshot()), &wanted),
        "and the store answers what it answered before, plus the records of the segment it just \
         closed — every one of them, in the order they were accepted"
    );
}

#[test]
fn a_pass_is_refused_while_a_merge_is_written_and_not_installed() {
    let (dir, mut store) = sealed_store("in-flight");
    let epoch = store.manifest().epoch();
    let manifest = dir.manifest_bytes();
    let plan = store.plan();
    let ran = store
        .merge(&plan)
        .unwrap_or_else(|error| panic!("a store of comparable segments merges: {error}"));

    match store.expire(&policy(), NOW) {
        Ok(report) => panic!(
            "a pass ran while a merge was written and not installed, and expired {} segment(s): \
             the merge is about to replace the index this pass just replaced, and the two answers \
             cannot both be the store",
            report.expired().len()
        ),
        Err(Error::Invalid { detail }) => assert!(
            !detail.trim().is_empty(),
            "the refusal has to say what is in the way: an operator reading the log has only the \
             sentence"
        ),
        Err(other) => panic!(
            "a pass refused while a merge is in flight was refused by `{other}`, and a store that \
             was asked to do something it cannot do answers `Error::Invalid`"
        ),
    }
    assert_eq!(
        store.manifest().epoch(),
        epoch,
        "and the refusal left the store exactly as it was"
    );
    assert_eq!(
        dir.manifest_bytes(),
        manifest,
        "down to the bytes of its index: a refusal that wrote something is not a refusal"
    );

    store.install(ran).unwrap_or_else(|error| {
        panic!("the merge the pass refused to race is still installable: {error}")
    });
    assert_eq!(
        store.manifest().segments().len(),
        SEGMENTS - plan.len() + 1,
        "so the merge is the store, which is what the pass was waiting for"
    );
    assert!(
        same(&records_of(&store.snapshot()), &sealed()),
        "and no record went missing in the meantime"
    );
}

#[test]
fn a_version_1_index_is_refused_rather_than_misread() {
    let (dir, store) = sealed_store("version");
    drop(store);
    let mut manifest = dir.manifest_bytes();
    assert!(
        manifest.len() > 4,
        "the index states a layout the module documents: a magic, a version byte, and the rest"
    );
    // The layout this build writes is `MAGIC | VERSION | epoch | floor | count | entries | seal`
    // (the module docs of `src/engine/compaction/mod.rs`), and version 1 stated neither the floor
    // nor a time range per segment. Its bytes are refused, not read as a version 2 index whose
    // entries happen to be shorter.
    manifest[4] = 1;
    std::fs::write(dir.file(MANIFEST), &manifest)
        .unwrap_or_else(|error| panic!("an older index is written: {error}"));
    let refused = refused_by(
        Segments::open(dir.path()).map(|_| ()),
        "a store whose index was written by the layout before this one",
    );
    assert!(
        refused.contains("1") && refused.contains("2"),
        "the refusal names both versions, so an operator who downgraded a binary knows what they \
         are looking at: {refused}"
    );
}

#[test]
fn the_retention_sources_do_not_reach_into_the_service() {
    for module in ["src/engine/retention", "src/engine/compaction"] {
        let directory = PathBuf::from(module);
        let mut files: Vec<PathBuf> = Vec::new();
        collect_rust(&directory, &mut files).expect("the module is part of the project");
        assert!(
            files.len() >= 2,
            "{} holds {} Rust file(s): the rule, the bucket arithmetic and the store are what this \
             step is made of, and a store whose retention lives in one file the size of a chapter is \
             a store nobody can review",
            directory.display(),
            files.len()
        );
        for file in &files {
            let source = std::fs::read_to_string(file).expect("a source file the project keeps");
            for forbidden in ["crate::service", "crate::cli"] {
                assert!(
                    !source.contains(forbidden),
                    "{} names `{forbidden}` — the daemon uses the engine, never the other way, and \
                     the files this step adds are where a new violation would appear \
                     (`lakelet/4.1-ingest-protocol` reads the same sources for it)",
                    file.display()
                );
            }
        }
    }

    let retention = std::fs::read_to_string("src/engine/retention/mod.rs").unwrap_or_else(|error| {
        panic!(
            "src/engine/retention/mod.rs cannot be read: {error} — the rule this step is about is \
             the file an operator reads first, and this is the half a test can hold"
        )
    });
    for name in ["Policy", "Plan", "ExpireReport", "cutoff", "straddling"] {
        assert!(
            retention.contains(name),
            "the module does not name `{name}`: the policy, the plan and the report are what this \
             step is made of, and a reader of the code has to find them"
        );
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
