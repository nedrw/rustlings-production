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

/// One `[[step]]` block, valid unless the caller means to break something.
pub(crate) fn step_toml(id: &str, teaches: &[&str]) -> String {
    format!(
        r#"
[[step]]
id = "{id}"
title = "Step {id}"
kind = "milestone"
origin = "teach"
entry = "docs/steps/en/mailer/{id}.md"
time_hint = "60m"
teaches = [{}]
run = {{ cmd = "cargo test -p mailer --test spec" }}
spec = ["spec/{id}/spec.rs"]
gates = ["fmt"]
"#,
        quoted(teaches)
    )
}

/// A `track.toml` whose steps are the given block sources.
pub(crate) fn track_toml(exit: &[&str], steps: &[String]) -> String {
    format!(
        "format_version = 1\nid = \"mailer\"\nindex = 1\ntitle = \"Mailer\"\nproject = \"app/mailer\"\nexit = [{}]\n{}",
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

/// Write a one-track course into `root` and load it through the real loader.
pub(crate) fn load_course(root: &Path, track: &str, ledger: &str) -> Course {
    write(root, "course/course.toml", MANIFEST_TOML);
    write(root, "course/capabilities.toml", ledger);
    write(root, "course/tracks/mailer/track.toml", track);
    Course::load(root).expect("fixture course should load")
}

fn quoted(items: &[&str]) -> String {
    items
        .iter()
        .map(|item| format!("\"{item}\""))
        .collect::<Vec<_>>()
        .join(", ")
}
