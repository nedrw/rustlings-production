//! probe = "test/fuzzing@apply"
//! title = "A seed, a corpus, and the smallest input that still breaks it"
//! hint = "Walk from inputs that work with a seed you can repeat, save the smallest input that broke the parser and nothing else, and leave the run reproducible."
//!
//! Step `lakelet/4.2-parser-fuzz` is about looking at the inputs nobody thought of. The parser next
//! door is handed bytes a stranger chose, and the cases a person writes are the ones they already
//! suspect — so the walk is what finds the rest. Five rules make the difference between a search and
//! a story:
//!
//! * **a walk is a promise about a seed, not a number.** The same seed has to produce the same
//!   inputs in this process, in the next one and on another machine, and another seed has to walk
//!   somewhere else. That is why the generator is written out here instead of taken from a crate: a
//!   dependency that changes its algorithm in a patch release would silently change what a seed
//!   means, and a finding nobody can reproduce is a story about a Tuesday;
//! * **the walk starts from inputs that work.** Every mutation begins at bytes the parser accepts —
//!   a frame this crate's own writer produced — and some inputs are left alone. An input a parser
//!   *accepts* is how a refusal is told apart from a parser that has stopped working;
//! * **every kind the format needs is in the walk.** Truncation (the capture that ends inside a
//!   frame: a client that died, a file that was cut), a bit flip, an edit to the length field, and
//!   inserted junk. A mutator that only truncates finds `Incomplete` and no refusal; one that only
//!   flips bits never hands anyone a capture that stops in the middle of a frame;
//! * **a corpus entry is a saved input, and only a bug earns one.** A refusal is an answer — the
//!   parser did its job — so saving one is a corpus that looks like coverage and holds nothing; a
//!   panic is a bug, and it becomes a line of text in the repository so it is replayed on every
//!   build instead of remembered;
//! * **a shrinker makes the report small.** The smallest input that still fails is the one that
//!   explains the failure. A byte nobody can remove without losing the failure is a byte the next
//!   reader does not have to think about.
//!
//! ## What you write
//!
//! * [`Corpus::add`], [`Corpus::contains`], [`Corpus::to_text`] and [`Corpus::from_text`] — the
//!   inputs that once broke a parser, and the text file they are kept in;
//! * [`mutate`] — one mutation of an input, chosen by a seeded generator;
//! * [`shrink`] — the smallest input that still fails, by removing one byte at a time.
//!
//! [`Rng`], [`Corpus`] itself (empty, with its accessors), [`Refused`], [`frame`], [`crc32`],
//! [`declared_length`], [`hex`] and [`unhex`] are given. The generator is given because the seed's
//! promise is the interface the tests read; the frame writer is given because the base of a walk has
//! to be bytes the *previous* step's parser accepts; the two halves of the hex format are given
//! because a writer and a reader that disagree about what a line is are one decision, not two.
//!
//! ## What the tests ask
//!
//! Seed 1 walks the same inputs twice and seed 7 walks elsewhere, with the digests in the message so
//! a failure says which part of the sequence moved. The base input is a frame the parser accepts,
//! and the walk hands some inputs back unchanged. All four kinds are reached, including one that
//! declares a payload above the limit. A walk's findings become corpus entries, each one shrunk
//! until no single byte can be removed, and the corpus never holds the input the parser accepts. The
//! corpus saves one lower-case hex line per entry and reads back exactly what it wrote — comments and
//! blank lines are not entries — and a line that is not hex is refused by naming the line it was on.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **the step's own mechanism is not in it**: there is no
//! `src/protocol/sniff.rs` to fall over, no `catch_unwind` around a decoder, no `tests/corpus/**`
//! directory read in path order, and no gate. What is here is what a walk *does* with a finding —
//! keep the seed, keep the smallest input, keep it as text, tell a bug from a refusal — which is the
//! part of the habit that decays.
//!
//! * **the failure is arithmetic, not a panic.** The predicate the shrinker minimises against is the
//!   *condition* a scanner falls over on — a header that declares more bytes than the capture holds —
//!   written as a function of the bytes. In the step it is the real panic, caught by the harness and
//!   printed in the corpus's own format before it is let through; a probe has no parser to unwind;
//! * **the corpus is one text value**, not a directory of `.txt` files read in path order. The
//!   decision being re-checked is the same (one entry per line, hex, comments allowed, a bad line
//!   refused by name) and the filesystem half belongs to the project;
//! * **60,000 inputs a seed is the harness's number**, and `slow:fuzz@3` is the gate that runs three
//!   seeds and floors how much each one searched. The tests here walk a few thousand, which is what
//!   reaches every kind of mutation in seconds;
//! * **no `cargo-fuzz`**, which is what a production project uses and what needs a nightly toolchain
//!   and a crate that is not installed (the step says so too). The generator below is the substitute
//!   a `must` gate can run offline: the same idea, a smaller search, and the counters are the
//!   judgement.

