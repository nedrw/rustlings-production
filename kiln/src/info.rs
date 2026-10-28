//! Course metadata: the manifest, the tracks and their steps.
//!
//! The files under `course/` are the single source of truth (plan §3 D3): the runner can be
//! replaced by `xtask`, an editor plugin or a web front end without the course content moving.
//! This module reads those files, enforces the conventions the linters and gates rely on, and
//! resolves step references. It never writes.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::caps::{CapRequirement, Ledger};
use crate::error::{KilnError, Result};
use crate::gates::GateSpec;

/// Metadata format version understood by this build of `kiln`.
pub const FORMAT_VERSION: u32 = 1;
/// Directory below the repository root holding every metadata file.
pub const COURSE_DIR: &str = "course";
/// File name of the course manifest.
pub const MANIFEST_FILE: &str = "course.toml";
/// File name of the capability ledger.
pub const LEDGER_FILE: &str = "capabilities.toml";
/// Directory below the course directory holding one directory per track.
pub const TRACKS_DIR: &str = "tracks";
/// File name of a track's metadata.
pub const TRACK_FILE: &str = "track.toml";

/// The course manifest, `course/course.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Metadata format version.
    pub format_version: u32,
    /// Locale holding the source text of every step; other locales fall back to it.
    pub default_locale: String,
    /// Track a bare `kiln` invocation continues.
    pub default_track: String,
    /// Message printed before the first step of a track.
    pub welcome: String,
    /// Message printed when the last step of a track is done.
    pub final_message: String,
}

/// How a step is worked on and judged (plan §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    /// Real code inside `app/<track>`, judged by spec tests plus gates (the main line).
    Milestone,
    /// A single-file exercise compiled in a throwaway crate (mechanism training).
    Drill,
    /// A document (runbook, decision record) judged by structure and links.
    Design,
}

impl StepKind {
    /// Lower-case name as it appears in metadata.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            StepKind::Milestone => "milestone",
            StepKind::Drill => "drill",
            StepKind::Design => "design",
        }
    }
}

impl fmt::Display for StepKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the step's prose may assume the learner already saw (plan §3 D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// First time this capability is taught at this depth: WHY/TASK/DONE WHEN/TRAP/FURTHER.
    Teach,
    /// Same capability, deeper: RECALL/DELTA, no WHY section.
    Deepen,
    /// Same capability, same depth, another project: RECALL/DELTA, stricter gates.
    Transfer,
}

