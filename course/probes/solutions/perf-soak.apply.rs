//! probe = "perf/soak@apply"
//! title = "What is still alive after a long run"
//! hint = "Judge every counter at every round against its band — a counter below its floor proves nothing and one the run never printed is a violation — and let a verdict need enough rounds, with a leak being a counter that moved between the halfway mark and the end."
//!
//! Step `lakelet/4.17-soak` is the question every performance number before it cannot answer. A
//! budget, a phase profile and a sampler all answer "how much did that cost"; none of them answers
//! *what is still there* after a process has been up for nine days. A store can be fast, honest about
//! every count it keeps, and still hold one file descriptor per connection until the machine stops at
//! the descriptor limit on day nine with no red build anywhere. The instrument for that question is
//! the cheapest one there is: run the cycle the service already does, thousands of times, against
//! **one** store, and read what is alive at the end.
//!
//! Four decisions, and each one is the difference between a soak and a loop:
//!
//! * **the counters are stocks, so the bounds are ceilings — and they have floors.**
//!   `open_tasks`, `open_handles`, `store_files`, `store_bytes` and `records` are counts of things
//!   that exist right now, and "about right" is not a verdict for a number that goes wrong by
//!   growing: the ceiling is "no worse than this". The floor is the other half — a soak that answered
//!   nothing at all has a steady state for the wrong reason, and a run that did too little work
//!   proves nothing;
//! * **the steady state is two marks, not a curve.** The run has to have lasted long enough to say
//!   anything, and then the counters at the halfway mark and at the end are the same counters:
//!   every cycle writes a segment and retention forgets one, so a store that is working returns to
//!   where it was, and a counter that grew is a leak with a name on it;
//! * **the clock is printed and never compared.** The same correct store takes twice as long on a
//!   shared machine and reports the same counts, and a leak that happens during a quiet night passes
//!   a wall-clock criterion. Minutes are what a human reads; the counters are what decides;
//! * **one violation names one counter.** A report that says "something grew" is the sentence this
//!   step exists to replace, so every violation carries the counter, the round it left its band, and
//!   both numbers — and one counter is one line, however long it has been wrong for.
//!
//! ## What you write
//!
//! * [`check`] — every counter of a transcript against the band the course's table gives it;
//! * [`steady`] — whether the run settled, or which counters moved between the two marks.
//!
//! [`Series`], [`Band`], [`Bounds`], [`Violation`] and [`Steady`] are given, and so is the bounds
//! table ([`TABLE`]): the counters are the step's five, the numbers are the course's, and a probe
//! that let you rename them would be judging a different soak.
//!
//! ## What the tests ask
//!
//! A counter below its floor is a violation naming that counter and the round — a soak whose store
//! answered nothing did too little work to prove anything. A counter above its ceiling is a violation
//! at the round it left its band. A counter the table bounds and the run never printed is a violation
//! of its own: a harness that stopped reporting a metric is a gate comparing nothing. A run with every
//! counter inside its band and its floor reached has nothing to report. The verdict needs enough
//! rounds — one reading is not a comparison, even when the caller asks for no rounds at all — and it
//! names a counter that moved between the halfway mark and the end with both of those numbers. The
//! allowance is a share of the halfway value, so ten permille of a thousand is ten bytes and ten
//! permille of zero is zero. And a counter that only grows is the leak the whole step exists to find,
//! named by both questions.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no run and no store**: the real gate runs `tests/soak.rs` — four
//! thousand cycles against one store, a batch through the protocol path to a real daemon in the same
//! process, a tick that closes a segment, retention that forgets, and one question per cycle — and
//! judges the lines it printed, which take minutes rather than milliseconds. What is re-checked here
//! is the accounting those lines decide by: the bands, the floors, the refusal to name nothing, and
//! the steady-state rule.
//!
//! Also missing on purpose: the transcript is the answer here, where the real harness *produces* it,
//! so nothing here can catch a counter that was counted at the wrong place — whether `store_files`
//! counts the files the manifest names rather than what is lying in the directory is
//! `lakelet/4.17-soak`'s own decision and its spec's; and there is no wall clock at all, not even as
//! evidence, because a duration is exactly the number this step decided not to judge.

use std::collections::BTreeMap;

