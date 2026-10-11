//! probe = "ops/capacity-cost@design"
//! title = "A capacity model is arithmetic and a comparison, in one integer lane"
//! hint = "Price the three terms, add them, compare each one with what the store wrote as a drift in per-mille that names the constant and the direction — and keep the tolerance the model states."
//!
//! Step `lakelet/4.20-capacity-model` asks the question the whole track's measurements cannot
//! answer: *what will a million events a day cost*, asked **before** the load arrives, by somebody
//! deciding how many machines to buy. The only honest answer is a model — a handful of numbers
//! somebody measured once, arithmetic over them, and a comparison that fails when one of them
//! stops matching the store. What makes it a design and not a page is the second half:
//!
//! * **the model and the comparison live in one lane.** A model nobody compares with a store goes
//!   on being quoted for a year in a confident voice; here the model's numbers, the store's own
//!   numbers and the distance between them come out of one call, so a constant that stopped
//!   matching turns a case red under the name of the quantity it answers for;
//! * **integers with units in the field names, and no float anywhere.** A cost that depends on how
//!   a machine rounds an `f64` is a cost two machines disagree about — `0.1 + 0.2` is not `0.3` —
//!   so every rate, every total and every drift is an integer, and the division happens once, at
//!   the end;
//! * **the model does not measure.** Its entire input is the two values handed to it: it never
//!   opens the store, reads the clock or asks the environment. A model that measures is a slow
//!   measurement, and it cannot answer the only question a model is for — what a store that does
//!   not exist yet would cost;
//! * **a drift is reported, never thrown away.** One row per quantity, with the constant the
//!   answer came out of, both numbers, the distance in whole per-mille of what the model said, and
//!   which way it is off. "The model is 20% out" without the direction is not something an
//!   operator can act on;
//! * **the tolerance is the model's own, stated once.** A model five percent off is inside the
//!   tolerance it claims for itself and a model twenty percent off is not — and the number is
//!   where the lane holds it down, so widening it is the same mistake as editing a baseline.
//!
//! ## What you write
//!
//! * [`Model::predicts`] — the model's version of what the run wrote: the same three multipliers,
//!   no disk;
//! * [`Model::agrees`] — one [`Drift`] per quantity, in a fixed order, and the verdict against the
//!   tolerance the model states;
//! * `Model::drift` — the one piece of arithmetic both of them are written against: the distance
//!   as a per-mille of the model's own answer, and the direction;
//! * [`Estimate::lines`] — the whole lane as `name=value` lines, integers only, in a fixed order;
//! * [`Estimate::text`] — those lines as one block, which is what a page carries.
//!
//! [`Model`], [`Usage`], [`Predicted`], [`Drift`], [`Direction`], [`Agreement`] and
//! [`Estimate::of`] are given: the shape of the question, the shape of the answer and the
//! vocabulary of a comparison are what a report, a page and a lane are all written against, and a
//! probe that let you invent them would be checking a different design.
//!
//! ## What the tests ask
//!
//! The model is its three terms and the total is exactly their sum, with the sample term linear in
//! the load and the segment term not: four times the samples is four times the samples' bytes, and
//! the model's own numbers are the ones the prose promises. An exact match is a drift of nothing
//! in every row and a direction of `exact`; a store that wrote more than the model said is `under`
//! and one that wrote less is `over`, so both directions are pinned. A model a fifth out is over
//! the bound and is reported with both numbers and the constant it came out of, while the rows
//! that agree are still in the list. The bound itself is inclusive: exactly the tolerance is
//! inside it and one per-mille more is not. A run with no load is a drift of nothing rather than a
//! division, and a `Usage` larger than any plausible store is still arithmetic rather than an
//! error. And the block: every value parses as an integer, two calls give the same bytes, the
//! lines are in the fixed order the documentation states, and the numbers it carries are the ones
//! the prose promises.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store**. The real step builds a store through the real write
//! path (`tests/budget_cost.rs`), asks it two queries, measures the three file sets on the disk
//! and compares the model with the store's own counters; a constant that stopped matching the
//! encoder under it turns the budget lane red. None of that is here: the run's numbers are handed
//! in as a [`Usage`], so what this file pins is the *arithmetic* — the three terms, the total, the
//! per-mille drift, its direction, and the one block both sides are read out of — and not the
//! measurement it was fitted to.
//!
//! Also missing on purpose: no money (the real step prices the month in micro-units of a list
//! price it writes down with a date and a region, and a probe that invented a price would be
//! teaching a number), no retention (the multiplier a person chooses, and the term that makes a
//! capacity question about *time*), no queries (the real model prices one pruned reading as a
//! floor rather than an average), no merge traffic or manifest bytes, and no latency — this model
//! prices bytes and not seconds. The per-series term is the real step's least exact one, because
//! its sidecar is one file per segment whose size does not depend on how many series share it;
//! here it is a plain multiplication of the same shape, and the page's argument about which way
//! that term errs is not re-checked.