impl Origin {
    /// Lower-case name as it appears in metadata.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Teach => "teach",
            Origin::Deepen => "deepen",
            Origin::Transfer => "transfer",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The command that judges a step (`run.cmd`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCmd {
    /// Command line, run in the track's learner project.
    ///
    /// Split on whitespace and executed without a shell: the course must not depend on GNU
    /// userland, so quoting and pipelines are deliberately impossible here.
    pub cmd: String,
    /// Seconds this step's commands may take, when the runner's default is not enough.
    ///
    /// A floor rather than a cap: `kiln --timeout` still raises it, and it applies to the step's
    /// gates as well as to its spec run. The step that needs it is the one whose command builds
    /// something — a container image with a dependency tree inside it is minutes on a machine that
    /// has never built one, and a deadline that kills the build reports "timed out" about a step
    /// that was working.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// One step, as declared in `track.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Local id, `<phase>.<index>-<slug>` (the fully qualified id adds the track).
    pub id: String,
    /// Human-readable title, used by `kiln list` and the step body.
    pub title: String,
    /// How the step is worked on and judged.
    pub kind: StepKind,
    /// What the prose may assume.
    pub origin: Origin,
    /// Step body in the source locale, relative to the course directory
    /// (`docs/steps/en/mailer/1.14-worker.md`). The i18n layer rewrites the locale segment.
    pub entry: String,
    /// Difficulty hint, `<minutes>m`. Not an acceptance criterion.
    pub time_hint: String,
    /// Up to three new concepts the step introduces (authoring.md §3.1).
    #[serde(default)]
    pub concepts: Vec<String>,
    /// Capability slots this step establishes; must match the ledger exactly.
    pub teaches: Vec<CapRequirement>,
    /// Slots this step builds on; each must be taught earlier.
    #[serde(default)]
    pub requires: Vec<CapRequirement>,
    /// Command that runs the spec tests.
    #[serde(default)]
    pub run: Option<RunCmd>,
    /// Read-only spec files, relative to the track directory.
    #[serde(default)]
    pub spec: Vec<String>,
    /// Scaffold patch, relative to the track directory.
    #[serde(default)]
    pub scaffold: Option<String>,
    /// Reference solution patch, relative to the track directory.
    #[serde(default)]
    pub solution: Option<String>,
    /// Gates that judge this step.
    #[serde(default)]
    pub gates: Vec<GateSpec>,
    /// One-line nudge; deepen/transfer steps only give the direction of the delta.
    #[serde(default)]
    pub hint: Option<String>,
    /// Container images this step needs, before any registry mirror is applied.
    #[serde(default)]
    pub images: Vec<String>,
    /// Deliberate re-teach of a capability at a depth it already reached (plan §3 D4 rule 2).
    #[serde(default)]
    pub reteach: Option<String>,
    /// Track this step belongs to; filled by the loader.
    #[serde(skip)]
    pub track: String,
    /// Phase index (`1.14`), parsed from the id by the loader — the key the ledger uses.
    #[serde(skip)]
    pub number: String,
}

impl Step {
    /// Fully qualified id used by the CLI and the docs, `<track>/<local id>`.
    #[must_use]
    pub fn qualified_id(&self) -> String {
        format!("{}/{}", self.track, self.id)
    }

    /// Phase index, e.g. `1.14`.
    #[must_use]
    pub fn number(&self) -> &str {
        &self.number
    }

    /// Absolute path of this step's body in the source locale.
    #[must_use]
    pub fn source_path(&self, course_dir: &Path) -> PathBuf {
        course_dir.join(&self.entry)
    }

    /// `true` if the reference names this step by local id or by phase index.
    #[must_use]
    pub fn answers_to(&self, reference: &str) -> bool {
        reference == self.id || reference == self.number
    }

    /// Must gates of this step, in declaration order.
    pub fn must_gates(&self) -> impl Iterator<Item = &GateSpec> {
        self.gates
            .iter()
            .filter(|gate| gate.tier == crate::gates::Tier::Must && !gate.slow)
    }

    /// How long this step's commands — its spec run and its gates — may take.
    ///
    /// The larger of what the step declares and what the runner was given: a declaration is about
    /// the artifact (building an image is minutes on a machine that has never built one), and
    /// `kiln --timeout` stays the way to raise it further. A step that declares nothing uses the
    /// runner's deadline exactly, which is every step but the one that builds a container image.
    #[must_use]
    pub fn deadline(&self, runner: Duration) -> Duration {
        let declared = self
            .run
            .as_ref()
            .and_then(|run| run.timeout)
            .map(Duration::from_secs);
        declared.map_or(runner, |declared| declared.max(runner))
    }
}

/// A track's metadata, `course/tracks/<id>/track.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    /// Metadata format version.
    pub format_version: u32,
    /// Track id, equal to the directory name (`mailer`).
    pub id: String,
    /// Position in the learning path (1 for T1), used to order tracks and to label them.
    pub index: u32,
    /// Human-readable title.
    pub title: String,
    /// The track's name in other locales, keyed by locale tag (`"zh-Hans"`).
    ///
    /// Step titles are translated in each body's front matter; a track has no body, so its name
    /// lives here. A missing entry means "use `title`", never an error.
    #[serde(default)]
    pub localized_titles: BTreeMap<String, String>,
    /// Learner workspace for this track, relative to the repository root (`app/mailer`).
    pub project: String,
    /// The track whose project this one builds on, if any: its steps declare a `path` dependency
    /// into that project (plan §3 D6 — T5 reusing T3's engine crate is the one cross-track
    /// dependency the course allows). The runner treats the required project as a prerequisite:
    /// `run`/`check` refuse with the reason when it is not there, and a sandbox lays its reference
    /// tree before judging this track's steps.
    #[serde(default)]
    pub requires_track: Option<String>,
    /// Capabilities this track must have established by its last step (rule R6).
    #[serde(default)]
    pub exit: Vec<CapRequirement>,
    /// The steps, in order.
    pub step: Vec<Step>,
}

impl Track {
    /// The track's name in `locale`, or the source title when it has none.
    #[must_use]
    pub fn title_in(&self, locale: &str) -> &str {
        self.localized_titles
            .get(locale)
            .map_or(self.title.as_str(), String::as_str)
    }