/// The bounds table, as data — the shape `course/tracks/lakelet/soak/4.17-soak.toml` keeps it.
///
/// Given, because these are the course's numbers: the counter, the floor a run has to reach, the
/// ceiling on what it may hold, and how much it may differ between the halfway mark and the end (in
/// permille of the halfway value). The ceilings carry the room a different machine and a different
/// `zstd` release need, and no more.
const TABLE: &[(&str, u64, u64, u64)] = &[
    // Tasks the service has spawned and not reaped. Two are alive for the whole run, and a run that
    // reads three is a run whose maintenance ticks are piling up.
    ("open_tasks", 2, 3, 0),
    // Files the store holds open between two cycles. Measured at zero: one is allowed for a build
    // that caches its log handle on purpose, and one *per cycle* is the defect.
    ("open_handles", 0, 1, 0),
    // Files the store itself names: six windows of history plus the manifest, measured at thirteen.
    ("store_files", 12, 14, 0),
    // What those files cost together, measured between 2734 and 2747 bytes and bounded well over
    // twice that, because a `zstd` release may answer different bytes for the same records.
    ("store_bytes", 2_000, 6_000, 10),
    // What the store holds. This is the control: it has to move while the other four stay flat,
    // because a store that is doing no work is steady for the wrong reason.
    ("records", 300, 3_000, 0),
];

/// What one counter is allowed to do over a soak: a band, not a ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// The least this counter may read. A stock below its floor is a run that did too little work to
    /// prove anything — a soak that finished three cycles has not seen the growth it exists to find.
    pub min: u64,
    /// The most it may read.
    pub max: u64,
    /// How much it may differ between the halfway mark and the end, in permille of the value it had
    /// at the halfway mark. Zero for a stock that has to be flat; not zero for a number a compressor
    /// decides.
    pub drift_permille: u64,
}

/// The bounds a soak is judged by, one band per counter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bounds {
    /// The table, in a sorted map so one table is one value and a report reads the same way twice.
    bands: BTreeMap<String, Band>,
}

impl Bounds {
    /// Bounds from a table of `counter`, floor, ceiling and drift allowance, as [`TABLE`] writes it.
    #[must_use]
    pub fn of(rows: &[(&str, u64, u64, u64)]) -> Self {
        Self {
            bands: rows
                .iter()
                .map(|(counter, min, max, drift_permille)| {
                    (
                        (*counter).to_owned(),
                        Band {
                            min: *min,
                            max: *max,
                            drift_permille: *drift_permille,
                        },
                    )
                })
                .collect(),
        }
    }

    /// What one counter is allowed to do, or `None` for a counter nothing bounds.
    #[must_use]
    pub fn band(&self, counter: &str) -> Option<&Band> {
        self.bands.get(counter)
    }

    /// Every counter the table bounds, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Band)> {
        self.bands
            .iter()
            .map(|(counter, band)| (counter.as_str(), band))
    }

    /// How many counters the table bounds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bands.len()
    }

    /// `true` when the table bounds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }
}

/// A soak's transcript: what every counter read, round by round, oldest round first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Series {
    /// How many rounds the run lasted.
    rounds: usize,
    /// One reading per round per counter, in a sorted map so two readings of one transcript agree.
    per_counter: BTreeMap<String, Vec<u64>>,
}

impl Series {
    /// A transcript from one row per counter, oldest round first.
    ///
    /// # Panics
    ///
    /// A row of a different length is a transcript nobody printed: a soak reads its counters in the
    /// same round, so the rows have to be as long as the run.
    #[must_use]
    pub fn of(rows: &[(&str, &[u64])]) -> Self {
        let rounds = rows.first().map_or(0, |(_, values)| values.len());
        for (counter, values) in rows {
            assert!(
                values.len() == rounds,
                "`{counter}` has {} reading(s) where the run has {rounds}: a transcript whose \
                 counters were read in different rounds is not a transcript",
                values.len()
            );
        }
        Self {
            rounds,
            per_counter: rows
                .iter()
                .map(|(counter, values)| ((*counter).to_owned(), values.to_vec()))
                .collect(),
        }
    }

    /// How many rounds the run lasted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rounds
    }

    /// `true` when the transcript has no rounds in it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rounds == 0
    }

    /// What one counter read, round by round, or `None` for a counter the run never printed.
    #[must_use]
    pub fn rounds(&self, counter: &str) -> Option<&[u64]> {
        self.per_counter.get(counter).map(Vec::as_slice)
    }

    /// Every counter the run printed, in name order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.per_counter.keys().map(String::as_str)
    }
}

