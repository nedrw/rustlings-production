//! Running the gates of one step.
//!
//! The runner answers three questions per gate: did it run, what did it find, and would it block
//! the step? A gate that could not run is never reported as a pass — that distinction is the whole
//! point of having `blocked` and `pending` outcomes (plan §10).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::gates::registry::{GateStatus, Outcome, lookup};
use crate::gates::{GateSpec, Tier};
use crate::info::{Course, Step, Track};

/// Number of output lines kept when a gate has to quote a failing command.
const DETAIL_LINES: usize = 12;

/// Everything a gate may look at.
#[derive(Debug)]
pub struct GateContext<'a> {
    /// The loaded course.
    pub course: &'a Course,
    /// Track the step belongs to.
    pub track: &'a Track,
    /// Step being judged.
    pub step: &'a Step,
    /// Absolute path of the learner's project for this track (`app/mailer`).
    pub project_dir: PathBuf,
    /// Deadline for each command the gate starts.
    pub timeout: Duration,
}

impl GateContext<'_> {
    /// Deadline for probing a tool: short, so a hung tool cannot stall a run.
    #[must_use]
    pub fn probe_timeout(&self) -> Duration {
        Duration::from_secs(30).min(self.timeout)
    }

    /// The learner's project directory, if it exists.
    #[must_use]
    pub fn existing_project(&self) -> Option<&Path> {
        self.project_dir
            .is_dir()
            .then_some(self.project_dir.as_path())
    }

    /// What a gate should say when the project it judges is missing.
    #[must_use]
    pub fn missing_project(&self) -> String {
        format!(
            "{} does not exist yet — the step's project content is not authored yet",
            self.project_dir.display()
        )
    }
}

/// What one gate did.
#[derive(Debug, Clone)]
pub struct GateResult {
    /// Gate as the step declared it.
    pub spec: GateSpec,
    /// Severity of the result.
    pub outcome: Outcome,
    /// One line explaining the result.
    pub detail: String,
    /// How long it took.
    pub elapsed: Duration,
}

impl GateResult {
    /// `true` when this result stops a `must` gate from passing.
    #[must_use]
    pub fn breaks_the_step(&self) -> bool {
        self.spec.tier == Tier::Must && matches!(self.outcome, Outcome::Failed | Outcome::Blocked)
    }

    /// Report line: `✓ fmt  formatting is uniform`.
    #[must_use]
    pub fn line(&self) -> String {
        format!(
            "{} {:<14} {}",
            self.outcome.marker(),
            self.spec.to_string(),
            self.detail
        )
    }
}

/// Run every gate of a step.
///
/// Slow gates are skipped unless `run_slow` is set: they belong to `kiln verify --slow` and
/// `selftest`, not to the fast feedback loop (plan §6).
#[must_use]
pub fn run_all(ctx: &GateContext<'_>, specs: &[GateSpec], run_slow: bool) -> Vec<GateResult> {
    specs
        .iter()
        .map(|spec| run_one(ctx, spec, run_slow))
        .collect()
}

/// `true` when every must gate passed (should gates only warn).
#[must_use]
pub fn all_must_passed(results: &[GateResult]) -> bool {
    !results.iter().any(GateResult::breaks_the_step)
}

fn run_one(ctx: &GateContext<'_>, spec: &GateSpec, run_slow: bool) -> GateResult {
    let started = Instant::now();
    let skip = |detail: String| GateResult {
        spec: spec.clone(),
        outcome: Outcome::Pending,
        detail,
        elapsed: started.elapsed(),
    };

    if spec.slow && !run_slow {
        return skip("slow gate: only run by `kiln verify --slow` and `selftest`".to_owned());
    }

    let Some(definition) = lookup(&spec.name) else {
        return skip(format!(
            "unknown gate `{}` — the catalogue is docs/plan.md §6",
            spec.name
        ));
    };

    match definition.status {
        GateStatus::Pending(reason) => {
            return skip(format!(
                "{}: {reason} (not implemented yet)",
                definition.purpose
            ));
        }
        GateStatus::Ready => {}
    }

    let Some(run) = definition.run else {
        return skip(format!("{} has no implementation", spec.name));
    };

    match run(ctx, spec) {
        Ok(outcome) => GateResult {
            spec: spec.clone(),
            outcome: outcome.outcome,
            detail: outcome.detail,
            elapsed: started.elapsed(),
        },
        Err(error) => GateResult {
            spec: spec.clone(),
            outcome: Outcome::Blocked,
            detail: error.to_string(),
            elapsed: started.elapsed(),
        },
    }
}

/// Explain a failing command in a few lines, without burying the report.
#[must_use]
pub fn clip(text: &str, lines: usize) -> String {
    let kept: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .take(lines.max(1))
        .collect();
    let total = text.lines().filter(|line| !line.trim().is_empty()).count();
    let mut joined = kept.join("\n    ");
    if total > kept.len() {
        joined.push_str(&format!("\n    … {} more line(s)", total - kept.len()));
    }
    joined
}

/// One-line summary of a failing command, used when the full output is not helpful.
#[must_use]
pub fn failure_detail(ran: &crate::process::Ran) -> String {
    if ran.timed_out {
        return format!("timed out after {:?}", ran.elapsed);
    }
    match ran.first_stderr_line() {
        Some(line) => line.trim().to_owned(),
        None => format!("exit code {:?}", ran.code),
    }
}

/// First lines of a command's output, for the report.
#[must_use]
pub fn output_detail(ran: &crate::process::Ran) -> String {
    let output = if ran.stderr.trim().is_empty() {
        &ran.stdout
    } else {
        &ran.stderr
    };
    clip(output, DETAIL_LINES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::registry::catalogue;

    #[test]
    fn clipping_keeps_the_head_and_says_how_much_was_left_out() {
        let text = "one\ntwo\n\nthree\nfour";
        assert_eq!(clip(text, 2), "one\n    two\n    … 2 more line(s)");
        assert_eq!(clip(text, 99), "one\n    two\n    three\n    four");
    }

    #[test]
    fn only_must_gates_can_stop_a_step() {
        let must = GateSpec::must("fmt");
        let should = "?pedantic".parse::<GateSpec>().unwrap();
        let failed = |spec: GateSpec| GateResult {
            spec,
            outcome: Outcome::Failed,
            detail: String::new(),
            elapsed: Duration::ZERO,
        };
        assert!(failed(must.clone()).breaks_the_step());
        assert!(!failed(should).breaks_the_step());
        assert!(!all_must_passed(&[failed(must)]));
    }

    #[test]
    fn the_catalogue_declares_which_gates_are_pending_and_why() {
        // A pending gate must say when it arrives; that is what keeps `kiln check` honest
        // while M0 is being written.
        let pending: Vec<&str> = catalogue()
            .iter()
            .filter(|gate| matches!(gate.status, GateStatus::Pending(_)))
            .map(|gate| gate.name)
            .collect();
        assert!(pending.len() >= 15, "{pending:?}");
        assert!(pending.contains(&"mutants"));
    }
}
