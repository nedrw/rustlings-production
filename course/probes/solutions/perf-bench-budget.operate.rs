//! probe = "perf/bench-budget@operate"
//! title = "From a red budget to the phase that moved"
//! hint = "Check each trace's phases against the total it stated, compare only the phases with a baseline count, judge the drift in permille on both sides of zero, and let the report name the biggest mover with both of its numbers."
//!
//! Step `lakelet/4.15-budget-regression` is what to do when a budget goes red. The shallower half of
//! this slot (`perf/bench-budget@apply`, `jobq/3.22`) is how a budget is *written*: counters instead
//! of seconds, a baseline the course owns, a ceiling for a name that ends in `_max`. This file is the
//! other half — the comparison somebody runs — and it adds the one thing a single number cannot say:
//! **which phase of the run moved.**
//!
//! Four decisions, and each one is the difference between a gate that is read and a gate that is
//! ignored:
//!
//! * **the phases are a pipeline, and the profile is where a number is read.** Seven phases in the
//!   order a query walks them — `headers`, `blocks`, `decompress`, `decode`, `filter`, `aggregate`,
//!   `render` — so a metric that moved names the part of the engine that paid, which is the sentence
//!   "this query got slower" can never be;
//! * **the drift is whole permille, and it has a sign on both sides.** No floats anywhere: a ratio
//!   that arrived as `0.9999999` is a comparison between two machines rather than two runs, and a
//!   phase that got much *cheaper* is a phase whose work went somewhere else — an improvement is a
//!   difference too, which is why the bound is on the size of the move and not on its direction;
//! * **a phase the baseline does not budget is a line, not a silence.** A counter that vanishes, one
//!   that never existed and one there is no share to compute are all reported by name, because a
//!   gate that skips what it cannot compare is a gate that stays green while the thing it measured
//!   is gone;
//! * **the whole is checked before the parts are.** Every trace states a total, and the phases have
//!   to add up to it: subtracting the parts of a whole that disagrees with itself is a gate measuring
//!   its own arithmetic, and the refusal names the side and both numbers.
//!
//! ## What you write
//!
//! * [`drift`] — one count against a baseline count, in permille;
//! * [`compare`] — a run against the baseline, phase by phase, or a refusal;
//! * [`report`] — the lines the gate prints: what could not be compared, then the phases that moved.
//!
//! [`Phase`], [`Trace`], [`Drift`], [`Uncompared`], [`Comparison`], [`Side`] and [`Refused`] are
//! given: the phases are the step's own list in the step's own order, and a probe that let you
//! rename them would be checking a different gate.
//!
//! ## What the tests ask
//!
//! The permille arithmetic: a tenth of a baseline is a hundred permille, a move smaller than one
//! permille is no move rather than one rounded up, and a baseline of zero that the run moved has no
//! share to be a drift of. A phase under its bound is not reported; a phase exactly at it is, and a
//! phase that got a hundred permille *cheaper* is reported too. A phase the baseline never budgeted,
//! a phase the baseline budgeted and the run did not report, and a phase whose baseline count is
//! zero are three different report lines and none of them is a silent pass. A trace whose phases do
//! not add up to the total it stated is refused, on either side, and a baseline that names no phase
//! at all is refused rather than green. A clean run says how many phases it compared. And the report
//! names the biggest mover with both numbers, with the phases' own order breaking a tie so that two
//! runs over the same pair of traces print the same lines.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no second build of the project**: the real gate builds the reference
//! solution and the learner's tree, runs `tests/attribution.rs` against each — twenty-four sealed
//! segments and two queries — and compares the thirty-six lines the harness printed with the course's
//! `budget/4.15-budget-regression.toml`. What is re-checked here is the comparison those numbers go
//! through: the identity, the permille arithmetic, the refusals and the line that names the phase.
//!
//! Also missing on purpose: nothing here *runs* a query, so nothing here can catch a trace whose
//! counts were assembled after the fact rather than charged where the work happened — the drill that
//! makes that visible (take the index out of the store and watch `decode` move while `filter` does
//! not) needs the engine and is `lakelet/4.15-budget-regression`'s own page. There is no history
//! either: the real step archives one baseline per step and a gate compares one pair of runs, and a
//! probe with a trend would be measuring a series of baselines rather than a budget. And there is no
//! `_max` here: a ceiling is a decision about how a *single* metric is judged and it belongs to the
//! baseline file the shallower probe reads, not to the comparison between two profiles.

