//! probe = "perf/bench-budget@apply"
//! title = "A budget a slow machine cannot fail"
//! hint = "Count what the code does instead of how long it took, compare each counter against the baseline the course shipped, and add a ratio guard — a small input hides the O(n^2) rewrite in absolute numbers."
//!
//! Step `jobq/3.22-bench-budget` is where this course decides what a performance gate may look at.
//! The answer is **counters, never seconds**: a wall-clock budget on a shared build machine is a
//! coin flip, and a gate that fails for reasons unrelated to the change is a gate everybody learns to
//! ignore. So the harness asks the code to count what it did — how many statements a claim took, how
//! many cells a scan examined — and the gate compares those numbers against a baseline the *course*
//! owns (handoff-m2 §1.3).
//!
//! Three decisions, and each one is the difference between a budget and a decoration:
//!
//! * **the baseline is content, not a machine.** It was measured once, from the reference solution,
//!   on the course's toolchain, and it is read from `course/tracks/jobq/budget/<step>.toml`. A
//!   number in the repository can be reviewed, argued with and changed on purpose;
//! * **a counter the harness stopped reporting is a failure.** Not a missing line in a report — a
//!   failure: a budget whose metric has vanished is comparing nothing, and that is exactly the state
//!   in which a gate stays green forever;
//! * **one metric is a ceiling, not a target.** Names ending in `_max` are one-sided (`scale_ratio_max`
//!   in the step's baseline), because some numbers are upper bounds somebody argued for rather than
//!   measurements: being *better* than a bound is the point of a bound.
//!
//! ## What you write
//!
//! * [`measure`] — run a workload and collect the counters it reported;
//! * [`compare`] — every metric in the baseline, against what the workload counted;
//! * [`guard`] — the same workload at `n` and at `2n`, and what the ratio between them says about
//!   the shape of the code.
//!
//! [`Counters`], [`Workload`], [`Budget`], [`Measure`], [`Report`], [`Verdict`] and [`Growth`] are
//! given: the baseline format and the harness protocol are the step's, and a probe that let you
//! rename them would be checking a different gate.
//!
//! ## What the tests ask
//!
//! A measurement is the workload's own counters, and two runs of one workload agree exactly — which
//! is the property that makes the gate machine-independent, and the one a stopwatch cannot have. A
//! budget that matches passes; the tolerance is a share of the baseline on **both** sides, so a
//! suspicious improvement is a difference too; a `_max` metric is a ceiling that a much smaller
//! number satisfies and a larger one does not; a counter the harness stopped reporting fails; a
//! counter the baseline does not know about is reported and does not fail; an empty baseline is
//! *blocked* rather than green. And the guard: a linear workload sits inside a ratio of 2.2 at twice
//! the input, a quadratic one is about 4, and the guard sees it at an input size where the absolute
//! numbers still look small.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no `cargo bench`, no timing and no course baseline file**: the
//! budgets below are the tests' own, and the real gate reads
//! `course/tracks/jobq/budget/<step-id>.toml` and parses `jobq-budget k=v` lines from the step's
//! harness. What is re-checked is the rule those numbers go through — the tolerance, the ceiling, the
//! vanished counter, the empty baseline — which is the part that decides whether the gate means
//! anything.
//!
//! Also missing on purpose: nothing here measures *time* at all, and that is the point rather than an
//! omission; there is no per-workload warm-up, no statistical test and no regression *history* (the
//! real step archives one baseline per step; a probe has one comparison), and no allocation counting
//! — a counter only exists if the code under test is willing to report it, and inventing one from
//! outside would be measuring the harness.

use std::collections::BTreeMap;

/// What a workload counted about itself.
///
/// Named `u64` counters in a sorted map, so a report reads the same way twice. Floats are
/// deliberately absent: a count that arrives as `12.9999` is a count that only exists because
/// somebody let a stopwatch into a budget.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    values: BTreeMap<String, u64>,
}

impl Counters {
    /// Nothing counted yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Count one more thing, or set a total the workload computed.
    pub fn record(&mut self, name: &str, value: u64) {
        self.values.insert(name.to_owned(), value);
    }

    /// What one counter says, if the workload reported it at all.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<u64> {
        self.values.get(name).copied()
    }

    /// A set of counters from pairs, for a baseline written down by hand and for the tests that
    /// want exact numbers rather than a workload.
    #[must_use]
    pub fn from_pairs(pairs: &[(&str, u64)]) -> Self {
        let mut counters = Self::new();
        for (name, value) in pairs {
            counters.record(name, *value);
        }
        counters
    }

    /// Every counter, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, u64)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
    }
}

