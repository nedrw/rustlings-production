//! Learner progress, `.kiln/state.toml`.
//!
//! Two things live here, and they are deliberately separate: *progress* (did the spec tests and
//! the must gates ever pass?) and *honesty* (was the reference solution applied, was a spec file
//! edited?). Plan §3 D2: the runner does not prevent the second, it records it, because a
//! self-study course that pretends its progress file is evidence would be lying.
//!
//! A third table, `capability`, answers a different question — *what has this learner proven, and
//! when* — and it is what `kiln review --due` (spaced repetition) and `kiln plan --fast` (skip
//! what is already proven) read. It is keyed by capability rather than by step because the ledger
//! and the learner's memory decay on different schedules.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::caps::{CapRequirement, CapabilityId, Level};
use crate::error::{KilnError, Result};
use crate::info::FORMAT_VERSION;

/// Directory below the repository root that holds runner state.
pub const STATE_DIR: &str = ".kiln";
/// File name of the progress file.
pub const STATE_FILE: &str = "state.toml";

/// Header written above the serialized progress so the file explains itself.
const HEADER: &str = "\
# Progress recorded by kiln. Personal state, not course content: it is git-ignored on purpose.
# spec_passed and gates_passed are the record of what actually ran; solution_applied,
# spec_modified and skipped say whether the result is the learner's own work (plan §3 D2).
# [capability] remembers what was proven and when, for `kiln review --due` and `kiln plan --fast`.
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
    /// The learner skipped this step on purpose (`kiln skip`), because every slot it teaches was
    /// proven elsewhere. Skipping is recorded, never passed off as progress (plan §3 D4).
    #[serde(default)]
    pub skipped: bool,
}

/// What the learner has proven about one capability (plan §3 D4, §10: spaced repetition).
///
/// One entry per capability, not per step: the ledger says *where a depth is first established*,
/// while this says *what this learner has shown and when*. Two different facts, and `kiln review
/// --due` exists because the second one decays.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityState {
    /// Deepest depth the learner has been judged at.
    pub level: Level,
    /// Unix timestamp of the last proof, if one was recorded.
    #[serde(default)]
    pub last_verified: Option<u64>,
    /// What proved it: a step id (`mailer/1.14-worker`) or a probe slug (`probe:...`).
    pub evidence: String,
}

impl CapabilityState {
    /// `true` when the proof came from a probe the learner passed, rather than from a step.
    ///
    /// `kiln plan --fast` skips a step only on this kind of evidence: a step that can be passed by
    /// following a scaffold is not proof that the capability survived a month away from it.
    #[must_use]
    pub fn from_probe(&self) -> bool {
        self.evidence.starts_with(EVIDENCE_PROBE_PREFIX)
    }
}

/// Prefix marking probe evidence in [`CapabilityState::evidence`].
pub const EVIDENCE_PROBE_PREFIX: &str = "probe:";

/// How far a step got, as `kiln list` shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// Nothing has passed yet.
    Pending,
    /// The spec tests pass; the gates have not (or not since).
    SpecPassed,
    /// Spec tests and must gates both passed.
    Done,
    /// Skipped on purpose (`kiln skip`), with the capability proven by a probe instead.
    Skipped,
}

impl StepStatus {
    /// Short marker used in listings.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            StepStatus::Pending => "·",
            StepStatus::SpecPassed => "◐",
            StepStatus::Done => "✓",
            StepStatus::Skipped => "»",
        }
    }

    /// Human-readable label used by `kiln caps`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            StepStatus::Pending => "not verified",
            StepStatus::SpecPassed => "spec passes, gates pending",
            StepStatus::Done => "verified",
            StepStatus::Skipped => "skipped (proved by probe)",
        }
    }

    /// `true` when the learner still has work to do here.
    #[must_use]
    pub fn is_unfinished(self) -> bool {
        matches!(self, StepStatus::Pending | StepStatus::SpecPassed)
    }
}

