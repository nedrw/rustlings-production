//! `fuzz@N` — the step's parser survives a bounded, seeded search, and every pinned input still
//! replays.
//!
//! Fuzzing in the course is two things, and only one of them needs a fuzzer. The *search* is a seeded,
//! deterministic walk over mutated inputs — the step's harness prints how many mutations it made and
//! how many pinned inputs it replayed, so the workload is a number a report can be held to instead of a
//! promise that somebody ran it once. The *corpus* is the interesting half: every input that ever found
//! a panic is pinned in the step's spec (read-only, hash-pinned by `spec-integrity`), and the gate
//! checks that the harness still replays all of them. A regression input that stopped being replayed is
//! a bug that can come back without anybody noticing.
//!
//! Production fuzzing is `cargo-fuzz` (libFuzzer, coverage-guided, needs nightly). It is not part of
//! the toolchain baseline, so it cannot be a `must` gate (plan §10); `kiln doctor` reports it and the
//! step's prose names it as what a real project uses. What this gate adds beyond a spec run is the
//! *amount* and the *corpus*: a parser can pass a handful of hand-written cases and still fall over on
//! the thousandth mutation, which is exactly the finding this step is about.

use std::time::Instant;

use serde::Deserialize;

use crate::error::Result;
use crate::gates::harness::{self, number, seconds, value};
use crate::gates::runner::GateContext;
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// The variable the harness reads to pick the walk's seed.
pub const SEED: &str = "KILN_FUZZ_SEED";

/// What the harness reports: mutations performed, pinned inputs replayed.
const ITERATIONS: &str = "iterations";
const CORPUS_CASES: &str = "corpus_cases";

/// What one step's fuzz file declares.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    /// Command that runs the harness, in the learner's project.
    command: String,
    /// Seeds the walk is repeated under, in the order the file lists them.
    seeds: Vec<u64>,
    /// Mutations the walk has to perform per seed.
    iterations_min: u64,
    /// How many regression inputs the step's spec pins. The harness says how many it replayed.
    corpus_cases: u64,
}

/// One seed's run of the harness.
#[derive(Debug, Clone, PartialEq)]
enum Attempt {
    /// The run passed and reported both counts.
    Ran {
        /// Seed the run was made under.
        seed: u64,
        /// Mutations it performed.
        iterations: f64,
        /// Pinned inputs it replayed.
        corpus_cases: f64,
    },
    /// The run did not pass — a panic, a compile error, a deadline.
    Failed {
        /// Seed the run was made under.
        seed: u64,
        /// One line saying why.
        reason: String,
    },
    /// The run passed but printed nothing this gate can judge: the course's harness prints it.
    Silent {
        /// What was missing, ready for the report.
        detail: String,
    },
}

/// Run the step's fuzz harness once per seed and check the workload and the corpus replay.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let Some((path, parameters)) = harness::parameters::<Parameters>(ctx, "fuzz")? else {
        return Ok(GateOutcome::blocked(format!(
            "{} has no fuzz file for {}, so there is no walk to judge — this is course content that \
             is not written yet, not a failure of yours",
            ctx.step.qualified_id(),
            harness::parameter_path(ctx, "fuzz").display()
        )));
    };

    let seeds = distinct(&parameters.seeds);
    if seeds.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "{} lists no seed, so the walk would run once under whatever the harness defaults to — \
             the seeds are course content and belong in that file",
            path.display()
        )));
    }

    let prefix = harness::prefix(ctx, "fuzz");
    let started = Instant::now();
    let mut attempts: Vec<Attempt> = Vec::new();
    for seed in &seeds {
        let environment = [(SEED.to_owned(), seed.to_string())];
        let ran =
            process::run_line_with_env(&parameters.command, project, ctx.timeout, &environment)?;
        attempts.push(attempt(&ran, *seed, &prefix));
    }

    Ok(grade(&attempts, &parameters, &path, started.elapsed()))
}

