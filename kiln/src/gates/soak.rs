//! `soak@N` — a long run of cycles, judged by the resources that are still alive at its end.
//!
//! The plan asks for "N minutes without a leak" (plan §6). This gate judges the leak and not the
//! minutes, for the reason every `must` slow gate is judged by counters (handoff M2 §1.1): a wall-clock
//! criterion on a shared machine turns a correct step red when somebody else is compiling, and the
//! interesting question — *is anything pile up?* — is a number the code under test can report without
//! involving the clock at all. So:
//!
//! * the course file names the command and a **floor** on the workload (`cycles_min`): a soak that
//!   stopped after a tenth of the cycles has not seen the growth it exists to find;
//! * every metric in that file is an **upper bound** (`open_tasks_max`, `open_handles_max`,
//!   `rss_growth_bytes_max`) — these are stocks, not throughputs, so "about right" is not a
//!   meaningful verdict and "not above this" is;
//! * the wall clock is printed as *evidence* (`4000 cycle(s) in 41s`) and never compared. A real long
//!   run is the same command with a longer workload (`LAKELET_SOAK_MINUTES=30` in this track's step),
//!   and it is judged the same way — a bigger sample of one question.
//!
//! The harness (`tests/soak.rs`) is the course's file, like `tests/budget.rs`: it drives the learner's
//! engine through more cycles than a test would, reads the process's own live counters at the end of
//! each one, and prints `<track>-soak <name>=<value>` lines for the last cycle. Because the harness is
//! course content, a harness that prints nothing is [`GateOutcome::blocked`] and never a failure of
//! the learner's.

use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::Result;
use crate::gates::harness::{self, Bounds, number, seconds, value};
use crate::gates::runner::GateContext;
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// The count of completed cycles, which the harness always reports.
const CYCLES: &str = "cycles";

/// What one step's soak file declares.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    /// Command that runs the soak, in the learner's project.
    command: String,
    /// Cycles the run has to complete. A floor, not a target.
    cycles_min: u64,
    /// Upper bounds, one per live counter the harness reports. A key may be written either as the
    /// counter's own name (`open_tasks`) or with the `_max` this gate's vocabulary gives it
    /// (`open_tasks_max`); both say the same thing here, because in a soak every number is a stock
    /// that has to stay down.
    metric: Bounds,
}

/// Run the step's soak and judge its live counters.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let Some((path, parameters)) = harness::parameters::<Parameters>(ctx, "soak")? else {
        return Ok(GateOutcome::blocked(format!(
            "{} has no soak file for {}, so there is no long run to judge — this is course content \
             that is not written yet, not a failure of yours",
            ctx.step.qualified_id(),
            harness::parameter_path(ctx, "soak").display()
        )));
    };

    let prefix = harness::prefix(ctx, "soak");
    let started = Instant::now();
    let ran = process::run_line(&parameters.command, project, ctx.timeout)?;
    let elapsed = started.elapsed();
    if !ran.success() {
        return Ok(GateOutcome::failed(format!(
            "the soak did not finish: {} (after {})",
            harness::failure_reason(&ran),
            seconds(elapsed)
        )));
    }

    let measured = match harness::measurements(&ran.stdout, &prefix) {
        Ok(measured) => measured,
        Err(detail) => return Ok(GateOutcome::blocked(detail)),
    };
    let Some(cycles) = value(&measured, CYCLES) else {
        return Ok(GateOutcome::blocked(format!(
            "`{}` printed no `{prefix} {CYCLES}=<n>` line, so there is no evidence the run soaked \
             anything — the harness the course scaffolds for this step (`tests/soak.rs`) is what \
             prints it",
            parameters.command
        )));
    };

    Ok(compare(&parameters, &measured, cycles, &path, elapsed))
}

