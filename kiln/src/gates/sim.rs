//! `sim@N` — the step's suite, run once per seed, under a deterministic simulation.
//!
//! A test that passes on one interleaving proves that one interleaving works. The scenarios this
//! gate judges put the step's own code under a *simulated* runtime (`turmoil` in this course): time is
//! virtual, the scheduler is the simulator's, and the seed decides which of the many legal orders a
//! run takes. The gate then does what a single run cannot:
//!
//! 1. **every seed passes** — a scenario that holds under one schedule and breaks under another is the
//!    bug this gate exists to find, and a fix that only works under the seed the author happened to
//!    try is not a fix;
//! 2. **the same seed replays** — the seed that the course file marks as `replay` is run twice, and the
//!    two transcripts have to be identical. That is what "deterministic" means, and it is a
//!    judgement, not an observation;
//! 3. **the seeds matter** — at least two seeds have to produce *different* transcripts. Three
//!    identical transcripts mean the suite never read the seed, so `sim@3` would have been one run
//!    reported three times.
//!
//! The project says how to run a scenario by printing lines that start with `<track>-sim`, one per
//! event it observed (`lakelet-sim flush segment=7 records=128`). The transcript *is* those lines, in
//! order: it is what two runs are compared through, so a scenario that observes nothing has nothing
//! to replay, which is why the seed list and the command live in a course file
//! (`course/tracks/<track>/sim/<step-id>.toml`) at the trust level of `spec/`.
//!
//! Nothing here reads a wall clock: the walk's duration is reported and never judged (handoff M2
//! §1.1 — a slow shared machine must not turn a passing step red).

use std::time::Instant;

use serde::Deserialize;

use crate::error::Result;
use crate::gates::harness::{self, digest, seconds, transcript};
use crate::gates::runner::GateContext;
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// The variable the scenario reads to pick its schedule.
///
/// Deliberately the same mechanism `conformance` uses for backends (`KILN_BACKEND`): a learner meets
/// "the judge re-runs my suite with a knob set" once in T3 and once more here, and the knob is the
/// only thing that changes between runs.
pub const SEED: &str = "KILN_SIM_SEED";

/// What one step's simulation file declares.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenarios {
    /// Command that runs the scenarios. Defaults to the step's own `run.cmd`.
    #[serde(default)]
    command: Option<String>,
    /// Seeds the run is repeated under, in the order the file lists them.
    seeds: Vec<u64>,
    /// The seed that is run a second time; the two transcripts have to match.
    replay: u64,
}

/// One seed's run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attempt {
    /// Seed the run was made under.
    seed: u64,
    /// The transcript it printed, or the one-line reason it failed.
    result: std::result::Result<String, String>,
}

impl Attempt {
    /// The digest a report names this run by, or the failure.
    fn summary(&self) -> String {
        match &self.result {
            Ok(text) => digest(text),
            Err(reason) => format!("seed {} ✗ ({reason})", self.seed),
        }
    }
}

/// Run the step's scenario under every seed the course file lists.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    let required = spec.numeric_param()? as usize;
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let Some((path, scenarios)) = harness::parameters::<Scenarios>(ctx, "sim")? else {
        return Ok(GateOutcome::blocked(format!(
            "{} has no simulation file for {}, so there is nothing to run under a seed — this is \
             course content that is not written yet, not a failure of yours",
            ctx.step.qualified_id(),
            harness::parameter_path(ctx, "sim").display()
        )));
    };

    let seeds = distinct(&scenarios.seeds);
    if seeds.len() < required {
        return Ok(GateOutcome::blocked(format!(
            "{} declares {} seed(s) ({}) and this step requires at least {required} — the seed list \
             is course content, so this is a gap in the step, not in your project",
            path.display(),
            seeds.len(),
            join(&seeds)
        )));
    }
    if !seeds.contains(&scenarios.replay) {
        return Ok(GateOutcome::blocked(format!(
            "{} asks to replay seed {} and does not list it, so the determinism half of this gate \
             could not be judged — the seed list ({}) and `replay` have to agree",
            path.display(),
            scenarios.replay,
            join(&seeds)
        )));
    }

    let command = match (&scenarios.command, ctx.step.run.as_ref()) {
        (Some(command), _) => command.clone(),
        (None, Some(run)) => run.cmd.clone(),
        (None, None) => {
            return Ok(GateOutcome::blocked(format!(
                "{} declares no `run.cmd` and {} names no `command`, so there is no scenario to run",
                ctx.step.qualified_id(),
                path.display()
            )));
        }
    };

    let prefix = harness::prefix(ctx, "sim");
    let started = Instant::now();
    let mut attempts: Vec<Attempt> = Vec::new();
    for seed in &seeds {
        attempts.push(attempt(&command, project, ctx.timeout, *seed, &prefix)?);
    }
    let replayed = attempt(&command, project, ctx.timeout, scenarios.replay, &prefix)?;

    Ok(grade(
        &attempts,
        &replayed,
        scenarios.replay,
        &prefix,
        &path,
        started.elapsed(),
    ))
}