/// One counter that is not where the bounds say it may be.
///
/// One entry per counter, and the class carries the counter, the round and the bound: a soak's report
/// has to name what moved, because "something grew" is the sentence this step replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// It read below its floor at this round: the run did too little work to prove anything.
    BelowMin {
        /// Which counter.
        counter: String,
        /// The round it read that at, counting from zero.
        round: usize,
        /// What it read.
        value: u64,
        /// The floor it broke.
        min: u64,
    },
    /// It read above its ceiling at this round.
    AboveMax {
        /// Which counter.
        counter: String,
        /// The round it read that at, counting from zero.
        round: usize,
        /// What it read.
        value: u64,
        /// The ceiling it broke.
        max: u64,
    },
    /// The bounds name a counter the run never printed. A gate whose metric has vanished is a gate
    /// comparing nothing, which is exactly the state one stays green in for ever.
    NeverReported {
        /// Which counter.
        counter: String,
    },
}

/// Whether a soak settled, and what stopped it when it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Steady {
    /// Enough rounds, and nothing moved between the two marks by more than its allowance.
    Steady,
    /// Fewer rounds than a verdict needs: a steady state is a claim about a run that lasted.
    TooShort {
        /// How many rounds the run printed.
        rounds: usize,
        /// How many a verdict needs, which is never fewer than two.
        needed: usize,
    },
    /// The counters that moved between the halfway mark and the end by more than their allowance: one
    /// entry per counter, with the value at the halfway mark and the value at the end.
    Moved {
        /// Counter, value at the halfway mark, value at the end.
        counters: Vec<(String, u64, u64)>,
    },
}

/// Every counter of a transcript against the band the bounds give it.
///
/// One entry per counter and no more: the round a counter is reported at is the *first* one at which
/// it left its band, because a report with four thousand lines for one leak is a report nobody reads.
/// A counter the bounds do not name is not judged — the table is the whole judgement — and a counter
/// the bounds name and the run does not print is a violation rather than a silence.
#[must_use]
pub fn check(samples: &Series, bounds: &Bounds) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (counter, band) in bounds.iter() {
        let Some(values) = samples.rounds(counter) else {
            violations.push(Violation::NeverReported {
                counter: counter.to_owned(),
            });
            continue;
        };
        let outside = values
            .iter()
            .enumerate()
            .find(|(_, value)| **value < band.min || **value > band.max);
        if let Some((round, value)) = outside {
            violations.push(if *value < band.min {
                Violation::BelowMin {
                    counter: counter.to_owned(),
                    round,
                    value: *value,
                    min: band.min,
                }
            } else {
                Violation::AboveMax {
                    counter: counter.to_owned(),
                    round,
                    value: *value,
                    max: band.max,
                }
            });
        }
    }
    violations
}

