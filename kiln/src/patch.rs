//! Unified diffs: the format scaffolds and reference solutions are written in.
//!
//! A track keeps two kinds of diff per step (authoring.md §5):
//!
//! * `scaffold/<step>.patch` — **add-only**. It may create files, and it may insert lines, but only
//!   inside a `kiln:scaffold:begin` … `kiln:scaffold:end` block. That restriction is what lets a
//!   learner edit their project freely without ever colliding with the course, and it is enforced
//!   here rather than trusted: a scaffold that removes a line is a mistake in the course.
//! * `solutions/<step>.patch` — an ordinary diff, applied on top of the scaffold state. It rewrites
//!   and removes lines, because that is what a reference implementation does.
//!
//! Both are applied through the same small engine, and both are planned before they are written:
//! [`plan`] computes the resulting files, [`Plan::apply`] writes them. Planning first is what makes
//! idempotence, conflict reporting and `kiln reset` fall out of one piece of logic.

use std::path::{Path, PathBuf};

use crate::error::{KilnError, Result};

/// Marker that opens a region scaffold patches may insert into.
pub const MARKER_BEGIN: &str = "kiln:scaffold:begin";
/// Marker that closes such a region.
pub const MARKER_END: &str = "kiln:scaffold:end";

/// How one line of a hunk is tagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Unchanged line, carried for context.
    Context,
    /// Line the patch adds.
    Add,
    /// Line the patch removes.
    Remove,
}

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Tag the patch gave it.
    pub kind: Kind,
    /// Text without the tag (and without the line ending).
    pub text: String,
}

/// One `@@ … @@` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// First line of the old file the hunk expects (1-based; 0 for an empty old side).
    pub old_start: usize,
    /// Lines of the old side the hunk describes.
    pub old_count: usize,
    /// Tagged lines, in order.
    pub lines: Vec<Line>,
}

impl Hunk {
    /// Lines that must exist in the file (`context` and `remove`).
    fn old_side(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|line| line.kind != Kind::Add)
            .map(|line| line.text.as_str())
            .collect()
    }

    /// Lines the file has after the hunk ran (`context` and `add`).
    fn new_side(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|line| line.kind != Kind::Remove)
            .map(|line| line.text.as_str())
            .collect()
    }

    /// `true` when the hunk only inserts lines.
    #[must_use]
    pub fn is_add_only(&self) -> bool {
        !self.lines.iter().any(|line| line.kind == Kind::Remove)
    }
}

/// What one file section of a patch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// The patch creates the file (`--- /dev/null`).
    Create,
    /// The patch edits an existing file.
    Modify,
    /// The patch deletes the file (`+++ /dev/null`); only produced by [`Patch::reversed`].
    Delete,
}

/// One file of a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path relative to the tree the patch is applied to.
    pub path: String,
    /// What the section does.
    pub section: Section,
    /// The hunks, in order.
    pub hunks: Vec<Hunk>,
}

/// A parsed patch: the file sections, in the order the patch lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    sections: Vec<FileDiff>,
}

impl Patch {
    /// Parse a unified diff.
    ///
    /// Git-style headers (`diff --git`, `index`, mode lines) are accepted and ignored, because
    /// course patches are written by hand and by `git diff` interchangeably. Two things are
    /// rejected rather than interpreted: `\ No newline at end of file` (a patch whose meaning
    /// depends on a missing final newline is not worth supporting for course content), and
    /// `rename from`/`copy from` (a rename is a delete plus a create, and applying one as a
    /// modification would write the content into the file the patch meant to remove it from).
    pub fn parse(text: &str) -> Result<Self> {
        if text.contains('\r') {
            return Err(KilnError::Invalid(
                "patch uses CRLF line endings; course patches are LF only".to_owned(),
            ));
        }
        let mut sections: Vec<FileDiff> = Vec::new();
        let mut lines = text.lines().peekable();

        while let Some(line) = lines.next() {
            if line.starts_with("\\ No newline") {
                return Err(KilnError::Invalid(
                    "patch uses `\\ No newline at end of file`, which is not supported".to_owned(),
                ));
            }
            if line.starts_with("rename ") || line.starts_with("copy ") {
                // `git diff` puts these before the `---`/`+++` pair, so this is where a rename has
                // to be caught: applying the section as a *modification* would write the content
                // into the file the patch meant to remove it from, and the new path would stay
                // missing.
                return Err(KilnError::Invalid(format!(
                    "patch renames or copies a file (`{line}`): course patches are generated with \
                     `git diff --no-renames`, so a rename is written as a delete plus a create"
                )));
            }
            let Some(old) = line.strip_prefix("--- ") else {
                continue;
            };
            let Some(new) = lines.next() else {
                return Err(KilnError::Invalid(format!(
                    "patch ends after `--- {old}`: the `+++` header is missing"
                )));
            };
            let new = new.strip_prefix("+++ ").ok_or_else(|| {
                KilnError::Invalid(format!(
                    "patch has `--- {old}` followed by `{new}`, expected a `+++` header"
                ))
            })?;
            let section = match (old.trim(), new.trim()) {
                ("/dev/null", _) => Section::Create,
                (_, "/dev/null") => Section::Delete,
                _ => Section::Modify,
            };
            let path = if section == Section::Create {
                header_path(new)?
            } else {
                header_path(old)?
            };

            let mut hunks = Vec::new();
            while let Some(peeked) = lines.peek() {
                if peeked.starts_with("@@") {
                    let header = lines.next().unwrap_or_default();
                    hunks.push(parse_hunk(header, &mut lines, &path)?);
                } else if peeked.starts_with("--- ") || peeked.starts_with("+++ ") {
                    break;
                } else if peeked.starts_with("diff --git")
                    || peeked.starts_with("index ")
                    || peeked.starts_with("similarity ")
                    || peeked.starts_with("new file mode")
                    || peeked.starts_with("deleted file mode")
                    || peeked.starts_with("old mode")
                    || peeked.starts_with("new mode")
                {
                    let _ = lines.next();
                } else {
                    break;
                }
            }
            if hunks.is_empty() {
                return Err(KilnError::Invalid(format!(
                    "patch section for `{path}` has no `@@` hunk"
                )));
            }
            sections.push(FileDiff {
                path,
                section,
                hunks,
            });
        }

        if sections.is_empty() {
            return Err(KilnError::Invalid(
                "patch has no file section: expected `--- a/<path>` and `+++ b/<path>`".to_owned(),
            ));
        }
        Ok(Self { sections })
    }