/// Run the command once under one seed and read the transcript it printed.
fn attempt(
    command: &str,
    project: &std::path::Path,
    timeout: std::time::Duration,
    seed: u64,
    prefix: &str,
) -> Result<Attempt> {
    let environment = [(SEED.to_owned(), seed.to_string())];
    let ran = process::run_line_with_env(command, project, timeout, &environment)?;
    let result = if ran.success() {
        let text = transcript(&ran.stdout, prefix);
        if text.is_empty() {
            Err(format!("printed no `{prefix}` line"))
        } else {
            Ok(text)
        }
    } else {
        Err(harness::failure_reason(&ran))
    };
    Ok(Attempt { seed, result })
}

/// The gate's answer from the runs.
fn grade(
    attempts: &[Attempt],
    replayed: &Attempt,
    replay_seed: u64,
    prefix: &str,
    path: &std::path::Path,
    elapsed: std::time::Duration,
) -> GateOutcome {
    let failed: Vec<&Attempt> = attempts
        .iter()
        .filter(|attempt| attempt.result.is_err())
        .collect();
    if !failed.is_empty() {
        return GateOutcome::failed(format!(
            "{} of {} seed(s) did not pass: {} (the walk took {})",
            failed.len(),
            attempts.len(),
            failed
                .iter()
                .map(|attempt| attempt.summary())
                .collect::<Vec<String>>()
                .join(", "),
            seconds(elapsed)
        ));
    }

    let first = attempts
        .iter()
        .find(|attempt| attempt.seed == replay_seed)
        .map(|attempt| attempt.result.clone().unwrap_or_default())
        .unwrap_or_default();
    match (&replayed.result, &first) {
        (Err(reason), _) => {
            return GateOutcome::failed(format!(
                "seed {replay_seed} passed once and not twice: the replayed run {reason} — the same \
                 seed has to make the same thing happen twice (the walk took {})",
                seconds(elapsed)
            ));
        }
        (Ok(second), _) if *second != first => {
            return GateOutcome::failed(format!(
                "the same seed produced two different transcripts: seed {replay_seed} came out as {} \
                 and {} — a scenario replays exactly, and this one does not. A map's iteration order, \
                 a wall-clock read or a task spawned without a fixed order is the usual cause; the \
                 fix is to make the observed order an output of the scenario, not of the machine",
                digest(&first),
                digest(second)
            ));
        }
        (Ok(_), _) => {}
    }

    let mut digests: Vec<String> = attempts.iter().map(Attempt::summary).collect();
    let distinct: Vec<String> = {
        let mut unique = digests.clone();
        unique.sort_unstable();
        unique.dedup();
        unique
    };
    if distinct.len() < 2 {
        return GateOutcome::failed(format!(
            "every seed produced the same transcript ({}): the scenario never read `{SEED}`, so {} \
             seeds checked one schedule. Print what the run observed — one `{prefix}` line per event — \
             and let the seed decide the order those events happen in ({})",
            digests.first().cloned().unwrap_or_default(),
            attempts.len(),
            path.display()
        ));
    }

    digests.sort_unstable();
    GateOutcome::passed(format!(
        "{} seed(s) ran, transcripts {} — seed {replay_seed} replayed identically (the walk took {})",
        attempts.len(),
        digests.join("/"),
        seconds(elapsed)
    ))
}

/// Seeds in file order, without repeats: two entries of the same seed are one run.
fn distinct(seeds: &[u64]) -> Vec<u64> {
    let mut unique: Vec<u64> = Vec::new();
    for seed in seeds {
        if !unique.contains(seed) {
            unique.push(*seed);
        }
    }
    unique
}

