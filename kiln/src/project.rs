//! The learner's project: getting a step ready to be judged, and undoing that again.
//!
//! A step does not judge an empty directory. Before anything is compiled, two things are put in
//! place (authoring.md §5), and both are *additive and repeatable*: the spec files are synced into
//! `tests/`, and the step's scaffold is applied. A learner can run `kiln run <step>` ten times and
//! the tenth run does exactly what the first did.
//!
//! The rules that make that safe, and how `reset` undoes them:
//!
//! * **the track skeleton** (`skeleton/`, used by a track's first step) creates files that are
//!   missing. A file the learner already has is never overwritten;
//! * **a scaffold patch** creates files and inserts lines, and nothing else (§5.1). Insertions only
//!   land inside `kiln:scaffold:begin` … `kiln:scaffold:end` blocks;
//! * **a reference solution** is an ordinary diff, applied on top of the scaffold state. It is
//!   strict on purpose: if the tree is not what the solution expects, it refuses rather than
//!   guessing;
//! * **`reset`** takes the scaffold's own additions back out — an inserted line that is still
//!   there, a created file that nobody has touched. Anything the learner edited is kept, and said
//!   so out loud. Removing the scaffold never removes somebody's work.

use std::path::{Path, PathBuf};

use crate::error::{KilnError, Result};
use crate::info::{Course, Step, Track};
use crate::patch::{self, Mode, Patch};
use crate::spec;

/// Directory below a track holding the starting skeleton of its project.
pub const SKELETON_DIR: &str = "skeleton";
/// Directory below a track holding the per-step scaffolds.
pub const SCAFFOLD_DIR: &str = "scaffold";
/// Directory below a track holding the reference solutions.
pub const SOLUTIONS_DIR: &str = "solutions";

/// What one file of the project went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// File, relative to the repository root.
    pub file: String,
    /// What happened to it.
    pub action: Action,
}

/// How a file was affected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The file was created.
    Created,
    /// The file was rewritten from course content: by `kiln spec restore`, by a scaffold, or by a
    /// reference solution.
    Restored,
    /// The file no longer exists.
    Removed,
    /// The file was already what the course expects.
    Unchanged,
    /// The file was left alone, because the learner owns it now.
    Kept,
}

impl Action {
    /// Stable machine-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Restored => "restored",
            Self::Removed => "removed",
            Self::Unchanged => "unchanged",
            Self::Kept => "kept",
        }
    }

    /// One word a learner reads in a report.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Restored => "restored",
            Self::Removed => "removed",
            Self::Unchanged => "already in place",
            Self::Kept => "kept yours",
        }
    }
}

/// One command's work on a project, ready to print or ignore.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Files that were touched, in the order they were handled.
    pub changes: Vec<Change>,
    /// Lines worth reading even when everything went well (a kept file, a missing marker block).
    pub notes: Vec<String>,
}

impl Outcome {
    /// `true` when nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes
            .iter()
            .all(|change| matches!(change.action, Action::Unchanged | Action::Kept))
            && self.notes.is_empty()
    }

    /// Files that were written or deleted.
    #[must_use]
    pub fn written(&self) -> Vec<&str> {
        self.changes
            .iter()
            .filter(|change| {
                matches!(
                    change.action,
                    Action::Created | Action::Restored | Action::Removed
                )
            })
            .map(|change| change.file.as_str())
            .collect()
    }

    /// One line per change, in the form `created  app/mailer/src/lib.rs`.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.changes
            .iter()
            .map(|change| format!("{:<14} {}", change.action.label(), change.file))
            .collect()
    }

    fn merge(&mut self, other: Outcome) {
        self.changes.extend(other.changes);
        self.notes.extend(other.notes);
    }
}