    /// The file sections, in order.
    #[must_use]
    pub fn sections(&self) -> &[FileDiff] {
        &self.sections
    }

    /// Paths the patch touches, in order.
    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        self.sections
            .iter()
            .map(|file| file.path.as_str())
            .collect()
    }

    /// The same patch applied backwards: additions become removals and vice versa.
    ///
    /// `kiln reset` uses this to take a scaffold back out again.
    #[must_use]
    pub fn reversed(&self) -> Self {
        let sections = self
            .sections
            .iter()
            .map(|file| FileDiff {
                path: file.path.clone(),
                section: match file.section {
                    Section::Create => Section::Delete,
                    Section::Delete => Section::Create,
                    Section::Modify => Section::Modify,
                },
                hunks: file
                    .hunks
                    .iter()
                    .map(|hunk| Hunk {
                        old_start: hunk.old_start,
                        old_count: hunk.old_count,
                        lines: hunk
                            .lines
                            .iter()
                            .map(|line| Line {
                                kind: match line.kind {
                                    Kind::Add => Kind::Remove,
                                    Kind::Remove => Kind::Add,
                                    Kind::Context => Kind::Context,
                                },
                                text: line.text.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Self { sections }
    }

    /// `Ok(())` when the patch creates files and inserts lines only.
    ///
    /// A scaffold has to satisfy this (authoring.md §5); a solution does not.
    pub fn require_add_only(&self) -> Result<()> {
        for file in &self.sections {
            match file.section {
                Section::Create | Section::Delete => {
                    // A creation written as a hunk instead of `/dev/null` is still caught below:
                    // its old side is not empty, so it would have to remove nothing.
                }
                Section::Modify => {}
            }
            for hunk in &file.hunks {
                if let Some(removed) = hunk.lines.iter().find(|line| line.kind == Kind::Remove) {
                    return Err(KilnError::Invalid(format!(
                        "scaffold patch removes `{}` from {}: scaffolds may only create files or \
                         insert lines (authoring.md §5)",
                        removed.text, file.path
                    )));
                }
            }
            if file.section == Section::Delete {
                return Err(KilnError::Invalid(format!(
                    "scaffold patch deletes {}: scaffolds may only create files or insert lines \
                     (authoring.md §5)",
                    file.path
                )));
            }
        }
        Ok(())
    }

    /// `true` when the patch creates the file at `path`.
    #[must_use]
    pub fn creates(&self, path: &str) -> bool {
        self.sections
            .iter()
            .any(|file| file.path == path && file.section == Section::Create)
    }
}

/// Strip the `a/` or `b/` prefix git puts in front of patched paths.
fn header_path(header: &str) -> Result<String> {
    let path = header.trim();
    let path = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);
    let path = path.strip_prefix("./").unwrap_or(path);
    let path = path.trim_end_matches('\t');
    if path.is_empty() || path.starts_with('/') || path.contains("..") {
        return Err(KilnError::Invalid(format!(
            "patch header `{header}` is not a relative path inside the project"
        )));
    }
    Ok(path.to_owned())
}

/// Parse one `@@ -a,b +c,d @@` header and the tagged lines under it.
fn parse_hunk<'a>(
    header: &str,
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    path: &str,
) -> Result<Hunk> {
    let malformed = || {
        KilnError::Invalid(format!(
            "{path}: `{header}` is not a hunk header: expected `@@ -1,3 +1,4 @@`"
        ))
    };
    let ranges = header
        .trim_start_matches('@')
        .trim()
        .split(' ')
        .take(2)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let (old, new) = match ranges.as_slice() {
        [old, new] => (old.clone(), new.clone()),
        _ => return Err(malformed()),
    };
    let old_start = parse_range(&old).ok_or_else(malformed)?.0;
    let (old_count, new_count) = (
        parse_range(&old).ok_or_else(malformed)?.1,
        parse_range(&new).ok_or_else(malformed)?.1,
    );

    // How many lines a hunk carries is *in the header*, and that is the only thing consulted here:
    // a hunk line's first character is part of the file's content, so a removed SQL comment arrives
    // as `--- …` and an added one as `+++ …` — reading those as file headers is a mistake the
    // header counts cannot make (they are what a real diff tool counts too).
    let mut hunk_lines = Vec::new();
    let (mut old_seen, mut new_seen) = (0, 0);
    while old_seen < old_count || new_seen < new_count {
        let Some(line) = lines.next() else {
            break;
        };
        let kind = match line.chars().next() {
            // An empty line is how a stripped-trailing-space context line arrives.
            None | Some(' ') => Kind::Context,
            Some('+') => Kind::Add,
            Some('-') => Kind::Remove,
            Some('\\') => {
                return Err(KilnError::Invalid(format!(
                    "{path}: `{line}` is not supported"
                )));
            }
            Some(_) => break,
        };
        let line = unescape(line);
        match kind {
            Kind::Add => new_seen += 1,
            Kind::Remove => old_seen += 1,
            Kind::Context => {
                old_seen += 1;
                new_seen += 1;
            }
        }
        hunk_lines.push(Line { kind, text: line });
    }

    if old_seen != old_count || new_seen != new_count {
        return Err(KilnError::Invalid(format!(
            "{path}: hunk `{header}` declares {old_count} old and {new_count} new line(s) but \
             carries {old_seen} and {new_seen}"
        )));
    }
    Ok(Hunk {
        old_start,
        old_count,
        lines: hunk_lines,
    })
}

/// One hunk line's text, without the tag that marked it.
fn unescape(line: &str) -> String {
    line.strip_prefix([' ', '+', '-'])
        .unwrap_or(line)
        .to_owned()
}

/// Split `-1,3` (or `-1`) into `(start, count)`.
fn parse_range(text: &str) -> Option<(usize, usize)> {
    let rest = text.strip_prefix(['-', '+'])?;
    let (start, count) = match rest.split_once(',') {
        Some((start, count)) => (start, count),
        None => (rest, "1"),
    };
    Some((start.parse().ok()?, count.parse().ok()?))
}

/// How strictly a patch has to fit the tree it is applied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A scaffold: add-only, insertions only inside marker blocks, and never overwrite what is
    /// already there.
    Scaffold,
    /// A scaffold taken back out (`kiln reset`). It removes what the scaffold put there, and like a
    /// scaffold it keeps anything the learner has edited.
    Revert,
    /// A reference solution: the tree must match the patch exactly, or nothing is written.
    Solution,
}

impl Mode {
    /// `true` when the mode must not overwrite the learner's files.
    fn lenient(self) -> bool {
        matches!(self, Mode::Scaffold | Mode::Revert)
    }
}

/// One planned file write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Path relative to the tree.
    pub path: String,
    /// What the file must look like for the write to be the patch's doing.
    pub before: Option<String>,
    /// The content after the patch.
    pub after: Option<String>,
}

/// Why a patch left a file alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The file already is what the patch would make it: nothing to do, and nothing to report.
    AlreadyDone,
    /// The file differs from what the patch expects and the patch will not overwrite it.
    KeptYours,
}