    /// Directory of this track's metadata, `course/tracks/<id>`.
    #[must_use]
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(COURSE_DIR).join(TRACKS_DIR).join(&self.id)
    }

    /// Directory of the learner's workspace for this track.
    #[must_use]
    pub fn project_dir(&self, root: &Path) -> PathBuf {
        root.join(&self.project)
    }

    /// Look up a step by local id (`1.14-worker`), by phase index (`1.14`) or by either with
    /// the track prefix (`mailer/1.14`).
    pub fn step(&self, reference: &str) -> Result<&Step> {
        let local = reference
            .split_once('/')
            .filter(|(track, _)| *track == self.id)
            .map_or(reference, |(_, local)| local);
        if local.trim().is_empty() {
            return Err(KilnError::NoStepGiven);
        }
        let matches: Vec<&Step> = self
            .step
            .iter()
            .filter(|step| step.answers_to(local))
            .collect();
        match matches.as_slice() {
            [] => Err(KilnError::UnknownStep {
                requested: format!("{}/{reference}", self.id),
                suggestion: suggestion(self.step.iter(), local),
            }),
            [one] => Ok(one),
            many => Err(KilnError::AmbiguousStep {
                requested: format!("{}/{reference}", self.id),
                matches: many.iter().map(|step| step.qualified_id()).collect(),
            }),
        }
    }

    /// Look up a step by the phase index the capability ledger uses.
    pub fn step_by_number(&self, number: &str) -> Result<&Step> {
        self.step(number)
    }

    /// `T1`-style label used in listings.
    #[must_use]
    pub fn label(&self) -> String {
        format!("T{}", self.index)
    }

    /// Validate the conventions every other rule depends on, and fill in the derived fields.
    fn normalize(&mut self, directory: &str) -> Result<()> {
        if self.format_version != FORMAT_VERSION {
            return Err(KilnError::Invalid(format!(
                "{}: unsupported format_version {}, expected {FORMAT_VERSION}",
                self.id, self.format_version
            )));
        }
        if self.id != directory {
            return Err(KilnError::Invalid(format!(
                "track id `{}` does not match its directory `{directory}`",
                self.id
            )));
        }
        for step in &mut self.step {
            let (number, slug) = step.id.split_once('-').ok_or_else(|| KilnError::StepId {
                id: step.id.clone(),
                track: self.id.clone(),
                reason: "missing the `<phase>.<index>-<slug>` separator".into(),
            })?;
            let well_formed = matches!(number.split_once('.'), Some((phase, index))
                if !phase.is_empty()
                    && !index.is_empty()
                    && phase.bytes().all(|b| b.is_ascii_digit())
                    && index.bytes().all(|b| b.is_ascii_digit()))
                && !slug.is_empty()
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !well_formed {
                return Err(KilnError::StepId {
                    id: step.id.clone(),
                    track: self.id.clone(),
                    reason: "expected `<phase>.<index>-<slug>`, e.g. `1.14-worker`".into(),
                });
            }
            if !matches!(step.time_hint.split_once('m'), Some((minutes, rest))
                if rest.is_empty() && !minutes.is_empty() && minutes.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(KilnError::Invalid(format!(
                    "{}/{}: time_hint `{}` must look like `90m`",
                    self.id, step.id, step.time_hint
                )));
            }
            if !step.entry.starts_with("docs/steps/") || step.entry.split('/').count() < 5 {
                return Err(KilnError::Invalid(format!(
                    "{}/{}: entry `{}` must look like `docs/steps/<locale>/<track>/<file>.md`",
                    self.id, step.id, step.entry
                )));
            }
            if step.teaches.is_empty() {
                return Err(KilnError::Invalid(format!(
                    "{}/{}: teaches nothing, so the step has no evidence to bind to",
                    self.id, step.id
                )));
            }
            for pair in [&step.teaches, &step.requires] {
                let mut seen = std::collections::BTreeSet::new();
                for requirement in pair.iter() {
                    if !seen.insert(requirement) {
                        return Err(KilnError::Invalid(format!(
                            "{}/{}: `{requirement}` is declared twice",
                            self.id, step.id
                        )));
                    }
                }
            }
            step.track.clone_from(&self.id);
            step.number = number.to_owned();
        }

        let mut numbers = std::collections::BTreeSet::new();
        for step in &self.step {
            if !numbers.insert(step.number.as_str()) {
                return Err(KilnError::Invalid(format!(
                    "{}: two steps claim phase index {}",
                    self.id, step.number
                )));
            }
        }
        Ok(())
    }
}

