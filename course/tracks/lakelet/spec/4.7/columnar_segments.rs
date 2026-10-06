//! Spec for `lakelet/4.7-columnar-segments`: the round trip is exact, the bytes are the format, and
//! the size is the reason the encoding exists.
//!
//! `kiln run` copies this file into the project as `tests/spec_columnar_segments.rs`; it is the
//! course's file, not the learner's (plan §3 D2). Fourteen cases, and the shape of them is the point:
//!
//! * **the round trip is exact and ordered.** Every value in, the same value out, in the same order —
//!   for an empty segment, one record, a thousand records over a thousand series, a flat line, and
//!   the values that break an encoder written in a hurry: `i64::MIN` next to `i64::MAX` under a
//!   delta, and `-0.0`, `±inf` and `NaN` under a run-length that compares with `==`;
//! * **the layout is bytes, not behaviour.** [`GOLDEN`] spells out the column block of four known
//!   records, byte by byte, exactly as `docs/format.md` states it — the same discipline
//!   `lakelet/4.1-ingest-protocol` brought to the wire, and the reason this track carries no
//!   `semver@baseline`. The **compressed** form is deliberately not pinned: a `zstd` release may
//!   answer different bytes for the same input, and failing a learner for a dependency's choice is
//!   not a test of anything;
//! * **size is measured, never timed.** The same records cost an order of magnitude fewer bytes as a
//!   segment than as rows, and a flat line costs one run per column — both counts, both the same
//!   number on a loaded machine as on an idle one (handoff M2 §1.1);
//! * **a stranger's bytes are refused.** A file that is not a segment, every truncation of one and
//!   every single flipped bit of one: each is an `Error::Protocol` that says what disagreed, never a
//!   panic and never a shorter answer. A log is replayed to a prefix; a segment is whole or refused;
//! * **the boundary holds.** `src/engine/**` may not name the service layer, read out of the sources
//!   as text (`jobq/3.1-engine-boundary`), and the files this step adds are where a new violation
//!   would appear.
//!
//! Nothing here sleeps, and nothing here measures a duration.

use std::path::{Path, PathBuf};

use lakelet::engine::segment::encode::{
    self, COUNT, DELTA, DIC, FRAME, LENGTH, NAME, PLAIN, RLE, TAG,
};
use lakelet::engine::segment::{self, compress, decode};
use lakelet::protocol::batch::Record;

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/format.md";

/// The metric names `tests/budget_segments.rs` prints and the course's baseline is written in.
const METRICS: [&str; 10] = [
    "samples",
    "series",
    "distinct_values",
    "samples_read",
    "mismatches_max",
    "column_block_bytes_max",
    "segment_bytes_max",
    "bytes_per_sample_max",
    "compression_ratio_max",
    "flat_column_block_bytes_max",
];

/// The tag names the page has to name, so the page and the constants are one vocabulary.
const TAGS: [&str; 4] = ["PLAIN", "RLE", "DELTA", "DIC"];

/// The four records the golden vector is taken from.
///
/// Chosen so that every one of the three encodings a real segment uses is the shorter one: two long
/// series names repeated twice each (a dictionary beats writing them out), a clock that advances by
/// a constant step (deltas beat eight bytes a stamp), and values that hold still for two samples at
/// a time (runs beat eight bytes a value) — one of those values being `-0.0`, whose bits are not the
/// bits of `0.0` and which a run-length that compared with `==` would lose.
fn golden_records() -> Vec<Record> {
    let epoch = 1_700_000_000_000i64;
    vec![
        record("cpu,host=web-01", epoch, 1.5),
        record("cpu,host=web-01", epoch + 1_000, 1.5),
        record("cpu,host=web-02", epoch + 2_000, -0.0),
        record("cpu,host=web-02", epoch + 3_000, -0.0),
    ]
}

