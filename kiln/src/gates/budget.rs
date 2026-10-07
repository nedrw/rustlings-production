//! `bench-budget@P` — the counts a step's harness reports stay within a budget of the course's
//! baseline.
//!
//! The gate exists because "it got slower" is not a review comment anybody can act on. The step ships
//! a harness (`tests/budget.rs`) that prints counts rather than times — statements per batch, cells
//! scanned, the ratio between two sizes — and the course keeps the number each count must stay near,
//! in `course/tracks/<track>/budget/<step-id>.toml` (handoff M2 §1). The gate runs the command that
//! file names, reads the `<track>-budget <name>=<value>` lines it prints, and compares:
//!
//! * every metric has to stay within ±P% of its baseline, so a count that doubles is a failure while
//!   a count that wobbles by a few percent is not;
//! * a name ending in `_max` is an upper bound instead, and may only go *down*: `scale_ratio_max` is
//!   "no worse than this", not "about this";
//! * a baseline metric the harness did not print is not judged, and a run that prints no metric at
//!   all is [`GateOutcome::blocked`] — never a pass, and never a failure of the learner, because the
//!   harness is the course's file and its command is the course's line.
//!
//! Nothing here measures wall-clock time. A count is the same number on a busy machine as on an idle
//! one, which is the difference between a gate a learner trusts and a gate they learn to re-run.
//!
//! The line format, the prefix and the course-side file are shared with `soak`, `fuzz` and `sim`
//! ([`crate::gates::harness`]) — including the reason the prefix carries the track id: T4's harnesses
//! print `lakelet-budget`, and the first version of this gate would have read `jobq-budget` over them.

use std::path::Path;

use serde::Deserialize;

use crate::error::Result;
use crate::gates::harness::{self, Bounds, Measured, number};
use crate::gates::runner::{GateContext, output_detail};
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// The course-side baseline of one step.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    /// Command that prints the counts, run in the learner's project.
    command: String,
    /// Baseline value per metric name.
    metric: Bounds,
}

/// Judge the step's counts against the course baseline.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    let percent = f64::from(spec.numeric_param()?);
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let Some((baseline_path, baseline)) = harness::parameters::<Baseline>(ctx, "budget")? else {
        return Ok(GateOutcome::blocked(format!(
            "{} has no bench baseline for {}, so there is nothing to compare against — this is \
             course content that is not written yet, not a failure of yours",
            ctx.step.qualified_id(),
            harness::parameter_path(ctx, "budget").display()
        )));
    };

    let prefix = harness::prefix(ctx, "budget");
    let ran = process::run_line(&baseline.command, project, ctx.timeout)?;
    let measured = match harness::measurements(&ran.stdout, &prefix) {
        Ok(measured) => measured,
        Err(detail) => return Ok(GateOutcome::blocked(detail)),
    };
    if measured.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "`{}` printed no `{prefix} <name>=<value>` line, so nothing could be compared — the \
             harness the course scaffolds for this step (`tests/budget.rs`) is what prints them:\n    {}",
            baseline.command,
            output_detail(&ran)
        )));
    }

    Ok(compare(
        percent,
        &baseline.metric,
        &measured,
        &baseline_path,
    ))
}