/// The model: what one unit of each thing costs, and the tolerance it claims for itself.
///
/// Three measured rates and one decision. The rates came out of a store this project ran, and the
/// step's page says which run; the tolerance is the model's own statement of how far it may be
/// from a measurement and still be called agreed, in whole per-mille, because a model that does
/// not state its own precision cannot be held to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// Bytes one sample costs once it is sealed into a segment.
    pub bytes_per_sample: u64,
    /// Bytes one series costs the sidecar that is written beside a segment.
    pub bytes_per_series: u64,
    /// Bytes one segment costs for its own frame and dictionary, whatever it holds.
    pub bytes_per_segment: u64,
    /// How far the model may be from what a store wrote and still be called agreed.
    pub agreement_permille: u64,
}

impl Model {
    /// The model this project ships.
    ///
    /// The three rates are the ones the step's harness measured — four bytes a sample, eighty-eight
    /// a series, a kilobyte a segment — rounded down to whole bytes, because a store writes whole
    /// bytes. The tolerance is forty per-mille, four percent: wide enough for the compressor's
    /// slack and narrow enough that a constant that stopped matching the store is outside it.
    pub const DEFAULT: Model = Model {
        bytes_per_sample: 4,
        bytes_per_series: 88,
        bytes_per_segment: 1024,
        agreement_permille: 40,
    };

    /// The model's version of what a store wrote, from the counts alone.
    ///
    /// Same question, same multipliers, no disk — the samples at `bytes_per_sample` each, the
    /// sidecar at `bytes_per_series` a series, the segments' own frame and dictionary at
    /// `bytes_per_segment` each, and their sum. This is what [`Model::agrees`] compares with the
    /// store's own numbers, and the reason it takes a count and not a path.
    #[must_use]
    pub fn predicts(&self, usage: &Usage) -> Predicted {
        let _ = usage;
        todo!(
            "ops/capacity-cost@design: the model's version of what the store wrote, from the counts"
        )
    }

    /// Compare the model with what a store wrote.
    ///
    /// One [`Drift`] per quantity, in a fixed order — the three terms, then their sum — and the
    /// verdict against the model's own tolerance. The sum's row names `bytes_per_sample` as the
    /// constant it came out of, because on every workload this model prices the samples are the
    /// term that dominates it; a constant that moved shows up on the term's own row as well, which
    /// is the row a reader diagnoses from.
    #[must_use]
    pub fn agrees(&self, usage: &Usage) -> Agreement {
        let _ = usage;
        todo!(
            "ops/capacity-cost@design: one drift per quantity, and the verdict against the tolerance"
        )
    }

    /// One quantity's distance from the store, in per-mille of what the model said, and the
    /// direction.
    ///
    /// The denominator is the model's own answer, which is what makes "the store wrote five percent
    /// more than the model predicted" the arithmetic it looks like. A model that said nothing at
    /// all is a drift of nothing only when the store wrote nothing either: anything else is a
    /// comparison this model cannot make, and `u64::MAX` per-mille is a figure no tolerance
    /// accepts.
    fn drift(
        &self,
        constant: &'static str,
        quantity: &'static str,
        model: u64,
        measured: u64,
    ) -> Drift {
        let _ = (constant, quantity, model, measured);
        todo!("ops/capacity-cost@design: one quantity's distance in per-mille, and the direction")
    }
}

