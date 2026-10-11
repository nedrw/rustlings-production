//! probe = "store/columnar@apply"
//! title = "A segment is columns, and a reader proves what it read"
//! hint = "Write the clock as steps and the values as their bits, and refuse a segment whose header and columns disagree instead of answering with the part of it that parsed."
//!
//! Step `lakelet/4.7-columnar-segments` is about the bytes a sample costs. A sample is
//! `(series, ts_millis, value)`, and a store that keeps one row per sample pays twenty-four bytes for
//! it — eight for the clock, eight for the value, eight for a short series name and its length —
//! while almost none of those bytes are information: the same series repeats in every row, the clock
//! advances by a step you could count on, and a value holds still for a while. Laid out by column,
//! those repetitions become *encodable*, and the step measures the same 1 024 records at 170 bytes
//! as a segment against 29 700 bytes as rows.
//!
//! The encoding is not free, and three of its costs are the whole step:
//!
//! * **it is a format.** Bytes laid out by hand with a version in front, and a seal over each
//!   column's length *and* its bytes — `lakelet/4.4-wal-recovery`'s shape, because a seal that
//!   covered the payload alone would let a flipped bit in a length verify and hand back bytes from
//!   the wrong offset;
//! * **a reader that cannot prove a value refuses rather than guesses.** A log is replayed to a
//!   prefix; a segment is whole or refused. Answering with the records that happened to decode
//!   before the bytes ran out turns a cut file into a shorter store — and it passes every case that
//!   only ever writes and reads whole files;
//! * **every length is compared with the bytes that are here before it is used.** As an end, as an
//!   offset, as a capacity: a file that states four billion bytes does not get to choose this
//!   process's allocation, or panic it;
//! * **exactness is bits.** `-0.0` has a sign and a `NaN` a payload that no comparison, no decimal
//!   round trip and no `format!` carries; and the clock is a count of milliseconds, so its steps are
//!   *wrapped* — `i64::MAX - i64::MIN` is not an `i64`, and an encoder that panics in a debug build
//!   while wrapping in a release one writes two different files for the same records.
//!
//! ## What you write
//!
//! * [`encode`] — the segment for a run of samples: the header, the clock column, the value column;
//! * [`decode`] — the reader: the samples the file proves, and a refusal naming what disagreed.
//!
//! [`Sample`], [`Column`], [`Refused`], the layout's constants, [`checksum`] and the varint pair
//! with [`zigzag`] are given. The helpers are given for the same reason `4.4`'s codec is: a byte
//! cursor is not this step's mistake, and a hand-rolled checksum in a course file would be wrong in
//! the ways nobody notices. What the seal *covers*, and where a reader refuses, is the exercise.
//!
//! ## The layout, byte by byte
//!
//! ```text
//!  0..4    LKLS        the magic of a segment file
//!  4       version     1 in this build
//!  5..13   count       u64 LE: how many samples the segment holds
//! 13..     the clock   [DELTA][length: u32 LE][bytes][seal: u32 LE]
//!  ..      the values  [PLAIN][length: u32 LE][bytes][seal: u32 LE]
//! ```
//!
//! The clock's bytes are the first stamp and then the steps between neighbours, each one
//! `put_varint(zigzag(step))`; the values are `value.to_bits()`, little end first, eight bytes each;
//! and a seal is [`checksum`] over the length field and then the column's bytes. Nothing follows the
//! value column, and a segment of no samples is a header, two empty columns and one seal each.
//!
//! ## What the tests ask
//!
//! A golden vector first: four known samples written out by hand as the bytes the layout states, so
//! the encoder is checked against the *file* and not against itself, and the numbers the prose
//! promises are pinned with it (the first stamp costs six bytes, a step of ten costs one, four
//! samples are a 72-byte file). Then the round trip, in order, over hundreds of samples and over the
//! values that break a careless encoder: `i64::MIN` next to `i64::MAX`, `-0.0` next to `0.0`, and
//! `NaN`, compared by bits. Then the size claim the encoding exists for: a clock that steps by ten
//! costs one byte a stamp where plain stamps cost eight. And then the refusals, which are the other
//! half of the step: a file that is not a segment, a version this build does not write, a tag it
//! does not write, a count that disagrees with the columns, a byte that trails them, *every*
//! truncation of a real segment, and *every* single flipped bit of one.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no compressor and no file on disk**: the step's format has four
//! columns (the series dictionary, the clock, the values), picks between two encodings per column by
//! length, pins a 123-byte golden vector and puts the whole block through one `zstd` frame; this
//! probe writes two columns and no dictionary, so `Sample` has no series name. The shape is the same
//! and it is the part a re-check is for: a header that carries the count, columns that are read
//! separately, a seal over each length and its bytes, and a reader that refuses what it cannot
//! prove. Compressing the block would add a dependency to a file that is compiled offline, and
//! pinning a compressor's bytes would test a `zstd` release instead of the learner.
//!
//! Missing on purpose: the size budget (`bytes_per_sample_max`) and the encodings that win it —
//! `RLE` for a flat value column, `DIC` for a repeated series, and the tie-break that decides
//! between two encodings of the same length —, the wall between `src/engine/**` and the service
//! layer, `unsafe-budget@0`, and compaction, which is `lakelet/4.8` and starts where one segment
//! meets another.

