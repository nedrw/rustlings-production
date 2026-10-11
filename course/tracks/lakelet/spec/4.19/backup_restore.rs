//! Spec for `lakelet/4.19-backup-restore`: the store as the unit an operator moves.
//!
//! `kiln run` copies this file into the project as `tests/spec_backup_restore.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Twenty cases, and the shape of them is the point:
//!
//! * **a copy is the store's own file set.** The manifest is the file list
//!   (`lakelet/4.8-compaction`), so what a copy names is the manifest, the segments the manifest
//!   names and the log's proven prefix — no sidecar index (derived state,
//!   `lakelet/4.14-index-pruning`), no `*.pending`, and nothing the manifest does not name;
//! * **a copy reads its source and never writes it.** It comes in through the store's manifest
//!   alone (`compaction::Manifest::load`, the read-only door this step is given) and not through an
//!   open, which creates `segments/`, flushes the directory and deletes every file the manifest
//!   does not name: an orphan and a half-written `segments.manifest.pending` are still there after
//!   a copy, and a source that *is* a read-only mount can be copied at all;
//! * **the copy's manifest is `lakelet/4.4-wal-recovery`'s log.** A torn tail entry is dropped with
//!   the bytes it occupied and the `Stop` that says so, which is a case that only holds if the format
//!   really is that log — the case writes the tail with `wal::segment::encode`;
//! * **an interruption is a value, not a race.** `Interruption::AfterBytes` stops the copy inside a
//!   file, that file is left partial and *unnamed*, and everything the copy did name verifies. A
//!   copy interrupted before its first byte is a manifest that names nothing, which is a different
//!   answer from a directory that holds no manifest at all;
//! * **a restore verifies before it writes.** A copy missing a file, a copy whose byte flipped and a
//!   copy that was cut short are three typed refusals with the numbers in the sentence, and the
//!   destination is exactly as it was found: no directory, nothing written;
//! * **a restore never overwrites and never merges.** A destination that already holds a store is
//!   refused and the store is left alone; a destination that holds a *copy* is the same answer, and a
//!   copy is not written over a store either;
//! * **the restored store is a store**, not a second format: `lakelet/4.8-compaction`'s open reads
//!   it, `lakelet/4.11-query-engine`'s executor answers the same question the same way, and
//!   `lakelet/4.18-cardinality-cost`'s ledger holds the same series — with the one difference a
//!   restore is *supposed* to have, which is what a missing index costs;
//! * **the page cannot drift from the code.** `docs/dr-drill.md` carries a fenced block that is one
//!   command's whole output, line for line, and the case re-runs that command and compares. A number
//!   edited by hand stops matching, which is the only way a page full of counts stays true
//!   (`lakelet/4.16-profiling`'s rule for its own page).
//!
//! Nothing here reads a clock, sleeps, or starts a thread. The store every case works on is written
//! through the real write path (`Spool` appends, `Segments::seal` closes), and the deaths are the
//! ones a copy and a restore can be asked for by name.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lakelet::engine::backup::{self, Interruption};
use lakelet::engine::cardinality::{Cap, Registry};
use lakelet::engine::compaction::{self, Segments};
use lakelet::engine::query::exec;
use lakelet::engine::query::parse::{self, Statement};
use lakelet::engine::spool::{FILE as LOG, Spool};
use lakelet::engine::wal::replay;
use lakelet::engine::wal::segment::{self, Stop};
use lakelet::protocol::batch::{self, Record};

use support::StoreDir;

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/dr-drill.md";

/// The first timestamp a fixture writes: 2023-11-14T22:13:20Z, a round number and not a date.
const BASE: i64 = 1_700_000_000_000;

/// The series a fixture's records are of.
const SERIES: [&str; 2] = ["cpu,host=h0", "cpu,host=h1"];

/// How far apart two records of one fixture are.
const STEP: i64 = 1_000;

/// One record of a fixture: index `at` in the store's history.
fn record(at: usize) -> Record {
    Record {
        series: SERIES[at % SERIES.len()].to_owned(),
        ts_millis: BASE + (at as i64) * STEP,
        value: (at as f64) / 4.0,
    }
}

/// A batch of `count` records, the first of them the `from`-th of the store's history.
fn batch(from: usize, count: usize) -> Vec<Record> {
    (from..from + count).map(record).collect()
}

/// A store of `segments` closed segments, each holding `per_segment` records, plus the log the write
/// path put those batches in.
///
/// The write path is the real one — a payload per batch through `Spool`, a segment per batch through
/// `Segments::seal` — because a copy is only interesting about a store that a deployment would have
/// built. The last batch is deliberately *not* sealed: a store whose every record is in a segment has
/// no log tail, and the log's proven prefix is half of what a copy is.
fn built(directory: &Path, segments: usize, per_segment: usize) -> Vec<Vec<Record>> {
    let mut spool = Spool::open(directory).expect("a directory this process owns is a store");
    let mut closed = Segments::open(directory).expect("the same directory is a segment store");
    let mut written: Vec<Vec<Record>> = Vec::new();
    for index in 0..=segments {
        let records = batch(index * per_segment, per_segment);
        spool
            .append(&batch::encode(&records))
            .expect("the write path takes what a client sent");
        if index < segments {
            closed.seal(&records).expect("a flush closes what it wrote");
        }
        written.push(records);
    }
    written
}

/// The same store, with **every** batch closed into a segment.
///
/// `lakelet/4.18-cardinality-cost`'s ledger is what needs this one: its prune refolds every series'
/// window onto the zones of the segments the store still holds, so a window is comparable with a
/// copy's only when the log holds no record a segment does not.
fn closed(directory: &Path, segments: usize, per_segment: usize) -> Vec<Vec<Record>> {
    let mut spool = Spool::open(directory).expect("a directory this process owns is a store");
    let mut closed = Segments::open(directory).expect("the same directory is a segment store");
    let mut written: Vec<Vec<Record>> = Vec::new();
    for index in 0..segments {
        let records = batch(index * per_segment, per_segment);
        spool
            .append(&batch::encode(&records))
            .expect("the write path takes what a client sent");
        closed.seal(&records).expect("a flush closes what it wrote");
        written.push(records);
    }
    written
}

/// The crc32 of a file, the checksum a copy's entries state.
fn crc32(path: &Path) -> u32 {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes);
    hasher.finalize()
}

/// A file's length, or a failure naming it.
fn length(path: &Path) -> u64 {
    std::fs::metadata(path)
        .unwrap_or_else(|error| panic!("{} cannot be measured: {error}", path.display()))
        .len()
}

/// Turn one byte of a file over, the way a device that wrote something else leaves it.
fn flip(path: &Path) {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("{} cannot be opened: {error}", path.display()));
    let at = length(path) / 2;
    file.seek(SeekFrom::Start(at))
        .expect("the file is that long");
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).expect("one byte is there");
    file.seek(SeekFrom::Start(at))
        .expect("the file is that long");
    file.write_all(&[byte[0] ^ 0xff])
        .expect("the byte can be written back");
}

/// Cut a file to `bytes`.
fn cut(path: &Path, bytes: u64) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("{} cannot be opened: {error}", path.display()))
        .set_len(bytes)
        .unwrap_or_else(|error| panic!("{} cannot be cut: {error}", path.display()));
}

/// Every file below `directory`, as `(name relative to it, how many bytes)`, in name order.
fn files_under(directory: &Path) -> Vec<(String, u64)> {
    let mut found: Vec<(String, u64)> = Vec::new();
    let mut pending: Vec<PathBuf> = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current)
            .unwrap_or_else(|error| panic!("{} cannot be listed: {error}", current.display()))
        {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            found.push((
                path.strip_prefix(directory)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
                length(&path),
            ));
        }
    }
    found.sort();
    found
}

