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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::caps::{CapRequirement, CapabilityId, Level};
use crate::error::{KilnError, Result};
use crate::info::FORMAT_VERSION;
use crate::scratch::{claim_within, contention};

/// Directory below the repository root that holds runner state.
pub const STATE_DIR: &str = ".kiln";
/// File name of the progress file.
pub const STATE_FILE: &str = "state.toml";
/// Lock file held while a save merges and renames (see [`State::save`]).
const STATE_LOCK: &str = "state.lock";

/// How long a save waits for another save's lock.
///
/// A save is a read, a merge and a rename — milliseconds — so five seconds means the holder is
/// wedged, not working.
const STATE_LOCK_WAIT: Duration = Duration::from_secs(5);

/// How old a save's lock file has to be before its owner counts as gone.
///
/// A gate's claim waits out a whole build before taking a dead owner's file over; a save holds its
/// lock for milliseconds, so a minute here is already a generous multiple, and a kiln run killed
/// mid-save does not block the next command for long.
const STATE_LOCK_STALE: Duration = Duration::from_secs(60);

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

/// What this session changed, so a save can write only that (see [`State::save`]).
///
/// Every mutating method marks the entry it touched; entries the session never touched are not its
/// business, and a save leaves the file's own version of them in place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Touched {
    /// Step ids this session recorded something for.
    steps: BTreeSet<String>,
    /// Capability ids this session recorded a proof for.
    capabilities: BTreeSet<String>,
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
    /// Entries this session touched. Not part of the file: it exists so the save can merge
    /// ([`State::save`]) instead of writing the whole in-memory copy over whatever is on disk.
    #[serde(skip)]
    touched: Touched,
}