/// One sample: the clock and the value, which are the two columns this file writes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// The clock, in milliseconds since the epoch.
    pub ts_millis: i64,
    /// The value. Two samples are the same sample when their bits are, which is what `-0.0` and
    /// `NaN` are here to say.
    pub value: f64,
}

/// Which of a segment's two columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The clock, written as its first stamp and then its steps.
    Stamps,
    /// The values, written plain: eight bytes of bits each.
    Values,
}

/// Why a segment was refused: what disagreed, named.
///
/// The vocabulary is the exercise's interface — `4.7`'s reader answers an `Error::Protocol` that
/// says what it found — so the variants are given and the tests read them: a refusal that says only
/// "bad file" leaves an operator with a file and no idea which part of it is not theirs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The file does not begin with this build's magic. `found` is what was there, and it is
    /// shorter than four bytes when the file was cut inside its magic.
    Magic {
        /// The first bytes of the file, or all of them.
        found: Vec<u8>,
    },
    /// The magic is there and the rest of the header is not: a file cut inside its header.
    Header {
        /// How many bytes are here.
        available: usize,
    },
    /// The version byte is one this build does not write.
    Version {
        /// The version it found.
        found: u8,
    },
    /// A column's tag is not the encoding this build writes in that column.
    Tag {
        /// The column.
        column: Column,
        /// The tag it found.
        tag: u8,
    },
    /// The bytes a column's frame needs are not here: a file cut inside a length, inside a column's
    /// bytes, or inside its seal.
    Truncated {
        /// The column.
        column: Column,
        /// How many bytes of column the frame states, or zero when the file ends before the length
        /// that would state it.
        stated: u32,
        /// How many bytes are left where the frame needed more.
        available: usize,
    },
    /// The seal does not match the length and the bytes it covers.
    Seal {
        /// The column.
        column: Column,
    },
    /// A column does not hold what the header's count says it holds: `found` is the count the
    /// column actually holds — stamps in the clock column, eight-byte values in the value column.
    Count {
        /// The column.
        column: Column,
        /// The count the header states.
        stated: u64,
        /// The count the column holds.
        found: u64,
    },
    /// Bytes after the last column, which no column accounts for.
    Trailing {
        /// How many.
        bytes: usize,
    },
}

/// The magic of a segment file.
pub const MAGIC: [u8; 4] = *b"LKLS";

/// The version of the layout this build writes.
pub const VERSION: u8 = 1;

/// How long a segment's header is: the magic, the version and the record count.
pub const HEADER: usize = 13;

/// How wide a column's tag is.
pub const TAG: usize = 1;

/// How wide a column's length is, as a `u32` little end first.
pub const LENGTH: usize = 4;

/// How wide a column's seal is, as a `u32` little end first.
pub const SEAL: usize = 4;