/// The names a copy's own manifest holds, in the order the copy wrote them.
fn named(directory: &Path) -> Vec<String> {
    backup::Manifest::load(directory)
        .unwrap_or_else(|error| {
            panic!(
                "{} holds no manifest this build can read: {error}",
                directory.display()
            )
        })
        .entries()
        .iter()
        .map(|entry| entry.name().to_owned())
        .collect()
}

/// Every record the store at `directory` answers with, in the manifest's order.
fn answers(directory: &Path) -> Vec<Record> {
    Segments::open(directory)
        .unwrap_or_else(|error| panic!("{} is not a store: {error}", directory.display()))
        .snapshot()
        .records()
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", directory.display()))
}

/// The records one segment file of a store holds.
fn segment_records(directory: &Path, name: &str) -> Vec<Record> {
    lakelet::engine::segment::read(&directory.join(name)).unwrap_or_else(|error| {
        panic!(
            "the segment {name} of {} cannot be read: {error}",
            directory.display()
        )
    })
}

/// How many bytes of a store's log a replay proves: the prefix a copy takes.
fn proven_bytes(directory: &Path) -> u64 {
    replay::recover(directory)
        .unwrap_or_else(|error| panic!("{}'s log cannot be replayed: {error}", directory.display()))
        .bytes()
}

/// One run of the real binary: what a script would have seen.
#[derive(Debug)]
struct Ran {
    /// The exit code.
    code: i32,
    /// What standard output received.
    stdout: String,
    /// What standard error received.
    stderr: String,
}

/// Run `lakeletq` with no environment and `/dev/null` on standard input.
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

/// The page this case reads, from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

/// The block a page quotes, and whether it carries a timing line.
///
/// The block is the fenced one that carries a **whole** drill run: its first line is where a drill's
/// output begins and it holds the digests a restore is judged by — the lines are what identify it,
/// not a heading, because the answer of a command is `name=value` lines with no prefix
/// (`lakelet/4.10-downsampling`'s contract). A page may quote a *fragment* of the same run elsewhere
/// — the two RPO numbers, say — and that is not the block this case compares.
fn quoted(page: &str) -> (Vec<String>, bool) {
    let lines: Vec<&str> = page.lines().collect();
    let anchor = lines
        .iter()
        .position(|line| line.starts_with("restored_digest="))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} does not quote a drill: a reader has to be able to see the digest over the \
                 records the restored store answers with, and no line of the page states one"
            )
        });
    let opening = lines[..anchor]
        .iter()
        .rposition(|line| line.trim_start().starts_with("```"))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} has a drill's line at {anchor} outside a fenced block: a reader has to be \
                 able to copy the block out of the page"
            )
        });
    let closing = anchor
        + lines[anchor..]
            .iter()
            .position(|line| line.trim_start().starts_with("```"))
            .unwrap_or_else(|| panic!("{PAGE} opens a fenced block it never closes"));
    assert!(
        lines
            .get(opening + 1)
            .is_some_and(|line| line.starts_with("write_policy=")),
        "{PAGE}'s block does not begin where a drill's output begins: quote the whole run — the \
         command's first line to its last — so that a reader sees the same thing the spec compares"
    );
    assert!(
        lines[..opening]
            .iter()
            .rev()
            .any(|line| line.contains("lakeletq drill")),
        "{PAGE} does not say which command produced the block it carries: state `lakeletq drill` in \
         the sentence above it"
    );
    let body: Vec<String> = lines[opening + 1..closing]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();
    (body, page.contains("restore_millis"))
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

/// One source file of the project, as text.
fn source(relative: &str) -> String {
    std::fs::read_to_string(relative).unwrap_or_else(|error| {
        panic!("{relative} cannot be read: {error} — this case is about it")
    })
}

#[test]
fn a_complete_copy_names_exactly_the_stores_files() {
    let source_dir = StoreDir::new("spec-backup-names");
    let copy = StoreDir::new("spec-backup-names-copy");
    let batches = built(source_dir.path(), 3, 4);

    // Two files that are *not* the store, planted where a crash leaves them: a segment the manifest
    // does not name (an orphan `lakelet/4.8-compaction` deletes on the next open) and a half-written
    // manifest. Neither is part of what a store is, so neither is part of a copy.
    std::fs::write(
        source_dir.path().join("segments/0000000099.lks"),
        b"an orphan",
    )
    .expect("writable");
    std::fs::write(
        source_dir.path().join(compaction::PENDING),
        b"a manifest nobody finished",
    )
    .expect("writable");

    let report = backup::copy(source_dir.path(), copy.path(), Interruption::None)
        .expect("a store this process built can be copied");
    let names = named(copy.path());

    let mut expected = vec![compaction::MANIFEST.to_owned()];
    for info in Segments::open(source_dir.path())
        .expect("the store opens")
        .manifest()
        .segments()
    {
        expected.push(format!(
            "{}/{info_index:0>10}.{extension}",
            compaction::DIRECTORY,
            info_index = info.index,
            extension = compaction::EXTENSION
        ));
    }
    expected.push(LOG.to_owned());
    assert_eq!(
        names, expected,
        "a copy names the store's manifest, every segment the manifest names in its order, and the \
         log — and this one named {names:?}"
    );
    assert_eq!(
        report.files(),
        expected.len() as u64,
        "the copy reports {} file(s) and named {}",
        report.files(),
        expected.len()
    );
    assert_eq!(
        report.entries(),
        expected.len() as u64,
        "every file of a complete copy is named: {} entries for {} files",
        report.entries(),
        expected.len()
    );
    assert!(
        !report.interrupted(),
        "a copy asked for everything was interrupted"
    );
    assert!(
        !copy
            .path()
            .join(compaction::DIRECTORY)
            .join("0000000099.lks")
            .exists(),
        "the copy took a file in {} that the manifest does not name: a file the manifest does not \
         name is an orphan a crash left behind, and a copy of a store is a copy of what the store is",
        compaction::DIRECTORY
    );
    assert!(
        !copy.path().join(compaction::PENDING).exists(),
        "the copy took `{}`, which is a manifest nobody finished: it is not part of what a store is",
        compaction::PENDING
    );
    assert!(
        !copy.path().join("index").exists(),
        "the copy took the sidecar indexes: they are derived state (`lakelet/4.14-index-pruning`), \
         and a store restored without them answers the same questions having read more"
    );
    assert_eq!(
        report.records(),
        batches[..3]
            .iter()
            .map(|records| records.len() as u64)
            .sum::<u64>(),
        "the copy reports {} record(s) for the segments it named and the store's three closed \
         segments hold {}",
        report.records(),
        batches[..3].iter().map(Vec::len).sum::<usize>()
    );
    assert_eq!(
        report.log_bytes(),
        proven_bytes(source_dir.path()),
        "the copy took {} byte(s) of log and the replay proves {}",
        report.log_bytes(),
        proven_bytes(source_dir.path())
    );
    assert_eq!(
        report.log_records(),
        batches.len() as u64,
        "the log holds one payload per batch, and this store was given {}",
        batches.len()
    );
}