/// Refuse to prepare a project that is not there.
///
/// A fresh clone has no `app/`: the first step of the track copies the skeleton there, and every
/// later step patches what the earlier ones left. Running a mid-track step in an empty tree has
/// always failed — this makes it fail with the reason and the command that fixes it, instead of
/// the patch parser's "the file does not exist" (which names no project and no next action).
///
/// The step whose scaffold *is* the skeleton is the one that creates the project, so it is the one
/// exception.
fn require_project(course: &Course, track: &Track, step: &Step, project: &Path) -> Result<()> {
    if project.is_dir() {
        return Ok(());
    }
    if matches!(
        load_scaffold(&track.dir(course.root()), step)?,
        Some(Scaffold::Skeleton(_))
    ) {
        return Ok(());
    }
    let first = track
        .step
        .first()
        .map_or_else(|| format!("{}/1", track.id), Step::qualified_id);
    Err(KilnError::MissingProject {
        project: relative_to(course.root(), project),
        first,
    })
}

/// Where a step's scaffold comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scaffold {
    /// The track's starting skeleton, copied into the project (a track's first step).
    Skeleton(PathBuf),
    /// A patch applied on top of the current project.
    Patch(Box<Patch>),
}

/// Load the scaffold a step declares.
///
/// `scaffold = "skeleton"` names the track skeleton; anything else is a patch file below the track
/// directory. A step with no scaffold is normal: it changes files the learner already owns
/// (authoring.md §5).
pub fn load_scaffold(track_dir: &Path, step: &Step) -> Result<Option<Scaffold>> {
    let Some(declared) = &step.scaffold else {
        return Ok(None);
    };
    if declared == SKELETON_DIR {
        let path = track_dir.join(SKELETON_DIR);
        if !path.is_dir() {
            return Err(KilnError::Invalid(format!(
                "{}/{}: scaffold `{SKELETON_DIR}` names a directory that does not exist ({})",
                step.track,
                step.id,
                path.display()
            )));
        }
        return Ok(Some(Scaffold::Skeleton(path)));
    }
    let path = track_dir.join(declared);
    let text = std::fs::read_to_string(&path).map_err(|source| KilnError::Read {
        path: path.clone(),
        source,
    })?;
    Ok(Some(Scaffold::Patch(Box::new(Patch::parse(&text)?))))
}

/// The reference solution of a step, or `None` when the step has none.
pub fn load_solution(track_dir: &Path, step: &Step) -> Result<Option<Patch>> {
    let Some(declared) = &step.solution else {
        return Ok(None);
    };
    let path = track_dir.join(declared);
    let text = std::fs::read_to_string(&path).map_err(|source| KilnError::Read {
        path: path.clone(),
        source,
    })?;
    Ok(Some(Patch::parse(&text)?))
}

/// Content a step needs before it can be judged at all, phrased for the learner.
///
/// The learner's project directory is deliberately *not* in this list: a scaffold creates it, so a
/// missing project is work not started, not content the course never wrote. `run`, `verify` and
/// `selftest` all ask this question, and "nothing to judge" (exit 3) means exactly an empty list of
/// answers here.
pub fn missing_content(course: &Course, track: &Track, step: &Step) -> Vec<String> {
    let track_dir = track.dir(course.root());
    let mut missing = Vec::new();

    // `entry` is the source-locale path, so "is the body written" is a plain file question here;
    // which locale a learner *reads* is the i18n layer's business.
    if !step.source_path(&course.dir()).is_file() {
        missing.push(format!("the step body is not written yet: {}", step.entry));
    }

    if step.spec.is_empty() {
        missing.push("the step declares no spec file".to_owned());
    }
    for relative in &step.spec {
        if !track_dir.join(relative).is_file() {
            missing.push(format!("its spec file is missing: {relative}"));
        }
    }
    match step.scaffold.as_deref() {
        Some(SKELETON_DIR) if !track_dir.join(SKELETON_DIR).is_dir() => missing.push(format!(
            "its scaffold `{SKELETON_DIR}` names a directory that does not exist"
        )),
        Some(scaffold) if scaffold != SKELETON_DIR && !track_dir.join(scaffold).is_file() => {
            missing.push(format!("its scaffold is missing: {scaffold}"));
        }
        _ => {}
    }
    if let Some(solution) = &step.solution
        && !track_dir.join(solution).is_file()
    {
        missing.push(format!("its reference solution is missing: {solution}"));
    }
    missing
}