impl SkipReason {
    /// One line for a report.
    #[must_use]
    pub fn detail(self) -> &'static str {
        match self {
            Self::AlreadyDone => "already in place, nothing to do",
            Self::KeptYours => {
                "is yours, so it was left untouched (the scaffold never overwrites \
                               your code)"
            }
        }
    }
}

/// Something the patch would not do, and why. Reported, never hidden: a scaffold that keeps the
/// learner's file is normal, a solution that cannot (or will not) apply is a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    /// Path the patch skipped.
    pub path: String,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// The result of looking at a patch and a tree without changing either.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Files that would change.
    pub edits: Vec<Edit>,
    /// Files the patch left alone.
    pub skipped: Vec<Skip>,
}

impl Plan {
    /// `true` when applying the patch would change nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Write the planned files, checking that the tree still matches the plan.
    ///
    /// A write that fails part way is taken back before the error is reported: a file the plan
    /// created is removed again, a file it modified or deleted gets its previous content back. One
    /// failed edit therefore costs a retry, not a tree that is half the patch and has to be
    /// repaired by hand before anything fits again.
    pub fn apply(&self, tree: &Path) -> Result<()> {
        let mut applied: Vec<&Edit> = Vec::new();
        if let Err(error) = self.write_edits(tree, &mut applied) {
            return match undo_edits(tree, &applied) {
                Ok(()) => Err(error),
                Err((path, source)) => Err(KilnError::Patch {
                    file: path.display().to_string(),
                    reason: format!(
                        "{error}; and putting the tree back failed too: {source}, so it may be \
                         half patched"
                    ),
                }),
            };
        }
        Ok(())
    }

