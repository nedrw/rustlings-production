//! Learner progress, `.kiln/state.toml`.
//!
//! Two things live here, and they are deliberately separate: *progress* (did the spec tests and
//! the must gates ever pass?) and *honesty* (was the reference solution applied, was a spec file
//! edited?). Plan §3 D2: the runner does not prevent the second, it records it, because a
//! self-study course that pretends its progress file is evidence would be lying.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{KilnError, Result};
use crate::info::FORMAT_VERSION;

/// Directory below the repository root that holds runner state.
pub const STATE_DIR: &str = ".kiln";
/// File name of the progress file.
pub const STATE_FILE: &str = "state.toml";

/// Header written above the serialized progress so the file explains itself.
const HEADER: &str = "\
# Progress recorded by kiln. Personal state, not course content: it is git-ignored on purpose.
# spec_passed and gates_passed are the record of what actually ran; solution_applied and
# spec_modified say whether the result is the learner's own work (plan §3 D2).
";

/// What the learner has achieved on one step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StepState {
    /// The step's spec tests passed at least once.
    #[serde(default)]
    pub spec_passed: bool,
    /// Every must gate passed at least once.
    #[serde(default)]
    pub gates_passed: bool,
    /// Unix timestamp of the last judgement, if any.
    #[serde(default)]
    pub last_run_unix: Option<u64>,
    /// The reference solution was applied here: progress is not evidence of the learner's work.
    #[serde(default)]
    pub solution_applied: bool,
    /// A spec file was modified after it was recorded.
    #[serde(default)]
    pub spec_modified: bool,
}

/// How far a step got, as `kiln list` shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// Nothing has passed yet.
    Pending,
    /// The spec tests pass; the gates have not (or not since).
    SpecPassed,
    /// Spec tests and must gates both passed.
    Done,
}

impl StepStatus {
    /// Short marker used in listings.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            StepStatus::Pending => "·",
            StepStatus::SpecPassed => "◐",
            StepStatus::Done => "✓",
        }
    }

    /// Human-readable label used by `kiln caps`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            StepStatus::Pending => "not verified",
            StepStatus::SpecPassed => "spec passes, gates pending",
            StepStatus::Done => "verified",
        }
    }
}

/// Progress of the whole course.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Format version of this file.
    pub format_version: u32,
    /// One entry per step the learner has judged, keyed by qualified step id.
    #[serde(default)]
    pub steps: BTreeMap<String, StepState>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            steps: BTreeMap::new(),
        }
    }
}

impl State {
    /// Path of the progress file below a repository root.
    #[must_use]
    pub fn path(root: &Path) -> PathBuf {
        root.join(STATE_DIR).join(STATE_FILE)
    }

    /// Read progress; a missing file is simply "nothing done yet".
    pub fn load(root: &Path) -> Result<Self> {
        let path = Self::path(root);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        let state: Self = toml::from_str(&text).map_err(|source| KilnError::Metadata {
            path: path.clone(),
            source,
        })?;
        if state.format_version > FORMAT_VERSION {
            return Err(KilnError::Invalid(format!(
                "{}: format_version {} is newer than this kiln understands ({FORMAT_VERSION})",
                path.display(),
                state.format_version
            )));
        }
        Ok(state)
    }

    /// Write progress, creating `.kiln/` if needed.
    pub fn save(&self, root: &Path) -> Result<()> {
        let path = Self::path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let body = toml::to_string_pretty(self).map_err(|source| KilnError::Encode {
            path: path.clone(),
            source,
        })?;
        std::fs::write(&path, format!("{HEADER}{body}"))
            .map_err(|source| KilnError::Write { path, source })
    }

    /// Progress entry of a step, created on first use.
    pub fn entry(&mut self, step_id: &str) -> &mut StepState {
        self.steps.entry(step_id.to_owned()).or_default()
    }

    /// How far a step got.
    #[must_use]
    pub fn status(&self, step_id: &str) -> StepStatus {
        let Some(state) = self.steps.get(step_id) else {
            return StepStatus::Pending;
        };
        match (state.spec_passed, state.gates_passed) {
            (true, true) => StepStatus::Done,
            (true, false) => StepStatus::SpecPassed,
            _ => StepStatus::Pending,
        }
    }

    /// Record the outcome of a spec run.
    pub fn record_spec(&mut self, step_id: &str, passed: bool) {
        let now = unix_now();
        let state = self.entry(step_id);
        state.spec_passed = passed;
        state.gates_passed &= passed;
        state.last_run_unix = Some(now);
    }