#[test]
fn every_entry_states_the_bytes_and_the_checksum_the_file_has() {
    let source_dir = StoreDir::new("spec-backup-entries");
    let copy = StoreDir::new("spec-backup-entries-copy");
    built(source_dir.path(), 2, 5);
    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");

    let loaded = backup::Manifest::load(copy.path()).expect("the copy's manifest is a log");
    assert_eq!(loaded.len(), 4, "two segments, a manifest and a log");
    assert_eq!(
        loaded.stop(),
        Stop::End,
        "a copy this process wrote ends on a whole entry: its own manifest is the file a restore \
         reads, and a walk that stopped anywhere else would be a copy that lost its last entry"
    );
    assert_eq!(loaded.discarded(), 0, "and it has no bytes nobody wrote");
    let mut kinds: Vec<&str> = loaded
        .entries()
        .iter()
        .map(|entry| entry.kind().name())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["log", "manifest", "segment", "segment"]);

    let epoch = Segments::open(source_dir.path())
        .expect("the store opens")
        .manifest()
        .epoch();
    for entry in loaded.entries() {
        let path = copy.path().join(entry.name());
        assert_eq!(
            length(&path),
            entry.bytes(),
            "{} is {} byte(s) and its entry states {}",
            entry.name(),
            length(&path),
            entry.bytes()
        );
        assert_eq!(
            crc32(&path),
            entry.crc32(),
            "{} hashes to {:08x} and its entry states {:08x}: the checksum is what a restore trusts \
             before it writes anything",
            entry.name(),
            crc32(&path),
            entry.crc32()
        );
        assert_eq!(
            entry.epoch(),
            if entry.kind() == backup::Kind::Manifest {
                Some(epoch)
            } else {
                None
            },
            "the entry for the store's own manifest is the one that states the epoch, and {} states \
             {entry:?}",
            entry.name()
        );
    }
    // The one file that states a range is a segment, and it states the range of its own records.
    let segments: Vec<&backup::Entry> = loaded
        .entries()
        .iter()
        .filter(|entry| entry.kind() == backup::Kind::Segment)
        .collect();
    let first = &segments[0];
    assert_eq!(
        (first.first_millis(), first.last_millis()),
        (Some(BASE as u64), Some(BASE as u64 + 4 * STEP as u64)),
        "a segment's entry states the span its five records cover: {first:?}"
    );
    assert_eq!(
        segments[0].records(),
        Some(5),
        "and how many records it holds: a restore reports what a store will answer with"
    );
}

#[test]
fn a_torn_tail_entry_is_dropped_with_the_bytes_it_occupied() {
    let source_dir = StoreDir::new("spec-backup-torn");
    let copy = StoreDir::new("spec-backup-torn-copy");
    built(source_dir.path(), 2, 3);
    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    let intact = named(copy.path());

    // A manifest is a *log*: the tail of one more entry, without the seal that would prove it, is
    // exactly what a machine that stopped while the copy named a file leaves behind. The bytes are
    // written with the log's own encoder, which is the claim this case is about.
    let torn = segment::encode(b"the entry a machine never finished");
    let kept = &torn[..torn.len() - 4];
    let manifest = copy.path().join(backup::manifest::FILE);
    let mut bytes = std::fs::read(&manifest).expect("the copy's manifest is there");
    bytes.extend_from_slice(kept);
    std::fs::write(&manifest, &bytes).expect("writable");

    let loaded =
        backup::Manifest::load(copy.path()).expect("a torn tail is not a refused manifest");
    assert_eq!(
        loaded
            .entries()
            .iter()
            .map(|entry| entry.name().to_owned())
            .collect::<Vec<String>>(),
        intact,
        "the bytes after the last whole entry are not an entry, and the entries before it are still \
         the copy's"
    );
    assert_eq!(
        loaded.stop(),
        Stop::Torn,
        "the tail stops inside a record, which is the `torn` stop `lakelet/4.4-wal-recovery` gives it \
         — a copy's manifest is that log and not a second format"
    );
    assert_eq!(
        loaded.discarded(),
        kept.len() as u64,
        "the walk reports {} byte(s) nobody can prove, and {} were written",
        loaded.discarded(),
        kept.len()
    );
    assert_eq!(
        loaded.len(),
        intact.len(),
        "and it keeps every entry before them"
    );

    // A whole record that is not an entry is the other answer: the bytes are a log this build wrote
    // and the payload is not an entry, so the copy is damaged rather than empty.
    let mut bytes = std::fs::read(&manifest).expect("the copy's manifest is there");
    bytes.clear();
    bytes.extend_from_slice(&segment::encode(b"not an entry this build wrote"));
    std::fs::write(&manifest, &bytes).expect("writable");
    let damaged = backup::Manifest::load(copy.path()).expect_err("a record that is not an entry");
    assert!(
        damaged.to_string().contains(backup::manifest::FILE),
        "the refusal names the manifest it is about: {damaged}"
    );
}

#[test]
fn an_interrupted_copy_names_a_prefix_and_never_the_file_it_cut() {
    let source_dir = StoreDir::new("spec-backup-cut");
    let copy = StoreDir::new("spec-backup-cut-copy");
    built(source_dir.path(), 3, 6);

    // Inside the *second* segment: past the store's own manifest and the first segment, part way
    // into the second. The budget is a count of file contents, so the case can say where the copy
    // stopped rather than hope it stopped somewhere useful.
    let manifest = length(&source_dir.path().join(compaction::MANIFEST));
    let first = length(&compaction::file_of(source_dir.path(), 0));
    let second = length(&compaction::file_of(source_dir.path(), 1));
    let room = second / 3;
    let budget = manifest + first + room;
    let report = backup::copy(
        source_dir.path(),
        copy.path(),
        Interruption::AfterBytes(budget),
    )
    .expect("an interrupted copy still reports what it did");

    let names = named(copy.path());
    assert!(
        report.interrupted(),
        "a copy that did not take every file is interrupted: {report:?}"
    );
    assert_eq!(
        names,
        [
            compaction::MANIFEST.to_owned(),
            format!(
                "{}/{index:0>10}.{extension}",
                compaction::DIRECTORY,
                index = 0,
                extension = compaction::EXTENSION
            ),
        ],
        "the copy names what it finished and nothing else: {names:?}"
    );
    assert_eq!(
        (report.files(), report.entries()),
        (3, 2),
        "three files anything was written for — the manifest, the first segment, and the second \
         segment it stopped inside — and two of them named"
    );
    assert_eq!(
        report.bytes(),
        budget,
        "the copy wrote {} byte(s) of file contents where it was allowed {budget}",
        report.bytes()
    );
    let partial = compaction::file_of(copy.path(), 1);
    assert_eq!(
        length(&partial),
        room,
        "the file the budget cut through is left partial, and it holds the bytes that fit: {room}"
    );
    // Every file it *did* name verifies against its entry, and the partial file is not one of them.
    for entry in backup::Manifest::load(copy.path())
        .expect("the copy's manifest is a log")
        .entries()
    {
        let path = copy.path().join(entry.name());
        assert_eq!(
            length(&path),
            entry.bytes(),
            "{} is the length it states",
            entry.name()
        );
        assert_eq!(
            crc32(&path),
            entry.crc32(),
            "{} hashes to what it states",
            entry.name()
        );
    }
}

#[test]
fn a_copy_interrupted_before_its_first_byte_names_nothing_which_is_not_the_same_as_no_copy() {
    let source_dir = StoreDir::new("spec-backup-before");
    let copy = StoreDir::new("spec-backup-before-copy");
    built(source_dir.path(), 1, 3);
    let report = backup::copy(source_dir.path(), copy.path(), Interruption::AfterBytes(0))
        .expect("a copy that was allowed no bytes is still a copy");

    assert_eq!(
        (report.files(), report.entries(), report.bytes()),
        (0, 0, 0),
        "a copy with no budget writes nothing and names nothing: {report:?}"
    );
    assert!(report.interrupted(), "and it says it was interrupted");
    let loaded = backup::Manifest::load(copy.path()).expect("the copy's manifest is there");
    assert!(
        loaded.is_empty() && loaded.entries().is_empty(),
        "the manifest is there and names nothing: a copy that was interrupted before its first file \
         is a copy with no files in it, and its epoch is nobody's — {}",
        loaded.epoch()
    );
    assert_eq!(
        loaded.epoch(),
        0,
        "a copy that proved no entry states no epoch, and a store at epoch 0 is the one it has never \
         changed shape: {loaded:?}"
    );
    assert_eq!(
        loaded.stop(),
        Stop::End,
        "an empty manifest is an empty log, which is the one file that is not a log *yet*"
    );

    // The other answer, and the one this case is named for: a directory that holds no copy at all is
    // a *typed refusal*, never an empty copy.
    let nowhere = StoreDir::new("spec-backup-nowhere");
    let refused = backup::Manifest::load(nowhere.path())
        .expect_err("a directory with no manifest is not a copy of nothing");
    assert!(
        matches!(refused, backup::Error::NoManifest { .. }),
        "loading a copy that is not there answered {refused:?}"
    );
    assert!(
        refused.to_string().contains(backup::manifest::FILE),
        "the refusal names the file an operator goes looking for: {refused}"
    );
    assert_eq!(
        refused.class(),
        3,
        "and it is the class `3`: the thing asked about is not there, which is a different sentence \
         from a copy that holds nothing"
    );
}