use std::collections::BTreeMap;

/// The seven phases of a run, in the order a query walks them.
///
/// Given, and in this order rather than alphabetically: `decode >= filter >= aggregate >= render` and
/// `headers >= blocks >= decompress` are the identities that make a profile a measurement rather
/// than seven tallies, and the order is the only order in which they can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// The plan's decision about the store: one per segment the plan examined.
    Headers,
    /// One per segment file whose block was read.
    Blocks,
    /// One per block inflated.
    Decompress,
    /// One per record read out of a block.
    Decode,
    /// One per record the predicates kept — the predicate, and not the range.
    Filter,
    /// One per value folded into a window.
    Aggregate,
    /// One per bucket written into an answer.
    Render,
}

impl Phase {
    /// Every phase, in the order a query walks them.
    pub const ALL: [Phase; 7] = [
        Phase::Headers,
        Phase::Blocks,
        Phase::Decompress,
        Phase::Decode,
        Phase::Filter,
        Phase::Aggregate,
        Phase::Render,
    ];

    /// The name a metric line and a JSON key spell this phase with.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Phase::Headers => "headers",
            Phase::Blocks => "blocks",
            Phase::Decompress => "decompress",
            Phase::Decode => "decode",
            Phase::Filter => "filter",
            Phase::Aggregate => "aggregate",
            Phase::Render => "render",
        }
    }

    /// The phase a metric line names, or `None` for a word this run does not have.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Phase::ALL.into_iter().find(|phase| phase.name() == name)
    }
}

/// One run's account of itself: a count per phase, and the total the report printed beside them.
///
/// Given, and both halves are needed. The counts are what the run charged where the work happened;
/// the total is the number the run *stated* — the `merged_total` line of the course's baseline — and
/// a phase the run did not charge is absent from the map rather than a zero nobody wrote down, which
/// is what lets [`compare`] tell "this run decoded nothing" from "this run never reported `decode`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    /// One count per phase the run charged, in a sorted map so a report reads the same way twice.
    counts: BTreeMap<Phase, u64>,
    /// The total the run stated, which its phases have to add up to.
    pub total: u64,
}

impl Trace {
    /// A trace from the phases a run charged and the total it stated, in the order the phases ran.
    #[must_use]
    pub fn of(charges: &[(Phase, u64)], total: u64) -> Self {
        Self {
            counts: charges.iter().copied().collect(),
            total,
        }
    }

    /// Whether this run charged this phase at all. A phase it charged a zero for is reported; one it
    /// never mentioned is not the same thing, and a gate that confused the two would judge an absence
    /// as a pass.
    #[must_use]
    pub fn has(&self, phase: Phase) -> bool {
        self.counts.contains_key(&phase)
    }

    /// What this run counted for one phase, and `0` for a phase it did not report.
    #[must_use]
    pub fn count(&self, phase: Phase) -> u64 {
        self.counts.get(&phase).copied().unwrap_or(0)
    }

    /// The phases added up.
    #[must_use]
    pub fn sum(&self) -> u64 {
        self.counts.values().copied().sum()
    }

    /// Every phase this run charged, in the order a query walks them.
    pub fn iter(&self) -> impl Iterator<Item = (Phase, u64)> {
        Phase::ALL
            .into_iter()
            .filter_map(|phase| self.counts.get(&phase).map(|count| (phase, *count)))
    }
}

/// One phase that moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drift {
    /// Which phase.
    pub phase: Phase,
    /// What the baseline says it cost.
    pub baseline: u64,
    /// What the run in hand counted.
    pub measured: u64,
    /// The move, in whole permille of the baseline: positive when the run did more work.
    pub permille: i64,
}