/// Compare every baseline metric with what the harness measured.
fn compare(
    percent: f64,
    baseline: &Bounds,
    measured: &[Measured],
    baseline_path: &Path,
) -> GateOutcome {
    let missing: Vec<&str> = baseline
        .keys()
        .filter(|name| {
            !measured
                .iter()
                .any(|measurement| &measurement.name == *name)
        })
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return GateOutcome::blocked(format!(
            "the harness did not report {}, which the course baseline ({}) has — without it this \
             step cannot be judged: the harness is the course's `tests/budget.rs`, so restore it with \
             `kiln reset` instead of editing it",
            missing.join(", "),
            baseline_path.display()
        ));
    }

    let mut failures: Vec<String> = Vec::new();
    let mut judged: Vec<String> = Vec::new();
    // Two rules are in play whenever the table carries an upper bound, and the pass line has to say so:
    // a `_max` metric is judged `≤ bound`, not `±P%`, and a report that calls both of them "within
    // ±P%" tells the learner the wrong thing about the metric that was measured (4.5's `--slow` run
    // printed it that way).
    let mut bounded = 0usize;
    for measurement in measured {
        let Some(limit) = baseline.get(&measurement.name) else {
            // A metric the course never measured has nothing to be compared against. Saying so here
            // would be noise on every step; the baseline is course content, so the fix is not the
            // learner's either way.
            continue;
        };
        judged.push(format!(
            "{} {}",
            measurement.name,
            number(measurement.value)
        ));
        if measurement.name.ends_with("_max") {
            bounded += 1;
            if measurement.value > *limit {
                failures.push(format!(
                    "{} is {} and has to stay ≤ {} (it is an upper bound)",
                    measurement.name,
                    number(measurement.value),
                    number(*limit)
                ));
            }
            continue;
        }
        let allowed = limit.abs() * percent / 100.0;
        if (measurement.value - limit).abs() > allowed {
            failures.push(format!(
                "{} is {}, {} of the baseline {} (more than ±{}%)",
                measurement.name,
                number(measurement.value),
                number(measurement.value - limit),
                number(*limit),
                number(percent)
            ));
        }
    }

    if !failures.is_empty() {
        return GateOutcome::failed(format!(
            "{} of {} metric(s) moved out of budget: {}",
            failures.len(),
            judged.len(),
            failures.join("; ")
        ));
    }
    GateOutcome::passed(format!(
        "{} metric(s) judged against the course baseline ({} within ±{}%, {} at or below an upper \
         bound): {}",
        judged.len(),
        judged.len() - bounded,
        number(percent),
        bounded,
        judged.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::Outcome;

    /// A baseline of two counts and one upper bound, as the course writes it.
    fn baseline() -> Bounds {
        Bounds::from([
            ("claim_statements".to_owned(), 12.0),
            ("scan_cells".to_owned(), 340.0),
            ("scale_ratio_max".to_owned(), 2.2),
        ])
    }

    fn result(stdout: &str) -> GateOutcome {
        let measured = harness::measurements(stdout, "jobq-budget").expect("the fixture parses");
        assert!(!measured.is_empty(), "the fixture has a metric");
        compare(10.0, &baseline(), &measured, Path::new("budget/3.22.toml"))
    }

    #[test]
    fn a_run_inside_the_budget_passes_and_names_the_counts() {
        let passed = result(
            "running 1 test\njobq-budget claim_statements=12\njobq-budget scan_cells=352\n\
             jobq-budget scale_ratio_max=2.0\ntest result: ok\n",
        );
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(
            passed.detail.contains("scan_cells 352"),
            "{}",
            passed.detail
        );
        assert!(passed.detail.contains("±10%"), "{}", passed.detail);
    }

    #[test]
    fn a_pass_line_names_both_rules_when_the_table_has_an_upper_bound() {
        // The table is two counts and one `_max`: a report that said "3 metric(s) within ±10%" would be
        // telling the reader that the upper bound was judged by the percentage rule, which it is not.
        let passed = result(
            "jobq-budget claim_statements=12\njobq-budget scan_cells=352\n\
             jobq-budget scale_ratio_max=2.0\n",
        );
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(
            passed.detail.contains("2 within ±10%")
                && passed.detail.contains("1 at or below an upper bound"),
            "{}",
            passed.detail
        );
    }

    #[test]
    fn a_count_outside_the_budget_fails_and_says_by_how_much() {
        let failed = result(
            "jobq-budget claim_statements=12\njobq-budget scan_cells=3400\n\
             jobq-budget scale_ratio_max=2.2\n",
        );
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.detail.contains("scan_cells"), "{}", failed.detail);
        assert!(failed.detail.contains("3060"), "{}", failed.detail);
        assert!(failed.detail.contains("baseline 340"), "{}", failed.detail);
    }

    #[test]
    fn an_upper_bound_only_has_to_stay_down() {
        // 2.3 is 4.5% away from 2.2: a ±10% rule would let it through, and an upper bound must not.
        let over = result(
            "jobq-budget claim_statements=12\njobq-budget scan_cells=340\n\
             jobq-budget scale_ratio_max=2.3\n",
        );
        assert_eq!(over.outcome, Outcome::Failed);
        assert!(over.detail.contains("upper bound"), "{}", over.detail);

        // Below the bound is always fine, however far below.
        let under = result(
            "jobq-budget claim_statements=12\njobq-budget scan_cells=340\n\
             jobq-budget scale_ratio_max=0.9\n",
        );
        assert_eq!(under.outcome, Outcome::Passed);
    }

    #[test]
    fn a_metric_the_harness_forgot_is_blocked_and_named() {
        let measured = harness::measurements("jobq-budget claim_statements=12\n", "jobq-budget")
            .expect("parses");
        let outcome = compare(10.0, &baseline(), &measured, Path::new("budget/3.22.toml"));
        assert_eq!(outcome.outcome, Outcome::Blocked);
        assert!(outcome.detail.contains("scan_cells"), "{}", outcome.detail);
        assert!(
            outcome.detail.contains("scale_ratio_max"),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn a_metric_with_no_baseline_is_not_judged() {
        let measured = harness::measurements(
            "jobq-budget claim_statements=12\njobq-budget scan_cells=340\n\
             jobq-budget scale_ratio_max=1.0\njobq-budget peak_rss=999999\n",
            "jobq-budget",
        )
        .expect("parses");
        let outcome = compare(10.0, &baseline(), &measured, Path::new("budget/3.22.toml"));
        assert_eq!(outcome.outcome, Outcome::Passed);
        assert!(!outcome.detail.contains("peak_rss"), "{}", outcome.detail);
    }

    #[test]
    fn a_line_that_is_not_a_count_is_reported_not_ignored() {
        let error = harness::measurements("jobq-budget scan_cells=fast\n", "jobq-budget")
            .expect_err("not a number");
        assert!(error.contains("not a number"), "{error}");
        let error = harness::measurements("jobq-budget just-a-word\n", "jobq-budget")
            .expect_err("not a metric");
        assert!(error.contains("jobq-budget"), "{error}");
        assert!(
            harness::measurements("nothing to see\n", "jobq-budget")
                .expect("no metrics")
                .is_empty(),
            "a run without metrics is not an error here: the caller reports it"
        );
    }

    #[test]
    fn a_baseline_the_course_does_not_have_is_readable() {
        let text = "command = \"cargo test -p jobq --test budget -- --nocapture\"\n\n[metric]\n\
                    claim_statements = 12\nscale_ratio_max = 2.2\n";
        let baseline: Baseline = toml::from_str(text).expect("the fixture parses");
        assert_eq!(baseline.metric.get("claim_statements"), Some(&12.0));
        assert_eq!(baseline.metric.get("scale_ratio_max"), Some(&2.2));
        assert!(baseline.command.contains("--nocapture"));
    }

    #[test]
    fn numbers_are_reported_the_way_a_person_writes_them() {
        assert_eq!(number(12.0), "12");
        assert_eq!(number(2.2), "2.2");
        assert_eq!(number(-100.0), "-100");
    }
}