/// The column block of [`golden_records`], spelled out: 123 bytes, and every one of them is the
/// format.
///
/// ```text
///  0..8    count: 4, as a u64
///  series:  tag 3 (DIC), 58 bytes — 2 names, then 2 runs of ids
///    02 00 00 00                        two names
///    0f 00 "cpu,host=web-01"            the first, 15 bytes
///    0f 00 "cpu,host=web-02"            the second
///    02 00 00 00                        two runs
///    02 00 00 00 00 00 00 00            twice entry 0
///    02 00 00 00 01 00 00 00            then twice entry 1
///  stamp:   tag 2 (DELTA), 14 bytes — the first stamp, then three steps
///    00 68 e5 cf 8b 01 00 00            1 700 000 000 000, as an i64
///    d0 0f                              zigzag(1000), three times
///  value:   tag 1 (RLE), 28 bytes — two runs of two
///    02 00 00 00                        two runs
///    02 00 00 00 00 00 00 00 00 00 f8 3f   twice 1.5 (0x3ff8000000000000)
///    02 00 00 00 00 00 00 00 00 00 00 80   twice -0.0 (0x8000000000000000)
/// ```
const GOLDEN: [u8; 123] = [
    0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x3a, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00,
    0x00, 0x0f, 0x00, 0x63, 0x70, 0x75, 0x2c, 0x68, 0x6f, 0x73, 0x74, 0x3d, 0x77, 0x65, 0x62, 0x2d,
    0x30, 0x31, 0x0f, 0x00, 0x63, 0x70, 0x75, 0x2c, 0x68, 0x6f, 0x73, 0x74, 0x3d, 0x77, 0x65, 0x62,
    0x2d, 0x30, 0x32, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x68, 0xe5, 0xcf,
    0x8b, 0x01, 0x00, 0x00, 0xd0, 0x0f, 0xd0, 0x0f, 0xd0, 0x0f, 0x01, 0x1c, 0x00, 0x00, 0x00, 0x02,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0x3f, 0x02,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
];

/// The regions of [`GOLDEN`], so a failure can say which part of the layout moved.
const REGIONS: [(&str, std::ops::Range<usize>); 4] = [
    ("the record count", 0..COUNT),
    ("the series column", COUNT..71),
    ("the timestamp column", 71..90),
    ("the value column", 90..123),
];

/// One record, as a client sends it.
fn record(series: &str, ts_millis: i64, value: f64) -> Record {
    Record {
        series: series.to_owned(),
        ts_millis,
        value,
    }
}

/// A store directory of this case's own, removed when it is dropped.
struct Dir {
    /// Where the segment lives.
    path: PathBuf,
}

impl Dir {
    /// Claim a directory for `label`.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lakelet-segment-spec-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("{} cannot be created: {error}", path.display()));
        Self { path }
    }

    /// A path inside it for a segment called `name`.
    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Write `records` as a segment of this case's own, read it back, and answer the path and the bytes.
fn round_trip(label: &str, records: &[Record]) -> (Dir, PathBuf, Vec<u8>, Vec<Record>) {
    let dir = Dir::new(label);
    let path = dir.file("segment");
    segment::write(&path, records).unwrap_or_else(|error| {
        panic!(
            "a segment of {} record(s) is written: {error}",
            records.len()
        )
    });
    let bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("the segment is readable: {error}"));
    let read = segment::read(&path)
        .unwrap_or_else(|error| panic!("the segment this step wrote is read back: {error}"));
    (dir, path, bytes, read)
}

/// `true` when every field of both records is the same, the value compared by *bits*.
fn same(left: &Record, right: &Record) -> bool {
    left.series == right.series
        && left.ts_millis == right.ts_millis
        && left.value.to_bits() == right.value.to_bits()
}

/// `true` when two record lists are the same record for record, including the value's bits.
fn same_all(left: &[Record], right: &[Record]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(l, r)| same(l, r))
}

/// A record list of `count` records over `series` series, whose values step every `hold` samples.
///
/// Series-major — one series' whole run of samples, then the next — so that the series column is
/// runs of one id, the clock advances by a constant step inside a run and jumps back once at each
/// boundary, and the values hold still for `hold` samples at a time. Those are the three shapes the
/// encodings in this format are for, and a workload that had none of them would measure the
/// compressor instead of the step.
fn workload(count: usize, series: usize, hold: usize) -> Vec<Record> {
    let per_series = count.div_ceil(series);
    let mut records = Vec::with_capacity(count);
    for number in 0..series {
        for position in 0..per_series {
            if records.len() >= count {
                break;
            }
            records.push(record(
                &format!("cpu,host=h{number}"),
                1_700_000_000_000 + position as i64 * 1_000,
                ((position / hold) % 16) as f64,
            ));
        }
    }
    records
}