/// Why a phase was not compared against a bound.
///
/// One class per reason, because a script branches on them and a reader acts on them differently: a
/// phase the run stopped reporting is a defect in the run, and a phase nobody has budgeted yet is a
/// line somebody has to decide about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uncompared {
    /// The run counts a phase no baseline budgets: new work, reported so that it can be budgeted.
    NoBaseline,
    /// The baseline's count is zero and the run's is not, so there is no share to compute a drift
    /// from — a phase that never did any work and now does is a change nobody can express in permille.
    NoBound,
    /// The baseline budgets this phase and the run did not report it: the metric went away, and a
    /// gate comparing nothing is a gate that stays green for ever.
    Vanished,
}

/// What one comparison of two traces found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// Every phase at or over the bound, biggest move first, the phases' own order breaking a tie.
    pub over: Vec<Drift>,
    /// Every phase that could not be compared, in the order a query walks them, and why.
    pub uncompared: Vec<(Phase, Uncompared)>,
    /// How many phases had a baseline count and a measurement to compare.
    pub compared: usize,
    /// The bound the moves were judged against, in permille.
    pub bound_permille: u64,
}

impl Comparison {
    /// The phase a reader looks at first: the biggest move there was.
    #[must_use]
    pub fn top(&self) -> Option<&Drift> {
        self.over.first()
    }

    /// `true` when the gate has nothing to report at all.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.over.is_empty() && self.uncompared.is_empty()
    }
}

/// Which of the two traces a check was about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The trace the course owns.
    Baseline,
    /// The trace the run in hand printed.
    Measured,
}

/// Why a comparison did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The baseline names no phase at all. A gate with nothing to compare is not a passing gate.
    EmptyBaseline,
    /// A trace whose phases do not add up to the total it stated.
    TotalDisagrees {
        /// Which of the two traces it was.
        side: Side,
        /// The total it stated.
        total: u64,
        /// What its phases actually add up to.
        sum: u64,
    },
}