/// Seeds as a report writes them.
fn join(seeds: &[u64]) -> String {
    seeds
        .iter()
        .map(u64::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::Outcome;
    use std::time::Duration;

    fn ok(seed: u64, text: &str) -> Attempt {
        Attempt {
            seed,
            result: Ok(text.to_owned()),
        }
    }

    fn failed(seed: u64, reason: &str) -> Attempt {
        Attempt {
            seed,
            result: Err(reason.to_owned()),
        }
    }

    fn graded(attempts: &[Attempt], replayed: Attempt) -> GateOutcome {
        grade(
            attempts,
            &replayed,
            1,
            "lakelet-sim",
            std::path::Path::new("course/tracks/lakelet/sim/4.4-wal-recovery.toml"),
            Duration::from_secs(12),
        )
    }

    #[test]
    fn the_declaration_is_read_the_way_the_course_writes_it() {
        let declared: Scenarios = toml::from_str(
            "# deterministic-simulation scenarios for lakelet/4.4\n\
             seeds = [1, 7, 20260101]\nreplay = 1\n\
             command = \"cargo test -p lakelet --test sim_wal\"\n",
        )
        .expect("the fixture parses");
        assert_eq!(declared.seeds, [1, 7, 20_260_101]);
        assert_eq!(declared.replay, 1);
        assert_eq!(
            declared.command.as_deref(),
            Some("cargo test -p lakelet --test sim_wal")
        );

        let bare: Scenarios = toml::from_str("seeds = [3]\nreplay = 3\n").expect("parses");
        assert_eq!(
            bare.command, None,
            "a file may leave the command to run.cmd"
        );
        assert!(
            toml::from_str::<Scenarios>("seeds = [1]\n").is_err(),
            "replay is required"
        );
    }

    #[test]
    fn a_seed_listed_twice_is_one_seed() {
        assert_eq!(distinct(&[7, 7, 1, 7, 2]), [7, 1, 2]);
    }

    #[test]
    fn three_seeds_that_pass_and_replay_pass_the_gate() {
        let passed = graded(
            &[ok(1, "a\nb"), ok(7, "c"), ok(20260101, "d")],
            ok(1, "a\nb"),
        );
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(passed.detail.contains("3 seed(s) ran"), "{}", passed.detail);
        assert!(
            passed.detail.contains("seed 1 replayed identically"),
            "{}",
            passed.detail
        );
        assert!(
            passed.detail.contains("the walk took 12s"),
            "{}",
            passed.detail
        );
    }

    #[test]
    fn one_seed_that_fails_names_itself_and_its_reason() {
        let failed_gate = graded(
            &[
                ok(1, "a"),
                failed(7, "101: panicked at tests/sim_wal.rs:41"),
            ],
            ok(1, "a"),
        );
        assert_eq!(failed_gate.outcome, Outcome::Failed);
        assert!(
            failed_gate.detail.contains("1 of 2"),
            "{}",
            failed_gate.detail
        );
        assert!(
            failed_gate.detail.contains("seed 7 ✗"),
            "{}",
            failed_gate.detail
        );
        assert!(
            failed_gate.detail.contains("panicked at"),
            "{}",
            failed_gate.detail
        );
    }

    #[test]
    fn a_seed_that_passes_twice_differently_is_not_deterministic() {
        let failed_gate = graded(&[ok(1, "a"), ok(7, "b")], ok(1, "a but elsewhere"));
        assert_eq!(failed_gate.outcome, Outcome::Failed);
        assert!(
            failed_gate.detail.contains("two different transcripts"),
            "{}",
            failed_gate.detail
        );
        assert!(
            failed_gate.detail.contains("seed 1"),
            "{}",
            failed_gate.detail
        );
    }

    #[test]
    fn seeds_that_never_change_the_transcript_check_one_schedule() {
        let failed_gate = graded(
            &[ok(1, "same"), ok(7, "same"), ok(9, "same")],
            ok(1, "same"),
        );
        assert_eq!(failed_gate.outcome, Outcome::Failed);
        assert!(
            failed_gate.detail.contains("never read `KILN_SIM_SEED`"),
            "{}",
            failed_gate.detail
        );
        assert!(
            failed_gate.detail.contains("3 seeds checked one schedule"),
            "{}",
            failed_gate.detail
        );
    }

    #[test]
    fn a_replayed_run_that_fails_is_reported_as_a_replay_and_not_as_a_run() {
        let failed_gate = graded(&[ok(1, "a"), ok(7, "b")], failed(1, "timed out after 25s"));
        assert_eq!(failed_gate.outcome, Outcome::Failed);
        assert!(
            failed_gate.detail.contains("passed once and not twice"),
            "{}",
            failed_gate.detail
        );
    }
}