/// The failure text of a decode of `bytes`, or a panic naming the state that should have failed.
fn refused(bytes: &[u8], what: &str) -> String {
    match segment::decode(bytes) {
        Ok(records) => panic!(
            "{what} was read as a segment of {} record(s) — a file that is not the layout this \
             build writes has to be refused, because a reader that guesses hands back somebody \
             else's bytes as a store",
            records.len()
        ),
        Err(error) => {
            assert_eq!(
                error.kind(),
                "protocol",
                "{what} is refused with a typed error naming what disagreed: {error}"
            );
            assert!(
                !error.to_string().is_empty(),
                "the refusal of {what} has to say what was wrong: {error}"
            );
            error.to_string()
        }
    }
}

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

#[test]
fn a_segment_round_trips_every_record_in_the_order_it_was_given() {
    let records = workload(2_000, 5, 4);
    let (_dir, path, _bytes, read) = round_trip("many", &records);
    assert!(
        same_all(&records, &read),
        "the {} record(s) that went in are the records that come out, in the same order: {:?} \
         against {:?} at the first difference",
        records.len(),
        records
            .iter()
            .zip(&read)
            .find(|(l, r)| !same(l, r))
            .map(|(l, _)| l),
        read.iter()
            .zip(&records)
            .find(|(l, r)| !same(l, r))
            .map(|(l, _)| l)
    );
    // And the file is the same segment: encoding its bytes again cannot answer something else.
    assert_eq!(
        segment::encode(&records).expect("a segment encodes"),
        std::fs::read(&path).expect("the file is there"),
        "`segment::encode` is the bytes `segment::write` puts on the device"
    );
}

#[test]
fn an_empty_segment_round_trips_as_no_records() {
    let (_dir, _path, bytes, read) = round_trip("empty", &[]);
    assert!(
        read.is_empty(),
        "a segment of no records reads back as no records: {read:?}"
    );
    assert!(
        bytes.len() >= segment::MIN_BYTES,
        "even an empty segment is a whole file — header, compressed block and seal — and this one \
         is {} byte(s), under the {} a segment cannot be shorter than",
        bytes.len(),
        segment::MIN_BYTES
    );
    assert!(
        segment::decode(&[]).is_err(),
        "an empty *file* is not an empty segment: a store with no records is a segment that says \
         so, and a file that was cut before anything was written is refused"
    );
}

#[test]
fn a_segment_of_one_record_round_trips() {
    let records = vec![record("cpu,h0", 1_700_000_000_000, -1.25)];
    let (_dir, _path, bytes, read) = round_trip("one", &records);
    assert!(
        same_all(&records, &read),
        "one record in, one record back: {read:?}"
    );
    assert!(
        bytes.len() >= segment::MIN_BYTES && bytes.len() < 1_024,
        "a segment of one record is a whole file and a small one — {} bytes",
        bytes.len()
    );
}

#[test]
fn a_flat_line_round_trips_and_its_columns_are_runs() {
    let records = vec![record("cpu,h0", 1_700_000_000_000, 1.5); 1_024];
    let (_dir, _path, _bytes, read) = round_trip("flat", &records);
    assert!(
        same_all(&records, &read),
        "a constant series with a constant clock and a constant value is 1 024 identical records, \
         and they come back as 1 024 identical records"
    );

    let block = encode::column_block(&records);
    let value = column(&block, 2).expect("the value column is framed in the block");
    assert_eq!(
        value.0, RLE,
        "a flat line's value column is its run-length encoding (tag {RLE}): every value has the \
         same bits, and eight bytes a record for one number is what this step exists to stop"
    );
    assert_eq!(
        value.1.len(),
        4 + 4 + 8,
        "one run of 1 024: a count, the run's length and the value — {} bytes, and a run-length \
         encoding that compared `f64`s with `==` would have produced one run per sample",
        value.1.len()
    );

    let stamp = column(&block, 1).expect("the timestamp column is framed in the block");
    assert_eq!(
        stamp.0, DELTA,
        "a constant clock is a delta encoding (tag {DELTA}) of zeros — a byte a stamp, not eight"
    );
    let series = column(&block, 0).expect("the series column is framed in the block");
    assert_eq!(
        series.0, DIC,
        "and one series repeated is a dictionary (tag {DIC}) with a single run"
    );
}

