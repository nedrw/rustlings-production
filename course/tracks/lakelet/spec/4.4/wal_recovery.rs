//! Spec for `lakelet/4.4-wal-recovery`: a checksummed log, the prefix a replay can prove, and the
//! boundary the write path promised.
//!
//! `kiln run` copies this file into the project as `tests/spec_wal_recovery.rs`; it is the course's
//! file, not the learner's. Fourteen cases, and the shape of them is the point:
//!
//! * **a record declares itself** — its length, its payload, and a seal over both, so a reader
//!   decides "this is a whole record of mine" from the bytes in front of it and nothing else. A
//!   changed byte in the length and a changed byte in the payload are two different seals, which is
//!   what makes that decision possible at all;
//! * **a replay stops at the first thing it cannot prove** — it never skips a record it could not
//!   verify, never hands back one it did not, and never panics: a capture of the bytes a crash left
//!   behind is exactly the input this exists for. The exhaustive half of that claim is
//!   `every_prefix_of_a_written_log_recovers_to_a_prefix_of_its_records`: one log of twenty
//!   records, one *cheap* call per truncation point from zero bytes to the whole file — a few
//!   thousand of them, which is why it is a plain case and not a slow one — because "random kill
//!   points recover" is a claim about every byte of the file;
//! * **the acknowledged boundary is the contract** — `Spool::durable_offset` is what the store
//!   promised, so a machine death (the file cut back to that offset, which is what the step's
//!   simulation does too) leaves exactly what was flushed; and opening a store puts it back in a
//!   state that can append again *at* the recovered end, never over it and never before it;
//! * **the daemon is judged as a process** — a store whose daemon was killed serves every record it
//!   acknowledged, and the daemon says what opening it had to discard; a clean stop flush closes the
//!   window the policy left open, which is the sentence `bin/lakeletd.rs` and `docs/crash-points.md`
//!   point 6 have carried since `lakelet/4.3-write-path-adr` wrote them down.
//!
//! Nothing here waits for time: the format and the replay are arithmetic over bytes, and the daemon
//! is waited for through its own log lines.

mod support;

use std::ops::Range;

use support::StoreDir;
use support::service::Service;

use lakelet::engine::Spool;
use lakelet::engine::spool::FILE;
use lakelet::engine::wal::Policy;
use lakelet::engine::wal::checksum;
use lakelet::engine::wal::replay;
use lakelet::engine::wal::segment::{self, Stop};
use lakelet::protocol::batch::{self, Record};
use lakelet::protocol::frame::Frame;

/// A payload that is not round-numbered, so a length/payload mix-up cannot pass by accident.
fn payload(marker: u8) -> Vec<u8> {
    (0..37u8).map(|index| marker.wrapping_add(index)).collect()
}

/// A payload of its own length and its own bytes, for the logs whose records differ in size.
fn sized(marker: u8, length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| marker.wrapping_mul(31).wrapping_add(index as u8))
        .collect()
}

/// The log those records are, as the bytes they are: the writer's own calls, in order.
///
/// Built here rather than through a store so that every offset below is known exactly — a cut has to
/// land *inside* a named record for the case to be about that record.
fn log(records: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (index, payload) in records.iter().enumerate() {
        let frame = if index == 0 {
            segment::introduce(payload)
        } else {
            segment::encode(payload)
        };
        bytes.extend_from_slice(&frame);
    }
    bytes
}

/// Where record `index`'s payload sits in `log(records)`: its frame starts `LENGTH` bytes before it.
///
/// The frame is `[length][payload][seal]`, and the header comes once, so this is the whole layout
/// the spec needs to know — and it reads all of it off the constants `segment` publishes.
fn payload_range(records: &[Vec<u8>], index: usize) -> Range<usize> {
    let before: usize = records[..index]
        .iter()
        .map(|payload| segment::OVERHEAD + payload.len())
        .sum();
    let start = segment::HEADER + before + segment::LENGTH;
    start..start + records[index].len()
}

/// What a log cut to `cut` bytes proves: how many of its records are whole, and how far they reach.
///
/// The answer the spec expects, written as arithmetic rather than read out of the code: a walk can
/// only be as good as the bytes a record needs, and the header has to be there before the first one
/// counts.
fn whole(records: &[Vec<u8>], cut: usize) -> (usize, usize) {
    if cut < segment::HEADER {
        return (0, 0);
    }
    let mut end = segment::HEADER;
    let mut count = 0;
    for payload in records {
        let next = end + segment::OVERHEAD + payload.len();
        if next > cut {
            break;
        }
        end = next;
        count += 1;
    }
    (count, end)
}