#[test]
fn a_complete_restore_yields_the_store_it_copied() {
    let source_dir = StoreDir::new("spec-restore-complete");
    let copy = StoreDir::new("spec-restore-complete-copy");
    let restored = StoreDir::new("spec-restore-complete-restored");
    built(source_dir.path(), 3, 5);
    let report =
        backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    let outcome = backup::restore(copy.path(), restored.path()).expect("a complete copy restores");

    assert_eq!(
        outcome.files(),
        report.entries(),
        "the restore wrote {} file(s) where the copy named {}",
        outcome.files(),
        report.entries()
    );
    assert_eq!(
        outcome.bytes(),
        report.bytes(),
        "and {} byte(s) where the copy took {}",
        outcome.bytes(),
        report.bytes()
    );
    assert_eq!(
        outcome.records(),
        report.records(),
        "and the segments it wrote hold the records the copy said they hold"
    );
    assert_eq!(
        outcome.log_records(),
        report.log_records(),
        "and the log it wrote proves the payloads the copy said it took"
    );

    let store = Segments::open(restored.path()).expect("a restored store opens");
    let original = Segments::open(source_dir.path()).expect("the store it came from opens");
    assert_eq!(
        store.manifest().epoch(),
        original.manifest().epoch(),
        "the manifest is copied, not rebuilt: the epoch says which store this is"
    );
    assert_eq!(store.manifest().floor(), original.manifest().floor());
    assert_eq!(
        outcome.epoch(),
        original.manifest().epoch(),
        "the restore states the epoch of the store it put back, which it has without opening it"
    );
    assert_eq!(
        answers(restored.path()),
        answers(source_dir.path()),
        "the restored store answers what the store it was copied from answered, record for record \
         and in the same order: a copy is a copy and not a re-encode"
    );
    assert_eq!(
        restored_log(restored.path()),
        restored_log(source_dir.path()),
        "and its log proves the same payloads: what a restore writes is what a replay will read"
    );
    assert_eq!(
        files_under(restored.path()).len() as u64,
        report.entries(),
        "and it holds exactly the files the copy named — nothing more, because a restore writes what \
         the manifest covered"
    );
}

/// The payloads a store's log proves, as the records they carry.
fn restored_log(directory: &Path) -> Vec<Record> {
    let recovered = replay::recover(directory).unwrap_or_else(|error| {
        panic!("{}'s log cannot be replayed: {error}", directory.display())
    });
    let mut records = Vec::new();
    for payload in recovered.records() {
        records.extend(batch::decode(payload).unwrap_or_else(|error| {
            panic!("{}'s log holds no batch: {error}", directory.display())
        }));
    }
    records
}

#[test]
fn restoring_an_interrupted_copy_writes_exactly_what_the_manifest_covered() {
    let source_dir = StoreDir::new("spec-restore-half");
    let copy = StoreDir::new("spec-restore-half-copy");
    let restored = StoreDir::new("spec-restore-half-restored");
    built(source_dir.path(), 3, 4);

    let manifest = length(&source_dir.path().join(compaction::MANIFEST));
    let first = length(&compaction::file_of(source_dir.path(), 0));
    let report = backup::copy(
        source_dir.path(),
        copy.path(),
        Interruption::AfterBytes(
            manifest + first + length(&compaction::file_of(source_dir.path(), 1)) / 2,
        ),
    )
    .expect("an interrupted copy is still a copy");
    assert!(
        report.interrupted(),
        "the copy in this case is the interrupted one"
    );

    let loaded = backup::Manifest::load(copy.path()).expect("the copy's manifest is a log");
    let named_files: Vec<(String, u64)> = loaded
        .entries()
        .iter()
        .map(|entry| (entry.name().to_owned(), entry.bytes()))
        .collect();
    let outcome = backup::restore(copy.path(), restored.path())
        .expect("a restore writes the files the manifest covered, interrupted copy or not");
    assert_eq!(
        files_under(restored.path()),
        named_files,
        "the restored directory is exactly the files the copy's manifest named: a restore invents \
         nothing and writes nothing the manifest did not cover"
    );
    assert_eq!(
        outcome.files() as usize,
        named_files.len(),
        "and it says it wrote {} file(s) of {}",
        outcome.files(),
        named_files.len()
    );

    // The records the restored *files* hold are the records the copy's segments stated, and they are
    // the source's own records — the copy of a file is that file.
    let mut from_files: u64 = 0;
    let mut restored_records: Vec<Record> = Vec::new();
    for entry in loaded.entries() {
        if entry.kind() != backup::Kind::Segment {
            continue;
        }
        let records = segment_records(restored.path(), entry.name());
        assert_eq!(
            records,
            segment_records(source_dir.path(), entry.name()),
            "the restored {} does not hold what the source's does",
            entry.name()
        );
        from_files += records.len() as u64;
        restored_records.extend(records);
    }
    assert_eq!(
        from_files,
        outcome.records(),
        "the restored segments hold {from_files} record(s) and the restore reports {}: the count is \
         the manifest's own, which is what an operator sizes the loss with",
        outcome.records()
    );
    let source_records = answers(source_dir.path());
    assert!(
        source_records.starts_with(&restored_records),
        "the restored store holds {} record(s) that are not the first {} of the source's {}: a copy \
         takes a prefix of the manifest's order, so what survives it is a prefix of the store",
        restored_records.len(),
        restored_records.len(),
        source_records.len()
    );
    assert!(
        restored_records.len() < source_records.len(),
        "the interrupted copy in this case is missing a segment, so it must have lost something"
    );

    // And the directory is not a store: the manifest it copied names a segment nobody kept, and a
    // store whose manifest names a file nobody kept must refuse rather than answer with a hole.
    let refused = Segments::open(restored.path())
        .expect_err("a manifest naming a segment nobody kept is not a store");
    assert!(
        refused.to_string().contains(&format!("segment {}", 1)),
        "the refusal names the segment the manifest states and the copy does not hold: {refused}"
    );
}