/// Sync the step's specs and apply its scaffold: everything `run` and `check` do before judging.
pub fn prepare(course: &Course, track: &Track, step: &Step) -> Result<Outcome> {
    let project = track.project_dir(course.root());
    require_project(course, track, step, &project)?;
    let mut outcome = Outcome::default();
    for sync in spec::sync(course.root(), track, step, &project, false)? {
        let action = match sync.action {
            spec::SyncAction::Created => Action::Created,
            spec::SyncAction::Restored => Action::Restored,
            spec::SyncAction::Unchanged => Action::Unchanged,
            spec::SyncAction::Kept => Action::Kept,
        };
        if action == Action::Kept {
            outcome.notes.push(format!(
                "{} is not the shipped spec; `kiln spec restore {}` puts the course copy back",
                sync.file,
                step.qualified_id()
            ));
        }
        outcome.changes.push(Change {
            file: sync.file,
            action,
        });
    }
    outcome.merge(apply_scaffold(course, track, step)?);
    Ok(outcome)
}

/// Apply the step's scaffold, if it has one.
pub fn apply_scaffold(course: &Course, track: &Track, step: &Step) -> Result<Outcome> {
    let track_dir = track.dir(course.root());
    let project = track.project_dir(course.root());
    require_project(course, track, step, &project)?;
    let Some(scaffold) = load_scaffold(&track_dir, step)? else {
        return Ok(Outcome::default());
    };

    let mut outcome = Outcome::default();
    match scaffold {
        Scaffold::Skeleton(skeleton) => {
            for (relative, content) in tree_files(&skeleton)? {
                let target = project.join(&relative);
                let file = relative_to(course.root(), &target);
                // A skeleton is a starting point, not an expectation: a file that is already there
                // is left exactly as it is, and that is the normal state after the step is done —
                // so it is reported as "already in place" rather than as a conflict.
                let action = match std::fs::read_to_string(&target) {
                    Ok(_) => Action::Unchanged,
                    Err(_) => {
                        if let Some(parent) = target.parent() {
                            std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
                                path: parent.to_path_buf(),
                                source,
                            })?;
                        }
                        std::fs::write(&target, &content).map_err(|source| KilnError::Write {
                            path: target.clone(),
                            source,
                        })?;
                        Action::Created
                    }
                };
                outcome.changes.push(Change { file, action });
            }
            if outcome.changes.is_empty() {
                outcome
                    .notes
                    .push(format!("the skeleton of {} is empty", track.label()));
            }
        }
        Scaffold::Patch(patch) => {
            let plan = patch::plan(&project, &patch, Mode::Scaffold)?;
            for skipped in &plan.skipped {
                if skipped.reason == patch::SkipReason::KeptYours {
                    outcome.notes.push(format!(
                        "{} {}",
                        relative_to(course.root(), &project.join(&skipped.path)),
                        skipped.reason.detail()
                    ));
                }
            }
            plan.apply(&project)?;
            for path in plan.created() {
                outcome.changes.push(Change {
                    file: relative_to(course.root(), &project.join(path)),
                    action: Action::Created,
                });
            }
            for path in plan.modified() {
                outcome.changes.push(Change {
                    file: relative_to(course.root(), &project.join(path)),
                    action: Action::Restored,
                });
            }
        }
    }
    Ok(outcome)
}

