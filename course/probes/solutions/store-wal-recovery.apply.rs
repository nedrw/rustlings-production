//! probe = "store/wal-recovery@apply"
//! title = "A log is a prefix, and replay stops where it can no longer prove"
//! hint = "Replay until the first byte you cannot prove and keep everything before it, cut the tail an open refuses, and make a second open find nothing left to discard."
//!
//! Step `lakelet/4.4-wal-recovery` is about what a store can still prove after a crash, and the
//! answer is one sentence: **a log is a prefix.** Five rules follow from it, and each one is a thing
//! recovery code gets wrong in a way nobody notices until the day it matters:
//!
//! * **replay stops at the first byte it cannot prove.** Not at the first bad *record*: at the first
//!   byte. A log is a prefix by construction, so "the record after a bad one" is an offset the bad
//!   record's length field described — it is not a place in the file, it is a conclusion drawn from
//!   bytes that have just been shown to be untrustworthy;
//! * **a gap is worse than a short log.** A store that returns records 1–10, drops 11 and returns 12
//!   has told every reader the series was continuous. Losing a tail is a smaller answer the counts
//!   report; inventing continuity is a wrong answer nobody can see;
//! * **a torn tail is not an error, a record the device changed is damage.** Bytes that stop inside
//!   a record are a write that was interrupted — the input this step exists for — and the record
//!   they cut is simply not a record. A record whose seal does not match is a different fact: every
//!   byte is here and they are not the bytes that were written, and the report names the byte it was
//!   found at;
//! * **opening a store is recovering it, and it cuts what it refused.** The replay happens at open,
//!   and the bytes no append finished are removed, so the next append continues at the recovered end
//!   instead of behind bytes nobody can read. A second open therefore finds the same records and
//!   nothing left to discard — a recovery that reported something new every time would make every
//!   count in the logs useless;
//! * **the file declares itself.** Four bytes of magic and a version make "this is not this store's
//!   log" a decision rather than a guess: without them, `hi!` and a log whose first write was cut
//!   after two bytes are the same bytes, and recovery would have to choose between calling one of
//!   them corrupt and calling the other empty.
//!
//! ## What you write
//!
//! * [`append_line`] — one record's bytes appended to the log, with the header written by the first
//!   record;
//! * [`replay`] — the records whose seals verified, how many bytes of the log that is, how many it
//!   threw away, and why it stopped;
//! * [`recover`] — opening a store: replay, cut, and answer what was found.
//!
//! [`Record`], [`bytes_of`], [`seal`], [`verifies`], [`crc32`], [`u32_at`], [`Stop`], [`Refused`] and
//! [`Recovered`] are given, with the layout constants. The record's bytes are the format — the step's
//! spec reads them through the same constants — and a probe that let you invent them would be
//! checking a different store; the refusal vocabulary is what a caller branches on, and the seal is
//! given for the reason in the last section.
//!
//! ## What the tests ask
//!
//! One append writes the header and one record, the next append writes only its record, and the
//! record's bytes are the layout the prose promises. Every cut point of a five-record log, from zero
//! bytes to the whole file, recovers to a prefix of the records, proves no more bytes than the file
//! holds, and accounts for all of them: a torn tail is never an error. A file that is not this log is
//! refused by naming what was found, a header cut short is a torn tail rather than a foreign file,
//! and an empty file is an empty store. A record whose seal changed is `Corrupt` at the byte it
//! starts at, nothing after it is handed back, and a cut inside the last record is `Torn` with the
//! records before it. A record sealed over its payload alone is not proved, because the length is
//! what decides where a record ends. And two opens in a row are the same store — the second cuts
//! nothing — after which the next append lands after the records that were kept.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no file, no `set_len`, no `fsync` and no store**: the log here is a
//! `Vec<u8>` and cutting it is `truncate` on the vector. The real step opens a store directory,
//! replays `ingest.log`, cuts the file itself, and answers through `Spool::recovered`; it is the same
//! decision one layer down, and the layer it leaves out is the filesystem.
//!
//! Four narrowings, stated rather than hidden:
//!
//! * **no flush policy.** When an append waits for the device, `durable_offset`, and the crash-point
//!   table that says what each death leaves are `lakelet/4.3-write-path-adr`'s — the `aware` half of
//!   this slot, which this step is required by and does not re-decide. Nothing here is about *when*
//!   bytes reach the device, only about what a reader can prove from the bytes that are there;
//! * **no simulator.** The step's `tests/sim_wal.rs` kills the process, the machine and the write in
//!   flight under `slow:sim@3`; every failure here is a value in a `Vec`. The sweep over every cut
//!   point is what stands in for the deaths, and that sweep is the step's own headline case rather
//!   than a smaller version of it;
//! * **the seal is given, and it covers the length as well as the payload.** That decision is the
//!   trap of the step (`a seal over the payload alone`), and it lives in the given [`seal`]: a reader
//!   that has only the bytes in front of it must decide where a record *ends*, so the length is part
//!   of what has to be proved. What this file re-checks is the reader's half — a record whose length
//!   moved, or whose seal covers only its payload, is not handed back;
//! * **no repair.** Recovery reads and cuts. It never rewrites a record, never invents one, and never
//!   rescues the record after a bad one.