    /// The writes themselves, in plan order; every edit is recorded as *about to be written*, so a
    /// failure in the write itself is undone too — `fs::write` truncates before it writes, and an
    /// edit that failed half way through its own write would otherwise leave a corrupted file the
    /// rollback never knows it owns.
    fn write_edits<'a>(&'a self, tree: &Path, applied: &mut Vec<&'a Edit>) -> Result<()> {
        for edit in &self.edits {
            let path = tree.join(&edit.path);
            if let Some(before) = &edit.before {
                let current = std::fs::read_to_string(&path).map_err(|source| KilnError::Read {
                    path: path.clone(),
                    source,
                })?;
                if &current != before {
                    return Err(KilnError::Patch {
                        file: edit.path.clone(),
                        reason: "the file changed while the patch was being planned".to_owned(),
                    });
                }
            } else if path.exists() {
                return Err(KilnError::Patch {
                    file: edit.path.clone(),
                    reason: "the file appeared while the patch was being planned".to_owned(),
                });
            }
            applied.push(edit);
            match &edit.after {
                Some(content) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
                            path: parent.to_path_buf(),
                            source,
                        })?;
                    }
                    std::fs::write(&path, content).map_err(|source| KilnError::Write {
                        path: path.clone(),
                        source,
                    })?;
                }
                None => {
                    std::fs::remove_file(&path).map_err(|source| KilnError::Write {
                        path: path.clone(),
                        source,
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Files the plan would create.
    #[must_use]
    pub fn created(&self) -> Vec<&str> {
        self.edits
            .iter()
            .filter(|edit| edit.before.is_none() && edit.after.is_some())
            .map(|edit| edit.path.as_str())
            .collect()
    }

    /// Files the plan would modify.
    #[must_use]
    pub fn modified(&self) -> Vec<&str> {
        self.edits
            .iter()
            .filter(|edit| edit.before.is_some() && edit.after.is_some())
            .map(|edit| edit.path.as_str())
            .collect()
    }

    /// Files the plan would delete.
    #[must_use]
    pub fn deleted(&self) -> Vec<&str> {
        self.edits
            .iter()
            .filter(|edit| edit.after.is_none())
            .map(|edit| edit.path.as_str())
            .collect()
    }
}

/// Work out what applying `patch` to `tree` would do, without touching anything.
pub fn plan(tree: &Path, patch: &Patch, mode: Mode) -> Result<Plan> {
    if mode == Mode::Scaffold {
        patch.require_add_only()?;
    }
    let mut plan = Plan::default();
    for file in patch.sections() {
        plan_section(tree, file, mode, &mut plan)?;
    }
    Ok(plan)
}

fn plan_section(tree: &Path, file: &FileDiff, mode: Mode, plan: &mut Plan) -> Result<()> {
    let path = tree.join(&file.path);
    let current = read_optional(&path)?;
    let skip = |plan: &mut Plan, reason: SkipReason| {
        plan.skipped.push(Skip {
            path: file.path.clone(),
            reason,
        });
    };

    match (file.section, current) {
        (Section::Create, Some(content)) => {
            let wanted = created_content(file);
            if content == wanted {
                skip(plan, SkipReason::AlreadyDone);
            } else if mode.lenient() {
                skip(plan, SkipReason::KeptYours);
            } else {
                return Err(KilnError::Patch {
                    file: file.path.clone(),
                    reason: "the file already exists; the reference solution expects to create it"
                        .to_owned(),
                });
            }
        }
        (Section::Create, None) => {
            plan.edits.push(Edit {
                path: file.path.clone(),
                before: None,
                after: Some(created_content(file)),
            });
        }
        (Section::Delete, None) => skip(plan, SkipReason::AlreadyDone),
        (Section::Delete, Some(content)) => {
            let wanted = removed_content(file);
            if content == wanted {
                plan.edits.push(Edit {
                    path: file.path.clone(),
                    before: Some(content),
                    after: None,
                });
            } else if mode.lenient() {
                skip(plan, SkipReason::KeptYours);
            } else {
                // A solution that left the file alone would report success while leaving a tree
                // that is not the reference solution's — the one thing `kiln solution` is for. It
                // refuses instead, and the file keeps the learner's edits.
                return Err(KilnError::Patch {
                    file: file.path.clone(),
                    reason: "the file is not the version the solution expects to delete — it has \
                             been changed since the scaffold, and the solution will not destroy it"
                        .to_owned(),
                });
            }
        }
        (Section::Modify, None) => {
            return Err(KilnError::Patch {
                file: file.path.clone(),
                reason: "the file does not exist".to_owned(),
            });
        }
        (Section::Modify, Some(content)) => {
            let outcome = apply_hunks(&file.path, &content, file, mode)?;
            match outcome {
                Some(after) => plan.edits.push(Edit {
                    path: file.path.clone(),
                    before: Some(content),
                    after: Some(after),
                }),
                None => skip(plan, SkipReason::AlreadyDone),
            }
        }
    }
    Ok(())
}

/// Content of a file a patch creates: the added lines of its hunks.
fn created_content(file: &FileDiff) -> String {
    let mut lines = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            if line.kind == Kind::Add {
                lines.push(line.text.as_str());
            }
        }
    }
    join_lines(&lines)
}

/// Content a patch removes: the removed lines of its hunks.
fn removed_content(file: &FileDiff) -> String {
    let mut lines = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            if line.kind == Kind::Remove {
                lines.push(line.text.as_str());
            }
        }
    }
    join_lines(&lines)
}