/// Take the step's scaffold back out, keeping everything the learner edited (`kiln reset`).
///
/// A track's first step gets its skeleton from the same place as every other step gets a patch, so
/// resetting it uses the skeleton's own files: a file that still matches the skeleton is removed,
/// and an edited one is kept with a note.
pub fn reset_scaffold(course: &Course, track: &Track, step: &Step) -> Result<Outcome> {
    let track_dir = track.dir(course.root());
    let project = track.project_dir(course.root());
    let mut outcome = Outcome::default();
    match load_scaffold(&track_dir, step)? {
        None => outcome.notes.push(format!(
            "{} declares no scaffold, so there is nothing to revert",
            step.qualified_id()
        )),
        Some(Scaffold::Skeleton(skeleton)) => {
            for (relative, content) in tree_files(&skeleton)? {
                let target = project.join(&relative);
                let file = relative_to(course.root(), &target);
                match std::fs::read_to_string(&target) {
                    Ok(existing) if existing == content => {
                        std::fs::remove_file(&target).map_err(|source| KilnError::Write {
                            path: target.clone(),
                            source,
                        })?;
                        outcome.changes.push(Change {
                            file,
                            action: Action::Removed,
                        });
                    }
                    Ok(_) => {
                        outcome.notes.push(format!("{file} was edited; kept yours"));
                        outcome.changes.push(Change {
                            file,
                            action: Action::Kept,
                        });
                    }
                    Err(_) => outcome.changes.push(Change {
                        file,
                        action: Action::Unchanged,
                    }),
                }
            }
        }
        Some(Scaffold::Patch(patch)) => {
            let plan = patch::plan(&project, &patch.reversed(), Mode::Revert)?;
            for skipped in &plan.skipped {
                if skipped.reason == patch::SkipReason::KeptYours {
                    outcome.notes.push(format!(
                        "reset left {} alone: {}",
                        relative_to(course.root(), &project.join(&skipped.path)),
                        skipped.reason.detail()
                    ));
                }
            }
            plan.apply(&project)?;
            for path in plan.created() {
                outcome.changes.push(Change {
                    file: relative_to(course.root(), &project.join(path)),
                    action: Action::Created,
                });
            }
            for path in plan.modified() {
                outcome.changes.push(Change {
                    file: relative_to(course.root(), &project.join(path)),
                    action: Action::Restored,
                });
            }
            for path in plan.deleted() {
                outcome.changes.push(Change {
                    file: relative_to(course.root(), &project.join(path)),
                    action: Action::Removed,
                });
            }
        }
    }
    Ok(outcome)
}

/// Apply the step's reference solution (`kiln solution`).
pub fn apply_solution(course: &Course, track: &Track, step: &Step) -> Result<Outcome> {
    let track_dir = track.dir(course.root());
    let project = track.project_dir(course.root());
    let Some(patch) = load_solution(&track_dir, step)? else {
        return Err(KilnError::NotAuthored {
            what: format!("the reference solution of {}", step.qualified_id()),
        });
    };
    let plan = patch::plan(&project, &patch, Mode::Solution)?;
    plan.apply(&project)?;

    let mut outcome = Outcome::default();
    for path in plan.created() {
        outcome.changes.push(Change {
            file: relative_to(course.root(), &project.join(path)),
            action: Action::Created,
        });
    }
    for path in plan.modified() {
        outcome.changes.push(Change {
            file: relative_to(course.root(), &project.join(path)),
            action: Action::Restored,
        });
    }
    for path in plan.deleted() {
        outcome.changes.push(Change {
            file: relative_to(course.root(), &project.join(path)),
            action: Action::Removed,
        });
    }
    Ok(outcome)
}

/// Every file below a directory, as `(path relative to the directory, contents)`.
///
/// `target/` and `Cargo.lock` are skipped: they are build output, not course content. Used by the
/// scaffold machinery (the skeleton is the same shape, read from a different place) and by
/// `kiln selftest`, which copies the whole course into a sandbox.
pub fn tree_files(root: &Path) -> Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Copy a directory tree, returning how many files were written.
pub fn copy_tree(from: &Path, to: &Path) -> Result<usize> {
    let files = tree_files(from)?;
    for (relative, content) in &files {
        let target = to.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&target, content).map_err(|source| KilnError::Write {
            path: target.clone(),
            source,
        })?;
    }
    Ok(files.len())
}

