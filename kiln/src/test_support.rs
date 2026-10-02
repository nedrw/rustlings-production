//! Shared helpers for tests.
//!
//! Test-only: the linters and the i18n checks need real files on disk, and the course has no
//! `tempfile` dependency to spend on them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::info::Course;

/// A directory under the system temporary directory, removed when it is dropped.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create an empty directory whose name says which test owns it.
    pub(crate) fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("kiln-test-{label}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temporary directory should be creatable");
        Self { path }
    }

    /// Root of the temporary directory.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Write a file below the temporary directory, creating parents, and return its path.
    pub(crate) fn write(&self, relative: &str, contents: &str) -> PathBuf {
        write(self.path(), relative, contents)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Write a file below `root`, creating parents, and return its path.
pub(crate) fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("temporary parents should be creatable");
    }
    std::fs::write(&path, contents).expect("temporary file should be writable");
    path
}

/// A minimal but valid course manifest.
pub(crate) const MANIFEST_TOML: &str = r#"
format_version = 1
default_locale = "en"
default_track = "mailer"
welcome = "welcome"
final_message = "done"
"#;

/// One `[[step]]` block, with the defaults of [`Step`].
pub(crate) fn step_toml(track: &str, id: &str, teaches: &[&str]) -> String {
    Step::new(track, id, teaches).toml()
}

/// One `[[step]]` block under construction.
///
/// Tests vary one field at a time, so the loop points are methods rather than hand-written TOML
/// — a fixture that cannot be written is a fixture that quietly stops testing anything.
pub(crate) struct Step {
    track: String,
    id: String,
    teaches: Vec<String>,
    origin: String,
    requires: Vec<String>,
    gates: Vec<String>,
    spec: bool,
    reteach: Option<String>,
}

impl Step {
    /// A milestone `teach` step with one spec file and one must gate.
    pub(crate) fn new(track: &str, id: &str, teaches: &[&str]) -> Self {
        Self {
            track: track.to_owned(),
            id: id.to_owned(),
            teaches: owned(teaches),
            origin: "teach".to_owned(),
            requires: Vec::new(),
            gates: vec!["fmt".to_owned()],
            spec: true,
            reteach: None,
        }
    }

    /// Set `origin` (and drop the spec, as a `deepen`/`transfer` step keeps its own evidence).
    pub(crate) fn origin(mut self, origin: &str) -> Self {
        self.origin = origin.to_owned();
        self
    }

    /// Declare `requires`.
    pub(crate) fn requires(mut self, requires: &[&str]) -> Self {
        self.requires = owned(requires);
        self
    }
    /// Replace the gate list.
    pub(crate) fn gates(mut self, gates: &[&str]) -> Self {
        self.gates = owned(gates);
        self
    }

    /// Include a spec file or not.
    pub(crate) fn spec(mut self, has_spec: bool) -> Self {
        self.spec = has_spec;
        self
    }

    /// Declare a deliberate re-teach.
    pub(crate) fn reteach(mut self, reason: &str) -> Self {
        self.reteach = Some(reason.to_owned());
        self
    }

    /// The `[[step]]` block.
    ///
    /// The spec path and the test target follow the conventions the real course is held to
    /// (`spec/<phase>.<index>/<name>.rs` becomes the target `spec_<name>`), so a fixture that
    /// would not pass `spec::check_step` cannot be written by accident.
    pub(crate) fn toml(&self) -> String {
        let phase = self
            .id
            .split_once('-')
            .map_or(self.id.as_str(), |(phase, _)| phase);
        let mut out = format!(
            "\n[[step]]\nid = \"{id}\"\ntitle = \"Step {id}\"\nkind = \"milestone\"\n\
             origin = \"{origin}\"\nentry = \"docs/steps/en/{track}/{id}.md\"\n\
             time_hint = \"60m\"\nteaches = [{}]\n",
            quoted_owned(&self.teaches),
            id = self.id,
            origin = self.origin,
            track = self.track,
        );
        if !self.requires.is_empty() {
            out.push_str(&format!("requires = [{}]\n", quoted_owned(&self.requires)));
        }
        if !self.gates.is_empty() || self.spec {
            out.push_str(&format!(
                "run = {{ cmd = \"cargo test -p {track} --test spec_spec\" }}\n",
                track = self.track
            ));
        }
        if self.spec {
            out.push_str(&format!("spec = [\"spec/{phase}/spec.rs\"]\n"));
        }
        out.push_str(&format!("gates = [{}]\n", quoted_owned(&self.gates)));
        if let Some(reason) = &self.reteach {
            out.push_str(&format!("reteach = \"{reason}\"\n"));
        }
        out
    }
}

/// A `track.toml` with the given exit capabilities and step blocks.
pub(crate) fn track_toml(id: &str, index: u32, exit: &[&str], steps: &[String]) -> String {
    format!(
        "format_version = 1\nid = \"{id}\"\nindex = {index}\ntitle = \"{id}\"\nproject = \"app/{id}\"\nexit = [{}]\n{}",
        quoted(exit),
        steps.join("\n")
    )
}

/// A `capabilities.toml` from `(capability, depth, position)` triples, grouped per capability.
pub(crate) fn ledger_toml(slots: &[(&str, &str, &str)]) -> String {
    let mut grouped: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (capability, level, position) in slots {
        grouped
            .entry(capability)
            .or_default()
            .push((level, position));
    }
    let mut out = String::from("format_version = 1\n");
    for (capability, levels) in grouped {
        out.push_str(&format!("\n[capability.\"{capability}\"]\n"));
        for (level, position) in levels {
            out.push_str(&format!("{level} = \"{position}\"\n"));
        }
    }
    out
}

/// A track fixture: id, position in the path, and the `track.toml` body.
pub(crate) type TrackFixture = (&'static str, u32, String);

/// Write a course with the given tracks and load it through the real loader.
pub(crate) fn load_course(root: &Path, tracks: &[TrackFixture], ledger: &str) -> Course {
    write(root, "course/course.toml", MANIFEST_TOML);
    write(root, "course/capabilities.toml", ledger);
    for (id, _, body) in tracks {
        write(root, &format!("course/tracks/{id}/track.toml"), body);
    }
    Course::load(root).expect("fixture course should load")
}

/// The loop of a track whose steps are the given blocks: `mailer`, index 1, one exit slot.
pub(crate) fn one_track(steps: &[String], exit: &[&str]) -> Vec<TrackFixture> {
    vec![("mailer", 1, track_toml("mailer", 1, exit, steps))]
}

fn quoted(items: &[&str]) -> String {
    items
        .iter()
        .map(|item| format!("\"{item}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Quote a list of already-owned strings.
fn quoted_owned(items: &[String]) -> String {
    quoted(&items.iter().map(String::as_str).collect::<Vec<_>>())
}

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}