/// A workload that counts what it did.
///
/// The harness cannot count anything itself — "how many statements did that claim take" is only
/// knowable inside the code that made them — so the subject reports its own work and the budget
/// compares what it said. That is also what makes the numbers deterministic: a counter cannot be
/// perturbed by a noisy neighbour on the build machine.
pub trait Workload {
    /// Do the work for `n` items and record what it cost.
    fn run(&mut self, n: usize, counters: &mut Counters);
}

/// What a step promised about its own numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budget {
    /// The counters the reference solution reported, measured once on the course's toolchain.
    pub baseline: Counters,
    /// How far a metric may drift from the baseline before it is a difference, as a percentage.
    pub tolerance_percent: u32,
}

/// What happened to one metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Measure {
    /// Inside the budget.
    Within {
        /// What the workload counted.
        value: u64,
        /// What the baseline says.
        baseline: u64,
    },
    /// Outside it.
    Over {
        /// What the workload counted.
        value: u64,
        /// What the baseline says.
        baseline: u64,
    },
    /// The baseline names a metric the workload did not report. The harness lost a counter, and the
    /// budget is comparing nothing until it comes back.
    Missing {
        /// What the baseline says it should be.
        baseline: u64,
    },
    /// A counter the baseline does not know about. New instrumentation, reported so that somebody
    /// can decide to budget it.
    Unbudgeted {
        /// What the workload counted.
        value: u64,
    },
}

/// What the whole budget said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every metric is within the budget.
    Pass,
    /// At least one is not, or one has gone missing.
    Fail,
    /// There is nothing to compare: an empty baseline is not a passing budget.
    Blocked,
}

/// The report: one entry per metric, in name order, and the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// What happened to each metric.
    pub measures: Vec<(String, Measure)>,
    /// What it adds up to.
    pub verdict: Verdict,
}

/// What the ratio guard saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Growth {
    /// Which counter was guarded.
    pub metric: String,
    /// `value(2n) / value(n)`.
    pub ratio: f64,
    /// The upper bound the step argued for.
    pub max_ratio: f64,
    /// Whether the ratio is inside it.
    pub verdict: Verdict,
}

/// Run `workload` for `n` items and answer with the counters it reported.
///
/// A fresh counter map per run: a measurement that inherits the previous one's totals is a number
/// that grows every time somebody re-runs the test.
#[must_use]
pub fn measure<W: Workload>(workload: &mut W, n: usize) -> Counters {
    let mut counters = Counters::new();
    workload.run(n, &mut counters);
    counters
}

/// Compare a measurement against the budget.
///
/// Every metric the baseline names is checked. A name ending in `_max` is a **ceiling**: the value
/// only has to be at or below it, because some of these numbers are bounds somebody argued for
/// (`scale_ratio_max`) rather than measurements. Everything else is two-sided: a sudden *drop* is a
/// difference worth looking at too, since a workload that got much cheaper usually means the
/// workload changed.
#[must_use]
pub fn compare(measured: &Counters, budget: &Budget) -> Report {
    let mut measures = Vec::new();
    let mut verdict = Verdict::Pass;
    if budget.baseline.iter().next().is_none() {
        return Report {
            measures,
            verdict: Verdict::Blocked,
        };
    }
    for (name, baseline) in budget.baseline.iter() {
        let measure = match measured.get(name) {
            None => {
                // Not a missing line in a report: a metric the harness stopped reporting is a
                // budget that is silently comparing nothing.
                verdict = Verdict::Fail;
                Measure::Missing { baseline }
            }
            Some(value) if name.ends_with("_max") => {
                if value <= baseline {
                    Measure::Within { value, baseline }
                } else {
                    verdict = Verdict::Fail;
                    Measure::Over { value, baseline }
                }
            }
            Some(value) => {
                let drift = value.abs_diff(baseline) * 100;
                if drift <= baseline * u64::from(budget.tolerance_percent) {
                    Measure::Within { value, baseline }
                } else {
                    verdict = Verdict::Fail;
                    Measure::Over { value, baseline }
                }
            }
        };
        measures.push((name.to_owned(), measure));
    }
    for (name, value) in measured.iter() {
        if budget.baseline.get(name).is_none() {
            measures.push((name.to_owned(), Measure::Unbudgeted { value }));
        }
    }
    measures.sort_by(|left, right| left.0.cmp(&right.0));
    Report { measures, verdict }
}