/// The four bytes at the start of every log this build writes.
pub const MAGIC: [u8; 4] = *b"LKLT";

/// The version of the format above: a file that says anything else is not this build's log.
pub const VERSION: u8 = 1;

/// The bytes a log starts with: [`MAGIC`], then [`VERSION`].
pub const HEADER_BYTES: [u8; 5] = [MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], VERSION];

/// How many bytes of a log come before its first record.
pub const HEADER: usize = HEADER_BYTES.len();

/// The length one record states in front of its payload: four bytes, little endian.
pub const LENGTH: usize = 4;

/// The bytes one seal takes: four, because it is a `u32` little endian.
pub const SEAL: usize = 4;

/// Bytes of framing around one record's payload: its [`LENGTH`] and its seal.
pub const OVERHEAD: usize = LENGTH + SEAL;

/// One record of the log: the payload one append carried.
///
/// Opaque on purpose. What a record *means* is the batch codec's business
/// (`lakelet/4.1-ingest-protocol`), and this file is about where a record ends on a device a crash
/// may have cut in the middle of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The bytes one append carried.
    pub payload: Vec<u8>,
}

impl Record {
    /// A record carrying `payload`.
    #[must_use]
    pub fn new(payload: impl Into<Vec<u8>>) -> Self {
        Self {
            payload: payload.into(),
        }
    }
}

/// Where a walk over a log's bytes stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Every byte of the file is whole records: nothing was left over.
    End,
    /// The bytes stop inside a record — the file holds fewer than the record states, or a record's
    /// own framing is cut before its length. A write that was interrupted leaves this, and so does a
    /// file whose header was cut short. It is not a refusal: it is the input recovery exists for.
    Torn,
    /// A whole record's worth of bytes is there and is not the record it claims: the seal does not
    /// match, so these bytes changed after they were written. The record is not handed back, and
    /// neither is anything after it.
    Corrupt {
        /// Where in the log the record that failed its seal starts.
        at: usize,
    },
}

impl Stop {
    /// The word a log line, a report and an operator call this stop by.
    ///
    /// Stable on purpose, like the step's: `end`, `torn` and `corrupt` are the three things recovery
    /// can find, and a report that spelled them differently in two places would make an operator
    /// compare two words for one fact.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Stop::End => "end",
            Stop::Torn => "torn",
            Stop::Corrupt { .. } => "corrupt",
        }
    }
}