#[test]
fn a_copy_missing_a_named_file_is_refused_and_leaves_the_destination_as_it_was() {
    let source_dir = StoreDir::new("spec-restore-missing");
    let copy = StoreDir::new("spec-restore-missing-copy");
    let scratch = StoreDir::new("spec-restore-missing-scratch");
    let restored = scratch.path().join("destination");
    built(source_dir.path(), 2, 3);
    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    let victim = compaction::file_of(copy.path(), 1);
    std::fs::remove_file(&victim).expect("the copy's own file");

    let refused = backup::restore(copy.path(), &restored)
        .expect_err("a copy missing a file it names is not restorable");
    assert!(
        matches!(
            refused,
            backup::Error::Damaged(backup::Damage::Missing { .. })
        ),
        "the refusal is a damaged copy: {refused:?}"
    );
    assert!(
        refused.to_string().contains(&format!(
            "{}/{index:0>10}.{extension}",
            compaction::DIRECTORY,
            index = 1,
            extension = compaction::EXTENSION
        )),
        "and it names the file: {refused}"
    );
    assert_eq!(
        refused.class(),
        4,
        "a damaged copy is the store's class, not the caller's"
    );
    assert!(
        !restored.exists(),
        "{} is there: a restore verifies every entry before it writes anything, so a refused one \
         leaves no directory at all — and a half-restored directory is one an operator would open, \
         which is the thing this order exists to prevent",
        restored.display()
    );

    // A destination that is already there is left exactly as it was, with whatever was in it.
    let existing = StoreDir::new("spec-restore-missing-existing");
    std::fs::write(existing.path().join("mine.txt"), b"mine").expect("writable");
    let refused = backup::restore(copy.path(), existing.path()).expect_err("still not restorable");
    assert!(matches!(refused, backup::Error::Damaged(_)));
    assert_eq!(
        files_under(existing.path()),
        [("mine.txt".to_owned(), 4)],
        "the destination holds what it held before the refused restore, and nothing else"
    );
}

#[test]
fn a_flipped_byte_and_a_cut_file_are_two_different_refusals() {
    let source_dir = StoreDir::new("spec-restore-damage");
    let flipped = StoreDir::new("spec-restore-damage-flipped");
    let short = StoreDir::new("spec-restore-damage-short");
    let target = StoreDir::new("spec-restore-damage-target");
    built(source_dir.path(), 2, 3);
    backup::copy(source_dir.path(), flipped.path(), Interruption::None).expect("copiable");
    backup::copy(source_dir.path(), short.path(), Interruption::None).expect("copiable");

    // The same length and different bytes: a device that wrote something else.
    flip(&flipped.path().join(compaction::file_of(Path::new(""), 1)));
    let refused =
        backup::restore(flipped.path(), target.path()).expect_err("a flipped byte is damage");
    let backup::Error::Damaged(backup::Damage::Checksum {
        name,
        expected,
        found,
    }) = &refused
    else {
        panic!("a file whose bytes changed is a checksum failure, and this is {refused:?}");
    };
    assert_ne!(expected, found, "a flipped byte changed the checksum");
    let sentence = refused.to_string();
    assert!(
        sentence.contains(name)
            && sentence.contains(&format!("{expected:08x}"))
            && sentence.contains(&format!("{found:08x}")),
        "the sentence names the file and both numbers, because an operator has to know which file \
         and by how much: {sentence}"
    );

    // A file that is not the length its entry states: the other damage, and the other answer.
    let victim = short.path().join(compaction::file_of(Path::new(""), 0));
    cut(&victim, length(&victim) - 7);
    let refused = backup::restore(short.path(), target.path()).expect_err("a cut file is damage");
    let backup::Error::Damaged(backup::Damage::Short {
        expected, found, ..
    }) = &refused
    else {
        panic!("a file that is not its stated length is a short file, and this is {refused:?}");
    };
    assert_eq!(
        *expected - *found,
        7,
        "the sentence states both lengths, and the difference is the seven bytes that are missing"
    );
    assert!(
        refused
            .to_string()
            .contains(&format!("{expected} bytes expected, {found} found")),
        "{}",
        refused
    );
    assert!(
        !target.path().join(compaction::MANIFEST).exists(),
        "neither refusal wrote the one file that makes a directory a store"
    );
}

#[test]
fn a_restore_never_writes_over_a_store() {
    let source_dir = StoreDir::new("spec-restore-occupied");
    let copy = StoreDir::new("spec-restore-occupied-copy");
    let other = StoreDir::new("spec-restore-occupied-other");
    built(source_dir.path(), 2, 3);
    // A *different* store: nothing about it is the copy's, so nothing in it may move.
    built(other.path(), 2, 3);
    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");

    let before: Vec<(String, u64, u32)> = files_under(other.path())
        .into_iter()
        .map(|(name, bytes)| {
            let crc = crc32(&other.path().join(&name));
            (name, bytes, crc)
        })
        .collect();
    let refused = backup::restore(copy.path(), other.path())
        .expect_err("a restore into a directory that holds a store is refused");
    assert!(
        matches!(refused, backup::Error::Occupied { .. }),
        "a destination that holds a store is refused as occupied: {refused:?}"
    );
    assert!(
        refused.to_string().contains(compaction::MANIFEST),
        "and the sentence names the file that makes it a store: {refused}"
    );
    assert_eq!(refused.class(), 4);
    let after: Vec<(String, u64, u32)> = files_under(other.path())
        .into_iter()
        .map(|(name, bytes)| {
            let crc = crc32(&other.path().join(&name));
            (name, bytes, crc)
        })
        .collect();
    assert_eq!(
        after, before,
        "the store that was already there moved: a restore never overwrites and never merges"
    );
}

#[test]
fn a_log_cut_mid_record_is_copied_to_the_prefix_a_replay_proves() {
    let source_dir = StoreDir::new("spec-copy-torn-log");
    let copy = StoreDir::new("spec-copy-torn-log-copy");
    let restored = StoreDir::new("spec-copy-torn-log-restored");
    built(source_dir.path(), 2, 3);

    // The machine died inside the write of the last record: the file is three bytes short of one,
    // which is the death `lakelet/4.4-wal-recovery` refuses to guess about.
    let log = source_dir.path().join(LOG);
    cut(&log, length(&log) - 3);
    let before = replay::recover(source_dir.path()).expect("the log can be replayed read-only");
    assert!(
        before.discarded() > 0,
        "this case is about a log with a tail nobody can prove, and this one has {} byte(s) of it",
        before.discarded()
    );

    let report =
        backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    assert_eq!(
        report.log_bytes(),
        before.bytes(),
        "the copy took {} byte(s) of log and the replay proved {}: a copy takes the prefix a replay \
         proves and not a byte of the tail nobody can read",
        report.log_bytes(),
        before.bytes()
    );
    assert_eq!(
        length(&copy.path().join(LOG)),
        before.bytes(),
        "and the file in the copy is that long"
    );

    backup::restore(copy.path(), restored.path()).expect("the copy is complete");
    let after = replay::recover(restored.path()).expect("the restored log can be replayed");
    assert_eq!(
        after.records(),
        before.records(),
        "the restored store's log holds {} payload(s) where the source's proved {}",
        after.records().len(),
        before.records().len()
    );
    assert_eq!(
        after.discarded(),
        0,
        "the restored log has no bytes a replay cannot prove: the tail the machine stopped inside \
         was never part of what the store is, so it is not part of a copy of it either — and the \
         source's own replay reported {} byte(s) of it",
        before.discarded()
    );
    assert_eq!(after.bytes(), before.bytes());
    assert_eq!(
        answers(restored.path()),
        answers(source_dir.path()),
        "and the segments of the two stores answer the same thing: a cut log is not a cut store"
    );
}