/// The bytes one length takes: four, little endian.
pub const LENGTH: usize = 4;

/// Bytes of a capture before its payload: four of length and one of version.
pub const HEADER: usize = 5;

/// The version this build's frames carry.
pub const VERSION: u8 = 1;

/// The largest payload this format accepts, in bytes (1 MiB).
pub const MAX_PAYLOAD: usize = 1_048_576;

/// The CRC-32 (ISO-HDLC) of `bytes`: the checksum a frame carries.
///
/// Given, and the same function `lakelet/4.1-ingest-protocol`'s decoder computes (`crc32fast` there,
/// written out bitwise here because a probe has no dependencies): the frames the tests start from are
/// frames that decoder accepts.
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

/// The bytes a client would send for `payload`.
///
/// Given because the base of a walk has to be bytes a parser *accepts*, and the writer that produces
/// them is the previous step's: a walk that starts from random bytes is a byte generator with a
/// fuzzer's name on it.
#[must_use]
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + payload.len() + LENGTH);
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.push(VERSION);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&crc32(payload).to_le_bytes());
    bytes
}

/// The length a capture's header declares, or `None` when it does not hold four bytes of one.
///
/// Given because the one dangerous field of a length-prefixed format is read by the mutator, by the
/// corpus and by the tests alike, and three readings of it would be three chances to disagree about
/// which four bytes it is.
#[must_use]
pub fn declared_length(capture: &[u8]) -> Option<u32> {
    let bytes = capture.get(..LENGTH)?;
    let mut array = [0u8; LENGTH];
    array.copy_from_slice(bytes);
    Some(u32::from_le_bytes(array))
}

/// The line of hex that stands for `bytes`.
///
/// One input per line, lower case, two digits a byte: the format survives `git diff`, a code review
/// and a copy-paste into an issue, where a binary blob does not.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// The bytes a line of hex stands for, or `None` when the line is not an even run of hex digits.
///
/// Given with [`hex`]: the two halves of the format are one decision, and a corpus whose reader
/// disagreed with the harness that wrote a line would be a corpus nobody could replay.
#[must_use]
pub fn unhex(line: &str) -> Option<Vec<u8>> {
    let digits = line.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for &[high, low] in digits.as_chunks::<2>().0 {
        bytes.push(digit(high)? << 4 | digit(low)?);
    }
    Some(bytes)
}

/// One hex digit, or `None` when the byte is not one.
///
/// Written out rather than taken from `u8::from_str_radix`, which also accepts a leading sign: a
/// corpus line `+f` is not an entry this format has.
fn digit(byte: u8) -> Option<u8> {
    let value = (byte as char).to_digit(16)?;
    u8::try_from(value).ok()
}

/// The generator's state: one word, and xorshift64*.
///
/// Written out here rather than taken from a crate, because a fuzzer's generator is part of its
/// *evidence*: the same seed has to produce the same inputs in this process, in the next one and on
/// another machine. Given because the seed's promise is the interface every test in this file reads.
pub struct Rng {
    /// The current word.
    state: u64,
}

impl Rng {
    /// A generator for `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        // A zero word is xorshift's fixed point, so zero is pushed off it or seed 0 would walk the
        // same input for ever.
        Self { state: seed | 1 }
    }

    /// The next word.
    pub fn next(&mut self) -> u64 {
        let mut word = self.state;
        word ^= word >> 12;
        word ^= word << 25;
        word ^= word >> 27;
        self.state = word;
        word.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `bound`, which has to be positive.
    ///
    /// The modulo's bias is not worth removing: a walk needs a sequence that is the same everywhere
    /// and long enough to wander, not a sample that is uniform to the last bit.
    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// Why a corpus text is not a corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A line is neither blank, nor a comment, nor an even number of hex digits.
    ///
    /// Refused rather than skipped, because a corpus entry nobody can read back is worse than a
    /// missing one: it looks like coverage.
    NotHex {
        /// The line, 1-based, as an editor counts them.
        line: usize,
        /// The text of the line it was read from.
        text: String,
    },
}