/// What a store actually used: the counts the model multiplies, and what the store wrote for each
/// of the three terms.
///
/// The counts are the question and the bytes are the store's own answer to it, both read off a run
/// by the caller — the model measures nothing, so this value is the whole of what it is allowed to
/// know. The three byte fields are what a harness reads off the store's files: the samples' share,
/// the sidecar's bytes, and what the segments' own frames and dictionaries cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// Samples the run sealed into segments.
    pub samples: u64,
    /// Distinct series those samples name.
    pub series: u64,
    /// Segments they were sealed into.
    pub segments: u64,
    /// Bytes the samples' own share of the segment files cost, as the store reports it.
    pub sample_bytes: u64,
    /// Bytes the sidecar files cost, as the store reports it.
    pub series_bytes: u64,
    /// Bytes the segments' frames and dictionaries cost, as the store reports it.
    pub segment_bytes: u64,
}

impl Usage {
    /// The run the step's page quotes: a million samples, a thousand series and twenty-four
    /// segments, with the bytes the store that ran it actually wrote — which is not the model's
    /// answer, and is inside the tolerance the shipped model claims.
    pub const DEFAULT: Usage = Usage {
        samples: 1_000_000,
        series: 1_000,
        segments: 24,
        sample_bytes: 4_010_000,
        series_bytes: 90_000,
        segment_bytes: 24_000,
    };

    /// What the store holds on the disk: the three terms together.
    ///
    /// The one number a capacity plan is about, and the same expression [`Predicted`] adds for the
    /// model's side, so the two cannot disagree about what a total is.
    #[must_use]
    pub const fn on_disk_bytes(&self) -> u64 {
        self.sample_bytes
            .saturating_add(self.series_bytes)
            .saturating_add(self.segment_bytes)
    }
}

/// The model's version of a [`Usage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Predicted {
    /// Bytes the model says the samples' share costs.
    pub sample_bytes: u64,
    /// Bytes the model says the sidecar costs.
    pub series_bytes: u64,
    /// Bytes the model says the segments' furniture costs.
    pub segment_bytes: u64,
    /// The three together.
    pub on_disk_bytes: u64,
}

/// Which way a model's answer is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The model and the store said exactly the same number.
    Exact,
    /// The model said more than the store wrote.
    Over,
    /// The model said less than the store wrote.
    Under,
}

impl Direction {
    /// The word a report uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Over => "over",
            Self::Under => "under",
        }
    }
}

/// One quantity the model answered, and how far its answer is from the store's.
///
/// The two names are not the same name: `constant` is what the model's answer came out of — the
/// thing that has to stop matching the store for this row to move — and `quantity` is what was
/// compared. Both numbers travel with the row, because "the model is 20% out" and nothing else is
/// not something an operator can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drift {
    /// The model constant the answer came out of.
    pub constant: &'static str,
    /// The quantity that was compared.
    pub quantity: &'static str,
    /// What the model says.
    pub model: u64,
    /// What the store wrote.
    pub measured: u64,
    /// How far apart the two are, in whole per-mille of what the model said.
    pub permille: u64,
    /// Which way.
    pub direction: Direction,
}

/// The verdict of comparing a model with a run, and the tolerance it was judged at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agreement {
    /// Every quantity compared, in a fixed order: the three terms, then their sum.
    pub drifts: Vec<Drift>,
    /// How far apart the model and the store may be, in per-mille — carried here so that a reader
    /// of the verdict is told the rule it was judged by.
    pub tolerance_permille: u64,
    /// `true` when every drift is inside the tolerance.
    pub within: bool,
}

/// The model and the comparison, as the lines a page quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Estimate {
    /// The model that answered.
    pub model: Model,
    /// The run it answered about.
    pub usage: Usage,
}

impl Estimate {
    /// The answer for one model and one run.
    ///
    /// The whole of this call's input is the two values, and both are integers: there is no clock,
    /// no path and no environment variable anywhere in this file, which is what makes two runs of
    /// it print the same bytes and a page quoting it re-runnable.
    #[must_use]
    pub const fn of(model: &Model, usage: &Usage) -> Self {
        Self {
            model: *model,
            usage: *usage,
        }
    }

