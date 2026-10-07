//! Spec for `lakelet/4.13-model-based-tests`: a seeded generator that replays, a model that knows
//! nothing about how the store keeps anything, and a walk that compares them after every operation.
//!
//! `kiln run` copies this file into the project as `tests/spec_model_based.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Seventeen cases, and the shape of them is the point:
//!
//! * **a seed is a sequence, and the sequence is arithmetic.** One seed in, the same numbers out,
//!   on any machine, in any build — a golden vector pins the stream so that a refactor cannot
//!   quietly move every walk in the project — and two seeds are two sequences, which is the half
//!   `slow:sim@3`'s "at least two transcripts differ" rule rests on. A zero seed is a seed like any
//!   other: the one state xorshift cannot leave is nudged off zero where the correction is visible;
//! * **a bounded draw keeps its bounds, and its word.** `below` is in range and covers it, `between`
//!   includes both ends, `pick` answers from the alphabet it was handed (and refuses nothing to
//!   draw from), `chance(0)` never and `chance(100)` always. The distribution case is a tenth of a
//!   range and a tenth of the draws: a walk needs a generator whose every part is reachable, not a
//!   statistician's generator;
//! * **the model is what a store was told.** A read returns what was appended, in the order it was
//!   appended; expiry moves one number and what is below it stops being an answer while it stays in
//!   the store; a series is compared exactly. Nothing in that file knows what a segment, a merge or
//!   a file is, which is the property the walk proves;
//! * **a broken sequence shrinks to something a person can read.** `shrink`'s contract is three
//!   sentences, and this file asserts all three — that the answer still fails, that its operations
//!   are the ones it was given in order, and that no single operation can be taken out and still
//!   leave a failure — against a walk whose model was never told what one flush closed;
//! * **the walks agree, and they replay.** The harness's own 12-operation walk agrees after every
//!   operation; a written-down sequence that reopens the store in the middle still answers what the
//!   model says; an expiry moves the floor and the answers shrink by exactly the records the model
//!   forgets; and a query is answered twice — by the store's executor over its segments, and by
//!   `lakelet/4.10-downsampling` over the model's records — with the same buckets;
//! * **the boundary holds.** `src/engine/**` may not name the service layer (`jobq/3.1-engine-boundary`),
//!   the generator is where a generator belongs, and the model — which is a *test's* idea of the
//!   store — names none of the engine's types. That is not a style rule: a model that knew what a
//!   segment was would be a second implementation of the store, and a walk against it would prove
//!   nothing about the first.
//!
//! Nothing here sleeps, nothing here reads a clock, and nothing here measures a duration: every
//! instant is a number the case passes in, and the generator is the only source of the rest.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lakelet::engine::compaction::Segments;
use lakelet::engine::query::downsample::{self, Aggregate, Range, Window};
use lakelet::engine::query::exec;
use lakelet::engine::query::parse::{Statement, parse};
use lakelet::engine::retention;
use lakelet::engine::rng::Rng;
use lakelet::protocol::batch::Record;

use support::model::Model;
use support::walk::{self, Defect, Dir, Op};

/// One hour, in milliseconds.
const HOUR: u64 = 3_600_000;

/// One minute, in milliseconds.
const MINUTE: u64 = 60_000;

/// The instant the cases' records start at: `472_222 * HOUR`, which is a bucket boundary for every
/// width this file counts in, so a case can name the boundary a cutoff snaps to.
const EPOCH: u64 = 472_222 * HOUR;

/// What the expiry case's policy keeps: an hour.
const KEEP: u64 = HOUR;

/// The bucket width that case's policy counts in: five minutes.
const PART: u64 = 5 * MINUTE;

/// Milliseconds between two records of one batch.
const STEP: i64 = 10_000;

/// One record of `series`, `offset` milliseconds after [`EPOCH`].
fn record(series: &str, offset: u64, value: f64) -> Record {
    Record {
        series: series.to_owned(),
        ts_millis: (EPOCH + offset) as i64,
        value,
    }
}