/// The same workload at `n` and at `2n`, and what the ratio says about its shape.
///
/// This is the guard a small input needs: at `n = 10`, a quadratic claim scan costs 100 cells and
/// looks like nothing, and at `2n` it costs 400 — a number that is still small and *four times* the
/// one before it. Absolute counters cannot see that, and a ratio cannot miss it. Missing metrics and
/// a zero at `n` are [`Verdict::Blocked`]: a ratio computed from nothing is not evidence.
#[must_use]
pub fn guard(at_n: &Counters, at_two_n: &Counters, metric: &str, max_ratio: f64) -> Growth {
    let single = at_n.get(metric);
    let double = at_two_n.get(metric);
    let (ratio, verdict) = match (single, double) {
        (Some(single), Some(double)) if single > 0 => {
            let ratio = double as f64 / single as f64;
            let verdict = if ratio <= max_ratio {
                Verdict::Pass
            } else {
                Verdict::Fail
            };
            (ratio, verdict)
        }
        _ => (0.0, Verdict::Blocked),
    };
    Growth {
        metric: metric.to_owned(),
        ratio,
        max_ratio,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workload that reports one statement per item: what a linear claim should cost.
    #[derive(Default)]
    struct Linear;

    impl Workload for Linear {
        fn run(&mut self, n: usize, counters: &mut Counters) {
            let mut statements = 0;
            for _ in 0..n {
                statements += 1;
            }
            counters.record("claim_statements", statements);
        }
    }

    /// The rewrite nobody means to write: one pass over the queue for every item claimed.
    #[derive(Default)]
    struct Quadratic;

    impl Workload for Quadratic {
        fn run(&mut self, n: usize, counters: &mut Counters) {
            let mut cells = 0_u64;
            for _ in 0..n {
                for _ in 0..n {
                    cells += 1;
                }
            }
            counters.record("claim_statements", n as u64);
            counters.record("scan_cells", cells);
        }
    }

    /// A workload that reports a counter the baseline has never heard of.
    #[derive(Default)]
    struct Instrumented;

    impl Workload for Instrumented {
        fn run(&mut self, n: usize, counters: &mut Counters) {
            counters.record("claim_statements", n as u64);
            counters.record("lock_waits", 3);
        }
    }

    fn budget(metric: &str, baseline: u64, tolerance_percent: u32) -> Budget {
        let mut counters = Counters::new();
        counters.record(metric, baseline);
        Budget {
            baseline: counters,
            tolerance_percent,
        }
    }

    #[test]
    fn a_measurement_is_the_workloads_own_counters_and_two_runs_agree() {
        let first = measure(&mut Linear, 100);
        assert_eq!(
            first.get("claim_statements"),
            Some(100),
            "the harness reads what the code counted: {first:?}"
        );
        assert_eq!(
            first.iter().count(),
            1,
            "and nothing else: a counter that appears from nowhere is a counter nobody can budget"
        );

        let again = measure(&mut Linear, 100);
        assert_eq!(
            first, again,
            "a slow machine cannot change a count, which is the whole reason this gate is not a \
             stopwatch"
        );
        assert_ne!(measure(&mut Linear, 250).get("claim_statements"), Some(100));
    }

    #[test]
    fn a_budget_that_matches_passes() {
        let measured = measure(&mut Linear, 100);
        let report = compare(&measured, &budget("claim_statements", 100, 10));
        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(
            report.measures,
            vec![(
                "claim_statements".to_owned(),
                Measure::Within {
                    value: 100,
                    baseline: 100
                }
            )],
            "one baseline metric, one line in the report"
        );
    }

    #[test]
    fn the_tolerance_is_a_share_of_the_baseline_on_both_sides() {
        let measured = |n: usize| measure(&mut Sized(n), 1);
        assert_eq!(measured(1).get("claim_statements"), Some(1));

        // A tenth of a hundred is ten: 110 and 90 are inside, 111 and 89 are not.
        for (value, verdict) in [
            (100_u64, Verdict::Pass),
            (110, Verdict::Pass),
            (90, Verdict::Pass),
            (111, Verdict::Fail),
            (89, Verdict::Fail),
        ] {
            let measured = Counters::from_pairs(&[("claim_statements", value)]);
            assert_eq!(
                compare(&measured, &budget("claim_statements", 100, 10)).verdict,
                verdict,
                "a value of {value} against a baseline of 100 at ten percent"
            );
        }

        // An improvement is a difference too: a claim that suddenly costs a third of what it did is
        // a claim whose workload changed, and somebody has to say so out loud.
        let faster = Counters::from_pairs(&[("claim_statements", 30)]);
        assert_eq!(
            compare(&faster, &budget("claim_statements", 100, 10)).verdict,
            Verdict::Fail,
            "the tolerance is two-sided on purpose"
        );
    }

    /// A workload whose one counter is the number it was asked for: enough to place a value in a
    /// baseline without another workload's help.
    struct Sized(usize);

    impl Workload for Sized {
        fn run(&mut self, _n: usize, counters: &mut Counters) {
            counters.record("claim_statements", self.0 as u64);
        }
    }

    #[test]
    fn a_metric_ending_in_max_is_a_ceiling_not_a_target() {
        let measured = Counters::from_pairs(&[("scan_cells_max", 100)]);
        assert_eq!(
            compare(&measured, &budget("scan_cells_max", 340, 10)).verdict,
            Verdict::Pass,
            "being well under a bound is what a bound is for"
        );
        let measured = Counters::from_pairs(&[("scan_cells_max", 341)]);
        assert_eq!(
            compare(&measured, &budget("scan_cells_max", 340, 10)).verdict,
            Verdict::Fail,
            "and one over it is a failure, however small the tolerance is"
        );
        let measured = Counters::from_pairs(&[("scan_cells_max", 374)]);
        assert_eq!(
            compare(&measured, &budget("scan_cells_max", 340, 10)).verdict,
            Verdict::Fail,
            "ten percent of 340 is 34, and 374 is inside that: a ceiling has no tolerance on the \
             way up, because the bound is the bound"
        );
    }

    #[test]
    fn a_counter_the_harness_stopped_reporting_fails_the_budget() {
        let mut baseline = Counters::new();
        baseline.record("claim_statements", 100);
        baseline.record("scan_cells", 340);
        let budget = Budget {
            baseline,
            tolerance_percent: 10,
        };

        let measured = measure(&mut Linear, 100);
        let report = compare(&measured, &budget);
        assert_eq!(
            report.verdict,
            Verdict::Fail,
            "`scan_cells` vanished from the harness, so the budget is comparing nothing: {report:?}"
        );
        assert_eq!(
            report.measures,
            vec![
                (
                    "claim_statements".to_owned(),
                    Measure::Within {
                        value: 100,
                        baseline: 100
                    }
                ),
                ("scan_cells".to_owned(), Measure::Missing { baseline: 340 }),
            ],
            "and the missing one is named rather than inferred from a shorter report"
        );
    }

    #[test]
    fn a_counter_the_baseline_does_not_know_is_reported_and_does_not_fail() {
        let measured = measure(&mut Instrumented, 100);
        let report = compare(&measured, &budget("claim_statements", 100, 10));
        assert_eq!(
            report.verdict,
            Verdict::Pass,
            "new instrumentation is new information, not a regression: {report:?}"
        );
        assert!(
            report
                .measures
                .contains(&("lock_waits".to_owned(), Measure::Unbudgeted { value: 3 })),
            "and it is reported, so that somebody can decide to budget it: {report:?}"
        );
    }

    #[test]
    fn an_empty_baseline_is_blocked_rather_than_green() {
        let measured = measure(&mut Linear, 100);
        let report = compare(
            &measured,
            &Budget {
                baseline: Counters::new(),
                tolerance_percent: 10,
            },
        );
        assert_eq!(
            report.verdict,
            Verdict::Blocked,
            "no metrics to compare is not the same as every metric being fine"
        );
        assert!(report.measures.is_empty());
    }

    #[test]
    fn the_ratio_guard_catches_the_rewrite_a_small_input_hides() {
        let linear = (measure(&mut Linear, 10), measure(&mut Linear, 20));
        let growth = guard(&linear.0, &linear.1, "claim_statements", 2.2);
        assert!(
            (growth.ratio - 2.0).abs() < 0.001,
            "twice the input, twice the work: {growth:?}"
        );
        assert_eq!(growth.verdict, Verdict::Pass);

        let quadratic = (measure(&mut Quadratic, 10), measure(&mut Quadratic, 20));
        assert_eq!(
            quadratic.0.get("scan_cells"),
            Some(100),
            "at ten items the quadratic scan costs a hundred cells, which looks like nothing"
        );
        let growth = guard(&quadratic.0, &quadratic.1, "scan_cells", 2.2);
        assert!(
            (growth.ratio - 4.0).abs() < 0.001,
            "and at double the input it costs four times as much: {growth:?}"
        );
        assert_eq!(
            growth.verdict,
            Verdict::Fail,
            "absolute counters at this size cannot see it; a ratio cannot miss it"
        );
        assert_eq!(growth.metric, "scan_cells");
        assert!((growth.max_ratio - 2.2).abs() < 0.001);

        // A counter that does not move is a ratio of one, and a metric the run did not report is
        // blocked rather than a ratio of zero.
        let flat = Counters::from_pairs(&[("lock_waits", 3)]);
        assert_eq!(
            guard(&flat, &flat, "lock_waits", 2.2).verdict,
            Verdict::Pass
        );
        assert_eq!(
            guard(&flat, &flat, "scan_cells", 2.2).verdict,
            Verdict::Blocked
        );
        let zero = Counters::from_pairs(&[("scan_cells", 0)]);
        assert_eq!(
            guard(&zero, &flat, "scan_cells", 2.2).verdict,
            Verdict::Blocked,
            "a ratio with a zero denominator is not a shape, it is a division"
        );
    }
}