/// The loaded course: manifest, ledger and every authored track.
#[derive(Debug)]
pub struct Course {
    root: PathBuf,
    manifest: Manifest,
    ledger: Ledger,
    tracks: Vec<Track>,
}

impl Course {
    /// Walk up from `start` until a directory containing `course/course.toml` is found.
    pub fn discover(start: &Path) -> Result<Self> {
        let mut current = Some(start);
        while let Some(directory) = current {
            if directory.join(COURSE_DIR).join(MANIFEST_FILE).is_file() {
                return Self::load(directory);
            }
            current = directory.parent();
        }
        Err(KilnError::NoCourse {
            start: start.to_path_buf(),
        })
    }

    /// Load the course whose repository root is `root`.
    pub fn load(root: &Path) -> Result<Self> {
        let course_dir = root.join(COURSE_DIR);
        let manifest = read_toml::<Manifest>(&course_dir.join(MANIFEST_FILE))?;
        if manifest.format_version != FORMAT_VERSION {
            return Err(KilnError::Invalid(format!(
                "{MANIFEST_FILE}: unsupported format_version {}, expected {FORMAT_VERSION}",
                manifest.format_version
            )));
        }
        let ledger = Ledger::load(&course_dir.join(LEDGER_FILE))?;
        let tracks = load_tracks(&course_dir)?;
        if !tracks
            .iter()
            .any(|track| track.id == manifest.default_track)
        {
            return Err(KilnError::Invalid(format!(
                "{MANIFEST_FILE}: default_track `{}` has no track metadata",
                manifest.default_track
            )));
        }
        check_track_dependencies(&tracks)?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            ledger,
            tracks,
        })
    }

    /// Repository root the course was loaded from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding the metadata files.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.root.join(COURSE_DIR)
    }

    /// The manifest.
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The capability ledger.
    #[must_use]
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// Every authored track, in path order.
    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The top-level directories the tracks keep their learner projects in (`app`, today).
    ///
    /// A learner project is a *working copy*, not course material: it is rewritten by `kiln run` and
    /// `kiln reset`, it is where a learner is mid-step with placeholders on purpose, and it is
    /// usually not under version control at all. Anything that judges the repository's own files
    /// (the repository-wide `links` report, `cargo xtask ci`) walks past these, so its answer does
    /// not depend on how far the person running it happens to have got.
    #[must_use]
    pub fn project_roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self
            .tracks
            .iter()
            .filter_map(|track| track.project.split('/').next())
            .filter(|component| !component.is_empty())
            .map(str::to_owned)
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    /// Look up a track by id.
    pub fn track(&self, id: &str) -> Result<&Track> {
        self.tracks
            .iter()
            .find(|track| track.id == id)
            .ok_or_else(|| self.unknown_track(id))
    }

    /// The track a bare `kiln` invocation continues.
    pub fn default_track(&self) -> Result<&Track> {
        self.track(&self.manifest.default_track)
    }

    /// Every authored step, in track then step order.
    pub fn steps(&self) -> impl Iterator<Item = &Step> {
        self.tracks.iter().flat_map(|track| track.step.iter())
    }

    /// Resolve a step reference: `1.14`, `1.14-worker`, `mailer/1.14`, `mailer/1.14-worker`.
    ///
    /// A bare reference must be unambiguous across the whole course — the prefix form is what
    /// removes the ambiguity once five tracks exist.
    pub fn step(&self, reference: &str) -> Result<&Step> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Err(KilnError::NoStepGiven);
        }
        if let Some((track_id, local)) = reference.split_once('/') {
            return self.track(track_id)?.step(local);
        }
        let matches: Vec<&Step> = self
            .steps()
            .filter(|step| step.answers_to(reference))
            .collect();
        match matches.as_slice() {
            [] => Err(KilnError::UnknownStep {
                requested: reference.to_owned(),
                suggestion: suggestion(self.steps(), reference),
            }),
            [one] => Ok(one),
            many => Err(KilnError::AmbiguousStep {
                requested: reference.to_owned(),
                matches: many.iter().map(|step| step.qualified_id()).collect(),
            }),
        }
    }

    /// The error for a track id nobody authored: a track the ledger plans says so, a typo does not.
    ///
    /// `kiln run relay/2.1` and `kiln list relay` have to answer "T2 is planned, nothing to judge
    /// yet" — the difference between `EXIT_NOT_AUTHORED` and `EXIT_ERROR` is the whole point of the
    /// exit-code contract, and before this only a track whose *metadata* existed could report it.
    fn unknown_track(&self, id: &str) -> KilnError {
        if let Some((track, _)) = id.split_once('/')
            && self.tracks.iter().any(|known| known.id == track)
        {
            return KilnError::StepWhereTrackIsExpected {
                given: id.to_owned(),
                track: track.to_owned(),
            };
        }
        if self
            .ledger
            .positions()
            .iter()
            .any(|(planned, _)| *planned == id)
        {
            return KilnError::NotAuthored {
                what: format!("track `{id}`"),
            };
        }
        KilnError::UnknownTrack(id.to_owned())
    }
}