/// The inputs that once broke the parser, in the order they were found.
///
/// The representation is given — a vector, in insertion order — because what a corpus entry *is* and
/// what one line of its file means are the decisions here, not the collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Corpus {
    /// The inputs, oldest first.
    entries: Vec<Vec<u8>>,
}

impl Corpus {
    /// An empty corpus: a fuzzer that has found nothing yet is a normal state, not an error.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The entries, in the order they were added.
    #[must_use]
    pub fn entries(&self) -> &[Vec<u8>] {
        &self.entries
    }

    /// How many inputs the corpus holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Is this corpus empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Save `bytes` as an entry, and answer whether it was new.
    ///
    /// An input the corpus already holds is not written twice: a file that keeps the same input nine
    /// times says nothing more than one that keeps it once, and its size stops being evidence.
    #[must_use]
    pub fn add(&mut self, bytes: &[u8]) -> bool {
        let _ = bytes;
        todo!("test/fuzzing@apply: save the input once, and answer whether it was new")
    }

    /// Does the corpus hold this input?
    #[must_use]
    pub fn contains(&self, bytes: &[u8]) -> bool {
        let _ = bytes;
        todo!("test/fuzzing@apply: is this input already saved?")
    }

    /// The corpus as the text a file holds: one line of hex per entry, in order.
    #[must_use]
    pub fn to_text(&self) -> String {
        todo!("test/fuzzing@apply: one line of hex per entry, in the order they were found")
    }

    /// The corpus a text stands for.
    ///
    /// A blank line and a line starting with `#` are not entries — the note that explains what a
    /// corpus is has to be able to live in the file — and any other line is an even number of hex
    /// digits, or it is refused.
    ///
    /// # Errors
    ///
    /// [`Refused::NotHex`], naming the line as an editor counts it and the text it holds.
    pub fn from_text(text: &str) -> Result<Self, Refused> {
        let _ = text;
        todo!(
            "test/fuzzing@apply: read the entries a corpus text holds, and refuse a line that is not one"
        )
    }
}

/// One mutation of `input`, chosen by `rng`.
///
/// Five kinds, and the last one is why the first four are worth anything: a walk whose every input is
/// broken says nothing about whether the parser still reads what it used to read.
///
/// * **truncate** — the capture that ends in the middle of a frame. A stream gives a reader that when
///   a client dies, and a file gives one that when it was cut;
/// * **flip** — one bit of one byte: a payload whose checksum no longer matches, a version that no
///   longer speaks for this build;
/// * **claim** — rewrite the four bytes of length, stamping the version byte that makes them a
///   header, with one of the lengths this format's arithmetic trips on: nothing, one byte, exactly
///   what the bytes hold after the header, a few bytes short of that, one byte more, the limit and
///   the limit plus one. A length taken from a random word would be refused by the limit before any
///   other part of a parser saw it, which is the hole a mutator that only flips bits has;
/// * **junk** — insert a few bytes: the header a capture tool wrote in front of a frame, the noise a
///   broken link put between two of them, or bytes of a protocol this decoder does not speak;
/// * **nothing** — the input, unchanged.
///
/// `input` is not modified: a mutation answers the input it made, so a walk can keep the base it
/// started from.
#[must_use]
pub fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let _ = (rng, input);
    todo!(
        "test/fuzzing@apply: one of the four kinds a length-prefixed format needs, or the input unchanged"
    )
}