    /// Record the outcome of a gate run.
    pub fn record_gates(&mut self, step_id: &str, passed: bool) {
        let now = unix_now();
        let state = self.entry(step_id);
        state.gates_passed = passed;
        state.last_run_unix = Some(now);
    }

    /// Record that the reference solution was applied here (plan §3 D2: recorded, not prevented).
    pub fn record_solution_applied(&mut self, step_id: &str, applied: bool) {
        let now = unix_now();
        let state = self.entry(step_id);
        state.solution_applied = applied;
        state.last_run_unix = Some(now);
    }

    /// Record that a spec file in the project is not the one the course shipped.
    pub fn record_spec_modified(&mut self, step_id: &str, modified: bool) {
        let state = self.entry(step_id);
        state.spec_modified = modified;
    }

    /// Forget what happened on one step: `kiln reset` starts it over.
    ///
    /// Deliberately partial — see [`StepState`] for which half is which. The progress flags are
    /// what the learner re-earns by redoing the step, so they go; the honesty markers are what keeps
    /// `kiln verify` truthful, so they stay.
    pub fn forget_progress(&mut self, step_id: &str) {
        let state = self.entry(step_id);
        state.spec_passed = false;
        state.gates_passed = false;
    }

    /// Number of steps whose spec tests and gates both pass.
    #[must_use]
    pub fn done_count(&self) -> usize {
        self.steps
            .values()
            .filter(|state| state.spec_passed && state.gates_passed)
            .count()
    }
}

/// Seconds since the Unix epoch, or 0 when the clock is before it.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn a_missing_state_file_is_an_empty_state() {
        let temp = TempDir::new("state-missing");
        let state = State::load(temp.path()).unwrap();
        assert_eq!(state, State::default());
        assert_eq!(state.status("mailer/1.1-workspace"), StepStatus::Pending);
    }

    #[test]
    fn progress_round_trips_through_the_file() {
        let temp = TempDir::new("state-round-trip");
        let mut state = State::default();
        state.record_spec("mailer/1.1-workspace", true);
        state.record_gates("mailer/1.1-workspace", true);
        state.save(temp.path()).unwrap();

        let reloaded = State::load(temp.path()).unwrap();
        assert_eq!(reloaded.status("mailer/1.1-workspace"), StepStatus::Done);
        assert_eq!(reloaded.done_count(), 1);
        assert!(
            reloaded.steps["mailer/1.1-workspace"]
                .last_run_unix
                .is_some()
        );

        let written = std::fs::read_to_string(State::path(temp.path())).unwrap();
        assert!(written.starts_with("# Progress recorded by kiln."));
    }

    #[test]
    fn a_failing_spec_run_takes_the_step_back_to_pending() {
        let mut state = State::default();
        state.record_spec("mailer/1.1-workspace", true);
        state.record_gates("mailer/1.1-workspace", true);
        assert_eq!(state.status("mailer/1.1-workspace"), StepStatus::Done);
        state.record_spec("mailer/1.1-workspace", false);
        assert_eq!(state.status("mailer/1.1-workspace"), StepStatus::Pending);
        assert!(!state.steps["mailer/1.1-workspace"].gates_passed);
    }

    #[test]
    fn a_spec_that_passes_without_gates_is_not_done() {
        let mut state = State::default();
        state.record_spec("mailer/1.1-workspace", true);
        assert_eq!(state.status("mailer/1.1-workspace"), StepStatus::SpecPassed);
        assert_eq!(state.done_count(), 0);
    }

    #[test]
    fn a_newer_state_file_is_refused_instead_of_silently_dropped() {
        let temp = TempDir::new("state-future");
        temp.write(".kiln/state.toml", "format_version = 99\n");
        let error = State::load(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("newer than this kiln understands"),
            "{error}"
        );
    }

    #[test]
    fn honesty_markers_survive_a_reset_but_progress_does_not() {
        let mut state = State::default();
        state.record_spec("mailer/1.1-workspace", true);
        state.record_gates("mailer/1.1-workspace", true);
        state.record_solution_applied("mailer/1.1-workspace", true);
        state.record_spec_modified("mailer/1.1-workspace", false);

        state.forget_progress("mailer/1.1-workspace");
        assert_eq!(state.status("mailer/1.1-workspace"), StepStatus::Pending);
        assert!(state.steps["mailer/1.1-workspace"].solution_applied);
        assert_eq!(state.done_count(), 0);
    }
}