#[test]
fn the_values_that_break_a_careless_encoder_round_trip() {
    // The records are chosen so that the encodings *under test* are the ones written. Two ends of the
    // `i64` range in alternation: every difference is ±1 once it wraps, so the timestamp column is a
    // delta and not eight bytes a stamp — while `i64::MAX - i64::MIN` is the subtraction that does
    // not fit, which is why plain `-` panics in debug and wraps in release. Values in pairs, so the
    // value column is a run-length; without that, plain wins and this case would pass for a build
    // whose delta and run-length paths cannot round trip these values at all.
    let stamps = [i64::MIN, i64::MAX];
    let values = [
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
        f64::MIN,
        f64::MIN,
        -0.0,
        -0.0,
        0.0,
        0.0,
        f64::NAN,
        f64::NAN,
        f64::INFINITY,
        f64::INFINITY,
    ];
    let records: Vec<Record> = (0..values.len())
        .map(|index| record("cpu,host=h0", stamps[index % stamps.len()], values[index]))
        .collect();

    let block = encode::column_block(&records);
    assert_eq!(
        column(&block, 1).expect("the timestamp column is framed").0,
        DELTA,
        "the timestamps are chosen so that the delta encoding is the shorter one — a case about a \
         delta has to be checked against a column written plain"
    );
    assert_eq!(
        column(&block, 2).expect("the value column is framed").0,
        RLE,
        "and the values are chosen so that the run-length encoding is the shorter one: a case about \
         `-0.0` and `NaN` that left the column plain would pass without touching the comparison \
         that gets them wrong"
    );

    let (_dir, _path, _bytes, read) = round_trip("extremes", &records);
    assert!(
        same_all(&records, &read),
        "every `i64` is a timestamp and every `f64` is a value, and the two lists have to come back \
         the way they went in — bits included, because `-0.0` and `0.0` are different values and \
         `NaN != NaN`; the first difference is {:?}",
        records
            .iter()
            .zip(&read)
            .find(|(written, read)| !same(read, written))
            .map(|(written, _)| written)
    );
    // The two zeros are the case `==` gets wrong, so they are checked one at a time.
    let zeros = read
        .iter()
        .filter(|record| record.value == 0.0)
        .map(|record| record.value.to_bits())
        .collect::<Vec<u64>>();
    assert_eq!(
        zeros,
        vec![
            (-0.0f64).to_bits(),
            (-0.0f64).to_bits(),
            0.0f64.to_bits(),
            0.0f64.to_bits()
        ],
        "`-0.0` and `0.0` are both in the input and both in the output, in the order they went in: a \
         run-length encoding that grouped by `==` would have merged the two into one run and lost the \
         sign of a zero, and no later step can put it back"
    );
    assert_eq!(
        read.iter()
            .filter(|record| record.value.is_nan())
            .map(|record| record.value.to_bits())
            .collect::<Vec<u64>>(),
        vec![f64::NAN.to_bits(), f64::NAN.to_bits()],
        "and the two `NaN`s came back with the payload they had: `f64::from_bits` is the only route \
         that carries one, and a comparison is the one thing that cannot be asked about it"
    );
}