/// Why these bytes are not this store's log.
///
/// A refusal is not a recovery. A file whose header is not this log is not a log with no records in
/// it, and telling those two apart is the whole reason a file declares itself. Each variant carries
/// what was found, because an operator holding a directory they did not expect needs to know which
/// file is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The file does not start with this log's magic.
    NotThisLog {
        /// The bytes the header holds, as far as the file has them.
        found: Vec<u8>,
    },
    /// The file announces a format version this build cannot replay.
    Version {
        /// The version the file announces.
        seen: u8,
        /// [`VERSION`], the version this build writes and reads.
        spoken: u8,
    },
}

/// What replaying a log proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    /// The records whose seals verified, in the order they were written.
    records: Vec<Record>,
    /// Bytes of the log that are proved: the header plus every whole record.
    verified: usize,
    /// Bytes of the file no walk could prove.
    discarded: usize,
    /// Why the walk stopped there.
    stop: Stop,
}

impl Recovered {
    /// What one replay proved.
    ///
    /// `verified` is the length of the proven prefix and `discarded` is what the file holds above it,
    /// so the two add up to the file's length: that is what makes the counts a report rather than a
    /// guess.
    #[must_use]
    pub fn new(records: Vec<Record>, verified: usize, discarded: usize, stop: Stop) -> Self {
        Self {
            records,
            verified,
            discarded,
            stop,
        }
    }

    /// The records whose seals verified, in the order they were written.
    #[must_use]
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// How many bytes of the log are proved: its header plus every whole record.
    ///
    /// The offset the store continues at: everything below it is a record whose seal verified, and
    /// everything above it is bytes no walk could prove.
    #[must_use]
    pub fn verified(&self) -> usize {
        self.verified
    }

    /// How many bytes of the file no walk could prove.
    #[must_use]
    pub fn discarded(&self) -> usize {
        self.discarded
    }

    /// Why the walk stopped where it did.
    #[must_use]
    pub fn stop(&self) -> Stop {
        self.stop
    }
}

/// The little-endian `u32` at `offset`, or `None` when the bytes do not hold four of them there.
///
/// Given because the length field and the seal are read through it: two readers of the same four
/// bytes are two chances for a reader to disagree with the writer about which field it is looking at.
#[must_use]
pub fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + LENGTH)?;
    let mut array = [0u8; LENGTH];
    array.copy_from_slice(slice);
    Some(u32::from_le_bytes(array))
}

/// The CRC-32 (ISO-HDLC) of `bytes`.
///
/// Given, and the checksum both of this project's formats share (`crc32fast` in the project, written
/// out bitwise here because a probe has no dependencies): what is being re-checked is which bytes a
/// seal covers, not how fast it is computed.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// The seal a record with this `length` and `payload` carries.
///
/// `length` is the payload's length as the record states it, and **both it and every payload byte**
/// are covered: every byte that decides how far a record reaches. A seal over the payload alone
/// would let a damaged length field move the end of a record to bytes the seal still covered.
#[must_use]
pub fn seal(length: u32, payload: &[u8]) -> u32 {
    let mut covered = length.to_le_bytes().to_vec();
    covered.extend_from_slice(payload);
    crc32(&covered)
}

/// `true` when `expected` is the seal these bytes carry.
#[must_use]
pub fn verifies(length: u32, payload: &[u8], expected: u32) -> bool {
    seal(length, payload) == expected
}

/// The bytes one record occupies: its length, its payload and its seal.
///
/// Given because the format is the step's — its spec reads the bytes through [`HEADER`], [`LENGTH`]
/// and [`OVERHEAD`] — and because a writer that got them wrong would make every test in this file a
/// test of the writer.
#[must_use]
pub fn bytes_of(record: &Record) -> Vec<u8> {
    let length = record.payload.len() as u32;
    let mut bytes = Vec::with_capacity(OVERHEAD + record.payload.len());
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&record.payload);
    bytes.extend_from_slice(&seal(length, &record.payload).to_le_bytes());
    bytes
}