fn join_lines(lines: &[&str]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Take back the edits applied so far, last one first: a created file is removed again, a modified
/// or deleted file gets its previous content back. The first failure stops the undo and is named,
/// because a tree the undo could not finish is a tree a person has to look at.
fn undo_edits(
    tree: &Path,
    applied: &[&Edit],
) -> std::result::Result<(), (PathBuf, std::io::Error)> {
    for edit in applied.iter().rev() {
        let path = tree.join(&edit.path);
        let undone = match &edit.before {
            Some(previous) => std::fs::write(&path, previous),
            // A file the plan created is simply gone again; failing to remove it costs nothing
            // that the failure being undone has not already cost.
            None => {
                let _ = std::fs::remove_file(&path);
                Ok(())
            }
        };
        if let Err(source) = undone {
            return Err((path, source));
        }
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(KilnError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Apply every hunk to `content`, or `None` when all of them were applied already.
fn apply_hunks(path: &str, content: &str, file: &FileDiff, mode: Mode) -> Result<Option<String>> {
    if content.contains('\r') {
        return Err(KilnError::Patch {
            file: path.to_owned(),
            reason: "the file uses CRLF line endings; the patch engine works on LF".to_owned(),
        });
    }
    let trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
    let mut changed = false;
    let mut cursor = 0usize;

    for (index, hunk) in file.hunks.iter().enumerate() {
        let old = hunk.old_side();
        let new = hunk.new_side();
        let expected = hunk.old_start.saturating_sub(1);
        // A scaffold's lines stay where they were put while the file around them grows: later steps
        // insert their own dependencies into the same marked block, and the lines this hunk was
        // written against are no longer its neighbours. Idempotence has to be a property of the
        // additions, not of their neighbourhood, so a scaffold whose added lines are all present
        // (inside a marked block) has nothing left to do.
        if mode == Mode::Scaffold && additions_are_present(&lines, hunk) {
            continue;
        }
        // The hunk's own line number wins when the file still matches there: a solution whose
        // earlier hunk inserted a copy of this hunk's context must not be pulled away from where it
        // was written by the copy being nearer to the cursor. Only a declared position that no
        // longer fits — the file grew above, a later step moved the block — falls through to the
        // nearest-content search. An *empty* old side (a pure insertion, `@@ -N,0 …`) matches
        // everywhere, so it cannot be trusted to say the file is unchanged there — and the position
        // it names was written against a file that earlier hunks in this same patch may already
        // have grown or shrunk. Those keep the cursor-aware search the fallback always used.
        let found = if !old.is_empty() && matches_at(&lines, &old, expected) {
            Some((expected, false))
        } else {
            match find(&lines, &old, cursor.max(expected)) {
                Some(position) => Some((position, false)),
                None => find(&lines, &new, cursor.min(expected)).map(|position| (position, true)),
            }
        };
        let Some((position, already)) = found else {
            return Err(KilnError::Patch {
                file: path.to_owned(),
                reason: format!(
                    "hunk {} does not fit: expected `{}`{}",
                    index + 1,
                    old.first().copied().unwrap_or(""),
                    context_hint(&lines, expected)
                ),
            });
        };
        if already {
            cursor = position + new.len();
            continue;
        }
        if mode == Mode::Scaffold {
            check_marker_block(path, &lines, hunk, position)?;
        }
        let replacement: Vec<String> = new.iter().map(|line| (*line).to_owned()).collect();
        lines.splice(position..position + old.len(), replacement);
        cursor = position + new.len();
        changed = true;
    }

    if !changed {
        return Ok(None);
    }
    let mut text = lines.join("\n");
    if trailing_newline || !text.is_empty() {
        text.push('\n');
    }
    Ok(Some(text))
}

/// Find `needle` in `haystack`, preferring the occurrence closest to `near`.
fn find(haystack: &[String], needle: &[&str], near: usize) -> Option<usize> {
    if needle.is_empty() {
        return (near <= haystack.len()).then_some(near.min(haystack.len()));
    }
    (0..=haystack.len())
        .filter(|start| matches_at(haystack, needle, *start))
        .min_by_key(|position| position.abs_diff(near))
}

/// `true` when `needle` sits at `start` in `haystack`.
fn matches_at(haystack: &[String], needle: &[&str], start: usize) -> bool {
    needle.iter().enumerate().all(|(offset, expected)| {
        haystack
            .get(start + offset)
            .is_some_and(|line| line == expected)
    })
}

/// Say what was found where the hunk expected its context, so the message can be acted on.
fn context_hint(lines: &[String], expected: usize) -> String {
    let Some(line) = lines.get(expected) else {
        return format!(", but the file has {} line(s)", lines.len());
    };
    format!(", but line {} is `{line}`", expected + 1)
}

/// Scaffold insertions may only land inside a `kiln:scaffold:begin` … `kiln:scaffold:end` block.
fn check_marker_block(path: &str, lines: &[String], hunk: &Hunk, position: usize) -> Result<()> {
    let mut offset = 0usize;
    for line in &hunk.lines {
        if line.kind == Kind::Add {
            let at = position + offset;
            if !inside_marker_block(lines, at) {
                return Err(KilnError::Invalid(format!(
                    "scaffold inserts `{}` into {path} outside a `{MARKER_BEGIN}`/`{MARKER_END}` \
                     block: scaffolds may only write inside marked regions (authoring.md §5)",
                    line.text
                )));
            }
        } else {
            offset += 1;
        }
    }
    Ok(())
}

/// `true` when every line this hunk adds is already in the file, inside a marked block.
///
/// This is what makes a scaffold idempotent across steps: the same `[dependencies]` block collects
/// insertions from several steps, so the context a hunk was written against can be gone while the
/// lines it adds are still exactly where it put them.
fn additions_are_present(lines: &[String], hunk: &Hunk) -> bool {
    hunk.lines
        .iter()
        .filter(|line| line.kind == Kind::Add)
        .all(|line| {
            lines.iter().enumerate().any(|(index, existing)| {
                existing == &line.text && inside_marker_block(lines, index)
            })
        })
}

/// `true` when `position` sits between a begin marker and its end marker.
fn inside_marker_block(lines: &[String], position: usize) -> bool {
    let is_marker = |line: &&String| line.contains(MARKER_BEGIN) || line.contains(MARKER_END);
    let opening = lines.iter().take(position).rev().find(is_marker);
    let closing = lines.iter().skip(position).find(is_marker);
    match (opening, closing) {
        (Some(open), Some(close)) => open.contains(MARKER_BEGIN) && close.contains(MARKER_END),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// A scaffold in its normal shape: one line inserted between the markers that are already in
    /// the file (authoring.md §5).
    const SCAFFOLD: &str = "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1,4 +1,5 @@\n [package]\n name = \"mailer\"\n # kiln:scaffold:begin\n+todo = \"deny\"\n # kiln:scaffold:end\n";

    /// A scaffold insertion that lands outside the marked region — a course bug.
    const OUTSIDE: &str = "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1,2 +1,3 @@\n [package]\n+rust-version = \"1.98\"\n name = \"mailer\"\n";

    /// The project state such a scaffold is applied to.
    const WITH_MARKERS: &str =
        "[package]\nname = \"mailer\"\n# kiln:scaffold:begin\n# kiln:scaffold:end\n";

    fn tree(label: &str, files: &[(&str, &str)]) -> TempDir {
        let temp = TempDir::new(label);
        for (path, content) in files {
            temp.write(path, content);
        }
        temp
    }

    #[test]
    fn a_hunk_header_is_parsed_into_ranges_and_lines() {
        let patch = Patch::parse(SCAFFOLD).expect("patch should parse");
        let section = patch.sections().first().expect("one section");
        assert_eq!(section.path, "Cargo.toml");
        assert_eq!(section.section, Section::Modify);
        let hunk = section.hunks.first().expect("one hunk");
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 4);
        assert_eq!(hunk.lines.len(), 5);
        assert!(hunk.is_add_only());
    }

    #[test]
    fn a_patch_without_a_file_section_is_rejected() {
        let error = Patch::parse("just prose\n").unwrap_err();
        assert!(error.to_string().contains("no file section"), "{error}");
    }

    #[test]
    fn content_that_looks_like_a_header_stays_content() {
        // The line a hunk carries decides nothing: a *removed* SQL comment arrives as `--- 0001 …`
        // and an added one as `+++ …`, because the `-`/`+` tag is added to the comment's own
        // dashes. Only the counts in the header say how many lines belong to the hunk, so a patch
        // that edits a file full of `--` comments has to parse like any other (found by
        // `mailer/1.9-postgres`'s solution, whose first migration is one big comment).
        let text = "--- a/migrations/0001.sql\n+++ b/migrations/0001.sql\n@@ -1,3 +1,2 @@\n\
                    --- 0001 — a comment\n -- a context comment\n-old line\n+new line\n";
        let patch = Patch::parse(text).expect("a patch of SQL comments should parse");
        let hunk = patch.sections().first().expect("one section").hunks.first();
        let hunk = hunk.expect("one hunk");
        assert_eq!(hunk.old_count, 3);
        assert_eq!(hunk.lines.len(), 4);
        assert_eq!(hunk.lines[0].kind, Kind::Remove);
        assert_eq!(hunk.lines[0].text, "-- 0001 — a comment");
        assert_eq!(hunk.lines[1].kind, Kind::Context);
        assert_eq!(hunk.lines[2].kind, Kind::Remove);
        assert_eq!(hunk.lines[3].kind, Kind::Add);
    }

    #[test]
    fn a_hunk_that_does_not_carry_what_its_header_promised_is_rejected() {
        // The counts are the contract; a truncated hunk is a broken patch, not a short one.
        let text = "--- a/lib.rs\n+++ b/lib.rs\n@@ -1,3 +1,3 @@\n context\n+added\n";
        let error = Patch::parse(text).unwrap_err();
        assert!(error.to_string().contains("declares 3 old"), "{error}");
    }

    #[test]
    fn a_removal_makes_a_patch_unusable_as_a_scaffold() {
        let text = "--- a/lib.rs\n+++ b/lib.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n context\n";
        let patch = Patch::parse(text).expect("patch should parse");
        let error = patch.require_add_only().unwrap_err();
        assert!(error.to_string().contains("removes `old`"), "{error}");
    }

    #[test]
    fn reversing_an_insertion_only_patch_is_what_reset_applies() {
        // Reset applies the patch backwards, so an insertion becomes a removal — and a patch
        // without additions is exactly what the reversal of a scaffold looks like.
        let reversed = Patch::parse(SCAFFOLD)
            .expect("patch should parse")
            .reversed();
        let hunk = &reversed.sections().first().expect("one section").hunks[0];
        assert!(!hunk.is_add_only());
        assert!(
            hunk.lines.iter().all(|line| line.kind != Kind::Add),
            "{hunk:?}"
        );
        assert_eq!(
            hunk.lines
                .iter()
                .filter(|line| line.kind == Kind::Remove)
                .count(),
            1
        );
    }

    #[test]
    fn a_scaffold_inserts_only_inside_a_marked_block() {
        let temp = tree("patch-marker-ok", &[("Cargo.toml", WITH_MARKERS)]);
        let patch = Patch::parse(SCAFFOLD).expect("patch should parse");
        let plan = plan(temp.path(), &patch, Mode::Scaffold).expect("plan should succeed");
        let edit = plan.edits.first().expect("one edit");
        let after = edit.after.clone().expect("content");
        assert!(
            after.contains("# kiln:scaffold:begin\ntodo = \"deny\"\n# kiln:scaffold:end"),
            "{after}"
        );
    }

    #[test]
    fn a_scaffold_that_inserts_outside_a_marked_block_is_refused() {
        let temp = tree("patch-marker-bad", &[("Cargo.toml", WITH_MARKERS)]);
        let patch = Patch::parse(OUTSIDE).expect("patch should parse");
        let error = plan(temp.path(), &patch, Mode::Scaffold).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("outside a `kiln:scaffold:begin`"),
            "{error}"
        );
    }

    #[test]
    fn a_solution_may_remove_and_rewrite_lines() {
        let temp = tree("patch-solution", &[("lib.rs", "fn main() {}\n")]);
        let text = "--- a/lib.rs\n+++ b/lib.rs\n@@ -1 +1,3 @@\n-fn main() {}\n+fn main() -> u8 {\n+    0\n+}\n";
        let patch = Patch::parse(text).expect("patch should parse");
        let plan = plan(temp.path(), &patch, Mode::Solution).expect("plan should succeed");
        plan.apply(temp.path()).expect("apply should succeed");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("lib.rs")).unwrap(),
            "fn main() -> u8 {\n    0\n}\n"
        );
    }

    #[test]
    fn applying_a_scaffold_twice_changes_nothing_the_second_time() {
        let temp = tree("patch-idempotent", &[("Cargo.toml", WITH_MARKERS)]);
        let patch = Patch::parse(SCAFFOLD).expect("patch should parse");
        plan(temp.path(), &patch, Mode::Scaffold)
            .expect("first plan")
            .apply(temp.path())
            .expect("first apply");
        let after_first = std::fs::read_to_string(temp.path().join("Cargo.toml")).unwrap();
        let second = plan(temp.path(), &patch, Mode::Scaffold).expect("second plan");
        assert!(second.is_empty(), "{second:?}");
        assert_eq!(second.skipped.len(), 1);
        second.apply(temp.path()).expect("second apply");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("Cargo.toml")).unwrap(),
            after_first
        );
    }

    #[test]
    fn a_scaffold_stays_applied_when_a_later_step_grows_the_same_block() {
        // The shape every `[dependencies]` block in a track has: step 4 inserts two lines, step 6
        // inserts three more into the same block. Re-running step 4 has to be a no-op, not a second
        // copy of its lines — the context it was written against is gone, the lines are not.
        let temp = tree("patch-grown-block", &[("Cargo.toml", WITH_MARKERS)]);
        let first = Patch::parse(SCAFFOLD).expect("patch should parse");
        plan(temp.path(), &first, Mode::Scaffold)
            .expect("first plan")
            .apply(temp.path())
            .expect("first apply");

        let grown = std::fs::read_to_string(temp.path().join("Cargo.toml"))
            .unwrap()
            .replace(
                "# kiln:scaffold:end",
                "another = \"1\"\n# kiln:scaffold:end",
            );
        std::fs::write(temp.path().join("Cargo.toml"), grown).unwrap();

        let again = plan(temp.path(), &first, Mode::Scaffold).expect("second plan");
        assert!(again.is_empty(), "{again:?}");
        again.apply(temp.path()).expect("second apply");
        let after = std::fs::read_to_string(temp.path().join("Cargo.toml")).unwrap();
        assert_eq!(
            after.matches("todo = \"deny\"").count(),
            1,
            "the line must not be inserted twice: {after}"
        );
    }

    #[test]
    fn a_hunk_that_does_not_fit_names_the_file_and_the_line() {
        let temp = tree(
            "patch-conflict",
            &[("Cargo.toml", "[package]\nname = \"other\"\n")],
        );
        let patch = Patch::parse(SCAFFOLD).expect("patch should parse");
        let error = plan(temp.path(), &patch, Mode::Scaffold).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Cargo.toml"), "{message}");
        assert!(message.contains("hunk 1 does not fit"), "{message}");
    }

    #[test]
    fn creating_a_file_is_idempotent_and_never_clobbers_the_learner() {
        let text =
            "--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1,2 @@\n+//! mailer.\n+pub fn run() {}\n";
        let patch = Patch::parse(text).expect("patch should parse");

        let temp = tree("patch-create", &[]);
        plan(temp.path(), &patch, Mode::Scaffold)
            .expect("plan")
            .apply(temp.path())
            .expect("apply");
        assert!(temp.path().join("src/lib.rs").is_file());
        let second = plan(temp.path(), &patch, Mode::Scaffold).expect("second plan");
        assert!(second.is_empty());

        temp.write("src/lib.rs", "//! mine\n");
        let third = plan(temp.path(), &patch, Mode::Scaffold).expect("third plan");
        assert!(third.is_empty());
        assert_eq!(third.skipped.first().unwrap().reason, SkipReason::KeptYours);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src/lib.rs")).unwrap(),
            "//! mine\n"
        );
    }

    #[test]
    fn a_solution_refuses_to_overwrite_a_file_it_did_not_expect() {
        let temp = tree("patch-solution-conflict", &[("src/lib.rs", "//! mine\n")]);
        let text = "--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1 @@\n+//! theirs\n";
        let patch = Patch::parse(text).expect("patch should parse");
        let error = plan(temp.path(), &patch, Mode::Solution).unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
    }

    #[test]
    fn reversing_a_patch_takes_an_insertion_back_out() {
        let temp = tree("patch-reverse", &[("Cargo.toml", WITH_MARKERS)]);
        let patch = Patch::parse(SCAFFOLD).expect("patch should parse");
        plan(temp.path(), &patch, Mode::Scaffold)
            .expect("plan")
            .apply(temp.path())
            .expect("apply");
        let reversed = patch.reversed();
        plan(temp.path(), &reversed, Mode::Revert)
            .expect("reverse plan")
            .apply(temp.path())
            .expect("reverse apply");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("Cargo.toml")).unwrap(),
            WITH_MARKERS
        );
        assert!(
            plan(temp.path(), &reversed, Mode::Revert)
                .expect("third plan")
                .is_empty()
        );
    }

    #[test]
    fn reversing_a_creation_keeps_a_file_the_learner_edited() {
        let text = "--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1 @@\n+//! scaffold\n";
        let patch = Patch::parse(text).expect("patch should parse");
        let temp = tree("patch-reverse-create", &[("src/lib.rs", "//! mine\n")]);
        let reversed = plan(temp.path(), &patch.reversed(), Mode::Revert).expect("reverse plan");
        assert!(reversed.is_empty());
        assert_eq!(
            reversed.skipped.first().unwrap().reason,
            SkipReason::KeptYours
        );
        reversed.apply(temp.path()).expect("apply");
        assert!(temp.path().join("src/lib.rs").is_file());

        let untouched = tree(
            "patch-reverse-create-clean",
            &[("src/lib.rs", "//! scaffold\n")],
        );
        plan(untouched.path(), &patch.reversed(), Mode::Revert)
            .expect("reverse plan")
            .apply(untouched.path())
            .expect("reverse apply");
        assert!(!untouched.path().join("src/lib.rs").exists());
    }

    #[test]
    fn a_missing_file_cannot_be_patched() {
        let temp = tree("patch-missing", &[]);
        let patch = Patch::parse(SCAFFOLD).expect("patch should parse");
        let error = plan(temp.path(), &patch, Mode::Solution).unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
    }

    #[test]
    fn a_renaming_patch_is_refused_instead_of_applied_as_a_modification() {
        // `git diff` writes a rename as `rename from` … `rename to`, and this applier only knows
        // delete and create: applying the section as a modification would put the content in the
        // file the patch meant to remove it from (found the hard way, with a `.sqlx` cache file
        // whose name *is* its hash).
        let text = "diff --git a/src/old.rs b/src/new.rs\n\
                    similarity index 62%\n\
                    rename from src/old.rs\n\
                    rename to src/new.rs\n\
                    index e69de29..4b825dc 100644\n\
                    --- a/src/old.rs\n\
                    +++ b/src/new.rs\n\
                    @@ -1 +1 @@\n\
                    -//! old\n\
                    +//! new\n";
        let error = Patch::parse(text).expect_err("a rename must not parse");
        assert!(
            error.to_string().contains("--no-renames"),
            "the error has to name the way to write it instead: {error}"
        );
    }

    #[test]
    fn a_patch_that_removes_a_line_from_a_marked_block_is_still_refused() {
        let text = "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1,3 +1,2 @@\n [package]\n-# kiln:scaffold:begin\n name = \"mailer\"\n";
        let temp = tree(
            "patch-scaffold-remove",
            &[("Cargo.toml", "[package]\nname = \"x\"\n")],
        );
        let patch = Patch::parse(text).expect("patch should parse");
        let error = plan(temp.path(), &patch, Mode::Scaffold).unwrap_err();
        assert!(error.to_string().contains("authoring.md §5"), "{error}");
    }

    #[test]
    fn an_apply_that_fails_part_way_puts_the_earlier_files_back() {
        // Two files are planned; the second is changed after planning, so the apply dies on it.
        // The first file — already written by then — has to read its old content afterwards: a
        // failed solution must not leave half of itself behind for the retry to trip over.
        let temp = tree(
            "patch-rollback",
            &[("a.rs", "//! a\n"), ("b.rs", "//! b\n")],
        );
        let text = "--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-//! a\n+//! changed\n\
                    --- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-//! b\n+//! changed\n";
        let patch = Patch::parse(text).expect("patch should parse");
        let plan = plan(temp.path(), &patch, Mode::Solution).expect("plan should succeed");
        temp.write("b.rs", "//! mine\n");

        let error = plan.apply(temp.path()).unwrap_err();
        assert!(error.to_string().contains("b.rs"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("a.rs")).unwrap(),
            "//! a\n",
            "the edit that did land was taken back"
        );

        // And the tree is whole again, so fixing the named file is enough for a retry.
        temp.write("b.rs", "//! b\n");
        plan.apply(temp.path()).expect("the retry succeeds");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("a.rs")).unwrap(),
            "//! changed\n"
        );
    }

    #[test]
    fn an_apply_that_fails_takes_a_created_file_back_out() {
        let temp = tree("patch-rollback-create", &[("b.rs", "//! b\n")]);
        let text = "--- /dev/null\n+++ b/a.rs\n@@ -0,0 +1 @@\n+//! created\n\
                    --- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-//! b\n+//! changed\n";
        let patch = Patch::parse(text).expect("patch should parse");
        let plan = plan(temp.path(), &patch, Mode::Solution).expect("plan should succeed");
        temp.write("b.rs", "//! mine\n");

        plan.apply(temp.path()).unwrap_err();
        assert!(
            !temp.path().join("a.rs").exists(),
            "the file the plan created is gone again"
        );
    }

    #[test]
    fn a_solution_refuses_to_delete_a_file_it_does_not_recognise() {
        let text = "--- a/generated.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-//! generated\n";
        let patch = Patch::parse(text).expect("patch should parse");

        // The learner's version differs from what the scaffold wrote: a solution that skipped the
        // deletion would report success while leaving a tree that is not the reference solution's.
        let edited = tree("patch-solution-delete", &[("generated.rs", "//! mine\n")]);
        let error = plan(edited.path(), &patch, Mode::Solution).unwrap_err();
        assert!(error.to_string().contains("delete"), "{error}");
        assert_eq!(
            std::fs::read_to_string(edited.path().join("generated.rs")).unwrap(),
            "//! mine\n",
            "and the learner's file keeps its edits"
        );

        // The same mismatch under `kiln reset` — which applies the deletion backwards, as a
        // creation — is the normal case: the revert keeps the learner's file.
        let revert = plan(edited.path(), &patch.reversed(), Mode::Revert).expect("revert plan");
        assert!(revert.is_empty(), "{revert:?}");
        assert_eq!(
            revert.skipped.first().unwrap().reason,
            SkipReason::KeptYours
        );

        // And the version the scaffold wrote is deleted as the solution says.
        let pristine = tree(
            "patch-solution-delete-ok",
            &[("generated.rs", "//! generated\n")],
        );
        plan(pristine.path(), &patch, Mode::Solution)
            .expect("plan")
            .apply(pristine.path())
            .expect("apply");
        assert!(!pristine.path().join("generated.rs").exists());
    }

    #[test]
    fn a_hunk_is_anchored_to_its_declared_line_before_the_content_search() {
        // The first hunk inserts a copy of the second hunk's context below it. The copy is nearer
        // to the cursor the first hunk leaves behind, but the second hunk's own line number still
        // fits, and that is where it belongs: a nearest-content search alone would edit the copy
        // the patch itself just wrote.
        let text = "--- a/lib.rs\n+++ b/lib.rs\n\
                    @@ -3,1 +3,3 @@\n two\n+ctx\n+block\n\
                    @@ -1,2 +1,2 @@\n ctx\n-block\n+BLOCK\n";
        let temp = tree("patch-anchor", &[("lib.rs", "ctx\nblock\ntwo\n")]);
        let patch = Patch::parse(text).expect("patch should parse");

        plan(temp.path(), &patch, Mode::Solution)
            .expect("plan")
            .apply(temp.path())
            .expect("apply");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("lib.rs")).unwrap(),
            "ctx\nBLOCK\ntwo\nctx\nblock\n"
        );
    }
}