#[test]
fn the_column_block_is_the_bytes_that_the_format_states() {
    // The constants the vector is written in, so a build that moved one of them fails here rather
    // than four cases later in a place that looks unrelated.
    assert_eq!(segment::MAGIC, *b"LKLS", "the magic of a segment file");
    assert_eq!(segment::VERSION, 1, "the layout's version");
    assert_eq!(
        encode::ORDER,
        ["series", "timestamp", "value"],
        "the columns of a segment, in the order the block writes them"
    );
    assert_eq!(
        segment::COLUMNS as usize,
        encode::ORDER.len(),
        "the header's column count is the number of columns the block writes"
    );
    assert_eq!(
        (PLAIN, RLE, DELTA, DIC),
        (0, 1, 2, 3),
        "the four tags, and the order the page states them in"
    );
    assert_eq!(
        (TAG, LENGTH, NAME, FRAME),
        (1, 4, 2, 5),
        "the widths every column's framing is made of"
    );

    let block = encode::column_block(&golden_records());
    assert_eq!(
        block.len(),
        GOLDEN.len(),
        "the column block of four records is {} bytes and this format states {} — a column whose \
         encoding changed length is a format that moved, and `docs/format.md` is where it is \
         written down",
        block.len(),
        GOLDEN.len()
    );
    for (name, region) in &REGIONS {
        let expected = GOLDEN
            .get(region.clone())
            .expect("the region is in the vector");
        let found = block.get(region.clone());
        assert_eq!(
            found,
            Some(expected),
            "{name} moved: the format states {:02x?} at bytes {:?} and this build wrote {:02x?} \
             — the golden vector is the layout, and a reader of the page has to be able to read \
             these bytes by hand",
            expected,
            region,
            found.unwrap_or_default()
        );
    }
    assert_eq!(
        block,
        GOLDEN.to_vec(),
        "and the whole block is the vector, down to the last byte"
    );
    assert_eq!(
        decode::records(&block).expect("the vector decodes"),
        golden_records(),
        "the vector is the records it was taken from, which is what makes it a round trip and not \
         just a shape"
    );
}

#[test]
fn a_thousand_series_round_trip_through_the_dictionary() {
    let records: Vec<Record> = (0..2_000)
        .map(|index| {
            record(
                &format!("cpu,host=node-{index},dc=eu-1"),
                1_700_000_000_000 + index as i64,
                index as f64 / 4.0,
            )
        })
        .collect();
    let (_dir, _path, bytes, read) = round_trip("dictionary", &records);
    assert!(
        same_all(&records, &read),
        "two thousand distinct series names are two thousand dictionary entries, and every record \
         has to come back with the name it went in with"
    );
    let block = encode::column_block(&records);
    let series = column(&block, 0).expect("the series column is framed");
    assert_eq!(
        series.0, PLAIN,
        "with every name different, the dictionary is the *longer* encoding and the column is \
         written plain: the choice is by length, not by which encoding sounds better ({bytes:?})"
    );
}

#[test]
fn an_encoding_is_chosen_by_length_and_a_tie_goes_to_plain() {
    // One record, a six-byte name: plain is `2 + 6` bytes, a dictionary is `4 + (2 + 6) + 4 + 8`.
    let one = vec![record("cpu,h0", 1, 1.0)];
    let block = encode::column_block(&one);
    assert_eq!(
        column(&block, 0).expect("the series column is framed").0,
        PLAIN,
        "a dictionary pays for its table, so for one short name plain is the shorter payload and \
         the column is plain"
    );

    // Three records of that name: 24 bytes either way — and a tie goes to the lower tag.
    let three = vec![
        record("cpu,h0", 1, 1.0),
        record("cpu,h0", 2, 1.0),
        record("cpu,h0", 3, 1.0),
    ];
    let plain = 3 * (2 + 6);
    let dictionary = 4 + (2 + 6) + 4 + 8;
    assert_eq!(
        plain, dictionary,
        "this case is worth writing only while the two encodings are exactly the same length"
    );
    let block = encode::column_block(&three);
    assert_eq!(
        column(&block, 0).expect("the series column is framed").0,
        PLAIN,
        "when both payloads are {plain} bytes the encoding with the lower tag wins, and that is \
         plain ({PLAIN}) — a rule a reader can check, instead of whichever branch the code tried \
         first"
    );

    // One byte longer, written three times, and the dictionary is shorter after all.
    let longer = vec![
        record("cpu,h00", 1, 1.0),
        record("cpu,h00", 2, 1.0),
        record("cpu,h00", 3, 1.0),
    ];
    let block = encode::column_block(&longer);
    assert_eq!(
        column(&block, 0).expect("the series column is framed").0,
        DIC,
        "a name one byte longer, three times over, and the dictionary is the shorter one — which is \
         the rule the golden vector's series column is written by"
    );
}