/// Append `record` to the log, writing the file's header first when the log is empty.
///
/// The header and the first record leave together: a file with a record in it and no header is not a
/// log this format can read. A store that has accepted nothing holds an empty file — which is also
/// the one file that is not a log *yet* and is not a foreign file either.
pub fn append_line(log: &mut Vec<u8>, record: &Record) {
    if log.is_empty() {
        log.extend_from_slice(&HEADER_BYTES);
    }
    log.extend_from_slice(&bytes_of(record));
}

/// Read `log` as this store's log: the records whose seals verified, and where the walk stopped.
///
/// Never panics, never reserves the length a record states — a length the bytes cannot satisfy ends
/// the walk as [`Stop::Torn`], like any other short tail — and never skips a record it cannot prove.
///
/// An empty file is an empty store, and a header cut short is a torn tail: a crash during the very
/// first append leaves exactly that. Anything else that is not this log is refused.
///
/// # Errors
///
/// [`Refused`] when the bytes are not this store's log at all.
pub fn replay(log: &[u8]) -> Result<Recovered, Refused> {
    if log.is_empty() {
        return Ok(Recovered::new(Vec::new(), 0, 0, Stop::End));
    }
    // A file shorter than the header has to be the beginning of one: a crash during the very first
    // append leaves exactly that, and `hi!` is not a log that lost its tail.
    if log.len() < HEADER {
        if HEADER_BYTES.starts_with(log) {
            return Ok(Recovered::new(Vec::new(), 0, log.len(), Stop::Torn));
        }
        return Err(Refused::NotThisLog {
            found: log.to_vec(),
        });
    }
    if log.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
        return Err(Refused::NotThisLog {
            found: log.get(..HEADER).unwrap_or_default().to_vec(),
        });
    }
    let seen = log[MAGIC.len()];
    if seen != VERSION {
        return Err(Refused::Version {
            seen,
            spoken: VERSION,
        });
    }

    let mut records = Vec::new();
    let mut cursor = HEADER;
    let mut stop = Stop::End;
    while cursor < log.len() {
        // Every read below answers `None` rather than panicking, and what the *file* holds decides
        // whether a record is there: the length is compared against the bytes before anything is
        // reserved with it.
        let Some(length) = u32_at(log, cursor) else {
            stop = Stop::Torn;
            break;
        };
        let start = cursor + LENGTH;
        let Some(end) = start.checked_add(length as usize) else {
            stop = Stop::Torn;
            break;
        };
        let Some(payload) = log.get(start..end) else {
            stop = Stop::Torn;
            break;
        };
        let Some(written) = u32_at(log, end) else {
            stop = Stop::Torn;
            break;
        };
        if !verifies(length, payload, written) {
            stop = Stop::Corrupt { at: cursor };
            break;
        }
        records.push(Record::new(payload));
        cursor = end + SEAL;
    }
    Ok(Recovered::new(records, cursor, log.len() - cursor, stop))
}