/// One count's move from a baseline count, in whole permille.
///
/// Integer arithmetic, and no floats anywhere in this file: a drift that arrived as `0.9999999`
/// eventually reads as a different number in a different process, and a gate that compares two
/// machines rather than two runs is the thing this whole lane exists to avoid. `None` is the honest
/// answer for a baseline of zero that the run moved — there is no share of nothing — while a zero
/// that is still zero has not moved and is `Some(0)`.
#[must_use]
pub fn drift(measured: u64, baseline: u64) -> Option<i64> {
    if baseline == 0 {
        return (measured == 0).then_some(0);
    }
    let moved = (i128::from(measured) - i128::from(baseline)) * 1000 / i128::from(baseline);
    Some(moved.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
}

/// Compare a run against the baseline it is judged by.
///
/// Every phase on either side is accounted for: the ones with a count on both sides are compared and
/// each is over its bound or inside it, and the ones a side does not report are named with the reason.
///
/// # Errors
///
/// [`Refused::EmptyBaseline`] when the baseline names no phase, and [`Refused::TotalDisagrees`] when
/// either trace's phases do not add up to the total it stated: the identity is checked rather than
/// assumed, because the comparison subtracts two profiles and a subtraction of parts that do not
/// make their whole is arithmetic about the gate rather than evidence about the run.
pub fn compare(
    baseline: &Trace,
    measured: &Trace,
    bound_permille: u64,
) -> Result<Comparison, Refused> {
    if baseline.iter().next().is_none() {
        return Err(Refused::EmptyBaseline);
    }
    for (side, trace) in [(Side::Baseline, baseline), (Side::Measured, measured)] {
        if trace.sum() != trace.total {
            return Err(Refused::TotalDisagrees {
                side,
                total: trace.total,
                sum: trace.sum(),
            });
        }
    }

    let mut over: Vec<Drift> = Vec::new();
    let mut uncompared: Vec<(Phase, Uncompared)> = Vec::new();
    let mut compared = 0;
    for phase in Phase::ALL {
        match (baseline.has(phase), measured.has(phase)) {
            (false, false) => {}
            (false, true) => uncompared.push((phase, Uncompared::NoBaseline)),
            (true, false) => uncompared.push((phase, Uncompared::Vanished)),
            (true, true) => match drift(measured.count(phase), baseline.count(phase)) {
                None => uncompared.push((phase, Uncompared::NoBound)),
                Some(permille) => {
                    compared += 1;
                    if permille.unsigned_abs() >= bound_permille {
                        over.push(Drift {
                            phase,
                            baseline: baseline.count(phase),
                            measured: measured.count(phase),
                            permille,
                        });
                    }
                }
            },
        }
    }
    over.sort_by_key(|moved| {
        (
            std::cmp::Reverse(moved.permille.unsigned_abs()),
            moved.phase,
        )
    });
    Ok(Comparison {
        over,
        uncompared,
        compared,
        bound_permille,
    })
}

/// The lines the gate prints.
///
/// What could not be compared first — each of those is a decision somebody has to make — and then
/// every phase that moved, biggest move first, so the first of those lines names the worst mover with
/// both of its numbers. A run with nothing to say still says what it compared, because a gate that
/// printed nothing would be indistinguishable from a gate that did not run.
#[must_use]
pub fn report(comparison: &Comparison) -> Vec<String> {
    let mut lines = Vec::new();
    for (phase, why) in &comparison.uncompared {
        let name = phase.name();
        lines.push(match why {
            Uncompared::NoBaseline => {
                format!("budget: {name} has no baseline count (new work, not compared)")
            }
            Uncompared::NoBound => format!(
                "budget: {name} has a baseline count of 0 (no share to compute a move from)"
            ),
            Uncompared::Vanished => {
                format!("budget: {name} was budgeted and the run did not report it")
            }
        });
    }
    for moved in &comparison.over {
        lines.push(format!(
            "budget: {} moved {} -> {} ({:+} permille, bound {})",
            moved.phase.name(),
            moved.baseline,
            moved.measured,
            moved.permille,
            comparison.bound_permille
        ));
    }
    if lines.is_empty() {
        lines.push(format!(
            "budget: {} phase(s) inside {} permille",
            comparison.compared, comparison.bound_permille
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selective profile of `budget/4.15-budget-regression.toml`: one segment opened of the
    /// twenty-four the plan examined, sixty records decoded, thirty kept by the predicate and thirty
    /// folded into one bucket. Its phases add up to the total it states.
    fn selective() -> Trace {
        Trace::of(
            &[
                (Phase::Headers, 24),
                (Phase::Blocks, 1),
                (Phase::Decompress, 1),
                (Phase::Decode, 60),
                (Phase::Filter, 30),
                (Phase::Aggregate, 30),
                (Phase::Render, 1),
            ],
            147,
        )
    }

    /// The same run with `decode` moved to `count`, and its stated total moved with it: the shape a
    /// gate is handed when one phase of one run changed.
    fn decoding(count: u64) -> Trace {
        Trace::of(
            &[
                (Phase::Headers, 24),
                (Phase::Blocks, 1),
                (Phase::Decompress, 1),
                (Phase::Decode, count),
                (Phase::Filter, 30),
                (Phase::Aggregate, 30),
                (Phase::Render, 1),
            ],
            87 + count,
        )
    }

    /// The arithmetic: whole permille of the baseline, signed, and honest about zero.
    #[test]
    fn a_move_is_whole_permille_of_the_baseline_and_it_carries_its_sign() {
        assert_eq!(
            drift(1120, 1000),
            Some(120),
            "a tenth more than the baseline is a hundred and twenty permille"
        );
        assert_eq!(
            drift(1000, 1000),
            Some(0),
            "a count that did not move has not moved"
        );
        assert_eq!(
            drift(999, 1000),
            Some(-1),
            "the sign is the direction: one thousandth of the baseline less is minus one permille"
        );
        assert_eq!(
            drift(890, 1000),
            Some(-110),
            "a run that got cheaper has moved too, which is why the bound is on the size of a move"
        );
        assert_eq!(
            drift(1000, 1001),
            Some(0),
            "a move smaller than one permille is no move at all: whole permille truncates towards \
             zero rather than rounding a counter up"
        );
        assert_eq!(
            drift(0, 0),
            Some(0),
            "a phase that did no work and still does none has not moved"
        );
        assert_eq!(
            drift(5, 0),
            None,
            "and a phase that did no work and now does some has no share to be a move of: the gate \
             reports it rather than dividing by nothing"
        );
    }

    /// The bound is a size rather than a direction, and a move exactly at it counts.
    #[test]
    fn a_phase_under_its_bound_is_not_reported_and_one_at_it_is() {
        for (count, reported, permille) in [
            (66_u64, true, 100_i64),
            (65, false, 83),
            (54, true, -100),
            (55, false, -83),
        ] {
            let run = compare(&selective(), &decoding(count), 100).expect("both traces add up");
            assert_eq!(
                run.over.len(),
                if reported { 1 } else { 0 },
                "decode 60 -> {count} against a bound of 100 permille: {:?}",
                report(&run)
            );
            if reported {
                let moved = run
                    .top()
                    .expect("a run that reported a move has a worst one");
                assert_eq!(moved.phase, Phase::Decode);
                assert_eq!(
                    (moved.baseline, moved.measured, moved.permille),
                    (60, count, permille),
                    "the line carries both numbers and the size of the move, not just the name"
                );
            } else {
                assert!(
                    run.clean(),
                    "a phase inside its bound leaves the gate with nothing to say: {:?}",
                    report(&run)
                );
            }
            assert_eq!(
                run.compared, 7,
                "every phase of the profile has a count on both sides and is compared"
            );
        }
    }

    /// The report names the worst mover with both of its numbers.
    #[test]
    fn the_report_names_the_worst_mover_with_both_numbers() {
        let baseline = Trace::of(&[(Phase::Decode, 1000), (Phase::Filter, 400)], 1400);
        let measured = Trace::of(&[(Phase::Decode, 1120), (Phase::Filter, 200)], 1320);
        let run = compare(&baseline, &measured, 100).expect("both traces add up");
        assert_eq!(
            run.top().map(|moved| moved.phase),
            Some(Phase::Filter),
            "the phase a reader looks at first is the biggest move, whichever direction it went"
        );
        assert_eq!(
            report(&run),
            vec![
                "budget: filter moved 400 -> 200 (-500 permille, bound 100)".to_owned(),
                "budget: decode moved 1000 -> 1120 (+120 permille, bound 100)".to_owned(),
            ],
            "one line per phase that moved, biggest first, with what the baseline says and what the \
             run counted in it"
        );
    }

    /// A run inside its bounds still says what it compared.
    #[test]
    fn a_run_inside_its_bounds_says_what_it_compared() {
        let run = compare(&selective(), &selective(), 100).expect("both traces add up");
        assert!(run.clean(), "one profile twice is no move at all: {run:?}");
        assert_eq!(run.compared, 7);
        assert_eq!(
            report(&run),
            vec!["budget: 7 phase(s) inside 100 permille".to_owned()],
            "a gate that printed nothing would be indistinguishable from a gate that did not run"
        );
    }

    /// A phase either side did not report is a line, and never a silence.
    #[test]
    fn a_phase_either_side_did_not_report_is_a_line_and_not_a_silent_pass() {
        // The same store and the same run, before that run wrote its first bucket.
        let without_render = Trace::of(
            &[
                (Phase::Headers, 24),
                (Phase::Blocks, 1),
                (Phase::Decompress, 1),
                (Phase::Decode, 60),
                (Phase::Filter, 30),
                (Phase::Aggregate, 30),
            ],
            146,
        );

        let new_work = compare(&without_render, &selective(), 100).expect("both traces add up");
        assert_eq!(
            new_work.uncompared,
            vec![(Phase::Render, Uncompared::NoBaseline)],
            "the run charged a phase no baseline budgets: a line for somebody to decide about"
        );
        assert_eq!(
            new_work.compared, 6,
            "every phase that has a baseline count is still compared"
        );
        assert!(
            !new_work.clean(),
            "a phase that could not be compared is not a pass: {:?}",
            report(&new_work)
        );
        assert_eq!(
            report(&new_work),
            vec!["budget: render has no baseline count (new work, not compared)".to_owned()],
            "and the line names it, rather than the report being one line shorter"
        );

        let vanished = compare(&selective(), &without_render, 100).expect("both traces add up");
        assert_eq!(
            vanished.uncompared,
            vec![(Phase::Render, Uncompared::Vanished)],
            "the baseline budgets a phase this run stopped reporting, which is the state a gate \
             stays green in for ever"
        );
        assert_eq!(
            report(&vanished),
            vec!["budget: render was budgeted and the run did not report it".to_owned()]
        );
    }

    /// A phase that did no work and now does has no share to be a move of.
    #[test]
    fn a_baseline_of_zero_that_the_run_moved_is_reported_and_not_computed() {
        let idle = Trace::of(&[(Phase::Decode, 60), (Phase::Render, 0)], 60);
        let still_idle = compare(&idle, &idle, 100).expect("both traces add up");
        assert!(
            still_idle.clean(),
            "a phase that did no work and still does none has not moved, and it is a row rather \
             than a missing line: {:?}",
            report(&still_idle)
        );
        assert_eq!(
            still_idle.compared, 2,
            "a zero in the baseline is a count like any other, and it is compared"
        );

        let began = Trace::of(&[(Phase::Decode, 60), (Phase::Render, 1)], 61);
        let moved = compare(&idle, &began, 100).expect("both traces add up");
        assert_eq!(
            moved.uncompared,
            vec![(Phase::Render, Uncompared::NoBound)],
            "`render` wrote no bucket in the baseline and one in this run: permille of nothing is \
             not a move, and it is not a pass either"
        );
        assert_eq!(
            report(&moved),
            vec![
                "budget: render has a baseline count of 0 (no share to compute a move from)"
                    .to_owned()
            ]
        );
    }

    /// The whole is checked before the parts are, on both sides.
    #[test]
    fn a_trace_whose_phases_do_not_add_up_to_its_total_is_refused() {
        let broken = Trace::of(&[(Phase::Decode, 60)], 100);
        assert_eq!(
            compare(&broken, &selective(), 100),
            Err(Refused::TotalDisagrees {
                side: Side::Baseline,
                total: 100,
                sum: 60,
            }),
            "the baseline's own arithmetic is checked before a single phase is compared"
        );
        assert_eq!(
            compare(&selective(), &broken, 100),
            Err(Refused::TotalDisagrees {
                side: Side::Measured,
                total: 100,
                sum: 60,
            }),
            "and the run's, for the same reason: subtracting the parts of a whole that disagrees \
             with itself is a gate measuring its own arithmetic"
        );
    }

    /// A baseline that names no phase is not a passing gate.
    #[test]
    fn a_baseline_that_names_no_phase_is_refused_rather_than_green() {
        let empty = Trace::of(&[], 0);
        assert_eq!(
            compare(&empty, &selective(), 100),
            Err(Refused::EmptyBaseline),
            "a gate with nothing to compare is not a gate that found nothing"
        );
        assert_eq!(
            compare(&empty, &empty, 100),
            Err(Refused::EmptyBaseline),
            "and two empty traces are two silences rather than two agreeing profiles"
        );
    }

    /// A tie is broken by the phases' own order, so two runs over one pair of traces print the same
    /// lines.
    #[test]
    fn two_phases_that_moved_the_same_amount_come_out_in_the_order_a_query_walks_them() {
        let baseline = Trace::of(&[(Phase::Decode, 1000), (Phase::Filter, 1000)], 2000);
        let measured = Trace::of(&[(Phase::Decode, 1200), (Phase::Filter, 1200)], 2400);
        let run = compare(&baseline, &measured, 100).expect("both traces add up");
        assert_eq!(
            run.over
                .iter()
                .map(|moved| moved.phase)
                .collect::<Vec<Phase>>(),
            vec![Phase::Decode, Phase::Filter],
            "two phases at two hundred permille each: the order of the list never depends on which \
             half of a map was read first"
        );
        assert_eq!(run.top().map(|moved| moved.phase), Some(Phase::Decode));
        assert_eq!(
            report(&run).len(),
            2,
            "and both movers are lines: the report is not a single sentence about a nameless change"
        );
    }
}