/// `count` records of `series`, ten seconds apart, starting `offset` milliseconds after [`EPOCH`].
fn batch(series: &str, offset: u64, count: usize) -> Vec<Record> {
    (0..count)
        .map(|index| {
            let at = offset + index as u64 * STEP as u64;
            record(series, at, index as f64 / 8.0)
        })
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

/// One bucket list as the triples a comparison is about: where a window starts, how many records it
/// held, and the value by bits.
fn buckets(list: &[downsample::Bucket]) -> Vec<(u64, u64, u64)> {
    list.iter()
        .map(|bucket| (bucket.start(), bucket.count(), bucket.value().to_bits()))
        .collect()
}

/// A store of a case's own, sealed from `batches` in order, with the model told the same records.
fn sealed(label: &str, batches: &[Vec<Record>]) -> (Dir, Segments, Model) {
    let dir = Dir::new(label);
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    let mut model = Model::new();
    for records in batches {
        store
            .seal(records)
            .unwrap_or_else(|error| panic!("a segment of a case's own records is closed: {error}"));
        model.append(records);
    }
    (dir, store, model)
}

/// One query, parsed the way the command line parses it.
fn parsed(text: &str) -> lakelet::engine::query::parse::Query {
    match parse(text) {
        Ok(Statement::Query(query)) | Ok(Statement::Explain(query)) => query,
        Err(refusal) => {
            panic!("`{text}` is a query this grammar has, and it refused it: {refusal}")
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

/// The text of a file of the project, or a sentence naming what is missing.
fn source(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("{path} cannot be read: {error} — this step's contract is written in it")
    })
}

#[test]
fn a_seed_is_a_sequence_and_the_same_seed_is_the_same_sequence() {
    // The golden vector: the first four numbers this build's generator answers for seed 1. They are
    // pinned because the whole walk is, and a refactor that quietly changed the stream would move
    // every seed of every step that walks with it rather than fail a case.
    let mut rng = Rng::new(1);
    let golden: Vec<u64> = (0..4).map(|_| rng.next_u64()).collect();
    assert_eq!(
        golden,
        vec![
            5_180_492_295_206_395_165,
            12_380_297_144_915_551_517,
            13_389_498_078_930_870_103,
            5_599_127_315_341_312_413
        ],
        "the first four numbers of seed 1 are the sequence this build's generator is defined by: \
         xorshift64* over the seed, which is arithmetic and not a dependency. A different sequence \
         here means the generator changed and every replay of every walk with it"
    );

    // The same seed is the same sequence: two generators built from one number answer the same
    // sixty-four numbers, with nothing shared between them.
    let mut first = Rng::new(1);
    let mut second = Rng::new(1);
    let sequence: Vec<u64> = (0..64).map(|_| first.next_u64()).collect();
    let again: Vec<u64> = (0..64).map(|_| second.next_u64()).collect();
    assert_eq!(
        sequence, again,
        "a generator is its seed and every draw moves the same state: two generators built from \
         seed 1 have to agree for ever, or a finding nobody can replay is a rumour"
    );

    // A different seed is a different sequence — the half `slow:sim@3` rests on, because a harness
    // whose generator ignored the seed would walk one schedule three times and pass.
    let mut seven = Rng::new(7);
    let other: Vec<u64> = (0..64).map(|_| seven.next_u64()).collect();
    assert_ne!(
        sequence, other,
        "seed 1 and seed 7 answer the same sixty-four numbers, so the seed reaches nothing: this \
         is the property `slow:sim@3` refuses three identical transcripts for"
    );
}

#[test]
fn a_zero_seed_is_a_seed_and_not_a_constant_stream() {
    // Zero is the one state xorshift cannot leave: all three shifts of it are zero, and the stream
    // would be the same number for ever. It is a legal seed all the same, because the first thing
    // anybody types into `KILN_SIM_SEED` is `0`.
    let mut zero = Rng::new(0);
    let drawn: Vec<u64> = (0..8).map(|_| zero.next_u64()).collect();
    assert!(
        drawn.iter().any(|value| *value != drawn[0]),
        "seed 0 answers {} eight times: the state was left at zero, and a generator that cannot \
         leave zero is a walk that walks one operation for ever",
        drawn[0]
    );
    assert!(
        drawn.iter().all(|value| *value != 0),
        "and none of them is zero: the state is nonzero after construction, so the multiply — an \
         odd constant, and therefore invertible — cannot answer zero"
    );

    // It is a seed like any other: not a special case the rest of the file has to know about.
    let mut one = Rng::new(1);
    let first: Vec<u64> = (0..8).map(|_| one.next_u64()).collect();
    assert_ne!(drawn, first, "and seed 0 is not seed 1 wearing a hat");
    let mut again = Rng::new(0);
    let repeated: Vec<u64> = (0..8).map(|_| again.next_u64()).collect();
    assert_eq!(drawn, repeated, "and it replays like every other seed");
}

#[test]
fn below_stays_in_its_range_and_covers_it() {
    let mut rng = Rng::new(4_133_000);
    for bound in [1u64, 2, 3, 7, 64, 1_000] {
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        for _ in 0..10_000 {
            let value = rng.below(bound);
            assert!(
                value < bound,
                "`below({bound})` answered {value}: a draw is in `0..bound`, and a walk that \
                 indexes a list with it depends on exactly that"
            );
            seen.insert(value);
        }
        assert_eq!(
            seen.len() as u64,
            bound,
            "`below({bound})` reached {} of the {bound} values in 10 000 draws: every part of an \
             alphabet has to be reachable, or the operation that lives there is never played",
            seen.len()
        );
    }

    // Nothing to draw from has one sane answer, and it is not a panic: a generator that panicked
    // here would fail inside somebody else's process.
    assert_eq!(
        rng.below(0),
        0,
        "`below(0)` is the index into a list of nothing, and the answer a caller can act on is the \
         first one"
    );
    assert_eq!(
        rng.below(1),
        0,
        "and `below(1)` has exactly one value to answer with"
    );
}

#[test]
fn every_tenth_of_the_range_gets_its_share_of_the_draws() {
    // A thousand values, ten tenths of them. This is not a test of a generator's statistics: it is
    // the claim that the *shape* of what a walk draws from is reachable — a stream that only ever
    // landed in the first tenth would walk one corner of a store for ever and pass every other case
    // in this file.
    let mut rng = Rng::new(20_260_101);
    let mut tenths = [0u32; 10];
    for _ in 0..10_000 {
        tenths[(rng.below(1_000) / 100) as usize] += 1;
    }
    for (index, count) in tenths.iter().enumerate() {
        assert!(
            (800..=1_200).contains(count),
            "the tenth {index} of `0..1000` came up {count} time(s) in 10 000 draws, and a thousand \
             values drawn ten thousand times is about a thousand each: the draws are not spread \
             over the range"
        );
    }
}

#[test]
fn between_includes_both_ends() {
    let mut rng = Rng::new(5);
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    for _ in 0..10_000 {
        let value = rng.between(3, 8);
        assert!(
            (3..=8).contains(&value),
            "`between(3, 8)` answered {value}: both ends are inside the span, because a clock, a \
             duration and a timestamp may all be at either end of one"
        );
        seen.insert(value);
    }
    assert_eq!(
        seen,
        BTreeSet::from([3, 4, 5, 6, 7, 8]),
        "every instant of the span is drawable, including the last: a half-open range would make \
         the millisecond a window ends on unreachable"
    );

    assert_eq!(
        rng.between(4, 4),
        4,
        "a span of one instant answers that instant"
    );
    assert_eq!(
        rng.between(9, 2),
        9,
        "and a span that names nothing — `high` at or below `low` — answers `low` rather than an \
         instant the caller did not ask about"
    );

    // Neither of those costs a draw, and that is the same rule `below(0)` states one level up: a
    // span with no range in it has no element to hand out. What a caller asked for decides where
    // in the stream the next number comes from, and a question with one answer in it is not a
    // draw.
    let mut asked = Rng::new(2_025);
    let mut straight = Rng::new(2_025);
    for _ in 0..8 {
        let _ = asked.between(4, 4);
        let _ = asked.between(9, 2);
    }
    assert_eq!(
        asked.next_u64(),
        straight.next_u64(),
        "sixteen spans that name nothing moved the stream: the two generators were built from one \
         seed and one of them was asked for nothing at all"
    );
    assert!(
        (-5..=-1).contains(&rng.between(-5, -1)),
        "the span is signed, because a record's timestamp is: a clock that was wrong lands before \
         the epoch, and a walk has to be able to follow it there"
    );
}

#[test]
fn pick_answers_from_the_list_it_was_given() {
    let mut rng = Rng::new(6);
    let alphabet = ["append", "flush", "merge", "expire", "reopen"];
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for _ in 0..10_000 {
        let chosen = rng.pick(&alphabet);
        assert!(
            alphabet.contains(chosen),
            "`pick` answered `{chosen}`, which is not in the alphabet it was handed"
        );
        seen.insert(chosen);
    }
    assert_eq!(
        seen.len(),
        alphabet.len(),
        "`pick` reached {} of the {} operations in 10 000 draws: a walk draws its next operation \
         from an alphabet, and one of them never being drawn is one of them never being tested",
        seen.len(),
        alphabet.len()
    );

    let only = ["merge"];
    assert_eq!(
        *rng.pick(&only),
        "merge",
        "an alphabet of one answers that one"
    );
}

#[test]
#[should_panic]
fn pick_has_nothing_to_answer_for_an_empty_alphabet() {
    // The other half of `below(0)`: an element of nothing is not a value this can answer with, and
    // an invented one would be a walk testing an operation nobody wrote.
    let mut rng = Rng::new(1);
    let empty: [&str; 0] = [];
    let _ = rng.pick(&empty);
}

#[test]
fn chance_keeps_its_word() {
    let mut rng = Rng::new(8);
    assert!(
        (0..1_000).all(|_| !rng.chance(0)),
        "`chance(0)` fired: zero in a hundred is never"
    );
    assert!(
        (0..1_000).all(|_| rng.chance(100)),
        "`chance(100)` did not fire: a hundred in a hundred is always"
    );
    assert!(
        (0..1_000).all(|_| rng.chance(150)),
        "`chance(150)` did not fire: a probability written as a whole number above a hundred is \
         not a refusal, it is `always`"
    );

    let mut fired = 0u32;
    for _ in 0..10_000 {
        if rng.chance(50) {
            fired += 1;
        }
    }
    assert!(
        (4_000..=6_000).contains(&fired),
        "`chance(50)` fired {fired} time(s) in 10 000 draws: a walk steps its clock backwards \
         about a quarter of the time, and a coin that is not a coin makes that step whatever the \
         generator happens to do"
    );
}

#[test]
fn the_model_answers_what_it_was_given_in_the_order_it_heard_it() {
    let mut model = Model::new();
    assert!(
        model.is_empty(),
        "a model of a store that was told nothing has nothing"
    );
    assert_eq!(model.len(), 0);
    assert_eq!(model.floor(), 0, "and it has forgotten nothing");
    assert!(model.answerable().is_empty());

    let first = batch("cpu,host=h0", 0, 3);
    let second = batch("cpu,host=h1", 5 * MINUTE, 2);
    let third = batch("cpu,host=h0", 10 * MINUTE, 1);
    model.append(&first);
    model.append(&second);
    model.append(&third);

    assert_eq!(model.len(), 6, "every record it was told about");
    let mut expected = first.clone();
    expected.extend(second.clone());
    expected.extend(third.clone());
    assert!(
        same(&model.answerable(), &expected),
        "a read returns what was appended, in the order it was appended — across the batches, and \
         across two series: the store hands a reader one list, and the model is that list"
    );
    assert!(
        same(
            &model.answerable_of("cpu,host=h0"),
            &[first, third].concat()
        ),
        "one series is the same rule narrowed to it, in the same order"
    );
    assert!(
        model.answerable_of("cpu,host=h2").is_empty(),
        "a series nobody sent is not a series this model has: the name is compared exactly, as the \
         store compares it"
    );
}

#[test]
fn the_floor_moves_forwards_and_what_it_forgot_stops_being_an_answer() {
    let mut model = Model::new();
    let old = batch("cpu,host=h0", 0, 4);
    let fresh = batch("cpu,host=h0", MINUTE, 4);
    model.append(&old);
    model.append(&fresh);

    // A pass whose cutoff is the start of the second batch: the four records at or after it are
    // what a read answers with, and the four below it are still in the store.
    let cutoff = EPOCH + MINUTE;
    model.expire(cutoff);
    assert_eq!(
        model.floor(),
        cutoff,
        "the floor is the cutoff the pass wrote down"
    );
    assert!(
        same(&model.answerable(), &fresh),
        "what is below the floor is not an answer any more, whatever the store still holds"
    );
    assert_eq!(
        model.len(),
        8,
        "and the count is what the store *has*: `lakelet/4.9-retention` deletes whole segments, so \
         the segment that straddles the cutoff keeps its old records — what the store holds and \
         what it answers are two questions"
    );

    // A promise that data is gone cannot be taken back: an earlier cutoff moves nothing.
    model.expire(cutoff - 30 * MINUTE);
    assert_eq!(
        model.floor(),
        cutoff,
        "a cutoff below the floor moves nothing: a policy whose window grew and a clock that \
         stepped backwards are both unable to make the store answer again"
    );

    // The boundary is half-open at the bottom only: a record whose timestamp *is* the floor is
    // still an answer (`lakelet/4.9-retention` keeps whole buckets, and the cutoff is the first
    // millisecond it keeps). This cutoff is exactly the timestamp of the second record of the
    // second batch.
    let on_a_record = EPOCH + MINUTE + STEP as u64;
    model.expire(on_a_record);
    assert_eq!(model.floor(), on_a_record);
    assert_eq!(
        model.answerable().len(),
        3,
        "the three records at or after the new floor are answers, and the one *at* it is one of \
         them: the cutoff is the first millisecond the store keeps, not the first it dropped"
    );
    assert_eq!(
        model.len(),
        8,
        "and the store still holds every record it was told about"
    );
}

#[test]
fn a_wrong_sequence_shrinks_to_the_operations_it_needs() {
    // A walk of eight operations in which the model is never told what the first flush closed: the
    // store keeps the records, the model does not, and the comparison after that flush is where the
    // two disagree. This is the failure the shrinker exists for, and what it has to produce is the
    // smallest sequence somebody can read in a bug report.
    let dir = Dir::new("model-based-shrink");
    let ops = vec![
        Op::Append {
            records: batch("cpu,host=h0", 0, 2),
        },
        Op::Flush,
        Op::Append {
            records: batch("cpu,host=h1", MINUTE, 3),
        },
        Op::Flush,
        Op::Append {
            records: batch("cpu,host=h2", 2 * MINUTE, 1),
        },
        Op::Flush,
        Op::Merge,
        Op::Reopen,
    ];
    let mut fails = |candidate: &[Op]| {
        !walk::run_with(candidate, dir.path(), Defect::ForgetFirstFlush).agreed()
    };

    let minimal = walk::shrink(&ops, &mut fails);
    assert_eq!(
        minimal.len(),
        2,
        "the smallest sequence that still disagrees has {} operation(s): the append whose records \
         the model was never told about, and the flush that closed them. Everything else — the \
         other two batches, the merge, the reopen — is not needed to see the store answer with a \
         record the model never heard",
        minimal.len()
    );
    assert!(
        matches!(minimal[0], Op::Append { .. }) && minimal[1] == Op::Flush,
        "and it is an append followed by a flush, in that order: {minimal:?}"
    );

    // The contract, asserted rather than assumed: the answer still fails, and no single operation
    // can be taken out of it and still leave a failure. That second half is what makes the sequence
    // in a bug report worth reading — a shrinker that stopped at the first shorter sequence would
    // report four operations where one pair would do.
    assert!(fails(&minimal), "the shrunk sequence still disagrees");
    assert!(
        !fails(&minimal[..1]),
        "the append alone does not: nothing was closed into a segment, so both halves answer \
         nothing and they agree"
    );
    assert!(
        !fails(&minimal[1..]),
        "and the flush alone does not: it closed no records, so there was nothing to forget"
    );
}

#[test]
fn a_short_walk_agrees_after_every_operation() {
    // The same walk the harness runs, cut down to twelve operations: random ones, from a seed,
    // against a real store — a directory of real segment files, a real manifest, real merges and
    // real expiries — compared with the model after every single one of them. Twelve is small
    // enough that a failing case names its operation by number, and this seed's twelve visit six
    // kinds of operation, merges and reopens included.
    let dir = Dir::new("model-based-short-walk");
    let ops = walk::plan(4, 12);
    let report = walk::run(&ops, dir.path());

    assert_eq!(
        report.played, 12,
        "the walk played every operation it was given"
    );
    assert!(
        report.agreed(),
        "the store and the model have to agree after every operation, not at the end: {}",
        report.mismatch.unwrap_or_default()
    );

    let kinds: BTreeSet<String> = ops
        .iter()
        .filter_map(|op| {
            walk::describe(op)
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
        .collect();
    assert!(
        kinds.len() >= 4,
        "the twelve operations are {} kind(s) of operation — a walk of one repeated operation is \
         not a walk, and a model that agrees with it proves nothing about a store",
        kinds.len()
    );
}

#[test]
fn a_walk_replays_what_it_was_seeded_with() {
    // `slow:sim@N` compares the lines a run printed with the lines a second run of the same seed
    // printed. That is a judgement about the harness, and the property it rests on is here: a seed
    // decides the operations *and* everything an operation prints, so nothing in a transcript may
    // be a path, an address or a clock reading.
    let dir = Dir::new("model-based-replay");
    let first = walk::run(&walk::plan(1, 16), dir.path());
    let again = walk::run(&walk::plan(1, 16), dir.path());
    assert!(first.agreed() && again.agreed(), "both replays agreed");
    assert_eq!(
        first.lines,
        again.lines,
        "seed 1 printed {} line(s) and then {} line(s) that differ: the same seed has to make the \
         same thing happen twice, or the walk is a flaky test wearing a simulation's name",
        first.lines.len(),
        again.lines.len()
    );

    // And two seeds are two walks: three identical transcripts would mean the harness never read
    // the seed, which is what `slow:sim@3` refuses.
    let other = walk::run(&walk::plan(7, 16), dir.path());
    assert!(other.agreed(), "seed 7 agreed too");
    assert_ne!(
        first.lines, other.lines,
        "seed 1 and seed 7 walked the same sixteen operations and printed the same transcript: the \
         seed reaches nothing, so the gate would be checking one schedule three times"
    );
}

#[test]
fn a_walk_that_reopens_in_the_middle_still_answers_the_same_records() {
    // Opening a store is what replays its log and reads its manifest (`lakelet/4.4-wal-recovery`),
    // and both of those touch the two things the model deliberately does not know about: a file of
    // appended bytes, and an index of segments. A walk that reopens in the middle is the claim that
    // neither of them is visible to a reader.
    let dir = Dir::new("model-based-reopen");
    let batches = [
        batch("cpu,host=h0", 0, 4),
        batch("cpu,host=h1", 10 * MINUTE, 3),
        batch("cpu,host=h0", 20 * MINUTE, 2),
    ];
    let ops = vec![
        Op::Append {
            records: batches[0].clone(),
        },
        Op::Flush,
        Op::Append {
            records: batches[1].clone(),
        },
        Op::Flush,
        Op::Reopen,
        Op::Append {
            records: batches[2].clone(),
        },
        Op::Flush,
        Op::Reopen,
    ];
    let report = walk::run(&ops, dir.path());
    assert!(
        report.agreed(),
        "a reopen is not an event in what a store was told, so it cannot change what a reader sees: \
         {}",
        report.mismatch.unwrap_or_default()
    );

    // And the store really is on the disk: the walk dropped it, so this opens it from the files it
    // left behind — the log replayed, the manifest read, the records back in the order they were
    // appended.
    let mut model = Model::new();
    for records in &batches {
        model.append(records);
    }
    let store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("the walk's own store directory opens again: {error}"));
    let answered = store
        .snapshot()
        .records()
        .unwrap_or_else(|error| panic!("a store this walk wrote reads back: {error}"));
    assert!(
        same(&answered, &model.answerable()),
        "a store opened from its files answers {} record(s) and the model {} — a reopen that \
         reordered them would be a merge's reordering seen through a log",
        answered.len(),
        model.answerable().len()
    );
}

#[test]
fn an_expiry_moves_the_floor_and_the_answers_shrink_with_it() {
    // Eight segments of four records, ten minutes apart, expired by a policy that keeps an hour and
    // counts in five-minute buckets. The cutoff is `now - keep` snapped *down* to a boundary, so the
    // pass removes whole segments and the records a read answers with lose a prefix — the model's
    // job is to know exactly how much.
    let dir = Dir::new("model-based-expire");
    let batches: Vec<Vec<Record>> = (0..8)
        .map(|index| batch("cpu,host=h0", index * 10 * MINUTE, 4))
        .collect();
    let mut model = Model::new();
    let mut store = Segments::open(dir.path())
        .unwrap_or_else(|error| panic!("a directory this process owns is a store: {error}"));
    for records in &batches {
        store
            .seal(records)
            .unwrap_or_else(|error| panic!("a sealed segment of a case's own records: {error}"));
        model.append(records);
    }
    assert_eq!(
        store.snapshot().records().expect("readable").len(),
        32,
        "every record of the store is an answer before anything expires"
    );

    // `now` ninety minutes in, keeping an hour: the cutoff is the boundary thirty minutes in, which
    // is inside the third segment's span.
    let policy = retention::Policy::new(KEEP, PART);
    let now = EPOCH + 90 * MINUTE;
    let cutoff = policy.cutoff(now);
    assert_eq!(
        cutoff,
        EPOCH + 30 * MINUTE,
        "the cutoff is `now - keep`, on a bucket boundary"
    );

    let report = store
        .expire(&policy, now)
        .unwrap_or_else(|error| panic!("a retention pass at a cutoff inside the store: {error}"));
    model.expire(cutoff);

    assert!(
        !report.is_empty(),
        "three whole segments end before the cutoff and they leave the store"
    );
    assert_eq!(
        store.floor(),
        model.floor(),
        "the floor the store wrote into its manifest is the floor the model moved: a read that \
         began before the pass answers what it would have answered, and a read after it is bounded \
         by the same number"
    );
    let answered = store.snapshot().records().expect("readable");
    assert_eq!(
        answered.len(),
        20,
        "the answers shrink by the twelve records the three whole segments held, and the segment \
         the cutoff lands inside keeps its old records rather than being rewritten around them"
    );
    assert!(
        same(&answered, &model.answerable()),
        "and what is left is exactly what the model says is answerable"
    );
    assert_eq!(
        model.len(),
        32,
        "while the store still holds all thirty-two: `len` is what it has, `answerable` is what it \
         answers"
    );

    // A second pass whose cutoff is below the floor moves nothing at all — no epoch, no file, no
    // floor — so there is nothing for the model to be told either.
    let again = store
        .expire(&policy, EPOCH + HOUR)
        .unwrap_or_else(|error| panic!("a second retention pass: {error}"));
    assert!(
        again.is_empty(),
        "the cutoff is below the floor, so nothing expires"
    );
    assert_eq!(
        store.floor(),
        cutoff,
        "and the floor is where the promise left it"
    );
    assert_eq!(
        model.floor(),
        cutoff,
        "which is what the model believes too"
    );
}

#[test]
fn a_merge_changes_what_the_store_is_and_not_what_a_reader_sees() {
    // Eight segments of four records each, and a merge that takes the oldest four of them. This is
    // the claim the whole step is built on, and it is made here deterministically rather than left
    // to a seed: the store's *shape* changes — the epoch moves and four segments become one — and
    // the records a reader is handed are the same thirty-two, in the same order.
    let batches: Vec<Vec<Record>> = (0..8)
        .map(|index| batch("cpu,host=h0", index * 10 * MINUTE, 4))
        .collect();
    let (_dir, mut store, model) = sealed("model-based-merge", &batches);
    let before = store
        .snapshot()
        .records()
        .unwrap_or_else(|error| panic!("a store of eight sealed segments reads back: {error}"));
    let epoch = store.snapshot().epoch();
    assert_eq!(before.len(), 32, "eight segments of four records");

    let report = store
        .compact()
        .unwrap_or_else(|error| panic!("a store of eight comparable segments merges: {error}"))
        .expect("the shipped policy has a plan for eight segments of one size");
    assert_eq!(
        report.merged().len(),
        4,
        "a merge takes the policy's run: `lakelet/4.8-compaction`'s width, and the four oldest \
         segments of a store whose sizes are all within the ratio"
    );
    assert_eq!(
        (report.segments_before(), report.segments_after()),
        (8, 5),
        "one segment where four were, at the position of the first of them"
    );
    assert!(
        store.snapshot().epoch() > epoch,
        "and the manifest is a new one: the epoch is the number that changes whenever the store \
         changes shape, and it is what a snapshot carries"
    );

    let after = store
        .snapshot()
        .records()
        .unwrap_or_else(|error| panic!("the merged store reads back: {error}"));
    assert_eq!(
        after.len(),
        32,
        "no record was lost by the merge and none was duplicated"
    );
    assert!(
        same(&after, &before),
        "and they are in the order they were appended: a merge writes its inputs' records into one \
         file without moving one in front of another"
    );
    assert!(
        same(&after, &model.answerable()),
        "which is what the model says a reader sees — a reader that could tell a merge had happened \
         would be a reader that had to hold a segment, and that is the bug this step exists to find"
    );
}

#[test]
fn a_query_is_answered_by_the_engine_and_by_the_model_the_same_way() {
    // The question is asked twice: once through `lakelet/4.11-query-engine` — a plan drawn from the
    // manifest, the segments it keeps, one read each — and once by `lakelet/4.10-downsampling` over
    // the records the model was told. Both have to answer the same buckets, because both are
    // answering about the same records.
    //
    // The window pass being shared is deliberate: the grid, the partial buckets and the five
    // aggregates are that step's spec's subject, and a walk that re-derived them would be a second
    // implementation of them. What this case judges is the store's bookkeeping — which segments a
    // query opens, and what a read of them is.
    let batches: Vec<Vec<Record>> = (0..4)
        .map(|index| batch("cpu,host=h0", index * HOUR, 6))
        .collect();
    let (_dir, store, model) = sealed("model-based-query", &batches);

    let text = format!(
        "SELECT mean(value) FROM {} WHERE ts BETWEEN {EPOCH} AND {} GROUP BY time({MINUTE}ms)",
        walk::METRIC,
        EPOCH + 4 * HOUR,
    );
    let query = parsed(&text);
    let outcome = exec::execute(&store.snapshot(), &query).unwrap_or_else(|refusal| {
        panic!("a query over a store of four segments is answered: {refusal}")
    });
    assert_eq!(
        outcome.answers().len(),
        1,
        "a query that groups by nothing has one answer, whatever the store holds"
    );
    assert_eq!(
        (outcome.segments_opened(), outcome.rows_scanned()),
        (4, 24),
        "the engine opened every segment and read every record: the answer comes from the store's \
         files and not from anywhere else"
    );

    let store_side = buckets(outcome.answers()[0].buckets());
    let model_side = buckets(
        downsample::downsample(
            &model.answerable(),
            None,
            Range::new(EPOCH, EPOCH + 4 * HOUR),
            Window::new(MINUTE),
            Aggregate::Mean,
        )
        .buckets(),
    );
    assert_eq!(
        store_side,
        model_side,
        "the executor answered {} bucket(s) and the model's own records make {}: a query reads the \
         segments a snapshot names, and what it answers is what the store was told",
        store_side.len(),
        model_side.len()
    );
    assert!(
        !store_side.is_empty(),
        "and the question was about a range that holds records, so this is a comparison of buckets \
         and not of two empty lists"
    );
}

#[test]
fn the_generator_is_engine_code_and_the_model_is_a_tests_idea_of_the_store() {
    // The boundary `lakelet/4.1-ingest-protocol` established, read out of the sources as text
    // (`jobq/3.1-engine-boundary`): the engine is what runs in a test, in a load generator and in a
    // repair tool, and none of those has a configuration file or a signal handler. This step adds
    // `src/engine/rng.rs`, which is where a new violation would appear.
    let engine = PathBuf::from("src/engine");
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&engine, &mut files).expect("the engine directory is part of the project");
    for file in &files {
        let text = std::fs::read_to_string(file).expect("a source file the project keeps");
        for forbidden in ["crate::service", "crate::cli"] {
            assert!(
                !text.contains(forbidden),
                "{} names `{forbidden}` — the daemon uses the engine, never the other way, and the \
                 file this step adds is where a new violation would appear",
                file.display()
            );
        }
    }

    // The generator is the engine's, and it is the surface this spec is written against: a
    // declaration the scaffold shipped and a body the learner wrote.
    let module = source("src/engine/mod.rs");
    assert!(
        module.contains("rng"),
        "the engine's module page does not name `rng`, so a reader of the engine never finds the \
         generator every walk in this project draws its operations from"
    );
    let generator = source("src/engine/rng.rs");
    assert!(
        generator.len() > 3_000,
        "`src/engine/rng.rs` holds {} byte(s): the arithmetic says something rather than holding a \
         heading and a `todo!()`",
        generator.len()
    );
    for declaration in [
        "pub const fn new",
        "pub fn next_u64",
        "pub fn below",
        "pub fn between",
        "pub fn pick",
        "pub fn chance",
    ] {
        assert!(
            generator.contains(declaration),
            "the generator does not state `{declaration}`, which is the surface this spec is \
             written against"
        );
    }

    // And the model is a *test's* idea of the store: it lives under `tests/`, and it names none of
    // the engine's types. That is the property the whole step is about — a model that knew what a
    // segment was would be a second implementation of the store, and a walk that compared the two
    // would prove nothing about the first. The check is on the file's text, which is how
    // `lakelet/4.9-retention`'s boundary case is written as well.
    let support = source("tests/support/mod.rs");
    for name in ["model", "walk"] {
        assert!(
            support.contains(name),
            "the fixtures do not declare `{name}`, so the harness and this spec cannot reach it"
        );
    }
    let model = source("tests/support/model.rs");
    assert!(
        model.len() > 2_000,
        "`tests/support/model.rs` holds {} byte(s): the model's rules are the work",
        model.len()
    );
    for forbidden in [
        "Segments",
        "Snapshot",
        "SegmentInfo",
        "Manifest",
        "Spool",
        "compaction",
    ] {
        assert!(
            !model.contains(forbidden),
            "the model names `{forbidden}`, and a model that knows how the store keeps a record is \
             a second store: what a reader sees is a prefix of what was appended, minus what the \
             floor forgot, and no file, index or merge may appear on this side of the comparison"
        );
    }

    // The harness is the device the gate reads, and it is the seed that decides its walk.
    let harness = source("tests/sim_model.rs");
    assert!(
        harness.contains("KILN_SIM_SEED"),
        "the harness does not read `KILN_SIM_SEED`, which is what `slow:sim@3` sets: a walk of one \
         fixed sequence would be one schedule reported three times"
    );
    assert!(
        harness.contains("shrink"),
        "and it does not shrink a failing walk, which is what turns a red log into a case somebody \
         can run"
    );
}