#[test]
fn a_restored_store_carries_the_same_series_ledger() {
    let source_dir = StoreDir::new("spec-restore-ledger");
    let copy = StoreDir::new("spec-restore-ledger-copy");
    let restored = StoreDir::new("spec-restore-ledger-restored");
    closed(source_dir.path(), 3, 4);
    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    backup::restore(copy.path(), restored.path()).expect("restorable");

    let source_ledger = Registry::of_store(source_dir.path(), Cap::DEFAULT).expect("the source");
    let restored_ledger = Registry::of_store(restored.path(), Cap::DEFAULT).expect("the restored");
    let names: Vec<&str> = source_ledger.tracked().map(|(name, _)| name).collect();
    assert_eq!(
        names,
        restored_ledger
            .tracked()
            .map(|(name, _)| name)
            .collect::<Vec<&str>>(),
        "the restored store's ledger holds a different set of series from the source's"
    );
    assert_eq!(
        names, SERIES,
        "the fixture's records name these series, and the ledger is rebuilt from them"
    );
    for (name, cost) in source_ledger.tracked() {
        let after = restored_ledger
            .cost_of(name)
            .unwrap_or_else(|| panic!("the restored ledger does not track {name}"));
        assert_eq!(
            after.records(),
            cost.records(),
            "{name}: the source's ledger charges {} record(s) and the restored one {}",
            cost.records(),
            after.records()
        );
        assert_eq!(
            after.bytes(),
            cost.bytes(),
            "{name}: the source's ledger charges {} byte(s) and the restored one {}",
            cost.bytes(),
            after.bytes()
        );
        // The direction matters, and it is `lakelet/4.14-index-pruning`'s rule seen from the other
        // side: the source's window is the *sidecar indexes'* — the whole span of every segment that
        // may hold the series — and a copy takes no indexes, so the restored window is the records'
        // own. It has to be *inside* the source's, which is what a window that had lost a record
        // could not be.
        assert!(
            after.first_millis() >= cost.first_millis()
                && after.last_millis() <= cost.last_millis(),
            "{name}: the restored window is [{}, {}] and the source's is [{}, {}] — the restored one \
             is the records' own and has to sit inside the indexes' one, and a ledger that had lost \
             a record would not",
            after.first_millis(),
            after.last_millis(),
            cost.first_millis(),
            cost.last_millis()
        );
    }
}

/// One query's answer, as the numbers a reader compares: `(start, count, value)` per bucket.
fn buckets(outcome: &exec::Outcome) -> Vec<(u64, u64, f64)> {
    outcome
        .answers()
        .iter()
        .flat_map(|answer| {
            answer
                .buckets()
                .iter()
                .map(|bucket| (bucket.start(), bucket.count(), bucket.value()))
        })
        .collect()
}

/// Ask a store one question through `lakelet/4.11-query-engine`'s executor.
fn asked(directory: &Path, text: &str) -> exec::Outcome {
    let store = Segments::open(directory)
        .unwrap_or_else(|error| panic!("{} is not a store: {error}", directory.display()));
    let query = match parse::parse(text).unwrap_or_else(|refusal| panic!("{text}: {refusal}")) {
        Statement::Query(query) | Statement::Explain(query) => query,
    };
    exec::execute(&store.snapshot(), &query)
        .unwrap_or_else(|refusal| panic!("{text} was refused: {refusal}"))
}

#[test]
fn the_restored_store_answers_a_query_the_source_answered() {
    let source_dir = StoreDir::new("spec-restore-query");
    let copy = StoreDir::new("spec-restore-query-copy");
    let restored = StoreDir::new("spec-restore-query-restored");
    let batches = built(source_dir.path(), 3, 4);
    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    backup::restore(copy.path(), restored.path()).expect("restorable");

    let from = BASE;
    let to = BASE + (batches.iter().map(Vec::len).sum::<usize>() as i64) * STEP;
    let text =
        format!("SELECT count(value) FROM cpu WHERE ts BETWEEN {from} AND {to} GROUP BY time(1m)");
    let source_answer = asked(source_dir.path(), &text);
    let restored_answer = asked(restored.path(), &text);
    assert!(
        !buckets(&source_answer).is_empty(),
        "the fixture's question has to have an answer for this case to say anything: {text}"
    );
    assert_eq!(
        buckets(&restored_answer),
        buckets(&source_answer),
        "the executor answers the restored store differently from the store it was copied from: a \
         copy is a copy — the same bytes in the same files — and not a re-encode that happens to \
         carry the same records"
    );
    assert_eq!(
        (
            restored_answer.rows_scanned(),
            restored_answer.rows_answered(),
            restored_answer.series_matched(),
        ),
        (
            source_answer.rows_scanned(),
            source_answer.rows_answered(),
            source_answer.series_matched(),
        ),
        "and it counts the same work: the plan is made from the manifest, and the manifest is copied"
    );
}

#[test]
fn the_operators_two_commands_answer_on_stdout_and_leave_one_sentence_on_stderr() {
    let source_dir = StoreDir::new("spec-cli-backup");
    let copy = StoreDir::new("spec-cli-backup-copy");
    let elsewhere = StoreDir::new("spec-cli-backup-elsewhere");
    built(source_dir.path(), 2, 3);
    let store = source_dir.path().to_str().expect("a UTF-8 path").to_owned();
    let to = copy.path().to_str().expect("a UTF-8 path").to_owned();

    let ran = lakeletq(&["backup", "--store", &store, "--to", &to]);
    assert_eq!(
        ran.code, 0,
        "a copy of a store this process built answers: {ran:?}"
    );
    assert_eq!(
        ran.stderr, "",
        "a command that answered has nothing to diagnose: stdout is the answer and stderr is empty"
    );
    let lines: Vec<(&str, &str)> = ran
        .stdout
        .lines()
        .map(|line| {
            line.split_once('=')
                .unwrap_or_else(|| panic!("`{line}` is not a `name=value` line of an answer"))
        })
        .collect();
    assert_eq!(
        lines.iter().map(|(name, _)| *name).collect::<Vec<&str>>(),
        [
            "files",
            "entries",
            "bytes",
            "records",
            "log_bytes",
            "log_records",
            "interrupted",
            "epoch",
            "floor",
        ],
        "the answer is one `name=value` per line, in a deterministic order"
    );
    assert_eq!(
        lines
            .iter()
            .find(|(name, _)| *name == "interrupted")
            .map(|(_, value)| *value),
        Some("false"),
        "a complete copy says so"
    );
    for (name, value) in &lines {
        assert!(
            value.parse::<u64>().is_ok() || *value == "false" || *value == "true",
            "`{name}={value}` is neither a count nor a flag"
        );
    }

    // `--json` is the same object and nothing else changes: a script that reads one can read the
    // other, and neither of them is a second answer.
    let json = lakeletq(&["backup", "--store", &store, "--to", &to, "--json"]);
    assert_eq!(
        json.code, 0,
        "`--json` changes the shape and not the outcome: {json:?}"
    );
    assert_eq!(json.stderr, "");
    let document: serde_json::Value = serde_json::from_str(&json.stdout).unwrap_or_else(|error| {
        panic!(
            "`--json` did not print one document: {error}\n{}",
            json.stdout
        )
    });
    let object = document
        .as_object()
        .unwrap_or_else(|| panic!("`--json` did not print one object: {}", json.stdout));
    assert_eq!(
        object.len(),
        lines.len(),
        "the object has a different number of keys from the lines it is the same answer as"
    );
    for (name, value) in &lines {
        let field = object
            .get(*name)
            .unwrap_or_else(|| panic!("the object does not carry `{name}`: {document}"));
        assert_eq!(
            field.to_string(),
            *value,
            "`{name}` is {value} on the lines and {field} in the object: `--json` is the same object"
        );
    }

    // Every way a call can be wrong, and the class each one is.
    let nowhere = elsewhere.path().join("nowhere");
    let absent = nowhere.to_str().expect("a UTF-8 path").to_owned();
    let ran = lakeletq(&["backup", "--store", &absent, "--to", &to]);
    assert_eq!(
        ran.code, 3,
        "a store that is not there is `3`, the thing asked about is not there: {ran:?}"
    );
    assert_eq!(ran.stdout, "", "and a refusal answers nothing: {ran:?}");
    assert_eq!(
        ran.stderr.lines().count(),
        1,
        "and one sentence of diagnosis: {ran:?}"
    );
    let ran = lakeletq(&["backup", "--store", &store]);
    assert_eq!(
        ran.code, 2,
        "a call that did not say where to write is the caller's: {ran:?}"
    );
    assert_eq!(ran.stdout, "");
    let ran = lakeletq(&["backup", "--store", &store, "--to", &to, "--depth", "9"]);
    assert_eq!(
        ran.code, 2,
        "a flag this command does not read is the caller's: {ran:?}"
    );
    assert!(ran.stderr.contains("--depth"), "and it names it: {ran:?}");
}