/// The tag for a column nothing is done to: the bytes as they are.
pub const PLAIN: u8 = 0;

/// The tag for a column written as runs.
pub const RLE: u8 = 1;

/// The tag for a column written as its first value and then its steps.
pub const DELTA: u8 = 2;

/// The tag for a column written as a table of names and a run of ids.
pub const DIC: u8 = 3;

/// A digest of `bytes`, as a `u32`.
///
/// FNV-1a 32, given rather than written: a segment's seal is not this step's lesson, and a
/// hand-rolled checksum in a course file is wrong in ways nobody would notice. What is checked here
/// is what the seal *covers*, and where a reader refuses.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Append `value` to `out` as a base-128 varint: seven bits a byte, the low bits first, every byte
/// but the last marked with the high bit.
///
/// Given because a byte cursor is not this step's mistake. The layout, the seal and the refusals
/// are.
pub fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Read the varint at `*at`, answering its value and moving `*at` past it, or `None` when the bytes
/// run out inside one.
///
/// `None` is a refusal the caller has to make something of: a file that ends inside a varint is a
/// file that cannot be proved, and the cursor not moving is what lets a reader say how much of the
/// column it did read.
pub fn take_varint(input: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *input.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// A signed step as an unsigned varint: zigzag, so a step of ten costs one byte and a step of minus
/// ten costs one byte too.
///
/// Given for the same reason the varint is: how a *step* is spelled is not where this step's
/// mistakes live, and a reader of the format needs one answer. The wrap-around arithmetic around it
/// is the learner's.
#[must_use]
pub fn zigzag(delta: i64) -> u64 {
    ((delta << 1) ^ (delta >> 63)) as u64
}

/// The step [`zigzag`] encoded.
#[must_use]
pub fn unzigzag(encoded: u64) -> i64 {
    ((encoded >> 1) as i64) ^ -((encoded & 1) as i64)
}

/// Write `samples` as a segment: the header, the clock column and the value column.
///
/// The layout is the one the prose states, down to the last byte, and the count in the header is
/// what a reader proves the two columns against. Every seal is [`checksum`] over a column's length
/// field and then its bytes.
#[must_use]
pub fn encode(samples: &[Sample]) -> Vec<u8> {
    todo!()
}

/// Read a segment, answering the samples it proves — or what disagreed.
///
/// The proofs, in the order a reader makes them, because a reader that skips one is a reader that
/// answers with somebody else's bytes as a store:
///
/// 1. the magic and the header, and a file too short to hold either is refused before anything is
///    read out of it;
/// 2. the version, because a layout this build does not write is not one it can read;
/// 3. each column in turn: its tag, a length compared with the bytes that are here, and a seal over
///    the length *and* the bytes;
/// 4. that nothing trails the last column;
/// 5. that each column holds what the header's count says — the clock exactly that many stamps and
///    no bytes left over, the values exactly that many eight-byte values.
///
/// # Errors
///
/// One [`Refused`] naming what disagreed. Never a panic, and never the records that happened to
/// decode before the bytes ran out: this is the half of `4.7` that separates a segment from a log.
pub fn decode(bytes: &[u8]) -> Result<Vec<Sample>, Refused> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clock the cases step from, so a payload's size is not the only thing that is pinned.
    const EPOCH: i64 = 1_700_000_000_000;

    /// How far apart the samples of the long cases are, in milliseconds: a step small enough that
    /// `zigzag(step)` is one varint byte.
    const STEP: i64 = 10;

    /// One sample.
    fn sample(ts_millis: i64, value: f64) -> Sample {
        Sample { ts_millis, value }
    }

    /// `count` samples whose clock steps by [`STEP`] and whose values change every sample.
    fn samples(count: usize) -> Vec<Sample> {
        (0..count)
            .map(|index| sample(EPOCH + index as i64 * STEP, index as f64 / 4.0))
            .collect()
    }

    /// One column's tag and bytes, and where the next column starts.
    ///
    /// The tests' reader: it reads the *layout* and proves nothing, because proving the bytes is
    /// [`decode`]'s job and a case that used the decoder to check the decoder would check nothing.
    fn frame(segment: &[u8], at: usize) -> (u8, &[u8], usize) {
        let tag = segment[at];
        let stated = u32::from_le_bytes(
            segment[at + TAG..at + TAG + LENGTH]
                .try_into()
                .expect("four length bytes"),
        ) as usize;
        let start = at + TAG + LENGTH;
        let data = &segment[start..start + stated];
        (tag, data, start + stated + SEAL)
    }

    /// The two columns of a segment, by the layout's own constants.
    fn columns(segment: &[u8]) -> ((u8, &[u8]), (u8, &[u8])) {
        let (stamps_tag, stamps, at) = frame(segment, HEADER);
        let (values_tag, values, end) = frame(segment, at);
        assert_eq!(
            end,
            segment.len(),
            "the two columns are the whole file: {end} of {} bytes",
            segment.len()
        );
        ((stamps_tag, stamps), (values_tag, values))
    }

    /// A segment written by hand, with the helpers the encoder has: `count` in the header, and two
    /// columns framed and sealed exactly as the layout states.
    ///
    /// The cases about a header disagreeing with its columns need a file that is wrong in *that* way
    /// and no other, and a file [`encode`] wrote cannot be one — so the tests write this one
    /// themselves.
    fn file(count: u64, stamps: &[u8], values: &[u8]) -> Vec<u8> {
        let mut segment = Vec::new();
        segment.extend_from_slice(&MAGIC);
        segment.push(VERSION);
        segment.extend_from_slice(&count.to_le_bytes());
        for (tag, data) in [(DELTA, stamps), (PLAIN, values)] {
            segment.push(tag);
            let length_at = segment.len();
            segment.extend_from_slice(&(data.len() as u32).to_le_bytes());
            segment.extend_from_slice(data);
            let seal = checksum(&segment[length_at..]);
            segment.extend_from_slice(&seal.to_le_bytes());
        }
        segment
    }

    /// Four known samples and the segment the layout states for them.
    ///
    /// Every column's bytes are written out here rather than computed by [`encode`]: the clock as
    /// the first stamp and then its steps, the values as their bits — one of them `-0.0`, whose bits
    /// are not the bits of `0.0`.
    fn golden() -> (Vec<Sample>, Vec<u8>) {
        let samples = vec![
            sample(EPOCH, 1.5),
            sample(EPOCH + STEP, 1.5),
            sample(EPOCH + 2 * STEP, -0.0),
            sample(EPOCH + 3 * STEP, -0.0),
        ];
        let mut stamps = Vec::new();
        put_varint(&mut stamps, zigzag(EPOCH));
        for _ in 1..samples.len() {
            put_varint(&mut stamps, zigzag(STEP));
        }
        let mut values = Vec::new();
        for sample in &samples {
            values.extend_from_slice(&sample.value.to_bits().to_le_bytes());
        }
        (samples, file(4, &stamps, &values))
    }

    #[test]
    fn the_layout_is_the_bytes_the_tests_write_by_hand() {
        let (samples, segment) = golden();

        assert_eq!(
            encode(&samples),
            segment,
            "the encoder writes the layout this file states: the header, the clock column as steps, \
             the value column as bits, and a seal over each column's length and bytes"
        );
        assert_eq!(
            decode(&segment).expect("the segment the layout states is one a reader accepts"),
            samples,
            "and the vector is the samples it was taken from, which is what makes it a round trip and \
             not just a shape"
        );

        // The numbers the prose promises, in the vector's own terms.
        let ((stamps_tag, stamps), (values_tag, values)) = columns(&segment);
        assert_eq!(
            (stamps_tag, values_tag),
            (DELTA, PLAIN),
            "the clock column is written as steps ({DELTA}) and the value column plain ({PLAIN}): a \
             sample is two columns and each one is written the way its own shape says"
        );
        assert_eq!(
            stamps.len(),
            6 + 3,
            "four stamps in nine bytes: the first one costs six bytes zigzagged at the epoch and \
             every step of ten after it one, where eight bytes a stamp would have been thirty-two"
        );
        assert_eq!(
            values.len(),
            4 * 8,
            "and the values are eight bytes each: thirty-two bytes for four of them, because this \
             probe's value column is plain"
        );
        assert_eq!(
            segment.len(),
            HEADER + 2 * (TAG + LENGTH + SEAL) + stamps.len() + values.len(),
            "and the file is the header, the two frames and nothing else: {} bytes for four samples",
            segment.len()
        );
    }

    #[test]
    fn a_segment_round_trips_every_sample_in_the_order_it_was_given() {
        let samples = samples(512);
        let segment = encode(&samples);

        assert_eq!(
            decode(&segment).expect("a segment this build wrote is one it reads"),
            samples,
            "every sample in, the same sample out, in the same order"
        );
        assert_eq!(
            encode(&decode(&segment).expect("decoded once")),
            segment,
            "and encoding the decoded samples again is the same bytes: the layout is a function of \
             the samples, so two runs write one file"
        );

        // A segment of no samples is a segment: a header, two empty columns and their seals.
        let empty = encode(&[]);
        assert!(
            empty.len() >= HEADER + 2 * (TAG + LENGTH + SEAL),
            "even a segment of no samples is a whole file, and this one is {} bytes",
            empty.len()
        );
        assert_eq!(
            decode(&empty).expect("an empty segment round trips"),
            Vec::new(),
            "and it reads back as no samples"
        );
        assert_eq!(
            decode(&[]),
            Err(Refused::Magic { found: Vec::new() }),
            "while an empty *file* is not an empty segment: a store with no samples is a segment \
             that says so, and a file that was cut before anything was written is refused"
        );
    }

    #[test]
    fn the_values_that_break_a_careless_encoder_come_back_with_their_bits() {
        // Two ends of the `i64` range in alternation: every step wraps rather than overflowing, and
        // a reader that subtracted would have panicked in a debug build and wrapped in a release
        // one — two different files for the same records.
        let stamps_in = [i64::MIN, i64::MAX];
        let values_in = [
            f64::NEG_INFINITY,
            f64::MIN,
            -0.0,
            0.0,
            f64::NAN,
            f64::INFINITY,
        ];
        let samples: Vec<Sample> = values_in
            .iter()
            .enumerate()
            .map(|(index, value)| sample(stamps_in[index % stamps_in.len()], *value))
            .collect();

        let read = decode(&encode(&samples)).expect("the extremes round trip");
        assert_eq!(
            read.iter().map(|s| s.ts_millis).collect::<Vec<i64>>(),
            samples.iter().map(|s| s.ts_millis).collect::<Vec<i64>>(),
            "every `i64` is a stamp, including the two that are an `i64` apart in no direction: the \
             steps are wrapped and the wrap is the identity"
        );
        assert_eq!(
            read.iter().map(|s| s.value.to_bits()).collect::<Vec<u64>>(),
            samples
                .iter()
                .map(|s| s.value.to_bits())
                .collect::<Vec<u64>>(),
            "and every value comes back with the bits it had: `-0.0` has a sign and a `NaN` a \
             payload that no comparison and no decimal round trip carries"
        );

        // The two zeros are the pair `==` gets wrong, so they are checked one at a time and in
        // order.
        assert_eq!(
            read.iter()
                .filter(|s| s.value == 0.0)
                .map(|s| s.value.to_bits())
                .collect::<Vec<u64>>(),
            vec![(-0.0f64).to_bits(), 0.0f64.to_bits()],
            "`-0.0` and `0.0` went in and both came out, in the order they were written: a value \
             column that compared with `==` would have merged them and lost the sign of a zero"
        );
        assert_eq!(
            read.iter()
                .filter(|s| s.value.is_nan())
                .map(|s| s.value.to_bits())
                .collect::<Vec<u64>>(),
            vec![f64::NAN.to_bits()],
            "and the `NaN` came back as the `NaN` it was, because `f64::from_bits` is the only route \
             that carries its payload"
        );
    }

    #[test]
    fn a_clock_that_steps_by_ten_costs_one_byte_a_stamp() {
        let count = 4_096usize;
        let segment = encode(&samples(count));
        let ((_, stamps), (_, values)) = columns(&segment);

        assert_eq!(
            stamps.len(),
            6 + (count - 1),
            "a clock that steps by ten is nine bytes and five: the first stamp is six bytes \
             zigzagged at the epoch and every step after it one"
        );
        assert!(
            stamps.len() * 4 < count * 8,
            "the clock column is {} bytes and eight bytes a stamp would be {}: an encoding that \
             stopped being an order of magnitude smaller would show up here",
            stamps.len(),
            count * 8
        );
        assert_eq!(
            values.len(),
            count * 8,
            "and the values are still eight bytes each, because this file has no run-length \
             encoding: what comes down here is the clock, and `RLE` is the step's answer for a \
             column that holds still"
        );
    }

    #[test]
    fn a_file_that_is_not_a_segment_is_refused() {
        assert_eq!(
            decode(b"hi! this is not a segment\n"),
            Err(Refused::Magic {
                found: b"hi! ".to_vec()
            }),
            "the header is what makes \"this is not this store's file\" a decision instead of a \
             guess"
        );
        assert_eq!(
            decode(b"LKL"),
            Err(Refused::Magic {
                found: b"LKL".to_vec()
            }),
            "a file cut inside its magic shows the bytes that are there, which is what an operator \
             holding half a download needs"
        );

        let mut headerless = MAGIC.to_vec();
        headerless.resize(HEADER - 1, 0);
        assert_eq!(
            decode(&headerless),
            Err(Refused::Header {
                available: HEADER - 1
            }),
            "the magic and not the header: a file with four bytes of a segment in it is still not a \
             segment"
        );

        let (_, segment) = golden();
        let mut future = segment.clone();
        future[MAGIC.len()] = 99;
        assert_eq!(
            decode(&future),
            Err(Refused::Version { found: 99 }),
            "a version this build does not write is refused with the version it found: an operator \
             holding a file from a newer build has to know which one"
        );
    }

    #[test]
    fn a_tag_this_build_does_not_write_and_a_byte_that_trails_the_columns_are_refused() {
        let (_, segment) = golden();

        // A tag the file states and this build does not write in that column. The tag is not inside
        // the seal — the seal covers a length and the bytes after it — so this is a file whose
        // framing says one thing and whose bytes are seal-clean, and the tag is what refuses it.
        let mut retagged = segment.clone();
        retagged[HEADER] = RLE;
        assert_eq!(
            decode(&retagged),
            Err(Refused::Tag {
                column: Column::Stamps,
                tag: RLE
            }),
            "the clock column is written as steps, and `RLE` in it is a refusal rather than a guess: \
             this build writes one encoding per column"
        );
        let (_, stamps, values_at) = frame(&segment, HEADER);
        let mut retagged = segment.clone();
        retagged[values_at] = DIC;
        assert_eq!(
            decode(&retagged),
            Err(Refused::Tag {
                column: Column::Values,
                tag: DIC
            }),
            "and a dictionary in the value column is refused the same way, on the column that was \
             retagged: {} stamps came before it",
            stamps.len()
        );

        let mut extended = segment.clone();
        extended.push(0);
        assert_eq!(
            decode(&extended),
            Err(Refused::Trailing { bytes: 1 }),
            "and a byte after the last column is a byte no column accounts for: the file is exactly \
             its layout, and a reader that ignored the tail would accept a file something else wrote \
             into"
        );
    }

    #[test]
    fn every_truncation_of_a_segment_is_refused() {
        let (_, segment) = golden();
        assert!(
            decode(&segment).is_ok(),
            "the segment this case cuts is one a reader accepts, so the sweep below is about the \
             cut and not about the fixture"
        );

        let mut refusals = Vec::new();
        for cut in 0..segment.len() {
            let prefix = segment.get(..cut).expect("a prefix of the segment");
            match decode(prefix) {
                Ok(samples) => panic!(
                    "the first {cut} of {} bytes was read as a segment of {} sample(s): a segment is \
                     whole or refused, and a reader that answers with the part of it that parsed \
                     turns a cut file into a shorter store",
                    segment.len(),
                    samples.len()
                ),
                Err(refused) => refusals.push(refused),
            }
        }
        assert_eq!(
            refusals.len(),
            segment.len(),
            "every one of the {} byte(s) was tried",
            segment.len()
        );

        // Three different things are wrong inside those cuts, and a reader that said one sentence
        // for all of them would tell an operator nothing about which file they are holding.
        assert_eq!(
            refusals[2],
            Refused::Magic {
                found: segment[..2].to_vec()
            },
            "a file cut inside its magic is that, and says which bytes it found"
        );
        assert_eq!(
            refusals[HEADER],
            Refused::Truncated {
                column: Column::Stamps,
                stated: 0,
                available: 0
            },
            "a file cut exactly at the header has no column tag at all, and `stated: 0` is how a \
             refusal says there was no length to state"
        );
        assert_eq!(
            refusals[segment.len() - 1],
            Refused::Truncated {
                column: Column::Values,
                stated: 32,
                available: 3
            },
            "and a file cut inside the values' seal states the length the frame had and the bytes \
             that are here: nothing about it is guessed at"
        );
        let distinct: std::collections::BTreeSet<String> = refusals
            .iter()
            .map(|refused| format!("{refused:?}"))
            .collect();
        assert!(
            distinct.len() > 3,
            "the refusals name what disagreed, so a cut in the header, a cut in a column's bytes and \
             a cut in a seal do not read the same: {} kinds of refusal",
            distinct.len()
        );
    }

    #[test]
    fn every_flipped_bit_of_a_segment_is_refused() {
        let (_, segment) = golden();
        assert!(
            decode(&segment).is_ok(),
            "the segment this case corrupts is one a reader accepts, so the sweep below is about \
             the flip and not about the fixture"
        );

        for offset in 0..segment.len() {
            let mut corrupted = segment.clone();
            let Some(byte) = corrupted.get_mut(offset) else {
                continue;
            };
            *byte ^= 0b0000_0001;
            assert!(
                decode(&corrupted).is_err(),
                "byte {offset} of {} was flipped and the segment was still read: every byte of a \
                 segment is covered by the header, by a seal over a column's length and bytes, or by \
                 the count the columns are proved against, and a reader that accepted these bytes \
                 would answer with a segment nobody wrote",
                segment.len()
            );
        }
    }

    #[test]
    fn a_count_that_disagrees_with_the_columns_is_refused() {
        let (samples, segment) = golden();
        let ((_, stamps), (_, values)) = columns(&segment);

        // The same two columns with a count that is one too many, and one too few: the seals still
        // hold — they cover each column's length and its bytes, not the header's count — so the
        // count is the only thing wrong with these files.
        assert_eq!(
            decode(&file(samples.len() as u64 + 1, stamps, values)),
            Err(Refused::Count {
                column: Column::Stamps,
                stated: 5,
                found: 4
            }),
            "a header that promises five stamps and a column that holds four is refused, and the \
             refusal names both numbers"
        );
        assert_eq!(
            decode(&file(samples.len() as u64 - 1, stamps, values)),
            Err(Refused::Count {
                column: Column::Stamps,
                stated: 3,
                found: 4
            }),
            "and the other direction: a count the column overshoots is the same disagreement, which \
             is what \"the header means something\" has to mean"
        );

        // Values in the same shape: the clock column is consistent with the count and the value
        // column is not, so the refusal is the value column's.
        assert_eq!(
            decode(&file(samples.len() as u64, stamps, &values[..3 * 8])),
            Err(Refused::Count {
                column: Column::Values,
                stated: 4,
                found: 3
            }),
            "a value column that holds three values where the header says four is refused as the \
             value column's disagreement, not the clock's"
        );
        assert_eq!(
            decode(&file(samples.len() as u64, stamps, values)).expect("the count they agree on"),
            samples,
            "and with the count they do agree on, the same columns are the segment they came from"
        );
    }
}