/// The smallest input that still makes `fires` true, by removing one byte at a time.
///
/// `fires` is the failure being minimised — in the step, a parser that panics on the input. The
/// result still fires, and no single byte of it can be removed without losing that: a report the next
/// reader can hold, instead of the four hundred bytes the walk stumbled on.
#[must_use]
pub fn shrink(input: &[u8], fires: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let _ = (input, fires);
    todo!("test/fuzzing@apply: remove one byte at a time while the input still fires")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many inputs the walks below ask for.
    ///
    /// Small on purpose: the search judged by *how much* it did is the harness's (60,000 inputs a
    /// seed, under the seeds the step's course-side file names), and this file has seconds.
    const WALK: usize = 2_000;

    /// A payload that is not round-numbered, so a length and a payload mixed up cannot pass.
    fn payload(marker: u8) -> Vec<u8> {
        (0..37u8).map(|index| marker.wrapping_add(index)).collect()
    }

    /// The input a walk starts from: a frame this file's own writer produced, which is what makes the
    /// walk a parser fuzzer rather than a byte generator.
    fn base() -> Vec<u8> {
        frame(&payload(1))
    }

    /// Every input a walk under `seed` produces, in order.
    fn walk(seed: u64, iterations: usize) -> Vec<Vec<u8>> {
        let mut rng = Rng::new(seed);
        let base = base();
        (0..iterations).map(|_| mutate(&mut rng, &base)).collect()
    }

    /// A number that stands for a whole walk, so a comparison that fails prints two numbers instead
    /// of two thousand inputs.
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

    /// How many payload bytes a capture's header says follow it.
    fn available(capture: &[u8]) -> usize {
        capture.len().saturating_sub(HEADER)
    }

    /// The failure the walk below looks for, as a harness sees it: handing the capture to a parser
    /// unwinds.
    ///
    /// There is no parser in this file, so this is the *condition* the step's scanner falls over on —
    /// a header that declares more bytes than the capture holds, a length-trusting slice away from a
    /// panic (`src/protocol/sniff.rs` is where the step's walk found one). A refusal is a longer
    /// story: it needs the parser this file does not have, and it is an answer rather than a finding.
    fn falls_over(capture: &[u8]) -> bool {
        declared_length(capture).is_some_and(|declared| declared as usize > available(capture))
    }

    /// `true` when `result` is `input` with bytes spliced in at one place.
    fn one_insertion(input: &[u8], result: &[u8]) -> bool {
        if result.len() <= input.len() {
            return false;
        }
        let count = result.len() - input.len();
        (0..=input.len()).any(|at| {
            result.get(..at) == input.get(..at) && result.get(at + count..) == input.get(at..)
        })
    }

    #[test]
    fn a_seed_walks_the_same_inputs_and_another_seed_walks_elsewhere() {
        let first = walk(1, WALK);
        let again = walk(1, WALK);
        assert_eq!(
            first.len(),
            WALK,
            "the walk visited {} of the {WALK} input(s) it was asked for",
            first.len()
        );
        assert!(
            first == again,
            "seed 1 produced a different walk the second time (digests {:#018x} and {:#018x}): a walk \
             that does not repeat is a finding nobody can reproduce, so every seed in a report stops \
             meaning anything",
            digest(&first),
            digest(&again)
        );

        let other = walk(7, WALK);
        assert!(
            first != other,
            "seed 7 walked exactly what seed 1 walked (digest {:#018x}): the generator does not use \
             its seed, and a walk that ignores its seed is a loop with a fuzzer's name on it",
            digest(&first)
        );
    }

    #[test]
    fn a_mutation_can_leave_its_input_alone_and_the_base_is_one_the_parser_accepts() {
        let base = base();
        assert!(
            !falls_over(&base),
            "the input a walk starts from is a frame the decoder accepts: a walk whose every input \
             is already broken says nothing about whether a parser still reads what it used to read"
        );
        assert!(
            walk(1, WALK).contains(&base),
            "not one of {WALK} inputs was left alone: an input a parser *accepts* is how a refusal is \
             told apart from a parser that has stopped working"
        );
    }

    #[test]
    fn every_kind_of_mutation_a_length_prefixed_format_needs_is_in_the_walk() {
        let base = base();
        let inputs = walk(1, WALK);

        // Truncation: the capture that ends in the middle of a frame.
        assert!(
            inputs
                .iter()
                .any(|input| base.starts_with(input) && input.len() < base.len()),
            "no input is a strict prefix of the one the walk started from: a mutator that only \
             truncates finds `Incomplete` and never a refusal"
        );

        // A flip: one bit, at a byte of the payload, so the checksum no longer matches.
        assert!(
            inputs.iter().any(|input| {
                if input.len() != base.len() {
                    return false;
                }
                let changed: Vec<usize> = (0..base.len())
                    .filter(|at| input[*at] != base[*at])
                    .collect();
                matches!(changed.as_slice(), [at] if *at >= HEADER
                    && (input[*at] ^ base[*at]).is_power_of_two())
            }),
            "no input differs from the original in exactly one payload bit: a mutator that only \
             truncates never reaches a parser's arithmetic"
        );

        // A length edit: the header claims a length the bytes around it do not have, while every byte
        // after the header is untouched. This is the arithmetic a length-prefixed format's bugs live
        // in, and the one kind a bit-flipping mutator cannot aim at.
        assert!(
            inputs.iter().any(|input| {
                input.len() == base.len()
                    && input.get(HEADER..) == base.get(HEADER..)
                    && declared_length(input) != declared_length(&base)
                    && input.get(LENGTH) == Some(&VERSION)
            }),
            "no input has a header that lies about its length: a length-prefixed parser's edges are \
             exactly the arithmetic this kind is for"
        );
        assert!(
            inputs
                .iter()
                .filter_map(|input| declared_length(input))
                .any(|declared| declared as usize > MAX_PAYLOAD),
            "no input declares a payload above the limit, which is the one length no reader will \
             buffer and the edge the walk exists to reach"
        );

        // Junk: bytes spliced in — the header a capture tool wrote in front of a frame.
        assert!(
            inputs.iter().any(|input| one_insertion(&base, input)),
            "no input is the original with bytes spliced into it: the junk in front of a frame is the \
             input that teaches a scanner not to give up at byte one"
        );
    }

    #[test]
    fn a_panic_earns_a_corpus_entry_and_a_refusal_does_not() {
        let mut corpus = Corpus::new();
        let mut rng = Rng::new(1);
        let base = base();
        let mut findings = 0;
        for _ in 0..WALK {
            let input = mutate(&mut rng, &base);
            if falls_over(&input) {
                findings += 1;
                // The entry that goes in is the smallest input that still makes the parser fall
                // over: a report the next reader can hold, and one a second input that shrank to
                // the same bytes does not write again.
                let smallest = shrink(&input, falls_over);
                let saved = corpus.add(&smallest);
                assert!(
                    saved || corpus.contains(&smallest),
                    "`add` answered that an input was not new and the corpus does not hold it                      either: {smallest:?}"
                );
            }
        }
        assert!(
            findings > 0,
            "a walk of {WALK} inputs broke nothing: a walk that never reaches the format's edges is \
             a loop with a fuzzer's name on it"
        );
        assert!(
            !corpus.is_empty(),
            "a finding with no corpus entry is a note"
        );
        assert!(
            !corpus.contains(&base),
            "the corpus holds the input the parser accepts: an entry is a saved *failure*, and one \
             that never broke anything makes a corpus look like coverage"
        );

        for entry in corpus.entries() {
            assert!(
                falls_over(entry),
                "the corpus holds an input that no longer breaks the parser: {entry:?}"
            );
            assert!(
                (0..entry.len()).all(|at| {
                    let mut less = entry.to_vec();
                    less.remove(at);
                    !falls_over(&less)
                }),
                "a corpus entry with a byte that can still be removed is a report nobody minimised: \
                 {entry:?}"
            );
        }
    }

    #[test]
    fn a_corpus_saves_one_hex_line_per_entry_and_reads_them_back() {
        let mut corpus = Corpus::new();
        assert!(corpus.is_empty(), "a corpus starts with nothing in it");
        assert_eq!(
            corpus.to_text(),
            "",
            "an empty corpus is an empty file, not a broken one"
        );

        let first = frame(&payload(1));
        let second = vec![0x0a, 0x00, 0xff];
        assert!(
            corpus.add(&first),
            "an input the corpus does not hold is new"
        );
        assert!(corpus.add(&second), "and so is the next one");
        assert_eq!(
            corpus.entries(),
            &[first.clone(), second.clone()][..],
            "a corpus keeps the inputs in the order they were found"
        );
        assert!(
            !corpus.add(&first),
            "an input the corpus already holds is not written twice: a file that keeps the same \
             input nine times says nothing more than one that keeps it once"
        );
        assert_eq!(corpus.len(), 2, "the second `add` changed nothing");
        assert!(
            corpus.contains(&first) && corpus.contains(&second),
            "both directions: an entry that was saved is in the corpus"
        );
        assert!(
            !corpus.contains(&frame(&payload(2))),
            "and an input that was never saved is not"
        );

        // The format is the one a reviewer reads in a diff: one input per line, lower-case hex, two
        // digits a byte, and the last line ends.
        let text = corpus.to_text();
        assert_eq!(
            text,
            format!("{}\n{}\n", hex(&first), hex(&second)),
            "one hex line per entry, in the order they were found"
        );
        assert!(
            text.ends_with('\n'),
            "the last line ends, or `while read` drops it: {text:?}"
        );
        assert_eq!(
            unhex(&hex(&second)),
            Some(second.clone()),
            "the two halves of the format are inverses, which is why this file gives both"
        );

        let again = Corpus::from_text(&text).expect("a text this corpus wrote is a corpus");
        assert_eq!(
            again.entries(),
            corpus.entries(),
            "the same inputs, in the same order"
        );
        assert_eq!(
            again.to_text(),
            text,
            "and the same text when written again"
        );
    }

    #[test]
    fn a_line_that_is_not_hex_is_refused_by_naming_the_line() {
        // A comment and a blank line are not inputs: the note that explains what a corpus is has to
        // be able to live in the file.
        let text = "# the frames that broke the scanner\n\n0a0b\n\n# and this one\nff\n";
        let corpus = Corpus::from_text(text).expect("comments and blank lines are not entries");
        assert_eq!(
            corpus.entries(),
            &[vec![0x0a, 0x0b], vec![0xff]][..],
            "two entries: the third line of the file and the sixth"
        );
        assert_eq!(
            Corpus::from_text("")
                .expect("an empty file is an empty corpus, not a broken one")
                .len(),
            0
        );

        for (text, line) in [("0102\n0z\n", 2usize), ("0102\nabc\n", 2), ("+#\n", 1)] {
            let refused =
                Corpus::from_text(text).expect_err("a line that is not hex is not an entry");
            match refused {
                Refused::NotHex {
                    line: at,
                    text: said,
                } => {
                    assert_eq!(
                        at, line,
                        "a corpus line that is not hex is refused by naming the line it was on: \
                         {said:?}"
                    );
                    assert_eq!(
                        said,
                        text.lines()
                            .nth(line - 1)
                            .expect("the line the refusal names")
                            .trim(),
                        "and the text it read, so the entry can be found in the file"
                    );
                }
            }
        }
    }

    #[test]
    fn a_shrinker_leaves_no_byte_it_could_still_remove() {
        let finding = walk(7, WALK)
            .into_iter()
            .find(|input| falls_over(input))
            .unwrap_or_else(|| panic!("no walk of {WALK} inputs broke the parser"));
        let small = shrink(&finding, falls_over);
        assert!(
            falls_over(&small),
            "the shrunk input no longer breaks the parser: {} byte(s) became {}",
            finding.len(),
            small.len()
        );
        assert!(
            small.len() <= finding.len(),
            "shrinking made the report bigger: {} byte(s) became {}",
            finding.len(),
            small.len()
        );
        for at in 0..small.len() {
            let mut less = small.clone();
            less.remove(at);
            assert!(
                !falls_over(&less),
                "the report still holds a byte that can be removed ({at} of {}): the smallest input \
                 that fails is the one that explains the failure",
                small.len()
            );
        }

        // The smallest report a failure allows is the failure's own: one that needs two bytes shrinks
        // to two and no further, and shrinking a fixed point again changes nothing.
        let needs_two = |bytes: &[u8]| bytes.len() >= 2 && bytes.contains(&0xff);
        assert_eq!(
            shrink(&[1, 0xff, 2, 0xff, 3, 0xff], needs_two).len(),
            2,
            "a failure that needs two bytes has a two-byte report, whatever the walk stumbled on"
        );
        let two = shrink(&[7, 0xff], needs_two);
        assert_eq!(
            shrink(&two, needs_two),
            two,
            "shrinking a fixed point changes nothing: {two:?}"
        );
    }
}