/// What one run of the harness reported.
fn attempt(ran: &process::Ran, seed: u64, prefix: &str) -> Attempt {
    if !ran.success() {
        return Attempt::Failed {
            seed,
            reason: harness::failure_reason(ran),
        };
    }
    let Ok(measured) = harness::measurements(&ran.stdout, prefix) else {
        return Attempt::Silent {
            detail: format!(
                "the run under seed {seed} printed a line starting with `{prefix}` that is not a \
                 counter"
            ),
        };
    };
    let (Some(iterations), Some(corpus_cases)) =
        (value(&measured, ITERATIONS), value(&measured, CORPUS_CASES))
    else {
        let missing = [ITERATIONS, CORPUS_CASES]
            .into_iter()
            .filter(|name| value(&measured, name).is_none())
            .collect::<Vec<&str>>()
            .join(" and ");
        return Attempt::Silent {
            detail: format!("the run under seed {seed} printed no `{prefix} {missing}=<n>` line"),
        };
    };
    Attempt::Ran {
        seed,
        iterations,
        corpus_cases,
    }
}

/// Judge the runs: enough search, and every pinned input replayed.
fn grade(
    attempts: &[Attempt],
    parameters: &Parameters,
    path: &std::path::Path,
    elapsed: std::time::Duration,
) -> GateOutcome {
    let failed: Vec<String> = attempts
        .iter()
        .filter_map(|attempt| match attempt {
            Attempt::Failed { seed, reason } => Some(format!("seed {seed} ✗ ({reason})")),
            _ => None,
        })
        .collect();
    if !failed.is_empty() {
        return GateOutcome::failed(format!(
            "{} of {} seed(s) did not pass: {} (the walk took {})",
            failed.len(),
            attempts.len(),
            failed.join(", "),
            seconds(elapsed)
        ));
    }

    // A run that passed but printed nothing this gate can judge is a hole in the course's harness, not
    // a failure of the learner's code — and never a pass.
    if let Some(Attempt::Silent { detail }) = attempts
        .iter()
        .find(|attempt| matches!(attempt, Attempt::Silent { .. }))
    {
        return GateOutcome::blocked(format!(
            "{detail} — the harness the course scaffolds for this step (`tests/fuzz_protocol.rs`) is \
             what prints the counts this step is judged by ({})",
            path.display()
        ));
    }

    let ran: Vec<(u64, f64, f64)> = attempts
        .iter()
        .filter_map(|attempt| match attempt {
            Attempt::Ran {
                seed,
                iterations,
                corpus_cases,
            } => Some((*seed, *iterations, *corpus_cases)),
            _ => None,
        })
        .collect();

    let thin: Vec<String> = ran
        .iter()
        .filter(|(_, iterations, _)| *iterations < parameters.iterations_min as f64)
        .map(|(seed, iterations, _)| format!("seed {seed} made {}", number(*iterations)))
        .collect();
    if !thin.is_empty() {
        return GateOutcome::failed(format!(
            "{} of {} seed(s) performed fewer than {} mutation(s) ({}): a parser that survives a \
             handful of mutations has been tested by a handful of mutations. The step's harness \
             (`tests/fuzz_protocol.rs`) is what decides how many a seed performs",
            thin.len(),
            attempts.len(),
            parameters.iterations_min,
            thin.join(", ")
        ));
    }

    let replayed = ran.iter().fold(
        f64::INFINITY,
        |lowest, (_, _, cases)| {
            if *cases < lowest { *cases } else { lowest }
        },
    );
    if replayed < parameters.corpus_cases as f64 {
        return GateOutcome::failed(format!(
            "the harness replayed {} of the {} regression input(s) this step pins — a pinned input \
             that is no longer replayed is a panic that can come back ({} states the count, and `kiln \
             spec restore` puts the spec back)",
            number(replayed),
            parameters.corpus_cases,
            path.display()
        ));
    }

    GateOutcome::passed(format!(
        "{} seed(s) ran, {} mutation(s) each, {} of {} pinned input(s) replayed (the walk took {})",
        attempts.len(),
        ran.iter()
            .map(|(_, iterations, _)| number(*iterations))
            .collect::<Vec<String>>()
            .join("/"),
        number(replayed),
        parameters.corpus_cases,
        seconds(elapsed)
    ))
}

