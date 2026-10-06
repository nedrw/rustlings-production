//! Spec for `lakelet/4.2-parser-fuzz`: a seeded walk over mutated inputs, and the corpus it leaves
//! behind.
//!
//! `kiln run` copies this file into the project as `tests/spec_parser_fuzz.rs`; it is the course's
//! file, not the learner's. Seven cases, and the shape of them is the point:
//!
//! * **a walk is a promise about a seed, not a promise about a number** — the same seed has to
//!   produce the same inputs, here and in any other process, and two seeds have to produce
//!   different ones. A generator that reads the clock is a test that fails once a week for reasons
//!   nobody can reproduce, and a walk that ignores its seed is a loop with a fuzzer's name on it.
//!   Both halves are checked by walking twice and comparing, with the two walks' digests in the
//!   message so a failure says *which* part of the sequence moved;
//! * **the walk is not all rubbish** — the decoder has three answers (a frame, a refusal, and "not
//!   enough bytes yet") and a walk has to reach all three. A mutator that only truncates produces
//!   none of the first two (every strict prefix is `Incomplete`) and a mutator that only flips
//!   bytes never hands anyone a frame that is cut short, so this case is what tells a search from a
//!   noise generator;
//! * **the scanner is a tool, and the tool takes everything** — one property covers the corpus, the
//!   hostile shapes written down below and every input the walk produces: a range the scanner
//!   reports stays inside the capture, never overlaps the one before it, and is a whole frame, so
//!   the caller can slice it and hand it to `frame::decode` without a second look. The scanner is
//!   also checked as a *scanner*: a frame with junk in front of it is still found, which is the
//!   difference between finding frames and refusing anything that is not exactly one;
//! * **the corpus is a test, not a note** — every input under `tests/corpus/**` is replayed through
//!   the decoder and the scanner, and there has to be at least one of them. Replaying is what makes
//!   the input that once broke something a case that runs on every build; a corpus that holds
//!   nothing is a fuzzer that found nothing, and a corpus entry that stopped being read is a panic
//!   that can come back unnoticed;
//! * **the format is text** — a corpus line that is not hex is refused with the file and the line,
//!   and the hex the corpus module writes back is the line it read. A corpus entry that cannot be
//!   read back is worse than a missing one: it looks like coverage.
//!
//! The corpus is reached through `lakelet::protocol::corpus`, the same reader the step's harness
//! uses (`tests/fuzz_protocol.rs`). One format, one reader: an entry cannot be counted one way by
//! the harness and read another way here.
//!
//! Nothing here writes into the project's tree: the corpus cases read the corpus, and the one case
//! that writes builds its own directory below the system's temporary directory and removes it
//! again.

use std::path::Path;

use lakelet::protocol::frame::{self, Decoded, Frame, VERSION};
use lakelet::protocol::{corpus, mutate, sniff};

/// How many inputs the cases that only need to see a walk ask it for.
///
/// Small on purpose. The search that is judged by *how much* it does is the harness's
/// (`60_000` inputs per seed, under the seeds the course-side file lists); this file is run on
/// every `kiln run`, so it walks enough to reach every mutation a mutator has and no more.
const WALK: usize = 2_000;

/// The payload of the frame the scanner cases are built around: 37 bytes, and not a round number,
/// so a length and a payload mixed up cannot pass by accident.
fn payload() -> Vec<u8> {
    (0..37u8)
        .map(|index| index.wrapping_mul(7).wrapping_add(3))
        .collect()
}

/// One whole frame, as a client would send it.
fn a_frame() -> Vec<u8> {
    Frame::new(payload()).encode()
}

/// What a scanner is handed besides the corpus: the captures a hostile peer, a half-written file
/// and a broken capture tool leave behind.
///
/// Each entry says what it is, because a failure that names the shape is a failure somebody can
/// act on — "the scanner fell over on a header whose payload never arrived" is a report.
fn hostile() -> Vec<(&'static str, Vec<u8>)> {
    let frame = a_frame();
    let mut corrupted = frame.clone();
    if let Some(byte) = corrupted.get_mut(9) {
        *byte ^= 0x20;
    }
    let mut headered = vec![0x00, 0x00, 0x00];
    headered.extend_from_slice(&frame);
    vec![
        ("a capture shorter than a frame's header", vec![0x00, 0x01]),
        ("a capture that is nothing at all", Vec::new()),
        (
            "a header whose payload never arrived",
            vec![0x05, 0x00, 0x00, 0x00, VERSION],
        ),
        (
            "a header that declares two bytes and has one",
            vec![0x02, 0x00, 0x00, 0x00, VERSION, 0x7f],
        ),
        (
            "a length above the protocol's limit",
            vec![0xff, 0xff, 0xff, 0xff, VERSION],
        ),
        (
            "a version this build does not speak",
            vec![0x01, 0x00, 0x00, 0x00, VERSION.wrapping_add(9)],
        ),
        ("a frame whose payload was corrupted", corrupted),
        ("junk that is not a frame", vec![0xaa; 64]),
        (
            "a frame with an empty payload",
            Frame::new(Vec::new()).encode(),
        ),
        ("a frame with three bytes in front of it", headered),
    ]
}