/// Judge the run: enough cycles, and every live counter inside its bound.
fn compare(
    parameters: &Parameters,
    measured: &[harness::Measured],
    cycles: f64,
    path: &std::path::Path,
    elapsed: Duration,
) -> GateOutcome {
    if cycles < parameters.cycles_min as f64 {
        return GateOutcome::failed(format!(
            "the run completed {} cycle(s) and this step's soak is {} — a soak that stops early has \
             not seen the growth it exists to find. The workload is the harness's, so the number to \
             compare with is `cycles_min` in {}; run the learner's long form of the same command for \
             a bigger sample",
            number(cycles),
            parameters.cycles_min,
            path.display()
        ));
    }

    let mut failures: Vec<String> = Vec::new();
    let mut judged: Vec<String> = Vec::new();
    let mut bounds: Vec<(&str, f64)> = Vec::new();
    for (name, bound) in &parameters.metric {
        // `open_tasks_max = 24` in the file is the bound of the harness's `open_tasks` counter: the
        // file says which way the number may go, and the harness prints what it measured.
        let counter = name.strip_suffix("_max").unwrap_or(name);
        if bounds.iter().any(|(seen, _)| *seen == counter) {
            continue;
        }
        bounds.push((counter, *bound));
    }

    for (counter, bound) in bounds {
        let Some(current) = value(measured, counter) else {
            return GateOutcome::blocked(format!(
                "the harness printed no `{counter}=` line and {} has a bound for it — without that \
                 counter this step cannot be judged: the harness is the course's `tests/soak.rs`, so \
                 restore it with `kiln reset` instead of editing it",
                path.display()
            ));
        };
        judged.push(format!("{counter} {}", number(current)));
        if current > bound {
            failures.push(format!(
                "{counter} is {} and has to stay ≤ {} (it is an upper bound)",
                number(current),
                number(bound)
            ));
        }
    }

    if !failures.is_empty() {
        return GateOutcome::failed(format!(
            "{} of {} live counter(s) are above their bound after {} cycle(s): {} — something the run \
             opened is still alive ({})",
            failures.len(),
            judged.len(),
            number(cycles),
            failures.join("; "),
            seconds(elapsed)
        ));
    }
    GateOutcome::passed(format!(
        "{} cycle(s) in {}, {} live counter(s) inside their bounds: {}",
        number(cycles),
        seconds(elapsed),
        judged.len(),
        judged.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::Outcome;

    /// A soak file of one floor and three upper bounds, as the course writes it.
    fn parameters() -> Parameters {
        toml::from_str(
            "command = \"cargo test -p lakelet --release --test soak -- --nocapture\"\n\
             cycles_min = 4000\n\n[metric]\nopen_tasks_max = 24\nopen_handles_max = 8\n\
             rss_growth_bytes_max = 120000000\n",
        )
        .expect("the fixture parses")
    }

    fn judged(stdout: &str) -> GateOutcome {
        let measured = harness::measurements(stdout, "lakelet-soak").expect("parses");
        let cycles = value(&measured, CYCLES).expect("the fixture reports cycles");
        compare(
            &parameters(),
            &measured,
            cycles,
            std::path::Path::new("course/tracks/lakelet/soak/4.17-soak.toml"),
            Duration::from_secs(41),
        )
    }

    #[test]
    fn a_run_that_stays_flat_passes_and_names_the_counters() {
        let passed = judged(
            "running 1 test\nlakelet-soak cycles=4000\nlakelet-soak open_tasks=2\n\
             lakelet-soak open_handles=1\nlakelet-soak rss_growth_bytes=9000000\n",
        );
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(
            passed.detail.contains("4000 cycle(s) in 41s"),
            "{}",
            passed.detail
        );
        assert!(passed.detail.contains("open_tasks 2"), "{}", passed.detail);
        assert!(
            passed.detail.contains("3 live counter(s)"),
            "{}",
            passed.detail
        );
    }

    #[test]
    fn a_counter_above_its_bound_fails_and_says_what_is_still_alive() {
        let failed = judged(
            "lakelet-soak cycles=4000\nlakelet-soak open_tasks=61\nlakelet-soak open_handles=1\n\
             lakelet-soak rss_growth_bytes=9000000\n",
        );
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(
            failed.detail.contains("open_tasks is 61"),
            "{}",
            failed.detail
        );
        assert!(failed.detail.contains("stay ≤ 24"), "{}", failed.detail);
        assert!(failed.detail.contains("1 of 3"), "{}", failed.detail);
    }

    #[test]
    fn a_run_that_stopped_early_is_not_a_soak() {
        let failed = judged(
            "lakelet-soak cycles=1200\nlakelet-soak open_tasks=2\nlakelet-soak open_handles=1\n\
             lakelet-soak rss_growth_bytes=9000000\n",
        );
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(
            failed.detail.contains("completed 1200 cycle(s)"),
            "{}",
            failed.detail
        );
        assert!(failed.detail.contains("is 4000"), "{}", failed.detail);
    }

    #[test]
    fn a_counter_the_harness_never_printed_blocks_instead_of_passing() {
        let blocked = judged(
            "lakelet-soak cycles=4000\nlakelet-soak open_tasks=2\nlakelet-soak open_handles=1\n",
        );
        assert_eq!(blocked.outcome, Outcome::Blocked);
        assert!(
            blocked.detail.contains("rss_growth_bytes"),
            "{}",
            blocked.detail
        );
    }
}