#[test]
fn the_restore_command_refuses_a_damaged_copy_and_a_destination_that_is_a_store() {
    let source_dir = StoreDir::new("spec-cli-restore");
    let copy = StoreDir::new("spec-cli-restore-copy");
    let restored = StoreDir::new("spec-cli-restore-restored");
    built(source_dir.path(), 2, 3);
    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    let from = copy.path().to_str().expect("a UTF-8 path").to_owned();
    let to = restored.path().to_str().expect("a UTF-8 path").to_owned();

    let ran = lakeletq(&["restore", "--from", &from, "--to", &to]);
    assert_eq!(ran.code, 0, "a copy this process built restores: {ran:?}");
    assert_eq!(ran.stderr, "");
    let names: Vec<&str> = ran
        .stdout
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    assert_eq!(
        names,
        ["files", "bytes", "records", "log_records", "epoch", "floor"],
        "the restore's answer is one `name=value` per line"
    );
    let recovered = Segments::open(restored.path()).expect("the restored store opens");
    assert_eq!(
        recovered.manifest().segments().len(),
        2,
        "and the store the command wrote is the store the copy was of"
    );

    // The same call again: the destination holds a store now, and a restore never overwrites one.
    let again = lakeletq(&["restore", "--from", &from, "--to", &to]);
    assert_eq!(
        again.code, 4,
        "a destination that already holds a store is refused, and refusing is `4`: {again:?}"
    );
    assert_eq!(again.stdout, "");
    assert!(
        !again.stderr.trim().is_empty(),
        "with a sentence: {again:?}"
    );

    // And the copy that lost a byte: the restore refuses, and it does not write the file that would
    // make the destination a store.
    let elsewhere = StoreDir::new("spec-cli-restore-elsewhere");
    let target = elsewhere.path().join("target");
    let target_arg = target.to_str().expect("a UTF-8 path").to_owned();
    flip(&copy.path().join(compaction::file_of(Path::new(""), 1)));
    let damaged = lakeletq(&["restore", "--from", &from, "--to", &target_arg]);
    assert_eq!(
        damaged.code, 4,
        "a damaged copy is refused, and a refusal is `4`: {damaged:?}"
    );
    assert_eq!(
        damaged.stdout, "",
        "with nothing on the answer stream: {damaged:?}"
    );
    assert!(
        damaged.stderr.contains(&format!(
            "{}/{index:0>10}.{extension}",
            compaction::DIRECTORY,
            index = 1,
            extension = compaction::EXTENSION
        )),
        "and a sentence naming the file an operator has to go and look at: {damaged:?}"
    );
    assert!(
        !target.exists(),
        "the refused restore left a directory at {}: it verifies the whole copy before it writes",
        target.display()
    );
}

#[test]
fn the_page_carries_the_drill_the_command_prints() {
    let text = page();
    let lowered = text.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !lowered.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a page with a blank in it is a page nobody has \
             written down"
        );
    }
    assert!(
        lowered.contains("rpo") && lowered.contains("rto"),
        "{PAGE} does not state the two numbers a backup is judged on: what the crash had already \
         lost (RPO) and how long putting it back takes (RTO)"
    );
    let (block, timed) = quoted(&text);
    assert!(
        !timed,
        "{PAGE} carries a timing line: the block is compared line for line, and a wall clock is not \
         the same number on two machines — the timed run is what the prose quotes, not the block"
    );
    let ran = lakeletq(&["drill"]);
    assert_eq!(ran.code, 0, "the drill a page quotes answers: {ran:?}");
    assert_eq!(ran.stderr, "", "and diagnoses nothing: {ran:?}");
    let printed: Vec<String> = ran.stdout.lines().map(str::to_owned).collect();
    assert_eq!(
        printed, block,
        "{PAGE}'s block is not what `lakeletq drill` prints: the page's numbers are a measurement, \
         and the way a page full of measurements stays true is that the command which produced them \
         is re-run and compared. Edit the page from a run, never from an intention."
    );
    assert!(
        block
            .iter()
            .any(|line| line.starts_with("restored_digest="))
            && block.iter().any(|line| line.starts_with("source_digest=")),
        "the block does not carry the two digests: a count is what a copy that lost a record and \
         invented another keeps"
    );

    // The one timing line, where a person measuring their own machine gets it.
    let mut command = Command::new(env!("CARGO_BIN_EXE_lakeletq"));
    command
        .args(["drill"])
        .env("LAKELET_DRILL_TIMED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().expect("the binary is built by cargo test");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        stdout.contains("restore_millis="),
        "`LAKELET_DRILL_TIMED=1 lakeletq drill` printed no timing line, so the RTO the page quotes \
         has no command behind it:\n{stdout}"
    );
    assert!(
        !text.contains("restore_millis="),
        "{PAGE} carries the timing line inside the block it quotes"
    );
}

#[test]
fn the_step_stays_inside_the_engine_and_does_not_parse_a_segment() {
    let module = PathBuf::from("src/engine/backup");
    let mut files = Vec::new();
    collect_rust(&module, &mut files).expect("src/engine/backup is part of the project");
    let mut names: Vec<String> = files
        .iter()
        .map(|path| {
            path.strip_prefix(&module)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["manifest.rs", "mod.rs", "restore.rs"],
        "the step's engine module is these three files and no others"
    );
    for file in &files {
        let text = std::fs::read_to_string(file).expect("a source file the project keeps");
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !text.contains(forbidden),
                "{} names `{forbidden}` — the command line uses the engine, never the other way \
                 (`lakelet/4.1-ingest-protocol`'s boundary, and every step since has kept it)",
                file.display()
            );
        }
    }
    for file in [
        "src/engine/backup/restore.rs",
        "src/engine/backup/manifest.rs",
    ] {
        let text = source(file);
        for forbidden in ["engine::segment::", "compaction::SegmentInfo"] {
            assert!(
                !text.contains(forbidden),
                "{file} names `{forbidden}`: a copy and a restore move bytes, and the store that \
                 comes out of them is read by `lakelet/4.8-compaction` and \
                 `lakelet/4.7-columnar-segments` — a second parser here would be a second answer to \
                 what a segment is"
            );
        }
    }
    let cli = source("src/cli/backup.rs");
    assert!(
        cli.contains("engine::backup"),
        "src/cli/backup.rs does not use the engine's copy and restore: the operator's side is where \
         the two directions meet, and this is the direction that is allowed"
    );
    for needed in ["backup::copy", "backup::restore"] {
        assert!(
            cli.contains(needed),
            "src/cli/backup.rs does not call `{needed}`"
        );
    }
}