fn collect(root: &Path, directory: &Path, files: &mut Vec<(String, String)>) -> Result<()> {
    let entries = std::fs::read_dir(directory).map_err(|source| KilnError::Read {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| KilnError::Read {
            path: directory.to_path_buf(),
            source,
        })?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(std::ffi::OsStr::to_str);
        if name == Some("Cargo.lock") || name == Some("target") {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, files)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                KilnError::Invalid(format!(
                    "{} is not below {}",
                    path.display(),
                    root.display()
                ))
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let content = std::fs::read_to_string(&path).map_err(|source| KilnError::Read {
            path: path.clone(),
            source,
        })?;
        files.push((relative, content));
    }
    Ok(())
}

/// A path as messages write it: relative to the repository root, with forward slashes.
fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, load_course, one_track, step_toml};

    /// A one-step course whose step uses the given scaffold declaration.
    fn course_with(temp: &TempDir, scaffold: Option<&str>) -> Course {
        let mut step = step_toml("mailer", "1.1-workspace", &["repo/workspace-layout@apply"]);
        if let Some(value) = scaffold {
            step.push_str(&format!("scaffold = \"{value}\"\n"));
        }
        load_course(
            temp.path(),
            &one_track(&[step], &["repo/workspace-layout@apply"]),
            &crate::test_support::ledger_toml(&[("repo/workspace-layout", "apply", "mailer/1.1")]),
        )
    }

    const SKELETON: &[(&str, &str)] = &[
        ("Cargo.toml", "[package]\nname = \"mailer\"\n"),
        ("src/main.rs", "fn main() {}\n"),
    ];

    fn write_skeleton(temp: &TempDir) {
        for (path, content) in SKELETON {
            temp.write(&format!("course/tracks/mailer/skeleton/{path}"), content);
        }
    }

    fn step_of(course: &Course) -> Step {
        course
            .default_track()
            .unwrap()
            .step
            .first()
            .unwrap()
            .clone()
    }

    #[test]
    fn a_skeleton_scaffold_creates_the_project_and_repeats_cleanly() {
        let temp = TempDir::new("project-skeleton");
        write_skeleton(&temp);
        let course = course_with(&temp, Some("skeleton"));
        let track = course.default_track().unwrap();
        let step = step_of(&course);

        let first = apply_scaffold(&course, track, &step).unwrap();
        assert_eq!(first.written().len(), 2, "{first:?}");
        assert!(temp.path().join("app/mailer/src/main.rs").is_file());

        let second = apply_scaffold(&course, track, &step).unwrap();
        assert!(second.written().is_empty(), "{second:?}");
        assert!(second.is_empty(), "{second:?}");
    }

    #[test]
    fn a_learner_file_is_never_overwritten_by_the_skeleton() {
        let temp = TempDir::new("project-skeleton-keep");
        write_skeleton(&temp);
        let course = course_with(&temp, Some("skeleton"));
        let track = course.default_track().unwrap();
        let step = step_of(&course);
        temp.write("app/mailer/src/main.rs", "fn main() { /* mine */ }\n");

        let outcome = apply_scaffold(&course, track, &step).unwrap();
        assert_eq!(
            outcome
                .changes
                .iter()
                .find(|change| change.file.ends_with("main.rs"))
                .map(|change| change.action),
            Some(Action::Unchanged)
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("app/mailer/src/main.rs")).unwrap(),
            "fn main() { /* mine */ }\n"
        );
    }

    #[test]
    fn resetting_a_skeleton_removes_untouched_files_and_keeps_edited_ones() {
        let temp = TempDir::new("project-skeleton-reset");
        write_skeleton(&temp);
        let course = course_with(&temp, Some("skeleton"));
        let track = course.default_track().unwrap();
        let step = step_of(&course);
        apply_scaffold(&course, track, &step).unwrap();
        temp.write("app/mailer/src/main.rs", "fn main() { /* mine */ }\n");

        let outcome = reset_scaffold(&course, track, &step).unwrap();
        assert!(!temp.path().join("app/mailer/Cargo.toml").exists());
        assert!(temp.path().join("app/mailer/src/main.rs").is_file());
        assert_eq!(outcome.written(), ["app/mailer/Cargo.toml"]);
        assert!(outcome.notes.iter().any(|note| note.contains("kept yours")));
    }

    #[test]
    fn preparing_a_step_syncs_the_specs_and_applies_the_scaffold() {
        let temp = TempDir::new("project-prepare");
        write_skeleton(&temp);
        let course = course_with(&temp, Some("skeleton"));
        let track = course.default_track().unwrap();
        let step = step_of(&course);
        temp.write("course/tracks/mailer/spec/1.1/spec.rs", "//! spec\n");

        let outcome = prepare(&course, track, &step).unwrap();
        assert!(temp.path().join("app/mailer/tests/spec_spec.rs").is_file());
        assert!(temp.path().join("app/mailer/src/main.rs").is_file());
        assert_eq!(outcome.written().len(), 3, "{outcome:?}");

        let again = prepare(&course, track, &step).unwrap();
        assert!(again.written().is_empty(), "{again:?}");
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn a_step_without_a_scaffold_prepares_only_its_specs() {
        let temp = TempDir::new("project-no-scaffold");
        let course = course_with(&temp, None);
        let track = course.default_track().unwrap();
        let step = step_of(&course);
        // The project exists by the time a scaffold-less step is judged: the track's first step is
        // what creates it, and a step that only syncs specs does not.
        temp.write("app/mailer/Cargo.toml", "[package]\nname = \"mailer\"\n");
        temp.write("course/tracks/mailer/spec/1.1/spec.rs", "//! spec\n");

        let outcome = prepare(&course, track, &step).unwrap();
        assert_eq!(outcome.written(), ["app/mailer/tests/spec_spec.rs"]);
        let reset = reset_scaffold(&course, track, &step).unwrap();
        assert!(reset.notes.first().unwrap().contains("no scaffold"));
    }

    #[test]
    fn a_solution_rewrites_the_files_it_patches() {
        let temp = TempDir::new("project-solution");
        write_skeleton(&temp);
        temp.write(
            "course/tracks/mailer/solutions/1.1-workspace.patch",
            "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1,3 @@\n-fn main() {}\n+fn main() {\n+    mailer::run();\n+}\n",
        );
        let course = course_with(&temp, Some("skeleton"));
        let track = course.default_track().unwrap();
        let mut step = step_of(&course);
        step.solution = Some("solutions/1.1-workspace.patch".to_owned());
        apply_scaffold(&course, track, &step).unwrap();

        let outcome = apply_solution(&course, track, &step).unwrap();
        assert_eq!(outcome.written(), ["app/mailer/src/main.rs"]);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("app/mailer/src/main.rs")).unwrap(),
            "fn main() {\n    mailer::run();\n}\n"
        );
        // A second run has nothing to do: the solution is already in place.
        let again = apply_solution(&course, track, &step).unwrap();
        assert!(again.changes.is_empty(), "{again:?}");

        // A tree the solution did not expect is refused rather than guessed at.
        temp.write("app/mailer/src/main.rs", "fn main() { /* mine */ }\n");
        let error = apply_solution(&course, track, &step).unwrap_err();
        assert!(error.to_string().contains("does not apply"), "{error}");
    }

    #[test]
    fn a_scaffold_that_would_not_fit_the_project_is_reported_not_ignored() {
        let temp = TempDir::new("project-scaffold-conflict");
        write_skeleton(&temp);
        temp.write(
            "course/tracks/mailer/scaffold/1.1-workspace.patch",
            "--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n # mailer\n+<!-- kiln:scaffold:begin -->\n",
        );
        let course = course_with(&temp, Some("scaffold/1.1-workspace.patch"));
        let track = course.default_track().unwrap();
        let step = step_of(&course);
        // The patch's target, so the project this test patches exists.
        temp.write("app/mailer/README.md", "# mailer\n");
        let error = apply_scaffold(&course, track, &step).unwrap_err();
        assert!(error.to_string().contains("README.md"), "{error}");
    }
}