/// Read one metadata file, attaching its path to any failure.
fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|source| KilnError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| KilnError::Metadata {
        path: path.to_path_buf(),
        source,
    })
}

/// Load every `course/tracks/*/track.toml`, ordered by path position.
/// Every `requires_track` names a track that exists, is not the track itself, and is not part of
/// a cycle — a dependency the runner cannot lay is a course that cannot start.
fn check_track_dependencies(tracks: &[Track]) -> Result<()> {
    for track in tracks {
        let Some(required) = &track.requires_track else {
            continue;
        };
        let Some(target) = tracks.iter().find(|other| &other.id == required) else {
            return Err(KilnError::Invalid(format!(
                "{}.toml: requires_track `{required}` has no track metadata",
                track.id
            )));
        };
        if target.id == track.id {
            return Err(KilnError::Invalid(format!(
                "{}.toml: requires_track names the track itself",
                track.id
            )));
        }
        // Follow the chain: with every link pointing at another existing track, the only way it
        // does not terminate is a loop, and any loop passes through this track again.
        let mut seen = std::iter::once(&track.id).collect::<std::collections::BTreeSet<_>>();
        let mut hop = target;
        while let Some(next_id) = &hop.requires_track {
            if !seen.insert(next_id) {
                return Err(KilnError::Invalid(format!(
                    "{}.toml: requires_track forms a cycle through `{}`",
                    track.id, next_id
                )));
            }
            let Some(next) = tracks.iter().find(|other| &other.id == next_id) else {
                break;
            };
            hop = next;
        }
    }
    Ok(())
}