/// Every input a walk under `seed` produces, in order.
fn walked(seed: u64, iterations: usize) -> Vec<Vec<u8>> {
    let mut inputs = Vec::with_capacity(iterations);
    mutate::walk(seed, iterations, |input| inputs.push(input.to_vec()));
    inputs
}

/// A number that stands for a whole walk, so a comparison that fails prints two numbers instead of
/// two thousand inputs.
fn digest(inputs: &[Vec<u8>]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for input in inputs {
        hash = (hash ^ input.len() as u64).wrapping_mul(0x0000_0100_0000_01b3);
        for byte in input {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// The corpus as the harness reads it, with the failure a missing corpus deserves.
fn cases() -> Vec<corpus::Case> {
    corpus::load(Path::new(corpus::DIR)).unwrap_or_else(|error| {
        panic!(
            "the corpus below {} cannot be read: {error} — a corpus entry is here to be replayed, \
             so it has to be readable",
            corpus::DIR
        )
    })
}

/// Where a corpus entry came from, as a failure names it.
fn origin(case: &corpus::Case) -> String {
    format!("{}/{}", corpus::DIR, case.file)
}

/// Hand `bytes` to both parsers and check what a range has to be.
///
/// A panic is the first failure of this case and needs no assertion: the scanner's contract says it
/// takes any bytes, including the ones a peer chose, so falling over is the bug. The three
/// assertions after it are the part a panic would not catch — a range that is outside the capture,
/// a range that overlaps the one before it, or a range that is not a whole frame.
fn parsed(bytes: &[u8], what: &str) {
    let _ = frame::decode(bytes);
    let mut previous = 0;
    for range in sniff::ranges(bytes) {
        assert!(
            range.start >= previous && range.end <= bytes.len(),
            "{what}: the scanner reported {range:?} in a capture of {} byte(s), after {previous}",
            bytes.len()
        );
        let Some(frame) = bytes.get(range.clone()) else {
            panic!("{what}: the range {range:?} is not a slice of the capture");
        };
        match frame::decode(frame) {
            Decoded::Frame(..) => {}
            other => panic!(
                "{what}: the scanner reported the range {range:?}, and the decoder reads it as \
                 {other:?} — a range is a whole frame, not a guess at one"
            ),
        }
        previous = range.end;
    }
}

#[test]
fn the_same_seed_walks_the_same_inputs() {
    let first = walked(1, WALK);
    let again = walked(1, WALK);
    assert_eq!(
        first.len(),
        WALK,
        "the walk visited {} of the {WALK} input(s) it was asked for",
        first.len()
    );
    assert!(
        first == again,
        "seed 1 produced a different walk the second time (digests {:#018x} and {:#018x}): a walk \
         that does not repeat is a walk nobody can reproduce, so a failure from it is a story \
         rather than a case",
        digest(&first),
        digest(&again)
    );
}

#[test]
fn another_seed_walks_somewhere_else() {
    let first = walked(1, WALK);
    let second = walked(7, WALK);
    assert!(
        first != second,
        "seed 7 walked exactly what seed 1 walked (digest {:#018x}): the generator does not use \
         its seed, so the seed a report names says nothing about the inputs that were tried",
        digest(&first)
    );
}

#[test]
fn the_walk_reaches_all_three_answers_of_the_decoder() {
    for seed in [1, 7] {
        let mut whole = 0usize;
        let mut refused = 0usize;
        let mut incomplete = 0usize;
        for input in walked(seed, WALK) {
            match frame::decode(&input) {
                Decoded::Frame(..) => whole += 1,
                Decoded::Invalid { .. } => refused += 1,
                Decoded::Incomplete { .. } => incomplete += 1,
            }
        }
        assert!(
            whole > 0,
            "seed {seed}: no input the decoder accepts. A walk that never produces a whole frame \
             cannot tell a parser that refuses the right bytes from one that refuses everything"
        );
        assert!(
            refused > 0,
            "seed {seed}: no input the decoder refuses. A walk of truncated frames produces \
             `Incomplete` and nothing else, and a refusal is where a parser's arithmetic is"
        );
        assert!(
            incomplete > 0,
            "seed {seed}: no input that is cut short. A walk that only flips bytes never hands \
             anyone a capture that ends in the middle of a frame, which is what a stream gives a \
             reader when a client dies"
        );
    }
}

#[test]
fn the_scanner_takes_everything_it_is_handed() {
    for (what, bytes) in hostile() {
        parsed(&bytes, what);
    }
    for case in cases() {
        parsed(
            &case.bytes,
            &format!("{}:{} — a corpus entry", origin(&case), case.line),
        );
    }
    for (index, input) in walked(1, WALK).iter().enumerate() {
        parsed(input, &format!("input {index} of the walk under seed 1"));
    }
}

#[test]
fn the_scanner_finds_a_frame_surrounded_by_junk() {
    let frame = a_frame();
    let mut capture = vec![0xaa; 5];
    capture.extend_from_slice(&frame);
    capture.extend_from_slice(&[0xaa; 3]);

    let ranges = sniff::ranges(&capture);
    assert_eq!(
        ranges.len(),
        1,
        "the capture holds one frame and the scanner reported {}: a scanner that gives up at the \
         first byte that is not a frame reports none, and one that trusts a length and a version \
         alone reports every accident in the junk ({ranges:?})",
        ranges.len()
    );
    let range = ranges.first().expect("the case above asked for one range");
    assert_eq!(
        capture.get(range.clone()),
        Some(frame.as_slice()),
        "the range the scanner reports is not the frame the capture holds: {range:?}"
    );
}

#[test]
fn every_corpus_entry_replays_without_being_a_surprise() {
    let cases = cases();
    assert!(
        !cases.is_empty(),
        "{} holds no `.txt` entry: the corpus is where the input a fuzzer found goes — the harness \
         (`tests/fuzz_protocol.rs`) prints that input when the walk falls over one — and a corpus \
         that holds nothing is a search that found nothing",
        corpus::DIR
    );
    for case in &cases {
        let where_from = format!("{}:{}", origin(case), case.line);
        // Replaying is the whole point of keeping an input: the decoder and the scanner are handed
        // the bytes that once broke one of them, on every build, for as long as the entry is here.
        parsed(&case.bytes, &where_from);
        let text = std::fs::read_to_string(Path::new(corpus::DIR).join(&case.file))
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", origin(case)));
        let line = text
            .lines()
            .nth(case.line - 1)
            .unwrap_or_else(|| panic!("{where_from} is not a line of the file it was read from"));
        assert_eq!(
            corpus::hex(&case.bytes),
            line.trim(),
            "{where_from} is not the entry it reads as: the format is one input per line, in hex"
        );
    }
}

#[test]
fn the_corpus_format_is_text_and_a_line_that_is_not_hex_is_refused() {
    let directory = std::env::temp_dir().join(format!("lakelet-corpus-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("parser"))
        .unwrap_or_else(|error| panic!("{} cannot be created: {error}", directory.display()));

    // A directory that is not there holds no input and is not an error: a fuzzer that has found
    // nothing yet is a normal state, and `every_corpus_entry_replays_without_being_a_surprise` is
    // the case that refuses an empty corpus.
    let missing = corpus::load(&directory.join("nothing-here"))
        .unwrap_or_else(|error| panic!("a corpus that is not there is an empty corpus: {error}"));
    assert!(
        missing.is_empty(),
        "a directory that does not exist held {missing:?}"
    );

    std::fs::write(
        directory.join("parser/one.txt"),
        "# a comment\n\n0102\n0a0B\n",
    )
    .unwrap_or_else(|error| panic!("the fixture could not be written: {error}"));
    let cases =
        corpus::load(&directory).unwrap_or_else(|error| panic!("the fixture is a corpus: {error}"));
    assert_eq!(
        cases.len(),
        2,
        "a comment and a blank line are not inputs: {cases:?}"
    );
    let first = cases.first().expect("the fixture holds two inputs");
    assert_eq!(first.bytes, [0x01, 0x02]);
    assert_eq!(first.line, 3, "the entry is on the third line of the file");
    assert_eq!(first.file, "parser/one.txt");
    assert_eq!(
        corpus::hex(&first.bytes),
        "0102",
        "the hex a corpus entry writes back is the text it read"
    );

    std::fs::write(directory.join("parser/two.txt"), "0102\n0z\n")
        .unwrap_or_else(|error| panic!("the fixture could not be written: {error}"));
    let error = corpus::load(&directory).expect_err("`0z` is not a byte in hex");
    let said = error.to_string();
    assert!(
        said.contains("two.txt:2"),
        "a corpus line that is not hex is refused by naming the file and the line: {said}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}