/// Whether the run settled: the same counters at the halfway mark and at the end.
///
/// The two marks are the round at `len() / 2` and the last round, and a counter is allowed to differ
/// between them by its band's `drift_permille` of the value it had at the halfway mark — a value of
/// zero has no share at all, so a counter that read nothing and now reads something has moved. Whether
/// the numbers are inside their bands is [`check`]'s question; the harness asks both, because a leak
/// and a stock outside its ceiling are two failures and the counters above the floor are what a steady
/// state is a claim about.
#[must_use]
pub fn steady(samples: &Series, bounds: &Bounds, needed: usize) -> Steady {
    let rounds = samples.len();
    let required = needed.max(2);
    if rounds < required {
        return Steady::TooShort {
            rounds,
            needed: required,
        };
    }
    let halfway = rounds / 2;
    let end = rounds - 1;
    let mut moved = Vec::new();
    for (counter, band) in bounds.iter() {
        let Some(values) = samples.rounds(counter) else {
            continue;
        };
        let (from, to) = (values[halfway], values[end]);
        let difference = (i128::from(to) - i128::from(from)).abs();
        let permitted = i128::from(from) * i128::from(band.drift_permille) / 1000;
        if difference > permitted {
            moved.push((counter.to_owned(), from, to));
        }
    }
    if moved.is_empty() {
        return Steady::Steady;
    }
    Steady::Moved { counters: moved }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six rounds of a store that is working: every counter where the reference solution put it, and
    /// the byte count moving the few bytes a compressor's varints decide.
    fn healthy() -> Series {
        Series::of(&[
            ("open_tasks", &[2, 2, 2, 2, 2, 2]),
            ("open_handles", &[0, 0, 0, 0, 0, 0]),
            ("store_files", &[13, 13, 13, 13, 13, 13]),
            ("store_bytes", &[2739, 2741, 2735, 2739, 2741, 2743]),
            ("records", &[384, 384, 384, 384, 384, 384]),
        ])
    }

    /// A counter below its floor is a run that did too little work to prove anything.
    #[test]
    fn a_counter_below_its_floor_is_a_violation_and_the_run_proved_nothing() {
        let idle = Series::of(&[
            ("open_tasks", &[2, 2, 2, 2]),
            ("open_handles", &[0, 0, 0, 0]),
            ("store_files", &[13, 13, 13, 13]),
            ("store_bytes", &[2740, 2740, 2740, 2740]),
            ("records", &[0, 0, 0, 0]),
        ]);
        assert_eq!(
            check(&idle, &Bounds::of(TABLE)),
            vec![Violation::BelowMin {
                counter: "records".to_owned(),
                round: 0,
                value: 0,
                min: 300,
            }],
            "one entry, and it names `records`: a soak whose store answered nothing has a steady \
             state for the wrong reason, which is what the floor is for"
        );
    }

    /// A counter above its ceiling is a violation, at the round it left its band.
    #[test]
    fn a_counter_above_its_ceiling_is_a_violation_at_the_round_it_left_the_band() {
        let leaking = Series::of(&[
            ("open_tasks", &[2, 2, 2, 2]),
            ("open_handles", &[0, 0, 0, 0]),
            ("store_files", &[13, 13, 20, 21]),
            ("store_bytes", &[2740, 2740, 3000, 3200]),
            ("records", &[384, 384, 384, 384]),
        ]);
        assert_eq!(
            check(&leaking, &Bounds::of(TABLE)),
            vec![Violation::AboveMax {
                counter: "store_files".to_owned(),
                round: 2,
                value: 20,
                max: 14,
            }],
            "one counter, one line, and the line carries the round it left its band — the file \
             count is seven over and the byte count is still inside what a compressor may do"
        );
    }

    /// A metric the harness stopped printing is a gate comparing nothing.
    #[test]
    fn a_counter_the_run_never_printed_is_a_violation() {
        let silent = Series::of(&[
            ("open_tasks", &[2, 2, 2, 2]),
            ("open_handles", &[0, 0, 0, 0]),
            ("store_files", &[13, 13, 13, 13]),
            ("records", &[384, 384, 384, 384]),
        ]);
        assert_eq!(
            check(&silent, &Bounds::of(TABLE)),
            vec![Violation::NeverReported {
                counter: "store_bytes".to_owned(),
            }],
            "the table bounds `store_bytes` and this run does not print it: a gate that skipped a \
             metric it cannot find would stay green while the thing it measured was gone"
        );
    }

    /// A run inside its bands has nothing to report.
    #[test]
    fn a_run_inside_its_bands_has_nothing_to_report() {
        assert_eq!(
            check(&healthy(), &Bounds::of(TABLE)),
            Vec::new(),
            "every counter inside its band and every floor reached"
        );
        let bounds = Bounds::of(TABLE);
        assert_eq!(bounds.len(), 5, "the table is the step's five counters");
        assert!(bounds.band("something_new").is_none());
        assert_eq!(
            healthy().names().count(),
            5,
            "and the transcript reports exactly those, which is what makes the gate a judgement"
        );
    }

    /// A verdict needs enough rounds to be a claim about a run.
    #[test]
    fn a_verdict_needs_enough_rounds_to_say_anything() {
        let bounds = Bounds::of(TABLE);
        let short = Series::of(&[
            ("open_tasks", &[2, 2, 2, 2]),
            ("open_handles", &[0, 0, 0, 0]),
            ("store_files", &[13, 13, 13, 13]),
            ("store_bytes", &[2740, 2740, 2740, 2740]),
            ("records", &[384, 384, 384, 384]),
        ]);
        assert_eq!(
            steady(&short, &bounds, 40),
            Steady::TooShort {
                rounds: 4,
                needed: 40,
            },
            "four rounds of a forty-round run is not a steady state: a train of counters that has \
             not lasted has said nothing about whether it settles"
        );

        let single = Series::of(&[
            ("open_tasks", &[2]),
            ("open_handles", &[0]),
            ("store_files", &[13]),
            ("store_bytes", &[2740]),
            ("records", &[384]),
        ]);
        assert_eq!(
            steady(&single, &bounds, 0),
            Steady::TooShort {
                rounds: 1,
                needed: 2,
            },
            "and a verdict needs two readings even when the caller asks for no rounds at all: one \
             reading is not a comparison"
        );
    }

    /// The steady state is the two marks, and the allowance is a share of the first one.
    #[test]
    fn a_counter_that_moved_between_the_marks_names_itself_with_both_numbers() {
        let bounds = Bounds::of(TABLE);
        assert_eq!(
            steady(&healthy(), &bounds, 4),
            Steady::Steady,
            "2743 bytes against 2739 is four out of 2739, inside the ten permille a compressor's \
             byte count is allowed: {:?}",
            steady(&healthy(), &bounds, 4)
        );

        let leaking = Series::of(&[
            ("open_tasks", &[2, 2, 2, 2, 2, 2]),
            ("open_handles", &[0, 0, 0, 0, 0, 0]),
            ("store_files", &[13, 13, 13, 13, 14, 15]),
            ("store_bytes", &[2739, 2739, 2739, 2739, 2739, 2739]),
            ("records", &[384, 384, 384, 384, 384, 384]),
        ]);
        assert_eq!(
            steady(&leaking, &bounds, 4),
            Steady::Moved {
                counters: vec![("store_files".to_owned(), 13, 15)],
            },
            "a counter that grew between the halfway mark and the end is a leak with a name on it, \
             and both of its numbers are in the report"
        );
    }

    /// The allowance is a share of the halfway value, and zero has no share.
    #[test]
    fn the_allowance_is_a_share_of_the_halfway_value_and_zero_has_no_share() {
        let bounds = Bounds::of(&[("store_bytes", 0, 10_000, 10)]);
        let from_zero = Series::of(&[("store_bytes", &[0, 0, 0, 0, 0, 5])]);
        assert_eq!(
            steady(&from_zero, &bounds, 4),
            Steady::Moved {
                counters: vec![("store_bytes".to_owned(), 0, 5)],
            },
            "ten permille of zero is zero: a counter that read nothing at the halfway mark and \
             something at the end has moved, whatever its allowance"
        );

        let at_the_edge = Series::of(&[("store_bytes", &[0, 0, 0, 1_000, 1_000, 1_010])]);
        assert_eq!(
            steady(&at_the_edge, &bounds, 4),
            Steady::Steady,
            "ten permille of a thousand is ten bytes, and a move of exactly the allowance is inside \
             it rather than over"
        );
        let over = Series::of(&[("store_bytes", &[0, 0, 0, 1_000, 1_000, 1_011])]);
        assert_eq!(
            steady(&over, &bounds, 4),
            Steady::Moved {
                counters: vec![("store_bytes".to_owned(), 1_000, 1_011)],
            },
            "and one byte past it is a move"
        );
    }

    /// The leak this whole step exists to find: a counter that only grows.
    #[test]
    fn a_counter_that_only_grows_is_caught_by_both_questions() {
        let growing = Series::of(&[
            ("open_tasks", &[2, 2, 2, 2]),
            ("open_handles", &[0, 0, 0, 0]),
            ("store_files", &[13, 14, 15, 16]),
            ("store_bytes", &[2739, 2739, 2740, 2740]),
            ("records", &[384, 384, 384, 384]),
        ]);
        let bounds = Bounds::of(TABLE);
        assert_eq!(
            check(&growing, &bounds),
            vec![Violation::AboveMax {
                counter: "store_files".to_owned(),
                round: 2,
                value: 15,
                max: 14,
            }],
            "one file per cycle, and the band is what makes it visible at the round it crossed"
        );
        assert_eq!(
            steady(&growing, &bounds, 4),
            Steady::Moved {
                counters: vec![("store_files".to_owned(), 15, 16)],
            },
            "a leak that would have been a sentence in a review is a counter that moved between \
             the halfway mark and the end"
        );
    }
}