fn load_tracks(course_dir: &Path) -> Result<Vec<Track>> {
    let tracks_dir = course_dir.join(TRACKS_DIR);
    let entries = std::fs::read_dir(&tracks_dir).map_err(|source| KilnError::Read {
        path: tracks_dir.clone(),
        source,
    })?;

    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| KilnError::Read {
            path: tracks_dir.clone(),
            source,
        })?;
        if entry.path().is_dir() {
            directories.push(entry.path());
        }
    }
    directories.sort();

    let mut tracks = Vec::with_capacity(directories.len());
    for directory in directories {
        let file = directory.join(TRACK_FILE);
        if !file.is_file() {
            return Err(KilnError::Invalid(format!(
                "{} has no {TRACK_FILE}",
                directory.display()
            )));
        }
        let mut track: Track = read_toml(&file)?;
        let name = directory
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        track.normalize(&name)?;
        tracks.push(track);
    }

    let mut indexes = std::collections::BTreeMap::new();
    for track in &tracks {
        if let Some(other) = indexes.insert(track.index, track.id.clone()) {
            return Err(KilnError::Invalid(format!(
                "tracks `{other}` and `{}` both claim index {}",
                track.id, track.index
            )));
        }
    }
    tracks.sort_by_key(|track| track.index);
    Ok(tracks)
}

/// " (closest: …)" hint for an unknown step reference, or an empty string.
fn suggestion<'a>(steps: impl Iterator<Item = &'a Step>, target: &str) -> String {
    nearest_step(steps, target)
        .map(|step| format!(" (closest: `{}`)", step.qualified_id()))
        .unwrap_or_default()
}

/// The step whose id or phase index is closest to `target`, if any is close enough.
fn nearest_step<'a>(steps: impl Iterator<Item = &'a Step>, target: &str) -> Option<&'a Step> {
    let threshold = (target.chars().count() / 3).max(2);
    let mut best: Option<(usize, &Step)> = None;
    for step in steps {
        let distance = edit_distance(&step.id, target).min(edit_distance(&step.number, target));
        if distance <= threshold && best.as_ref().is_none_or(|(best, _)| distance < *best) {
            best = Some((distance, step));
        }
    }
    best.map(|(_, step)| step)
}