/// How a capability was proven, for the report lines that name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence<'a> {
    /// Judged on a step: the step's id.
    Step(&'a str),
    /// Passed as a probe exercise: the probe's slug.
    Probe(&'a str),
}

impl fmt::Display for Evidence<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Evidence::Step(step) => f.write_str(step),
            Evidence::Probe(slug) => write!(f, "{EVIDENCE_PROBE_PREFIX}{slug}"),
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
    /// One entry per capability the learner has proven, keyed by capability id
    /// (`async/graceful-shutdown`). Written when a step is done and when a probe passes.
    #[serde(default)]
    pub capability: BTreeMap<String, CapabilityState>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            steps: BTreeMap::new(),
            capability: BTreeMap::new(),
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
            _ if state.skipped => StepStatus::Skipped,
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
    /// `kiln verify` truthful, so they stay. A skip is undone too: after a reset the step is real
    /// work again, which is exactly what `kiln reset` is for.
    pub fn forget_progress(&mut self, step_id: &str) {
        let state = self.entry(step_id);
        state.spec_passed = false;
        state.gates_passed = false;
        state.skipped = false;
    }

    /// Record that a step the learner skipped is behind them now.
    pub fn record_skipped(&mut self, step_id: &str, skipped: bool) {
        let state = self.entry(step_id);
        state.skipped = skipped;
    }

    /// What this learner has proven about one capability, if anything.
    #[must_use]
    pub fn capability(&self, id: &CapabilityId) -> Option<&CapabilityState> {
        self.capability.get(id.as_str())
    }

    /// Record that a capability was proven at a depth, and when.
    ///
    /// A deeper proof replaces a shallower one; a shallower one never overwrites a deeper record,
    /// because the record answers "how deep has this learner been judged" and nothing un-judges it.
    /// The timestamp is refreshed on every proof, which is what `kiln review --due` reads.
    pub fn record_capability(&mut self, requirement: &CapRequirement, evidence: Evidence<'_>) {
        let now = unix_now();
        let entry = self
            .capability
            .entry(requirement.id.to_string())
            .or_insert_with(|| CapabilityState {
                level: requirement.level,
                last_verified: None,
                evidence: String::new(),
            });
        if requirement.level.rank() >= entry.level.rank() {
            entry.level = requirement.level;
        }
        entry.last_verified = Some(now);
        entry.evidence = evidence.to_string();
    }

    /// Record every slot a step establishes, with that step as the evidence.
    pub fn record_step_capabilities(&mut self, requirements: &[CapRequirement], step_id: &str) {
        for requirement in requirements {
            self.record_capability(requirement, Evidence::Step(step_id));
        }
    }

    /// Number of steps whose spec tests and gates both pass.
    #[must_use]
    pub fn done_count(&self) -> usize {
        self.steps
            .values()
            .filter(|state| state.spec_passed && state.gates_passed)
            .count()
    }

    /// Number of steps the learner skipped on purpose.
    #[must_use]
    pub fn skipped_count(&self) -> usize {
        self.steps
            .values()
            .filter(|state| !state.spec_passed && !state.gates_passed && state.skipped)
            .count()
    }
}

/// Whole days between two Unix timestamps (later first).
#[must_use]
pub fn days_between(later: u64, earlier: u64) -> u64 {
    later.saturating_sub(earlier) / 86_400
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

    fn capability(text: &str) -> CapRequirement {
        text.parse().expect("a capability requirement")
    }

    #[test]
    fn a_skipped_step_is_not_progress_and_keeps_counting_as_unfinished() {
        let mut state = State::default();
        state.record_skipped("relay/2.1-project-skeleton", true);
        assert_eq!(
            state.status("relay/2.1-project-skeleton"),
            StepStatus::Skipped
        );
        assert_eq!(state.done_count(), 0);
        assert_eq!(state.skipped_count(), 1);
        assert!(!StepStatus::Skipped.is_unfinished());

        // A step that was skipped and then really done stops being a skip.
        state.record_spec("relay/2.1-project-skeleton", true);
        state.record_gates("relay/2.1-project-skeleton", true);
        assert_eq!(state.status("relay/2.1-project-skeleton"), StepStatus::Done);

        // And `kiln reset` puts the work back on the list.
        state.forget_progress("relay/2.1-project-skeleton");
        state.record_skipped("relay/2.1-project-skeleton", true);
        state.forget_progress("relay/2.1-project-skeleton");
        assert_eq!(
            state.status("relay/2.1-project-skeleton"),
            StepStatus::Pending
        );
    }

    #[test]
    fn capability_records_remember_the_deepest_proof_and_its_evidence() {
        let mut state = State::default();
        state.record_capability(
            &capability("db/migrations@apply"),
            Evidence::Step("mailer/1.9"),
        );
        let record = state
            .capability(&CapabilityId::parse("db/migrations").unwrap())
            .expect("a record");
        assert_eq!(record.level, Level::Apply);
        assert_eq!(record.evidence, "mailer/1.9");
        assert!(!record.from_probe());
        assert!(record.last_verified.is_some());

        // A deeper proof raises the level; a shallower one cannot lower it.
        state.record_capability(
            &capability("db/migrations@design"),
            Evidence::Probe("db-migrations.design"),
        );
        let record = state
            .capability(&CapabilityId::parse("db/migrations").unwrap())
            .unwrap();
        assert_eq!(record.level, Level::Design);
        assert!(record.from_probe());
        state.record_capability(&capability("db/migrations@apply"), Evidence::Step("x/1.1"));
        assert_eq!(
            state
                .capability(&CapabilityId::parse("db/migrations").unwrap())
                .unwrap()
                .level,
            Level::Design
        );
    }

    #[test]
    fn capability_records_round_trip_and_older_state_files_still_load() {
        let temp = TempDir::new("state-capability");
        temp.write(
            ".kiln/state.toml",
            "format_version = 1\n\n[steps.\"mailer/1.9-postgres\"]\nspec_passed = true\n",
        );
        let mut state = State::load(temp.path()).expect("a state file without capability records");
        assert_eq!(state.status("mailer/1.9-postgres"), StepStatus::SpecPassed);

        state.record_step_capabilities(&[capability("db/migrations@apply")], "mailer/1.9-postgres");
        state.save(temp.path()).unwrap();
        let reloaded = State::load(temp.path()).unwrap();
        let record = reloaded
            .capability(&CapabilityId::parse("db/migrations").unwrap())
            .expect("the record survived the file");
        assert_eq!(record.evidence, "mailer/1.9-postgres");
    }
}