    /// The whole lane as `name=value` lines, in this order, integers only:
    ///
    /// the run's counts (`samples`, `series`, `segments`), the model's rates (`bytes_per_sample`,
    /// `bytes_per_series`, `bytes_per_segment`), the model's answer (`sample_bytes`, `series_bytes`,
    /// `segment_bytes`, `on_disk_bytes`), the store's own numbers (`measured_sample_bytes`,
    /// `measured_series_bytes`, `measured_segment_bytes`, `measured_on_disk_bytes`), one
    /// `drift_<quantity>_permille` line per drift in the order [`Model::agrees`] reports them, the
    /// tolerance (`agreement_permille`), and the verdict (`within`, `1` or `0`).
    ///
    /// The order is fixed because the block is compared line for line by a page and by a lane: two
    /// readers of one run have to be able to read down the same columns.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let _ = self;
        todo!("ops/capacity-cost@design: the whole lane as `name=value` lines, integers only")
    }

    /// The same lines as one block, newline-terminated, which is what a page carries.
    #[must_use]
    pub fn text(&self) -> String {
        let _ = self;
        todo!("ops/capacity-cost@design: the lines as one block")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row one quantity's drift is in.
    ///
    /// A quantity the comparison cannot name is a quantity nothing answered for, so this panics
    /// rather than answering `None`: every number the block prints has to be one a drift covers.
    fn drift_of(agreement: &Agreement, quantity: &str) -> Drift {
        agreement
            .drifts
            .iter()
            .copied()
            .find(|drift| drift.quantity == quantity)
            .unwrap_or_else(|| panic!("the comparison reported no drift for `{quantity}`"))
    }

    /// The value one line of an estimate states.
    fn value(lines: &[String], name: &str) -> u64 {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('=')?.parse().ok())
            .unwrap_or_else(|| panic!("no line `{name}=…` among {lines:?}"))
    }

    /// A run of these counts whose bytes are the model's own answer, so that a case about
    /// something else starts from a model that agrees with its store.
    fn agreed(model: &Model, samples: u64, series: u64, segments: u64) -> Usage {
        let counts = Usage {
            samples,
            series,
            segments,
            sample_bytes: 0,
            series_bytes: 0,
            segment_bytes: 0,
        };
        let predicted = model.predicts(&counts);
        Usage {
            sample_bytes: predicted.sample_bytes,
            series_bytes: predicted.series_bytes,
            segment_bytes: predicted.segment_bytes,
            ..counts
        }
    }

    #[test]
    fn the_model_is_its_three_terms_and_their_sum() {
        let model = Model::DEFAULT;
        let usage = Usage::DEFAULT;
        let predicted = model.predicts(&usage);
        assert_eq!(
            predicted.sample_bytes,
            usage.samples.saturating_mul(model.bytes_per_sample),
            "the samples' bytes are the count times the rate, in integers"
        );
        assert_eq!(
            predicted.series_bytes,
            usage.series.saturating_mul(model.bytes_per_series)
        );
        assert_eq!(
            predicted.segment_bytes,
            usage.segments.saturating_mul(model.bytes_per_segment)
        );
        assert_eq!(
            predicted.on_disk_bytes,
            predicted
                .sample_bytes
                .saturating_add(predicted.series_bytes)
                .saturating_add(predicted.segment_bytes),
            "the total is the three terms added, written as one expression over them so that a \
             case can falsify it rather than a page assert it: {predicted:?}"
        );

        // Linear in the load: four times the samples is four times the samples' bytes, exactly,
        // and the terms that do not follow the load do not move at all.
        let four = Usage {
            samples: usage.samples.saturating_mul(4),
            ..usage
        };
        assert_eq!(
            model.predicts(&four).sample_bytes,
            predicted.sample_bytes.saturating_mul(4),
            "twice the events is twice the bytes, with no curve to fit — which is the whole reason \
             a capacity model is worth writing"
        );
        assert_eq!(model.predicts(&four).series_bytes, predicted.series_bytes);
        assert_eq!(
            model.predicts(&four).segment_bytes,
            predicted.segment_bytes,
            "and the segments' furniture does not follow the load: four times the samples sealed \
             into the same segments costs the same"
        );
    }

    #[test]
    fn an_exact_match_is_a_drift_of_nothing_and_each_direction_is_named() {
        let model = Model::DEFAULT;
        let exact = agreed(&model, 1_000, 10, 2);
        let agreement = model.agrees(&exact);
        assert!(
            agreement.within,
            "a store that wrote exactly what the model said is inside any tolerance: {agreement:?}"
        );
        assert_eq!(
            agreement.drifts.len(),
            4,
            "three terms and their sum, and no quantity is left out: {:?}",
            agreement.drifts
        );
        assert!(
            agreement
                .drifts
                .iter()
                .all(|drift| drift.permille == 0 && drift.direction == Direction::Exact),
            "an exact match is a drift of nothing in every row: {:?}",
            agreement.drifts
        );

        // The store wrote four bytes more than the model said: the model is *under*, and both
        // numbers travel with the row.
        let heavier = Usage {
            sample_bytes: exact.sample_bytes.saturating_add(4),
            ..exact
        };
        let under = drift_of(&model.agrees(&heavier), "sample_bytes");
        assert_eq!(
            (under.model, under.measured),
            (exact.sample_bytes, exact.sample_bytes + 4),
            "a drift carries what the model said and what the store wrote"
        );
        assert_eq!((under.permille, under.direction), (1, Direction::Under));
        assert_eq!(under.direction.as_str(), "under");

        // And four bytes fewer is the other answer: a model that over-predicts is not the same
        // news to an operator as one that under-predicts.
        let lighter = Usage {
            sample_bytes: exact.sample_bytes.saturating_sub(4),
            ..exact
        };
        let over = drift_of(&model.agrees(&lighter), "sample_bytes");
        assert_eq!(
            (over.permille, over.direction),
            (1, Direction::Over),
            "the direction is carried apart from the distance, because half an answer is no answer"
        );
    }

    #[test]
    fn a_model_a_fifth_out_is_outside_the_bound_and_is_reported_with_both_numbers() {
        let model = Model::DEFAULT;
        // Twenty-five thousand samples at four bytes is a hundred thousand predicted; the store
        // wrote a hundred and twenty thousand, which is a fifth more: two hundred per-mille.
        let run = Usage {
            samples: 25_000,
            series: 0,
            segments: 0,
            sample_bytes: 120_000,
            series_bytes: 0,
            segment_bytes: 0,
        };
        let agreement = model.agrees(&run);
        let sample = drift_of(&agreement, "sample_bytes");
        assert_eq!(
            (
                sample.constant,
                sample.model,
                sample.measured,
                sample.permille,
                sample.direction
            ),
            ("bytes_per_sample", 100_000, 120_000, 200, Direction::Under),
            "the row names the constant it came out of, and it carries both numbers and the way"
        );
        assert!(
            !agreement.within,
            "two hundred per-mille is outside the forty the model claims for itself: {agreement:?}"
        );
        assert_eq!(
            agreement.tolerance_permille, model.agreement_permille,
            "and the verdict carries the rule it was judged by, so a reader is not left guessing"
        );
        assert_eq!(
            agreement.drifts.len(),
            4,
            "the rows that agree are reported too: a comparison that dropped them could not answer \
             for the model as a whole"
        );
        assert_eq!(
            drift_of(&agreement, "series_bytes").permille,
            0,
            "a term nothing was written for is a drift of nothing, not a missing row"
        );
    }

    #[test]
    fn the_bound_is_inclusive_at_the_tolerance_the_model_states() {
        let model = Model::DEFAULT;
        let run = Usage {
            samples: 25_000,
            series: 0,
            segments: 0,
            sample_bytes: 104_000,
            series_bytes: 0,
            segment_bytes: 0,
        };
        assert_eq!(
            drift_of(&model.agrees(&run), "sample_bytes").permille,
            40,
            "four thousand over a hundred thousand is forty per-mille"
        );
        assert!(
            model.agrees(&run).within,
            "a model exactly at the tolerance it states is inside it: the bound is the number the \
             model claims, not a margin somebody widened to make a run pass"
        );

        let past = Usage {
            sample_bytes: 104_100,
            ..run
        };
        assert_eq!(drift_of(&model.agrees(&past), "sample_bytes").permille, 41);
        assert!(
            !model.agrees(&past).within,
            "and one per-mille past it is out: {model:?}"
        );
        assert_eq!(
            model.agreement_permille, 40,
            "which the shipped model states as four percent — the one number that can hide a \
             drifting model, so it lives in the model where the lane holds it down"
        );
    }

    #[test]
    fn no_load_and_too_much_load_are_both_arithmetic() {
        let model = Model::DEFAULT;
        let nothing = Usage {
            samples: 0,
            series: 0,
            segments: 0,
            sample_bytes: 0,
            series_bytes: 0,
            segment_bytes: 0,
        };
        assert_eq!(
            model.predicts(&nothing).on_disk_bytes,
            0,
            "a run with no load is priced at nothing"
        );
        let agreement = model.agrees(&nothing);
        assert!(
            agreement.within
                && agreement
                    .drifts
                    .iter()
                    .all(|drift| drift.permille == 0 && drift.direction == Direction::Exact),
            "and the model that said nothing is a drift of nothing when the store wrote nothing: \
             that is the division this arithmetic has to answer for: {agreement:?}"
        );

        // A model with no answer at all against a store that wrote something is a comparison it
        // cannot make, and the answer says so instead of accepting it.
        let wrote = Usage {
            sample_bytes: 500,
            ..nothing
        };
        let drift = drift_of(&model.agrees(&wrote), "sample_bytes");
        assert_eq!(drift.direction, Direction::Under);
        assert!(
            !model.agrees(&wrote).within,
            "a model that said nothing is outside any tolerance a store with data can be held to: \
             {drift:?}"
        );

        // A load larger than any store that has ever run: arithmetic, and saturating rather than
        // overflowing — a capacity question is about stores that do not exist yet.
        let huge = Usage {
            samples: u64::MAX,
            series: u64::MAX,
            segments: u64::MAX,
            sample_bytes: u64::MAX,
            series_bytes: u64::MAX,
            segment_bytes: u64::MAX,
        };
        let predicted = model.predicts(&huge);
        assert_eq!(
            predicted.sample_bytes,
            u64::MAX,
            "the rate times a count nobody has run saturates: a big question is a question, not an \
             error"
        );
        assert_eq!(predicted.on_disk_bytes, u64::MAX);
        assert_eq!(
            model.agrees(&huge).drifts.len(),
            4,
            "and every quantity is still compared"
        );
        let lines = Estimate::of(&model, &huge).lines();
        assert!(
            lines.iter().all(|line| line
                .split_once('=')
                .is_some_and(|(_, value)| value.parse::<u64>().is_ok())),
            "and the block is still integers: {lines:?}"
        );
    }

    #[test]
    fn the_one_lane_is_integers_only_in_a_fixed_order_and_the_same_answer_twice() {
        let model = Model::DEFAULT;
        let usage = Usage::DEFAULT;
        let estimate = Estimate::of(&model, &usage);
        let lines = estimate.lines();
        let names: Vec<&str> = lines
            .iter()
            .map(|line| line.split('=').next().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            [
                "samples",
                "series",
                "segments",
                "bytes_per_sample",
                "bytes_per_series",
                "bytes_per_segment",
                "sample_bytes",
                "series_bytes",
                "segment_bytes",
                "on_disk_bytes",
                "measured_sample_bytes",
                "measured_series_bytes",
                "measured_segment_bytes",
                "measured_on_disk_bytes",
                "drift_sample_bytes_permille",
                "drift_series_bytes_permille",
                "drift_segment_bytes_permille",
                "drift_on_disk_bytes_permille",
                "agreement_permille",
                "within",
            ],
            "the block's lines and their order are an interface a page and a lane both read: \
             {lines:?}"
        );
        for line in &lines {
            let (_, said) = line.split_once('=').expect("a line is `name=value`");
            assert!(
                said.parse::<u64>().is_ok(),
                "every value is an integer, and no float is anywhere in the lane: {line}"
            );
        }
        assert_eq!(
            estimate.text(),
            format!("{}\n", lines.join("\n")),
            "the block is the lines and a final newline, which is what a page carries"
        );
        assert_eq!(
            Estimate::of(&model, &usage).text(),
            estimate.text(),
            "the same two values give the same bytes twice: the whole input of this answer is two \
             values of integers, so there is no clock, no path and no environment in it to make \
             two runs disagree"
        );
        assert_eq!(
            value(&lines, "on_disk_bytes"),
            4_112_576,
            "the model's answer for the shipped run — a million samples at four bytes, a thousand \
             series at eighty-eight, twenty-four segments at a kilobyte"
        );
        assert_eq!(
            value(&lines, "measured_on_disk_bytes"),
            usage.on_disk_bytes()
        );
        assert_eq!(value(&lines, "drift_sample_bytes_permille"), 2);
        assert_eq!(value(&lines, "drift_series_bytes_permille"), 22);
        assert_eq!(value(&lines, "drift_segment_bytes_permille"), 23);
        assert_eq!(value(&lines, "drift_on_disk_bytes_permille"), 2);
        assert_eq!(value(&lines, "agreement_permille"), 40);
        assert_eq!(
            value(&lines, "within"),
            1,
            "the shipped model agrees with the run the page quotes, and the page says so by \
             printing this block rather than by asserting it in prose"
        );
    }
}