#[test]
fn a_file_that_is_not_a_segment_is_refused() {
    let dir = Dir::new("foreign");
    let foreign = dir.file("notes.txt");
    std::fs::write(&foreign, b"hi! this is not a segment\n").expect("the fixture is written");
    let message = match segment::read(&foreign) {
        Ok(records) => panic!(
            "a text file was read as a segment of {} record(s) — the header is what makes \"this is \
             not this store's file\" a decision instead of a guess",
            records.len()
        ),
        Err(error) => {
            assert_eq!(
                error.kind(),
                "protocol",
                "a foreign file is a protocol refusal: {error}"
            );
            error.to_string()
        }
    };
    assert!(
        !message.is_empty(),
        "the refusal says what disagreed: {message}"
    );

    // The same file with the right four bytes and a version this build does not write.
    let mut version = b"LKLS".to_vec();
    version.push(99);
    version.push(segment::COLUMNS);
    version.resize(segment::MIN_BYTES, 0);
    let message = refused(&version, "a segment of a version this build cannot read");
    assert!(
        message.contains("99"),
        "the refusal names the version it found, because an operator holding a file from a newer \
         build needs to know which one: {message}"
    );

    // A file that was cut before it was anything at all.
    let _ = refused(b"LKL", "a file cut inside its magic");
}

#[test]
fn every_truncation_of_a_segment_is_refused() {
    let records = workload(64, 4, 2);
    let bytes = segment::encode(&records).expect("the records encode");
    let mut refusals: Vec<String> = Vec::new();
    for cut in 0..bytes.len() {
        let prefix = bytes.get(..cut).expect("a prefix of the segment");
        refusals.push(refused(
            prefix,
            &format!("the first {cut} of {} bytes", bytes.len()),
        ));
    }
    assert!(
        refusals.len() == bytes.len(),
        "every one of the {} byte(s) was tried",
        bytes.len()
    );
    // A log is replayed to a prefix; a segment is whole or refused. Three different things are
    // wrong in those three refusals and a reader that said one sentence for all of them would tell
    // an operator nothing about which file they are holding.
    let distinct: std::collections::BTreeSet<&String> = refusals.iter().collect();
    assert!(
        distinct.len() > 1,
        "the refusals name what disagreed, so a file cut inside its header and a file cut inside \
         its block do not read the same"
    );
}

#[test]
fn every_flipped_bit_of_a_segment_is_refused() {
    let records = workload(16, 2, 2);
    let bytes = segment::encode(&records).expect("the records encode");
    assert!(
        segment::decode(&bytes).is_ok(),
        "the segment this case corrupts is one a reader accepts, so the sweep below is about the \
         flip and not about the fixture"
    );
    for offset in 0..bytes.len() {
        let mut corrupted = bytes.clone();
        let Some(byte) = corrupted.get_mut(offset) else {
            continue;
        };
        *byte ^= 0b0000_0001;
        assert!(
            segment::decode(&corrupted).is_err(),
            "byte {offset} of {} was flipped and the segment was still read: every byte of a \
             segment is covered by the header, the seal or the length the header states, and a \
             reader that accepted these bytes would answer with a segment nobody wrote",
            bytes.len()
        );
    }
}