/// Seeds in file order, without repeats.
fn distinct(seeds: &[u64]) -> Vec<u64> {
    let mut unique: Vec<u64> = Vec::new();
    for seed in seeds {
        if !unique.contains(seed) {
            unique.push(*seed);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::Outcome;
    use std::time::Duration;

    fn parameters() -> Parameters {
        toml::from_str(
            "command = \"cargo test -p lakelet --test fuzz_protocol -- --nocapture\"\n\
             seeds = [1, 7, 20260101]\niterations_min = 20000\ncorpus_cases = 6\n",
        )
        .expect("the fixture parses")
    }

    fn reported(seed: u64, iterations: f64, corpus: f64) -> Attempt {
        Attempt::Ran {
            seed,
            iterations,
            corpus_cases: corpus,
        }
    }

    fn judged(attempts: &[Attempt]) -> GateOutcome {
        grade(
            attempts,
            &parameters(),
            std::path::Path::new("course/tracks/lakelet/fuzz/4.2-parser-fuzz.toml"),
            Duration::from_secs(18),
        )
    }

    #[test]
    fn the_declaration_is_read_the_way_the_course_writes_it() {
        let declared = parameters();
        assert_eq!(declared.seeds, [1, 7, 20_260_101]);
        assert_eq!(declared.iterations_min, 20_000);
        assert_eq!(declared.corpus_cases, 6);
        assert_eq!(distinct(&[7, 7, 1]), [7, 1]);
        assert!(
            toml::from_str::<Parameters>("seeds = [1]\n").is_err(),
            "the rest is required"
        );
    }

    #[test]
    fn a_walk_that_searched_and_replayed_everything_passes() {
        let passed = judged(&[
            reported(1, 20_000.0, 6.0),
            reported(7, 20_000.0, 6.0),
            reported(20_260_101, 20_512.0, 6.0),
        ]);
        assert_eq!(passed.outcome, Outcome::Passed);
        assert!(passed.detail.contains("3 seed(s) ran"), "{}", passed.detail);
        assert!(
            passed.detail.contains("20000/20000/20512"),
            "{}",
            passed.detail
        );
        assert!(
            passed.detail.contains("6 of 6 pinned input(s)"),
            "{}",
            passed.detail
        );
    }

    #[test]
    fn a_walk_that_barely_mutated_anything_is_too_thin_to_count() {
        let failed = judged(&[reported(1, 20_000.0, 6.0), reported(7, 120.0, 6.0)]);
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(
            failed.detail.contains("1 of 2 seed(s)"),
            "{}",
            failed.detail
        );
        assert!(
            failed.detail.contains("seed 7 made 120"),
            "{}",
            failed.detail
        );
    }

    #[test]
    fn a_regression_input_that_stopped_being_replayed_fails() {
        let failed = judged(&[reported(1, 20_000.0, 4.0), reported(7, 20_000.0, 6.0)]);
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(
            failed.detail.contains("replayed 4 of the 6"),
            "{}",
            failed.detail
        );
    }

    #[test]
    fn a_panic_in_the_walk_names_its_seed_and_reason() {
        let broken = Attempt::Failed {
            seed: 7,
            reason: "101: panicked at tests/fuzz_protocol.rs:63".to_owned(),
        };
        let failed = judged(&[reported(1, 20_000.0, 6.0), broken]);
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.detail.contains("seed 7 ✗"), "{}", failed.detail);
        assert!(failed.detail.contains("panicked at"), "{}", failed.detail);
    }

    #[test]
    fn a_harness_that_printed_no_counters_blocks_instead_of_failing_the_learner() {
        let run = process::Ran {
            code: Some(0),
            stdout: "running 1 test\ntest result: ok\n".to_owned(),
            stderr: String::new(),
            timed_out: false,
            elapsed: Duration::from_secs(1),
        };
        let silent = attempt(&run, 1, "lakelet-fuzz");
        assert!(matches!(silent, Attempt::Silent { .. }), "{silent:?}");
        // With another seed's counts intact, the silence is still what the gate reports.
        let blocked = judged(&[reported(7, 20_000.0, 6.0), silent]);
        assert_eq!(blocked.outcome, Outcome::Blocked);
        assert!(
            blocked.detail.contains("corpus_cases"),
            "{}",
            blocked.detail
        );
        assert!(blocked.detail.contains("seed 1"), "{}", blocked.detail);
    }

    #[test]
    fn a_run_that_failed_is_reported_before_a_silent_one() {
        // Both problems in one walk: the panic is the learner's and comes first, because it is the one
        // they can act on.
        let silent = Attempt::Silent {
            detail: "the run under seed 7 printed no `lakelet-fuzz iterations=<n>` line".to_owned(),
        };
        let broken = Attempt::Failed {
            seed: 1,
            reason: "101: panicked at tests/fuzz_protocol.rs:63".to_owned(),
        };
        let failed = judged(&[broken, silent]);
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.detail.contains("panicked at"), "{}", failed.detail);
    }
}