/// Levenshtein distance, used only for "did you mean" hints.
fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.chars().enumerate() {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(row + 1);
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous.get(column).copied().unwrap_or_default()
                + usize::from(left_char != *right_char);
            let deletion = previous.get(column + 1).copied().unwrap_or_default() + 1;
            let insertion = current.get(column).copied().unwrap_or_default() + 1;
            current.push(deletion.min(insertion).min(substitution));
        }
        previous = current;
    }
    previous.get(right.len()).copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACK: &str = r#"
        format_version = 1
        id = "mailer"
        index = 1
        title = "Mailer"
        project = "app/mailer"
        exit = ["err/typed-errors@apply"]

        [[step]]
        id = "1.4-config"
        title = "Typed configuration"
        kind = "milestone"
        origin = "teach"
        entry = "docs/steps/en/mailer/1.4-config.md"
        time_hint = "90m"
        teaches = ["cfg/layered-config@apply"]
        run = { cmd = "cargo test -p mailer --test spec_config" }
        spec = ["spec/1.4/config.rs"]
        gates = ["fmt", "clippy@1"]

        [[step]]
        id = "1.5-errors"
        title = "Errors as part of the API"
        kind = "milestone"
        origin = "teach"
        entry = "docs/steps/en/mailer/1.5-errors.md"
        time_hint = "60m"
        teaches = ["err/typed-errors@apply"]
        requires = ["cfg/layered-config@apply"]
        run = { cmd = "cargo test -p mailer --test spec_errors" }
        spec = ["spec/1.5/errors.rs"]
        gates = ["fmt"]
    "#;

    fn track() -> Track {
        let mut track: Track = toml::from_str(TRACK).unwrap();
        track.normalize("mailer").unwrap();
        track
    }

    #[test]
    fn the_loader_fills_track_and_phase_index() {
        let track = track();
        let step = track.step("1.5").unwrap();
        assert_eq!(step.track, "mailer");
        assert_eq!(step.number(), "1.5");
        assert_eq!(step.qualified_id(), "mailer/1.5-errors");
        assert_eq!(step.requires.len(), 1);
        assert_eq!(step.gates.len(), 1);
    }

    #[test]
    fn steps_answer_to_both_number_and_id() {
        let track = track();
        assert_eq!(track.step("1.4").unwrap().id, "1.4-config");
        assert_eq!(track.step("1.4-config").unwrap().number(), "1.4");
        assert_eq!(track.step("mailer/1.4").unwrap().number(), "1.4");
    }

    #[test]
    fn an_unknown_step_suggests_the_closest_one() {
        let track = track();
        let error = track.step("1.6").unwrap_err();
        assert!(
            error.to_string().contains("closest: `mailer/1.4-config`"),
            "{error}"
        );
    }

    #[test]
    fn a_step_id_that_breaks_the_convention_is_rejected() {
        let mut track: Track = toml::from_str(TRACK).unwrap();
        track.step.truncate(1);
        track.step[0].id = "config".into();
        let error = track.normalize("mailer").unwrap_err();
        assert!(matches!(error, KilnError::StepId { .. }), "{error}");
    }

    #[test]
    fn a_track_that_does_not_match_its_directory_is_rejected() {
        let mut track: Track = toml::from_str(TRACK).unwrap();
        let error = track.normalize("relay").unwrap_err();
        assert!(error.to_string().contains("does not match its directory"));
    }

    #[test]
    fn an_entry_outside_the_locale_layout_is_rejected() {
        let mut track: Track = toml::from_str(TRACK).unwrap();
        track.step[0].entry = "mailer/1.4-config.md".into();
        let error = track.normalize("mailer").unwrap_err();
        assert!(error.to_string().contains("docs/steps/<locale>"), "{error}");
    }

    #[test]
    fn a_duplicated_requirement_is_rejected() {
        let mut track: Track = toml::from_str(TRACK).unwrap();
        let duplicate = track.step[0].teaches[0].clone();
        track.step[0].teaches.push(duplicate);
        let error = track.normalize("mailer").unwrap_err();
        assert!(error.to_string().contains("declared twice"), "{error}");
    }

    #[test]
    fn a_step_without_teaches_is_rejected() {
        let mut track: Track = toml::from_str(TRACK).unwrap();
        track.step[0].teaches.clear();
        let error = track.normalize("mailer").unwrap_err();
        assert!(error.to_string().contains("teaches nothing"), "{error}");
    }

    #[test]
    fn two_steps_cannot_share_a_phase_index() {
        let mut track: Track = toml::from_str(TRACK).unwrap();
        track.step[1].id = "1.4-errors".into();
        let error = track.normalize("mailer").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("two steps claim phase index 1.4"),
            "{error}"
        );
    }

    #[test]
    fn unknown_metadata_fields_are_rejected() {
        let text = format!("{TRACK}\n        extra = 1\n");
        assert!(toml::from_str::<Track>(&text).is_err());
    }

    #[test]
    fn edit_distance_matches_the_classic_examples() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }

    /// The repository's own metadata is the first thing every command loads, so a broken
    /// `course/` must fail here rather than in a learner's terminal.
    #[test]
    fn the_repository_course_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("kiln lives one directory below the repository root");
        let course = Course::load(root).unwrap();

        assert_eq!(course.manifest().default_track, "mailer");
        assert_eq!(course.tracks().len(), 7, "T1-T7 have metadata");

        let relay = course.track("relay").unwrap();
        assert_eq!(relay.step.len(), 24);
        assert_eq!(relay.project, "app/relay");

        let jobq = course.track("jobq").unwrap();
        assert_eq!(jobq.step.len(), 27);
        assert_eq!(jobq.project, "app/jobq");
        assert_eq!(jobq.index, 3, "the path order is T1, T2, T3, T4");

        let lakelet = course.track("lakelet").unwrap();
        assert_eq!(lakelet.step.len(), 24);
        assert_eq!(lakelet.project, "app/lakelet");
        assert_eq!(lakelet.index, 4, "the path order is T1, T2, T3, T4");

        let mailer = course.default_track().unwrap();
        assert_eq!(mailer.step.len(), 22);
        assert_eq!(mailer.project, "app/mailer");
        assert!(
            mailer.exit.len() >= 6,
            "the exit capabilities of §5 are declared"
        );

        // Counts of curriculum.md §3: 114 capabilities, 153 slots, 136 steps pinned.
        assert_eq!(course.ledger().capability_count(), 114);
        assert_eq!(course.ledger().slot_count(), 153);
        assert_eq!(course.ledger().positions().len(), 136);

        // A step reference resolves by number, by id, and with the track prefix.
        for reference in ["1.14", "1.14-worker", "mailer/1.14", "mailer/1.14-worker"] {
            let step = course.step(reference).unwrap();
            assert_eq!(step.qualified_id(), "mailer/1.14-worker");
            assert_eq!(step.number(), "1.14");
        }
        let error = course.step("mailer/9.9").unwrap_err();
        assert!(matches!(error, KilnError::UnknownStep { .. }), "{error}");
    }
    #[test]
    fn a_track_may_build_on_another_and_only_another() {
        use crate::test_support::{
            MANIFEST_TOML, TempDir, ledger_toml, load_course, step_toml, track_toml,
            track_toml_requiring, write,
        };
        let mailer = [step_toml("mailer", "1.1-a", &["a/b@apply"])];
        let dependent = [step_toml("relay", "2.1-x", &["a/c@apply"])];
        let fixture = |requires: &str| {
            vec![
                (
                    "mailer",
                    1,
                    track_toml("mailer", 1, &["a/b@apply"], &mailer),
                ),
                (
                    "relay",
                    2,
                    track_toml_requiring("relay", 2, &["a/c@apply"], &dependent, requires),
                ),
            ]
        };

        // A named, existing, different track: the link loads and is kept.
        let temp = TempDir::new("requires-track");
        let course = load_course(temp.path(), &fixture("mailer"), &ledger_toml(&[]));
        assert_eq!(
            course.track("relay").unwrap().requires_track.as_deref(),
            Some("mailer")
        );
        drop(course);

        // Anything else is refused at load, with the reason in the message.
        for (label, requires, expected) in [
            (
                "a-ghost",
                "ghost",
                "requires_track `ghost` has no track metadata",
            ),
            ("itself", "relay", "names the track itself"),
        ] {
            let temp = TempDir::new(&format!("requires-{label}"));
            write(temp.path(), "course/course.toml", MANIFEST_TOML);
            write(temp.path(), "course/capabilities.toml", &ledger_toml(&[]));
            for (id, _, body) in fixture(requires) {
                write(
                    temp.path(),
                    &format!("course/tracks/{id}/track.toml"),
                    &body,
                );
            }
            let error = Course::load(temp.path()).unwrap_err();
            assert!(error.to_string().contains(expected), "{label}: {error}");
        }
        // The two-track cycle: both links are fine alone, together they are a loop.
        let temp = TempDir::new("requires-cycle");
        write(temp.path(), "course/course.toml", MANIFEST_TOML);
        write(temp.path(), "course/capabilities.toml", &ledger_toml(&[]));
        write(
            temp.path(),
            "course/tracks/mailer/track.toml",
            &track_toml_requiring("mailer", 1, &["a/b@apply"], &mailer, "relay"),
        );
        write(
            temp.path(),
            "course/tracks/relay/track.toml",
            &track_toml_requiring("relay", 2, &["a/c@apply"], &dependent, "mailer"),
        );
        let error = Course::load(temp.path()).unwrap_err();
        assert!(error.to_string().contains("cycle"), "{error}");
    }
}