#[test]
fn a_copy_reads_the_store_and_is_never_written_over_a_store() {
    let source_dir = StoreDir::new("spec-copy-reads");
    let copy = StoreDir::new("spec-copy-reads-copy");
    let other = StoreDir::new("spec-copy-reads-other");
    built(source_dir.path(), 3, 4);
    // The store the copy is *not* allowed to be written over, built the same way.
    built(other.path(), 2, 3);

    // What the store's own files are before the copy: the manifest, the segments it names and the
    // log, by length and checksum. A copy reads them, and nothing in this step writes to the store
    // it copies — the one thing an open of a store does is delete what the manifest does not name
    // (`lakelet/4.8-compaction`'s one recovery rule), which is why the fixture plants no orphan here.
    let store_files = |directory: &Path| -> Vec<(String, u64, u32)> {
        let mut files: Vec<PathBuf> = vec![directory.join(compaction::MANIFEST)];
        for info in Segments::open(directory)
            .expect("the store opens")
            .manifest()
            .segments()
        {
            files.push(compaction::file_of(directory, info.index));
        }
        files.push(directory.join(LOG));
        files
            .into_iter()
            .map(|path| {
                (
                    path.strip_prefix(directory)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    length(&path),
                    crc32(&path),
                )
            })
            .collect()
    };
    let named_then = store_files(source_dir.path());
    let report =
        backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");
    assert_eq!(
        report.files(),
        named_then.len() as u64,
        "a copy of the store took {} file(s) where the store holds {}",
        report.files(),
        named_then.len()
    );
    let named_now = store_files(source_dir.path());
    assert_eq!(
        named_now, named_then,
        "a copy moved the store it copied: a backup is a read, and a tool that changed its source \
         would change the thing it was asked to preserve"
    );

    // The other direction, which is the one that has to be refused: a destination that *is* a store.
    // `other` was built by the same fixture, so it holds a `segments.manifest` and no copy manifest.
    let before = files_under(other.path());
    let refused = backup::copy(source_dir.path(), other.path(), Interruption::None)
        .expect_err("a copy is not written over a store");
    assert!(
        matches!(refused, backup::Error::Occupied { .. }),
        "copying a store over another store answered {refused:?}"
    );
    assert_eq!(refused.class(), 4);
    assert_eq!(
        files_under(other.path()),
        before,
        "the store the copy was pointed at moved: a copy writes its own directory and never over a \
         store"
    );

    // A copy *directory* is not a store: it holds a `segments.manifest` too, and running the same
    // copy again is what an operator retrying a backup does.
    let again = backup::copy(source_dir.path(), copy.path(), Interruption::None)
        .expect("a copy replaces the copy that is already there");
    assert_eq!(
        again.entries(),
        report.entries(),
        "the second copy names {} file(s) and the first named {}: a copy truncates its own manifest, \
         so a retry is one copy and not two stacked",
        again.entries(),
        report.entries()
    );
}

#[test]
fn a_copy_reads_the_source_and_leaves_an_orphan_and_a_half_written_manifest_alone() {
    let source_dir = StoreDir::new("spec-copy-read-only");
    let copy = StoreDir::new("spec-copy-read-only-copy");
    built(source_dir.path(), 2, 3);

    // The two files a crash leaves behind, planted where it would leave them: a segment the
    // manifest does not name, and a manifest nobody finished. **An open of this store deletes
    // both** — it creates `segments/`, flushes the directory and removes every file the manifest
    // does not name — so a copy that read its source through an open would take them with it, and a
    // source on a read-only mount could not be copied at all. What this case pins is the door a
    // copy comes in by instead: the store's own manifest, read without opening the store.
    let orphan = source_dir
        .path()
        .join(compaction::DIRECTORY)
        .join("0000000099.lks");
    let pending = source_dir.path().join(compaction::PENDING);
    std::fs::write(&orphan, b"an orphan").expect("a directory this process owns is writable");
    std::fs::write(&pending, b"a manifest nobody finished").expect("and so is this one");
    let before = files_under(source_dir.path());

    backup::copy(source_dir.path(), copy.path(), Interruption::None).expect("copiable");

    assert_eq!(
        files_under(source_dir.path()),
        before,
        "a copy changed the store it read. A backup is a read, and an open is a write: it creates \
         `{}`, flushes the directory and deletes every file the manifest does not name — this \
         store's `{}` and its `{}` among them. The source was {before:?}",
        compaction::DIRECTORY,
        compaction::PENDING,
        orphan
            .strip_prefix(source_dir.path())
            .unwrap_or(&orphan)
            .display()
    );
    assert_eq!(
        std::fs::read(&orphan).expect("the orphan is still where it was"),
        b"an orphan",
        "a copy wrote over a file the manifest does not name: the copy either took a file that is \
         no part of the store or edited one"
    );
    assert_eq!(
        std::fs::read(&pending).expect("the half-written manifest is still where it was"),
        b"a manifest nobody finished",
        "a copy finished — or removed — a manifest the store itself never finished"
    );

    // And what the copy took is still the store's own file list: a file the manifest does not name
    // is not part of what a store is, so it is not part of a copy of one either. The list is asked
    // of the manifest through the same door, so this half cannot disagree with the half above.
    let mut expected = vec![compaction::MANIFEST.to_owned()];
    for info in compaction::Manifest::read(source_dir.path())
        .expect("the store's manifest, read without opening the store")
        .segments()
    {
        expected.push(format!(
            "{}/{index:0>10}.{extension}",
            compaction::DIRECTORY,
            index = info.index,
            extension = compaction::EXTENSION
        ));
    }
    expected.push(LOG.to_owned());
    assert_eq!(
        named(copy.path()),
        expected,
        "a copy of a store that holds two files the manifest does not name still names the \
         manifest, the segments the manifest names in their order, and the log — and nothing else"
    );
}

#[test]
fn a_drill_builds_the_store_it_was_pointed_at_and_leaves_nothing_behind() {
    let named = StoreDir::new("spec-drill-store");
    let store = named.path().to_str().expect("a UTF-8 path").to_owned();

    // A drill given a directory builds its store *there* and prints the same block: what `--store`
    // decides is where the evidence is, and not what it says — nothing in a drill's answer names a
    // path, which is what makes the block on the page the same bytes on any machine.
    let ran = lakeletq(&["drill", "--store", &store]);
    assert_eq!(
        ran.code, 0,
        "a drill pointed at a directory answers: {ran:?}"
    );
    assert_eq!(ran.stderr, "", "and diagnoses nothing: {ran:?}");
    let plain = lakeletq(&["drill"]);
    assert_eq!(
        ran.stdout, plain.stdout,
        "the drill prints a different answer depending on where its store is: the two runs differ \
         in nothing a reader would act on, so they have to be the same bytes"
    );
    assert!(
        named.path().join(compaction::MANIFEST).is_file(),
        "the drill was given {} and left no store there: `--store` names where the store it copies \
         is built",
        named.path().display()
    );
    let built = Segments::open(named.path()).expect("the store a drill built is a store");
    assert_eq!(
        built.manifest().segments().len(),
        2,
        "the drill's script closes two segments before it models the machine death, and the store \
         it was pointed at has {}",
        built.manifest().segments().len()
    );
    assert_eq!(
        answers(named.path()).len(),
        8,
        "and those two segments hold the eight records the script sealed: the drill's machine death \
         takes what no flush closed, not what a flush closed"
    );

    // A drill that named no store builds one of this process's own and removes it: the command is a
    // rehearsal, not a workspace. Called in this process rather than as a child, so that the
    // directory a child would name after *its* process id cannot be in the way.
    let mine = std::env::temp_dir().join(format!("lakelet-drill-{}", std::process::id()));
    assert!(
        !mine.exists(),
        "{} is left over from an earlier drill in this process",
        mine.display()
    );
    let outcome = lakelet::cli::backup::drill(&[], false);
    assert_eq!(
        outcome.code(),
        0,
        "a drill that was given no directory of its own answers: {outcome:?}"
    );
    assert_eq!(outcome.stderr(), "");
    assert!(
        outcome.stdout().starts_with("write_policy="),
        "and its answer is the measurements `DRILL_LINES` states, one per line: {}",
        outcome.stdout()
    );
    assert!(
        !mine.exists(),
        "a drill that was given no store left its own behind at {}: a rehearsal that fills a \
         temporary directory on every run is a rehearsal nobody runs twice",
        mine.display()
    );
}