/// Write `bytes` as the store's log file, the way a device that a crash stopped leaves them.
fn write(store: &StoreDir, bytes: &[u8]) {
    let path = store.path().join(FILE);
    std::fs::write(&path, bytes)
        .unwrap_or_else(|error| panic!("{} could not be written: {error}", path.display()));
}

/// The store's log file as it is on disk right now.
fn file(store: &StoreDir) -> Vec<u8> {
    let path = store.path().join(FILE);
    std::fs::read(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

/// Cut the store's log file to `bytes`, which is how this file models the machine stopping.
fn cut(store: &StoreDir, bytes: u64) {
    let path = store.path().join(FILE);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|handle| handle.set_len(bytes))
        .unwrap_or_else(|error| {
            panic!(
                "{} could not be cut to {bytes} byte(s): {error}",
                path.display()
            )
        });
}

#[test]
fn a_record_is_a_length_its_payload_and_a_seal() {
    let record = payload(1);
    let frame = segment::encode(&record);
    assert_eq!(
        frame.len(),
        segment::OVERHEAD + record.len(),
        "a record is its payload plus {} byte(s) of framing — the length that says how far it reaches \
         and the seal that says it is whole; a reader that had to look anywhere else for either \
         would not be reading the device",
        segment::OVERHEAD,
    );
    let first = segment::introduce(&record);
    assert_eq!(
        first.len(),
        segment::HEADER + segment::OVERHEAD + record.len(),
        "the log's first record carries the header that declares the file"
    );
    assert_eq!(
        first.get(..segment::HEADER),
        Some(&segment::HEADER_BYTES[..]),
        "the header is exactly `segment::HEADER_BYTES`: magic, then the version this build writes"
    );

    // And the walk reads back what the writer wrote, in order.
    let written = vec![payload(2), payload(3)];
    let bytes = log(&written);
    let scanned = segment::scan(&bytes).expect("a log this crate just wrote is a log");
    assert_eq!(
        scanned.records(),
        written.as_slice(),
        "two records in, two out"
    );
    assert_eq!(
        scanned.verified(),
        bytes.len(),
        "every byte of a whole log is a byte of a record the walk proved"
    );
    assert_eq!(
        scanned.stop(),
        Stop::End,
        "a file that ends on a record boundary stopped at its end"
    );
}

#[test]
fn a_seal_covers_the_length_and_every_payload_byte() {
    let payload = payload(4);
    let length = payload.len() as u32;
    let seal = checksum::seal(length, &payload);
    assert!(
        checksum::verifies(length, &payload, seal),
        "the seal of these bytes verifies against these bytes"
    );
    assert!(
        !checksum::verifies(length + 1, &payload, seal),
        "the seal covers the *length*: a record whose length field was damaged must not verify \
         against the length it now states, because the length is what the reader believes to find \
         the record's end"
    );
    let mut changed = payload.clone();
    changed[0] = changed[0].wrapping_add(1);
    assert!(
        !checksum::verifies(length, &changed, seal),
        "and it covers every payload byte: one flipped bit is a different seal"
    );
    assert_ne!(
        checksum::seal(length, &changed),
        seal,
        "a changed payload byte is a changed seal, not a coincidence"
    );
}

#[test]
fn a_file_that_is_not_this_log_is_refused_and_a_header_cut_short_is_not_a_file() {
    let store = StoreDir::new("wal-foreign");
    write(
        &store,
        b"# a README somebody dropped in a store directory\n",
    );
    let refused = replay::recover(store.path()).expect_err("a README is not a log");
    assert_eq!(
        refused.kind(),
        "protocol",
        "a file that is not this log is a typed protocol error and never a store with no records \
         in it: {refused}"
    );
    assert!(
        Spool::open(store.path()).is_err(),
        "and opening the store says so too — a store that read somebody else's file as an empty \
         one would answer every later query with nothing, and nobody would know why"
    );

    // The same magic, a version this build does not speak: still refused, and for its own reason.
    let mut future = segment::HEADER_BYTES.to_vec();
    future[segment::MAGIC.len()] = segment::VERSION.wrapping_add(1);
    future.extend_from_slice(&segment::encode(&payload(5)));
    let refused =
        segment::scan(&future).expect_err("a version this build cannot replay is refused");
    assert_eq!(refused.kind(), "protocol", "{refused}");

    // A log whose *header* was cut short is a different answer: it is this log, part-written.
    let bytes = log(&[payload(6), payload(7)]);
    let short = bytes
        .get(..segment::HEADER - 1)
        .expect("the log is longer than its header");
    let scanned = segment::scan(short)
        .expect("the beginning of a log is not a foreign file, however little of it arrived");
    assert!(scanned.records().is_empty(), "no record is there yet");
    assert_eq!(
        scanned.verified(),
        0,
        "and none of those bytes is a byte of a record"
    );
    assert_eq!(
        scanned.stop(),
        Stop::Torn,
        "a file cut inside its own header is a torn log, which is not the same answer as a file \
         that was never a log"
    );
}

#[test]
fn a_flipped_byte_ends_the_replay_at_the_record_it_changed() {
    let records: Vec<Vec<u8>> = (0..5)
        .map(|index| sized(index, 20 + index as usize))
        .collect();
    let bytes = log(&records);
    let victim = payload_range(&records, 2);
    // The whole frame the changed byte is in: its length, its payload and its seal.
    let frame = victim.start - segment::LENGTH..victim.end + checksum::LEN;
    assert_eq!(
        frame.len(),
        segment::OVERHEAD + records[2].len(),
        "the seal sits at the end of the record it covers"
    );

    // A payload byte.
    let mut flipped = bytes.clone();
    flipped[victim.start + victim.len() / 2] ^= 0xFF;
    let scanned = segment::scan(&flipped).expect("damaged bytes are not a foreign file");
    assert_eq!(
        scanned.records(),
        &records[..2],
        "the record that changed is not handed back, and neither is anything after it: {scanned:?}"
    );
    assert_eq!(
        scanned.stop(),
        Stop::Corrupt,
        "a whole record's worth of bytes that is not the record it claims is corruption, and it \
         is a different sentence from a file that stops in the middle of one"
    );
    assert_eq!(
        scanned.verified(),
        frame.start,
        "the walk proved every byte before the record it refused"
    );

    // The seal itself, which is the byte a device that wrote something else changes.
    let mut resealed = bytes.clone();
    resealed[frame.end - 1] ^= 0xFF;
    assert_eq!(
        segment::scan(&resealed)
            .expect("damaged bytes are not a foreign file")
            .records(),
        &records[..2],
        "a changed seal is a record the walk will not hand back"
    );

    // The length field, which the seal covers for exactly this reason: the reader uses it to find
    // where the record ends, so a length the seal did not cover would move the end of a record
    // nobody could then check.
    let mut stretched = bytes;
    stretched[frame.start] ^= 0xFF;
    let scanned = segment::scan(&stretched).expect("damaged bytes are not a foreign file");
    assert_eq!(
        scanned.records(),
        &records[..2],
        "a damaged length cannot move the record's end somewhere the seal still covers: {scanned:?}"
    );
    assert_ne!(
        scanned.stop(),
        Stop::End,
        "and it is not a clean end either"
    );
}

#[test]
fn a_record_the_replay_cannot_prove_is_not_skipped_over() {
    let records: Vec<Vec<u8>> = (0..5).map(|index| sized(index, 24)).collect();
    let mut bytes = log(&records);
    let victim = payload_range(&records, 1);
    bytes[victim.start] ^= 0xFF;

    let scanned = segment::scan(&bytes).expect("one damaged record does not make the file foreign");
    assert_eq!(
        scanned.records(),
        &records[..1],
        "the records after the damaged one are *there* and are not part of the answer: resyncing \
         would mean trusting the length of a record that just failed its seal, and a store that \
         returns records 1, then 3, 4 and 5 has told every reader the series was continuous"
    );
    for later in &records[2..] {
        assert!(
            !scanned.records().contains(later),
            "a record the replay could not prove ended the replay; nothing after it may appear"
        );
    }
    assert_eq!(
        scanned.verified(),
        victim.start - segment::LENGTH,
        "the proved bytes stop where the bad record starts"
    );
    assert_eq!(
        bytes.len() - scanned.verified(),
        (records[1..])
            .iter()
            .map(|payload| segment::OVERHEAD + payload.len())
            .sum::<usize>(),
        "and every byte from there to the end of the file is reported as discarded, not as a \
         record"
    );
}

#[test]
fn every_prefix_of_a_written_log_recovers_to_a_prefix_of_its_records() {
    let records: Vec<Vec<u8>> = (0..20)
        .map(|index| sized(index as u8, 1 + (index * 7) % 13))
        .collect();
    let bytes = log(&records);
    let mut cut_at = 0usize;
    while cut_at <= bytes.len() {
        let prefix = bytes
            .get(..cut_at)
            .expect("the cut is inside the file by construction");
        let scanned = segment::scan(prefix).unwrap_or_else(|error| {
            panic!(
                "a prefix of this log ({cut_at} of {} byte(s)) was refused as a foreign file: \
                 {error} — every truncation point of a log has to recover, because a kill point \
                 is a truncation nobody chose",
                bytes.len()
            )
        });
        let (count, end) = whole(&records, cut_at);
        assert_eq!(
            scanned.records(),
            &records[..count],
            "cut at {cut_at} byte(s): the records that came through whole are the first {count}, \
             each one intact and in order"
        );
        assert_eq!(
            scanned.verified(),
            end,
            "cut at {cut_at} byte(s): the proved bytes end after the last whole record"
        );
        assert_eq!(
            prefix.len() - scanned.verified(),
            cut_at - end,
            "cut at {cut_at} byte(s): everything after the last whole record is reported as \
             discarded rather than read as a record"
        );
        assert_eq!(
            scanned.stop(),
            if cut_at == end { Stop::End } else { Stop::Torn },
            "cut at {cut_at} byte(s): a file that ends on a record boundary ended, and one that \
             stops inside a record is torn"
        );
        cut_at += 1;
    }
}

#[test]
fn an_empty_store_is_an_empty_log_and_an_unreadable_one_is_a_store_error() {
    // No file at all: a store this process has never written to.
    let missing = StoreDir::new("wal-missing");
    let recovered = replay::recover(missing.path())
        .expect("a store directory with no log in it yet is not an error");
    assert!(recovered.records().is_empty(), "no records come out of it");
    assert_eq!(recovered.bytes(), 0, "nothing was proved");
    assert_eq!(recovered.discarded(), 0, "and nothing was discarded");
    assert_eq!(
        recovered.stop(),
        Stop::End,
        "an empty file stopped at its end"
    );

    // An empty file, which is what a store that has accepted nothing holds: the header arrives with
    // the log's first record, so a store nobody has written to is zero bytes long.
    write(&missing, b"");
    let recovered =
        replay::recover(missing.path()).expect("an empty file is a log with no records");
    assert!(recovered.records().is_empty());
    assert_eq!(recovered.discarded(), 0);

    // A fresh store, opened: it accepts records and answers an empty list until it has some.
    let store = StoreDir::new("wal-empty");
    let spool = Spool::open(store.path()).expect("a directory this process owns is a store");
    assert!(
        spool.is_empty(),
        "a store that has accepted nothing holds nothing"
    );
    assert_eq!(
        spool.records().expect("an empty store reads back empty"),
        Vec::<Vec<u8>>::new()
    );
    assert_eq!(
        spool.recovered().bytes(),
        0,
        "opening a store that was never written to recovered nothing, and that is not a \
         discarded tail"
    );
    assert_eq!(spool.recovered().discarded(), 0);

    // A file that exists and cannot be read is a store failure, not an empty log: the two are
    // different afternoons, and only one of them is a bug in the disk.
    let broken = StoreDir::new("wal-unreadable");
    std::fs::create_dir_all(broken.path().join(FILE)).expect("a directory where the log goes");
    let failed = replay::recover(broken.path()).expect_err("a directory is not a log");
    assert_eq!(
        failed.kind(),
        "store",
        "a log that cannot be read is a store error: {failed}"
    );
}

#[test]
fn every_cut_point_of_a_store_opens_to_the_prefix_it_can_prove() {
    let store = StoreDir::new("wal-cut");
    let records: Vec<Vec<u8>> = (0..4)
        .map(|index| sized(index, 20 + index as usize))
        .collect();
    let bytes = log(&records);
    let mut cut_at = 0usize;
    while cut_at <= bytes.len() {
        write(&store, &bytes);
        cut(&store, cut_at as u64);

        let spool = Spool::open(store.path()).expect("a store whose log was cut is still a store");
        let (count, end) = whole(&records, cut_at);
        let recovered = spool.recovered();
        assert_eq!(
            recovered.records(),
            &records[..count],
            "a store cut at {cut_at} byte(s): recovery proves the {count} record(s) whose seals \\
             verify and stops at the one the cut left incomplete"
        );
        assert_eq!(
            recovered.bytes(),
            end as u64,
            "a store cut at {cut_at} byte(s): and it continues at the end of the last record it \\
             proved"
        );
        assert_eq!(
            recovered.discarded(),
            (cut_at - end) as u64,
            "a store cut at {cut_at} byte(s): the bytes no append finished are reported, not \\
             quietly kept"
        );
        assert_eq!(
            recovered.stop(),
            if cut_at == end { Stop::End } else { Stop::Torn },
            "a store cut at {cut_at} byte(s): a file that ends on a record boundary ended, and \\
             one that stops inside a record is torn"
        );
        assert_eq!(
            file(&store).len(),
            end,
            "a store cut at {cut_at} byte(s): opening it cut every byte it refused — including \\
             the single byte a cut one byte past the last record leaves, which is the smallest \\
             torn tail a device can produce. A store that only *read* the log would leave that \\
             byte there and write the next append behind it, where no reader could ever see it"
        );
        assert_eq!(
            spool
                .records()
                .expect("the store reads back what it proved"),
            &records[..count],
            "a store cut at {cut_at} byte(s): its own reader answers the records the replay proved"
        );
        assert_eq!(
            spool.len(),
            end as u64,
            "a store cut at {cut_at} byte(s): it counts the log it has, not the log the file used \\
             to be"
        );
        assert_eq!(
            spool.durable_offset(),
            spool.len(),
            "a store cut at {cut_at} byte(s): what a previous process left is taken as given, byte \\
             for byte"
        );
        cut_at += 1;
    }
}

#[test]
fn a_reader_that_was_not_opened_since_the_cut_says_so_instead_of_answering_a_prefix() {
    let store = StoreDir::new("wal-strict");
    let mut spool = Spool::with_policy(store.path(), Policy::Never)
        .expect("a directory this process owns is a store");
    for index in 0..4u8 {
        spool
            .append(&payload(index))
            .expect("a store that can be written to");
    }

    // The file is cut *behind a store that is already open*, which is the one shape a caller can
    // meet without opening anything: the reader has bytes that stop mid-record.
    let whole_file = file(&store);
    cut(&store, whole_file.len() as u64 - 5);
    let refused = spool
        .records()
        .expect_err("a log that stops mid-record is not a shorter log");
    assert_eq!(
        refused.kind(),
        "protocol",
        "the reader refuses what it cannot prove instead of answering three records as if they \
         were the store: {refused}"
    );

    // Recovery is the reader that does answer a prefix, and a store that is *opened* now cuts the
    // tail and reads back whole.
    drop(spool);
    let spool = Spool::open(store.path()).expect("a store whose log was cut");
    let incomplete = segment::OVERHEAD + payload(3).len();
    assert_eq!(
        spool.recovered().discarded(),
        (incomplete - 5) as u64,
        "a record the cut left incomplete is discarded *whole*: it is not a record, so no part of \
         it is one — the bytes before the last whole record are what a store keeps"
    );
    assert_eq!(
        spool
            .records()
            .expect("a store that has been opened reads back")
            .len(),
        3,
        "the three records the cut left whole"
    );
}

#[test]
fn a_recovered_store_appends_where_the_log_ended() {
    let store = StoreDir::new("wal-continue");
    let records: Vec<Vec<u8>> = (0..5)
        .map(|index| sized(index, 16 + index as usize))
        .collect();
    let bytes = log(&records);
    let proved = segment::HEADER
        + records[..3]
            .iter()
            .map(|payload| segment::OVERHEAD + payload.len())
            .sum::<usize>();
    write(&store, &bytes);
    cut(&store, proved as u64 + segment::LENGTH as u64 + 1);

    let mut spool = Spool::open(store.path()).expect("a store whose log was cut");
    assert_eq!(spool.recovered().records(), &records[..3]);
    let extra = payload(99);
    spool
        .append(&extra)
        .expect("a recovered store takes the next record");
    let mut expected = records[..3].to_vec();
    expected.push(extra.clone());
    assert_eq!(
        spool.records().expect("the store reads back what it holds"),
        expected,
        "the appended record lands after the three that were proved, and nothing between them was \
         lost"
    );
    assert_eq!(
        file(&store).get(..proved),
        Some(&bytes[..proved]),
        "and the recovered prefix is the same bytes it was: recovery continues a log, it does not \
         rewrite one"
    );

    // A second open: no tail, nothing discarded, the same records.
    drop(spool);
    let spool = Spool::open(store.path()).expect("a store that was recovered once");
    assert_eq!(
        spool.recovered().discarded(),
        0,
        "a store that ended on a record boundary discards nothing when it is opened again"
    );
    assert_eq!(
        spool.len(),
        proved as u64 + segment::OVERHEAD as u64 + extra.len() as u64,
        "its length is the recovered prefix plus the record that was appended after it"
    );
}

#[test]
fn recovering_a_store_twice_is_the_same_store() {
    let store = StoreDir::new("wal-idempotent");
    let records: Vec<Vec<u8>> = (0..4).map(|index| sized(index, 18)).collect();
    let bytes = log(&records);
    let proved = segment::HEADER
        + records[..2]
            .iter()
            .map(|payload| segment::OVERHEAD + payload.len())
            .sum::<usize>();
    write(&store, &bytes);
    cut(&store, proved as u64 + segment::LENGTH as u64 + 3);

    let first = {
        let spool = Spool::open(store.path()).expect("a store whose log was cut");
        (
            spool.recovered().records().to_vec(),
            spool.recovered().bytes(),
            spool.recovered().discarded(),
        )
    };
    assert!(
        first.2 > 0,
        "the cut left bytes the first recovery had to refuse"
    );

    let spool = Spool::open(store.path()).expect("the same store, opened again");
    assert_eq!(
        spool.recovered().records().to_vec(),
        first.0,
        "there is no second chance at a log: what the first recovery proved is what the second \
         one finds"
    );
    assert_eq!(spool.recovered().bytes(), first.1);
    assert_eq!(
        spool.recovered().discarded(),
        0,
        "and once the tail is gone, there is nothing left to discard"
    );
    assert_eq!(
        file(&store).len(),
        first.1 as usize,
        "the file did not grow or shrink on the second open"
    );
}

#[test]
fn the_durable_boundary_is_exactly_what_a_machine_death_leaves() {
    let store = StoreDir::new("wal-boundary");
    let records: Vec<Vec<u8>> = (0..5).map(|index| sized(index, 21)).collect();
    let mut spool = Spool::with_policy(store.path(), Policy::Never)
        .expect("a directory this process owns is a store");
    for payload in &records[..3] {
        spool
            .append(payload)
            .expect("a store that can be written to");
    }
    spool
        .sync()
        .expect("a store directory that can be written to");
    let promised = spool.durable_offset();
    for payload in &records[3..] {
        spool
            .append(payload)
            .expect("a store that can be written to");
    }
    assert!(
        spool.durable_offset() < spool.len(),
        "without a flush the two records just written are not on the device: {} durable of {} \
         written, and `never` promises exactly that",
        spool.durable_offset(),
        spool.len()
    );
    let expected = segment::HEADER
        + records[..3]
            .iter()
            .map(|payload| segment::OVERHEAD + payload.len())
            .sum::<usize>();
    assert_eq!(
        promised, expected as u64,
        "the boundary is a byte offset into the log, which is what makes it a thing a death can be \
         measured against"
    );
    drop(spool);

    // The machine stops: the page cache goes with it, and the file is exactly as far as the store
    // had flushed. (This is the same model the step's simulation uses, one death at a time.)
    cut(&store, promised);
    let spool = Spool::open(store.path()).expect("a store whose log was cut");
    assert_eq!(
        spool
            .records()
            .expect("the store reads back what it promised"),
        &records[..3],
        "a machine death takes what was never flushed and nothing that was"
    );
    assert_eq!(
        spool.recovered().bytes(),
        promised,
        "the recovered end is the boundary the write path promised, to the byte"
    );
    assert_eq!(
        spool.recovered().discarded(),
        0,
        "the file was cut *at* a record boundary, so nothing here was torn — only lost"
    );
}

#[test]
fn a_daemon_that_was_killed_and_restarted_serves_the_records_it_acknowledged() {
    let mut service = Service::start("wal-killed", &[]);
    service.send(&wire(&frames()));
    let line = service.wait_for_line("accepted=3");
    assert!(
        line.contains("connection closed"),
        "the daemon accepted the three frames before it died: {line}"
    );

    // A process death: the kernel outlived it, so the page cache did too and *nothing written is
    // lost*, whatever the policy says about the device.
    service.kill();
    service.restart();
    let recovered = service.wait_for_line("store recovered");
    assert!(
        recovered.contains("records=3"),
        "the daemon says how many records opening the store proved: {recovered}"
    );
    assert!(
        recovered.contains("discarded=0"),
        "and a process death leaves no torn tail for it to discard: {recovered}"
    );

    let spool = Spool::open(service.store().path()).expect("the store the daemon writes");
    let expected: Vec<Vec<u8>> = records().iter().map(batch_of).collect();
    assert_eq!(
        spool.records().expect("records the store can read back"),
        expected,
        "every record the daemon acknowledged is there after the kill, in the order it was accepted"
    );
}

#[test]
fn a_clean_stop_flushes_the_window_the_policy_left_open() {
    let mut service = Service::start("wal-clean-stop", &[]);
    service.send(&wire(&frames()));
    service.wait_for_line("accepted=3");
    service.stop();
    let code = service.wait_for_exit();
    assert_eq!(code, 0, "a requested stop is not a failure");

    let flushed = service.wait_for_line("flushed the store");
    let written = field(&flushed, "written");
    let durable = field(&flushed, "durable");
    assert_eq!(
        written, durable,
        "a clean stop waits for the device once, whatever the policy is, so the two numbers the \
         daemon prints are the same one: {flushed} — a store that stops with `durable` behind \
         `written` is the window `docs/crash-points.md` point 6 used to describe"
    );

    // What that buys: a restart after a clean stop costs nothing.
    let spool = Spool::open(service.store().path()).expect("the store the daemon writes");
    assert_eq!(
        spool.recovered().discarded(),
        0,
        "the log the clean stop left ends on a record boundary, so the next start has nothing to \
         throw away"
    );
    assert_eq!(
        spool
            .records()
            .expect("records the store can read back")
            .len(),
        3,
        "and it serves the records that were acknowledged"
    );
}

/// The `key=<number>` value in a daemon line, as a number.
fn field(line: &str, key: &str) -> u64 {
    let (_, rest) = line.split_once(&format!("{key}=")).unwrap_or_else(|| {
        panic!(
            "the line `{line}` carries no `{key}=` — the stop path reports the two numbers that \
                make a clean stop checkable"
        )
    });
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("`{key}=` in `{line}` is not a number: {error}"))
}

/// A record a client sends, as `lakelet/4.1-ingest-protocol`'s fixture sends them.
fn records() -> Vec<Record> {
    vec![
        Record {
            series: "cpu,host=h0".to_owned(),
            ts_millis: 1_700_000_000_000,
            value: 12.5,
        },
        Record {
            series: "cpu,host=h1".to_owned(),
            ts_millis: 1_700_000_001_000,
            value: -3.25,
        },
        Record {
            series: "mem,host=h0".to_owned(),
            ts_millis: 1_700_000_002_000,
            value: 0.0,
        },
    ]
}

/// A record's bytes, as the store holds them: the batch the client sent.
fn batch_of(record: &Record) -> Vec<u8> {
    batch::encode(std::slice::from_ref(record))
}

/// The bytes a client sends for the three records, batched one per frame.
fn wire(frames: &[Vec<u8>]) -> Vec<u8> {
    frames.iter().flatten().copied().collect()
}

/// The frames a client would send for `records`, batched one record per frame.
fn frames() -> Vec<Vec<u8>> {
    let mut frames: Vec<Vec<u8>> = Vec::new();
    for record in &records() {
        frames.push(Frame::new(batch_of(record)).encode());
    }
    frames
}