#[test]
fn a_segment_is_a_small_fraction_of_the_rows_it_holds() {
    let records = workload(1_024, 8, 16);
    let rows = lakelet::protocol::batch::encode(&records);
    let (_dir, _path, bytes, read) = round_trip("size", &records);
    assert!(
        same_all(&records, &read),
        "the round trip this measurement is about holds"
    );
    assert!(
        bytes.len() * 10 < rows.len(),
        "the same 1 024 records cost {} bytes as rows on the wire and {} bytes as a segment — the \
         columns and the compressor are worth an order of magnitude, and a run-length or delta \
         encoding that stopped shrinking anything would show up here",
        rows.len(),
        bytes.len()
    );
    assert!(
        bytes.len() * 4 <= records.len(),
        "and at most a quarter of a byte per sample on the device: {} byte(s) for {} sample(s)",
        bytes.len(),
        records.len()
    );

    // The compressor is judged the same way, and it is judged for the *limit* it obeys as well:
    // whoever decompresses a file is choosing how much memory to spend on that file's behalf.
    let block = encode::column_block(&records);
    let frame = compress::compress(&block).expect("a column block compresses");
    assert_eq!(
        compress::decompress(&frame, block.len()).expect("the frame decompresses"),
        block,
        "the column block survives the compressor exactly"
    );
    assert!(
        compress::decompress(&frame, block.len() - 1).is_err(),
        "a frame that would decompress to more than the caller allowed is refused: a compressed \
         block can describe far more output than it costs to store, and the limit is what keeps a \
         damaged file from choosing this process's allocation"
    );
}

#[test]
fn the_segment_sources_do_not_reach_into_the_service() {
    let engine = PathBuf::from("src/engine/segment");
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&engine, &mut files).expect("the segment module is part of the project");
    assert!(
        files.len() >= 4,
        "{} holds {} Rust file(s): the layout, the encoder, the decoder and the compressor are what \
         this step delivers",
        engine.display(),
        files.len()
    );
    for file in &files {
        let source = std::fs::read_to_string(file).expect("a source file the project keeps");
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !source.contains(forbidden),
                "{} names `{forbidden}` — the daemon uses the engine, never the other way, and this \
                 module is new code the rule applies to",
                file.display()
            );
        }
    }
}

#[test]
fn the_page_states_the_layout_and_the_budget_the_gate_compares() {
    let page = page();
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a page that still has a blank in it is the page \
             nobody wrote"
        );
    }
    for required in [
        "LKLS",
        "golden vector",
        "zstd",
        "i64::MIN",
        "-0.0",
        "NaN",
        "Error::Protocol",
        "upper bound",
        "cargo test -p lakelet --test budget_segments",
        "bench-budget",
        "1024",
    ] {
        assert!(
            page.contains(required),
            "{PAGE} does not state `{required}`, which is part of what makes this format a \
             contract instead of an implementation detail"
        );
    }
    for tag in TAGS {
        assert!(
            page.contains(tag),
            "{PAGE} does not name the `{tag}` encoding: the page and the constants have to be one \
             vocabulary, or a reader has to guess which number a paragraph is about"
        );
    }
    for metric in METRICS {
        assert!(
            page.contains(metric),
            "{PAGE} does not state `{metric}`, which the harness prints and the course's baseline \
             is written in"
        );
    }
    let order: Vec<usize> = ["series", "timestamp", "value"]
        .iter()
        .map(|column| {
            page.find(column)
                .unwrap_or_else(|| panic!("{PAGE} does not name the `{column}` column"))
        })
        .collect();
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "{PAGE} names the columns out of the order the block writes them in ({order:?})"
    );
}

/// The framed column at `index` of a column block: its tag and its payload.
///
/// The spec reads the block the way the format defines it rather than through a decoder, so that a
/// case about *which* encoding was chosen cannot be answered by the decoder that would have to know.
fn column(block: &[u8], index: usize) -> Option<(u8, &[u8])> {
    let mut cursor = COUNT;
    for _ in 0..index {
        let length = u32_at(block, cursor.checked_add(TAG)?)? as usize;
        cursor = cursor.checked_add(FRAME)?.checked_add(length)?;
    }
    let tag = *block.get(cursor)?;
    let length = u32_at(block, cursor.checked_add(TAG)?)? as usize;
    let start = cursor.checked_add(FRAME)?;
    let end = start.checked_add(length)?;
    block.get(start..end).map(|payload| (tag, payload))
}

/// The `u32` these four bytes are, little endian, or `None` when there are not four of them.
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(LENGTH)?)?;
    let mut array = [0u8; LENGTH];
    array.copy_from_slice(slice);
    Some(u32::from_le_bytes(array))
}

/// Every `.rs` file below `directory`, recursively.
fn collect_rust(directory: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rust(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    Ok(())
}