impl Default for State {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            steps: BTreeMap::new(),
            capability: BTreeMap::new(),
            touched: Touched::default(),
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
    ///
    /// Only a *missing* file means that. Any other read failure is an error, never a fresh start:
    /// the state file is the learner's whole history and nothing else records it, so a file that
    /// cannot be read (permissions, encoding) has to stop the command rather than be answered with
    /// an empty state that the next save would write over the real one.
    pub fn load(root: &Path) -> Result<Self> {
        let path = Self::path(root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(KilnError::Read { path, source }),
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
    ///
    /// Two properties, both there because the file is the learner's whole history:
    ///
    /// * The write is *atomic*: the content goes to a temporary file that is renamed over this one,
    ///   so a crash cannot leave a half-written file that every later command refuses to load.
    /// * The write *merges*: only the entries this session touched ([`State::entry`] and the
    ///   `record_*` methods mark them) are taken from memory, over whatever the file holds now. A
    ///   long-lived `kiln watch` and a command beside it can therefore each record their steps
    ///   without the second save erasing the first; entries this session never touched are written
    ///   back as the file has them, not as this session's stale copy of them. The merge is per
    ///   entry, not per field: two sessions that both touched the *same* step are last-writer-wins
    ///   on that entry, which is the price of not locking the file for a whole session.
    ///
    /// The merge and the rename happen under `.kiln/state.lock`, so two saves cannot interleave.
    /// A crash mid-write can still strand the temporary file (nothing runs on the way down), so
    /// each save, already holding the lock, sweeps any temporary files earlier crashes left.
    pub fn save(&self, root: &Path) -> Result<()> {
        let path = Self::path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let lock = root.join(STATE_DIR).join(STATE_LOCK);
        let Some(_claim) = claim_within(&lock, STATE_LOCK_WAIT, STATE_LOCK_STALE)? else {
            return Err(KilnError::Busy {
                message: contention(&lock),
            });
        };
        // The claim means no other save is in flight, so any temporary file still sitting here was
        // stranded by a crash; nothing reads them, but a directory that fills with near-copies of
        // the progress file is a directory a person has to puzzle out.
        if let Ok(entries) = std::fs::read_dir(root.join(STATE_DIR)) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name
                    .to_string_lossy()
                    .starts_with(&format!("{STATE_FILE}.tmp-"))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let mut merged = Self::load(root)?;
        for (id, entry) in &self.steps {
            if self.touched.steps.contains(id) {
                merged.steps.insert(id.clone(), entry.clone());
            }
        }
        for (id, record) in &self.capability {
            if self.touched.capabilities.contains(id) {
                merged.capability.insert(id.clone(), record.clone());
            }
        }
        let body = toml::to_string_pretty(&merged).map_err(|source| KilnError::Encode {
            path: path.clone(),
            source,
        })?;
        write_atomically(&path, &format!("{HEADER}{body}"))
    }

    /// Progress entry of a step, created on first use.
    ///
    /// Calling this marks the step as touched: the save writes what the caller goes on to put in
    /// the entry, even when that is *less* than what the file had (a reset), which is why reading
    /// an entry goes through [`State::status`] and the getters instead.
    pub fn entry(&mut self, step_id: &str) -> &mut StepState {
        self.touched.steps.insert(step_id.to_owned());
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
        self.touched.capabilities.insert(requirement.id.to_string());
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

/// Write `contents` to `path` by writing a temporary file beside it and renaming it over `path`.
///
/// A plain `fs::write` truncates first: a crash between truncate and write leaves a half file that
/// no later command can parse. The rename is atomic, so a reader sees either the whole previous
/// file or the whole new one, and the temporary file is flushed before it is renamed so the bytes
/// are on the disk the name arrives.
fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    let temp = path.with_file_name(format!(
        "{}.tmp-{}",
        path.file_name().map_or_else(
            || STATE_FILE.to_owned(),
            |name| name.to_string_lossy().into_owned()
        ),
        std::process::id()
    ));
    let result = write_through(&temp, contents).and_then(|()| {
        std::fs::rename(&temp, path).map_err(|source| KilnError::Write {
            path: path.to_path_buf(),
            source,
        })
    });
    if result.is_err() {
        // A leftover temporary file is worse than none: it is a partial state file waiting to be
        // confused with the real one by anything that globs the directory.
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Create `path` with `contents`, flushed.
fn write_through(path: &Path, contents: &str) -> Result<()> {
    let mut file = std::fs::File::create(path).map_err(|source| KilnError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(contents.as_bytes())
        .map_err(|source| KilnError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.sync_all().map_err(|source| KilnError::Write {
        path: path.to_path_buf(),
        source,
    })
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
    fn a_state_file_that_cannot_be_read_is_an_error_not_a_fresh_start() {
        // Bytes that are not UTF-8 stand in for every read failure that is not a missing file
        // (permissions, a hardware hiccup): the old behaviour answered all of them with an empty
        // state, and the next save then wrote that emptiness over the learner's whole history.
        let temp = TempDir::new("state-unreadable");
        std::fs::create_dir_all(temp.path().join(STATE_DIR)).unwrap();
        std::fs::write(State::path(temp.path()), [0xFF_u8, 0xFE, 0x00]).unwrap();
        let error = State::load(temp.path()).unwrap_err();
        assert!(matches!(error, KilnError::Read { .. }), "{error}");

        // Saving refuses for the same reason: the merge reads what is on the file first, and a
        // file that cannot be read is never overwritten with something that does not contain it.
        let mut state = State::default();
        state.record_spec("mailer/1.1-workspace", true);
        let error = state.save(temp.path()).unwrap_err();
        assert!(matches!(error, KilnError::Read { .. }), "{error}");
    }

    #[test]
    fn a_save_from_another_session_does_not_erase_what_it_did_not_see() {
        // The shape of `kiln watch` beside an interactive command: two sessions load the same
        // empty state, each records its own step, and both records have to survive. Writing the
        // whole in-memory copy — the old save — would make the second save erase the first.
        let temp = TempDir::new("state-merge");
        let mut watch = State::load(temp.path()).unwrap();
        let mut command = State::load(temp.path()).unwrap();

        watch.record_spec("mailer/1.1-workspace", true);
        watch.record_gates("mailer/1.1-workspace", true);
        watch.save(temp.path()).unwrap();

        command.record_spec("mailer/1.2-runbook", true);
        command.save(temp.path()).unwrap();

        let reloaded = State::load(temp.path()).unwrap();
        assert_eq!(reloaded.status("mailer/1.1-workspace"), StepStatus::Done);
        assert_eq!(
            reloaded.status("mailer/1.2-runbook"),
            StepStatus::SpecPassed
        );
    }

    #[test]
    fn a_reset_survives_the_merge_because_the_entry_it_forgot_is_touched() {
        // The other side of the merge: the entry this session *did* touch has to be written even
        // when what it now says is less than what the file had. A reset that left the file's own
        // copy in place would un-reset nothing.
        let temp = TempDir::new("state-reset-merge");
        temp.write(
            ".kiln/state.toml",
            "format_version = 1\n\n[steps.\"mailer/1.1-workspace\"]\nspec_passed = true\n\
             gates_passed = true\n",
        );
        let mut state = State::load(temp.path()).unwrap();
        assert_eq!(state.status("mailer/1.1-workspace"), StepStatus::Done);
        state.forget_progress("mailer/1.1-workspace");
        state.save(temp.path()).unwrap();

        let reloaded = State::load(temp.path()).unwrap();
        assert_eq!(reloaded.status("mailer/1.1-workspace"), StepStatus::Pending);
    }

    #[test]
    fn a_save_leaves_no_lock_or_temporary_file_behind() {
        let temp = TempDir::new("state-clean-dir");
        let mut state = State::default();
        state.record_spec("mailer/1.1-workspace", true);
        state.save(temp.path()).unwrap();

        let names: Vec<String> = std::fs::read_dir(temp.path().join(STATE_DIR))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![STATE_FILE.to_owned()],
            "the lock went with the save's end and the temporary file was renamed away: {names:?}"
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