/// Open the store whose log is `log`: replay it, cut the bytes no append finished, and answer what
/// was found.
///
/// Opening a store is recovering it, and recovering it twice is the same store: the tail the first
/// open refused is gone, so the second one proves the same records and finds nothing left to
/// discard. What a refused open must not do is touch the file at all — a foreign file that was cut
/// by a command that answered "not this store" would be the worst of both.
///
/// # Errors
///
/// [`Refused`] when the bytes are not this store's log: a foreign file is not cut.
pub fn recover(log: &mut Vec<u8>) -> Result<Recovered, Refused> {
    let recovered = replay(log)?;
    // Cut before answering, so the next append continues at the recovered end instead of behind
    // bytes nobody can read.
    log.truncate(recovered.verified());
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload that is not round-numbered, so a length and a payload mixed up cannot pass.
    fn payload(marker: u8) -> Vec<u8> {
        (0..11u8).map(|index| marker.wrapping_add(index)).collect()
    }

    /// One record, with its payload decided by `marker`.
    fn record(marker: u8) -> Record {
        Record::new(payload(marker))
    }

    /// The bytes one record takes, which is also the distance between two records in a log.
    fn stride() -> usize {
        OVERHEAD + payload(0).len()
    }

    /// A log of `markers`, written one append at a time, the way a store writes one.
    fn log(markers: &[u8]) -> Vec<u8> {
        let mut log = Vec::new();
        for marker in markers {
            append_line(&mut log, &record(*marker));
        }
        log
    }

    /// Where each of `markers` starts, so a test can name the byte it damages.
    fn starts(markers: &[u8]) -> Vec<usize> {
        (0..markers.len())
            .map(|index| HEADER + index * stride())
            .collect()
    }

    #[test]
    fn a_log_round_trips_and_its_first_record_declares_the_file() {
        assert_eq!(HEADER, 5, "four bytes of magic and one of version");
        assert_eq!(
            OVERHEAD, 8,
            "a record's framing is its four-byte length and its four-byte seal"
        );

        let mut log = Vec::new();
        append_line(&mut log, &record(1));
        append_line(&mut log, &record(2));
        let mut written = HEADER_BYTES.to_vec();
        written.extend_from_slice(&bytes_of(&record(1)));
        written.extend_from_slice(&bytes_of(&record(2)));
        assert_eq!(
            log, written,
            "the header is written with the first record and never again, and an append writes the \
             record's own bytes"
        );

        // The record's bytes are the layout the prose promises: a length over a payload over a seal.
        let bytes = bytes_of(&record(1));
        assert_eq!(
            bytes.len(),
            stride(),
            "a record is its framing plus its payload"
        );
        let stated = u32_at(&bytes, 0).expect("four bytes of length");
        assert_eq!(
            stated as usize,
            payload(1).len(),
            "the length counts the payload"
        );
        let written_seal = u32_at(&bytes, LENGTH + payload(1).len()).expect("four bytes of seal");
        assert!(
            verifies(stated, &payload(1), written_seal),
            "the seal the writer wrote is the seal of the length and the payload together"
        );
        assert_ne!(
            written_seal,
            crc32(&payload(1)),
            "and the seal is not its payload's checksum alone: a reader with only the bytes in front \
             of it has to decide where a record ends, so the length is part of what is proved"
        );

        let recovered = replay(&log).expect("a log this file wrote is this file's log");
        assert_eq!(
            recovered.records(),
            &[record(1), record(2)][..],
            "the records, in the order they were written"
        );
        assert_eq!(recovered.verified(), log.len(), "the whole file is proved");
        assert_eq!(recovered.discarded(), 0, "and nothing was thrown away");
        assert_eq!(recovered.stop(), Stop::End, "the walk consumed the file");
    }

    #[test]
    fn every_cut_point_of_a_written_log_recovers_to_a_prefix_of_its_records() {
        let markers = [1u8, 2, 3, 4, 5];
        let written = log(&markers);
        let ends: Vec<usize> = (1..=markers.len())
            .map(|count| HEADER + count * stride())
            .collect();
        let all: Vec<Record> = markers.iter().map(|marker| record(*marker)).collect();

        for cut in 0..=written.len() {
            let bytes = written.get(..cut).expect("a prefix of the log");
            let whole = ends.iter().filter(|end| **end <= cut).count();
            let recovered = replay(bytes).unwrap_or_else(|refused| {
                panic!(
                    "a prefix of this store's own log was refused after {cut} byte(s): {refused:?}"
                )
            });
            assert_eq!(
                recovered.records().len(),
                whole,
                "a {cut}-byte file holds {whole} whole record(s) and the walk proved {}: a torn tail \
                 is not an error, and a record is not proved by being nearly here",
                recovered.records().len()
            );
            assert!(
                all.starts_with(recovered.records()),
                "the records of a {cut}-byte prefix are not a prefix of the records that were written"
            );
            assert!(
                recovered.verified() + recovered.discarded() == cut,
                "every byte of a {cut}-byte file is either proved or discarded: {} proved and {} \
                 discarded",
                recovered.verified(),
                recovered.discarded()
            );
            assert!(
                recovered.verified() <= cut,
                "the walk proved {} byte(s) of a {cut}-byte file",
                recovered.verified()
            );
        }
    }

    #[test]
    fn a_file_that_is_not_this_log_is_refused_and_a_cut_header_is_not_a_file() {
        assert_eq!(
            replay(b"hi!"),
            Err(Refused::NotThisLog {
                found: b"hi!".to_vec()
            }),
            "a file that does not declare itself is refused rather than read as a store with no \
             records: without the header, `hi!` and a log whose first write was cut after two bytes \
             are the same bytes"
        );

        // The other way round: the beginning of a header *is* this log's beginning, and a crash
        // during the very first append leaves exactly that.
        for cut in 1..HEADER {
            let bytes = HEADER_BYTES.get(..cut).expect("a prefix of the header");
            let recovered = replay(bytes).expect("the beginning of this log is not a foreign file");
            assert_eq!(recovered.records(), &[] as &[Record]);
            assert_eq!(
                recovered.verified(),
                0,
                "nothing is proved before the header is whole"
            );
            assert_eq!(recovered.discarded(), cut);
            assert_eq!(
                recovered.stop(),
                Stop::Torn,
                "a cut header is a tail, not a refusal"
            );
        }

        // And a store that has accepted nothing is an empty store: not foreign, not torn.
        assert_eq!(
            replay(&[]).expect("an empty store is a store"),
            Recovered::new(Vec::new(), 0, 0, Stop::End),
            "an empty file is an empty store"
        );

        // A version this build cannot replay is its own refusal, named with both versions.
        let mut future = HEADER_BYTES.to_vec();
        future[MAGIC.len()] = VERSION.wrapping_add(1);
        assert_eq!(
            replay(&future),
            Err(Refused::Version {
                seen: VERSION.wrapping_add(1),
                spoken: VERSION
            }),
            "a version this build cannot replay is refused, not read as an empty store"
        );

        // And a refused open leaves the file alone.
        let mut foreign = b"hi!".to_vec();
        assert!(recover(&mut foreign).is_err());
        assert_eq!(foreign, b"hi!", "a file that is not this log is not cut");
    }

    #[test]
    fn a_torn_tail_is_not_damage_and_a_record_the_device_changed_is() {
        let markers = [1u8, 2, 3];
        let written = log(&markers);
        let starts = starts(&markers);

        // A write that was interrupted three bytes into the last record.
        let torn = written.get(..written.len() - 3).expect("a cut log");
        let recovered = replay(torn).expect("a torn tail is the input this step exists for");
        assert_eq!(
            recovered.stop(),
            Stop::Torn,
            "the bytes stop inside a record"
        );
        assert_eq!(
            recovered.records(),
            &[record(1), record(2)][..],
            "the record whose write was cut is not a record, and the ones before it are"
        );
        assert_eq!(
            recovered.verified(),
            starts[2],
            "the proven prefix ends where the record that was cut starts"
        );
        assert_eq!(
            recovered.discarded(),
            torn.len() - starts[2],
            "the discarded bytes are the whole of the record the crash cut, not only the three that \
             never arrived: the bytes the record would have occupied are not data either"
        );

        // A device that wrote the whole write with one byte changed: every byte is here, and they are
        // not the bytes that were written.
        let mut damaged = written.clone();
        damaged[starts[1] + LENGTH] ^= 0x01;
        let recovered = replay(&damaged).expect("damage is a report, not a refusal to read a file");
        assert_eq!(
            recovered.stop(),
            Stop::Corrupt { at: starts[1] },
            "the report names the byte the record that failed its seal starts at"
        );
        assert_eq!(
            recovered.records(),
            &[record(1)][..],
            "the record that changed is not handed back"
        );
        assert_eq!(recovered.verified(), starts[1]);
        assert_eq!(recovered.discarded(), damaged.len() - starts[1]);

        // And the three words a log line prints are stable.
        assert_eq!(Stop::End.name(), "end");
        assert_eq!(Stop::Torn.name(), "torn");
        assert_eq!(recovered.stop().name(), "corrupt");
    }

    #[test]
    fn a_record_the_replay_cannot_prove_is_not_skipped_over() {
        let markers = [1u8, 2, 3, 4];
        let written = log(&markers);
        let starts = starts(&markers);
        let mut damaged = written.clone();
        damaged[starts[1] + LENGTH + 2] ^= 0x01;

        let recovered = replay(&damaged).expect("damage is a report, not a refusal");
        assert_eq!(
            recovered.records(),
            &[record(1)][..],
            "a store that returned records 1, 3 and 4 has told every reader the series was \
             continuous: the answer is the prefix, and nothing after the record that failed"
        );
        assert_eq!(
            recovered.verified(),
            starts[1],
            "the proven prefix ends where the record that failed starts — its length field is not a \
             place in the file, it is a conclusion drawn from bytes that have just been shown to be \
             untrustworthy"
        );
    }

    #[test]
    fn a_seal_over_the_payload_alone_does_not_prove_a_record() {
        // The step's trap, from the reader's side. This second record's seal is the checksum of its
        // payload and nothing else — which is what a seal that ignored the length would accept. The
        // seal the format writes covers the length too, so a length field that moved cannot hand back
        // bytes that were never a record.
        let mut forged = HEADER_BYTES.to_vec();
        forged.extend_from_slice(&bytes_of(&record(1)));
        forged.extend_from_slice(&0u32.to_le_bytes());
        forged.extend_from_slice(&crc32(&[]).to_le_bytes());

        let recovered = replay(&forged).expect("these bytes are still this store's log");
        assert_eq!(
            recovered.stop(),
            Stop::Corrupt {
                at: HEADER + bytes_of(&record(1)).len()
            },
            "a record whose seal covers only its payload is not proved: the seal is over the length \
             too, and the length is what decides where a record ends"
        );
        assert_eq!(
            recovered.records(),
            &[record(1)][..],
            "the record that was not proved is not handed back, and neither is anything after it"
        );
    }

    #[test]
    fn recovering_a_store_cuts_the_tail_and_the_second_open_finds_nothing_left_to_discard() {
        let markers = [1u8, 2, 3];
        let whole = log(&markers);
        let mut store = whole.clone();
        // A machine died inside the last append.
        store.truncate(whole.len() - 3);

        let on_disk = store.len();
        let first = recover(&mut store).expect("a cut log is still this store's log");
        assert_eq!(
            first.records(),
            &[record(1), record(2)][..],
            "the records the appends finished"
        );
        assert_eq!(first.stop(), Stop::Torn);
        assert_eq!(
            first.verified(),
            HEADER + 2 * stride(),
            "the proven prefix is the header and the two records the appends finished"
        );
        assert_eq!(
            first.discarded(),
            on_disk - first.verified(),
            "the bytes no walk could prove are the whole of the record the crash cut"
        );
        assert_eq!(
            store.len(),
            first.verified(),
            "opening the store cut the bytes no append finished: a store that only reads leaves them \
             there, and the next append lands behind bytes nobody can read"
        );

        // Recovery is idempotent: a second start proves the same records, continues at the same
        // offset, and has nothing left to discard.
        let second = recover(&mut store).expect("the store the first open left behind");
        assert_eq!(second.records(), first.records(), "the same records");
        assert_eq!(
            second.verified(),
            first.verified(),
            "the same end of the log"
        );
        assert_eq!(second.discarded(), 0, "the tail is already gone");
        assert_eq!(
            second.stop(),
            Stop::End,
            "and there is no tail left to stop at"
        );

        // The next append continues at the recovered end, never over a record that was kept.
        append_line(&mut store, &record(4));
        let after = recover(&mut store).expect("the store after an append");
        assert_eq!(
            after.records(),
            &[record(1), record(2), record(4)][..],
            "an append after recovery lands after the records that were kept"
        );
        assert_eq!(after.stop(), Stop::End);
        assert_eq!(after.discarded(), 0);
    }
}
