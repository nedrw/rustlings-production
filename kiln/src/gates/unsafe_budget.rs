//! `unsafe-budget@N` — no unexplained `unsafe` in production code, and none at all when the step
//! says zero (handoff M2 §1).
//!
//! The point of the gate is the *reason*, not the count: `unsafe` is a promise the compiler cannot
//! check, so every block has to say in a `// SAFETY:` note what makes it sound. Three things are
//! judged, and each one is a sentence the reader of a diff would otherwise have to write:
//!
//! * the production sources **this step writes** — every `src/**/*.rs` file its scaffold or its
//!   reference solution touches — hold at most `N` occurrences of the word `unsafe`. A file the step
//!   never touches belongs to the step that wrote it: without that scope, the graduation tree's
//!   `src/ring.rs` (`5.19`'s) would count against every earlier step's zero, and `verify` could not
//!   judge a finished project's early steps at all (handoff-m4 段 14);
//! * every occurrence has a `// SAFETY:` note within the three lines above it;
//! * when `N` is `0`, the project's manifest denies `unsafe_code` — the compiler is then the one
//!   holding the line, and an `unsafe` block cannot appear without an explicit `allow`.
//!
//! Why `N` and not always zero: the measure of `unsafe/budget@apply` is knowing where it is
//! justified, and a step that ships one documented `unsafe` block with the note next to it has
//! learned exactly that. Why the lint is only required at `N == 0`: a manifest that denies
//! `unsafe_code` while the gate allows two occurrences would be two rules disagreeing, and the one a
//! reader believes would be whichever they read first.
//!
//! Test code is not production code: `#[cfg(test)]`-gated items inside a source file are skipped, and
//! the walk over `src/` never reaches `tests/`. Two things are read as text rather than compiled
//! ([`crate::gates::lexer`] keeps `unsafe` inside a string or a comment from being counted), which is
//! why the gate needs no tool beyond the file system and answers in milliseconds.

use std::ops::Range;
use std::path::Path;

use serde::Deserialize;

use crate::error::{KilnError, Result};
use crate::gates::lexer::{Class, classify, is_identifier_byte};
use crate::gates::runner::GateContext;
use crate::gates::sources;
use crate::gates::{GateOutcome, GateSpec};

/// How many lines above an `unsafe` its `// SAFETY:` note may sit.
const LOOKBACK: usize = 3;

/// The word as it is written in Rust.
const UNSAFE: &str = "unsafe";

/// The marker that makes an `unsafe` explained.
const SAFETY: &str = "SAFETY:";

/// One `unsafe` in production code.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Occurrence {
    /// File, relative to the project.
    file: String,
    /// 1-based line number.
    line: usize,
    /// `true` when a `// SAFETY:` note is within the three lines above.
    documented: bool,
}

impl Occurrence {
    /// `src/queue.rs:42` — how a report names the offender.
    fn place(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// Count the `unsafe` in the step's production code and judge how it is explained.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    let allowed = spec.numeric_param()? as usize;
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let track_dir = ctx.track.dir(ctx.course.root());
    let written = sources::patched_sources(&track_dir, ctx.step)?;
    if written.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "{}'s scaffold and reference solution touch no `src/**/*.rs`, so there is no production \
             code this step is answerable for",
            ctx.step.qualified_id()
        )));
    }
    let files: Vec<String> = written
        .into_iter()
        .filter(|path| project.join(path).is_file())
        .collect();
    if files.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "none of the production files this step writes exists in {} yet",
            ctx.project_dir.display()
        )));
    }
    let mut found: Vec<Occurrence> = Vec::new();
    for file in &files {
        let text =
            std::fs::read_to_string(project.join(file)).map_err(|source| KilnError::Read {
                path: project.join(file),
                source,
            })?;
        found.extend(occurrences(file, &text));
    }

    if found.len() > allowed {
        return Ok(GateOutcome::failed(format!(
            "{} `unsafe` occurrence(s) in the production files this step writes, and this step allows \
             {allowed}: {}",
            found.len(),
            found
                .iter()
                .map(Occurrence::place)
                .collect::<Vec<String>>()
                .join(", ")
        )));
    }
    if let Some(offender) = found.iter().find(|occurrence| !occurrence.documented) {
        return Ok(GateOutcome::failed(format!(
            "`unsafe` at {} has no `// {SAFETY}` note in the {LOOKBACK} lines above it — an `unsafe` \
             block is a promise the compiler cannot check, and the note is where the promise is \
             written down",
            offender.place()
        )));
    }

    // The deny holds while the project is free of `unsafe`. Once a later step legitimately writes
    // some, the manifest cannot keep denying it, and this step cannot re-deny it: the promise this
    // check enforces — "the next block is a compile error, not a review finding" — has nothing left
    // to protect.
    let project_total: usize = production_sources(project)?
        .iter()
        .map(|file| {
            std::fs::read_to_string(project.join(file))
                .map(|text| occurrences(file, &text).len())
                .unwrap_or(0)
        })
        .sum();
    let denied = denies_unsafe_code(project);
    if deny_required(allowed, denied, project_total) {
        return Ok(GateOutcome::failed(format!(
            "`unsafe-budget@0` asks for no `unsafe` at all, so the project has to deny it — add \
             `[lints.rust] unsafe_code = \"deny\"` to {} (the compiler then rejects the next block \
             instead of this gate)",
            project.join("Cargo.toml").display()
        )));
    }

    Ok(GateOutcome::passed(passed_line(
        found.len(),
        files.len(),
        allowed,
        &project.join("Cargo.toml"),
        denied,
    )))
}

/// Whether this step's `@0` still needs the manifest to deny `unsafe_code`.
///
/// It does while the project holds no `unsafe` anywhere: the deny is what turns the learner's next
/// block into a compile error. Once a later step writes `unsafe` on purpose, the deny cannot stand,
/// and an earlier step judged in that tree must not be failed for a manifest it no longer controls.
fn deny_required(allowed: usize, denied: bool, project_total: usize) -> bool {
    allowed == 0 && !denied && project_total == 0
}

/// The pass line: the counts, the note clause only when there is an occurrence for it to describe, and
/// the policy this step is under.
///
/// A step at `@0` printed "0 `unsafe` occurrence(s) … , each with a `// SAFETY:` note" before this:
/// nothing in it is false, and it still reads as if a note were expected from a step that has no
/// `unsafe` at all (measured on `lakelet/4.7-columnar-segments`, whose line is read by every step from
/// there on).
fn passed_line(
    occurrences: usize,
    files: usize,
    allowed: usize,
    manifest: &Path,
    denied: bool,
) -> String {
    let notes = if occurrences == 0 {
        String::new()
    } else {
        format!(", each with a `// {SAFETY}` note")
    };
    let policy = if allowed == 0 {
        if denied {
            format!(", and `unsafe_code` is denied in {}", manifest.display())
        } else {
            " (a later step raised the budget, so the manifest no longer denies it)".to_owned()
        }
    } else {
        format!(" (this step allows {allowed})")
    };
    format!(
        "{occurrences} `unsafe` occurrence(s) in the {files} production file(s) this step writes\
         {notes}{policy}"
    )
}

/// The `unsafe` occurrences of one file's production code.
fn occurrences(file: &str, text: &str) -> Vec<Occurrence> {
    let classes = classify(text);
    let tests = test_regions(text, &classes);
    let lines = lines_of(text, &classes);
    let notes: Vec<bool> = lines
        .iter()
        .map(|line| has_safety_note(line.text, line.classes))
        .collect();

    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        for column in unsafe_words(line.text, line.classes) {
            let offset = line.offset + column;
            if tests
                .iter()
                .any(|region| region.start <= offset && offset < region.end)
            {
                continue;
            }
            let documented = (1..=LOOKBACK).any(|back| earlier_note(&notes, index, back));
            found.push(Occurrence {
                file: file.to_owned(),
                line: index + 1,
                documented,
            });
        }
    }
    found
}

/// One line of a source file with its classes, and where it starts in the file.
#[derive(Debug, Clone, Copy)]
struct Line<'a> {
    /// Byte offset the line starts at.
    offset: usize,
    /// The line, without its newline.
    text: &'a str,
    /// The class of every byte of `text`.
    classes: &'a [Class],
}

/// The lines of a file, each carrying its own slice of the class list.
///
/// Slicing per line is what lets a gate read a line at a time while the lexer stays right about the
/// block comment or the raw string that spans several of them.
fn lines_of<'a>(text: &'a str, classes: &'a [Class]) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for chunk in text.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        lines.push(Line {
            offset,
            text: line,
            classes: classes.get(offset..offset + line.len()).unwrap_or_default(),
        });
        offset += chunk.len();
    }
    lines
}

/// `true` when the line carries a `// SAFETY:` note in one of its comments.
fn has_safety_note(line: &str, classes: &[Class]) -> bool {
    let commented: String = line
        .bytes()
        .zip(classes)
        .filter(|(_, class)| **class == Class::Comment)
        .map(|(byte, _)| byte as char)
        .collect();
    commented.contains(SAFETY)
}

/// The `unsafe` words in the code of one line, as byte offsets.
fn unsafe_words(line: &str, classes: &[Class]) -> Vec<usize> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if is_code(classes, at) && stands_alone(bytes, at) {
            found.push(at);
        }
        at += 1;
    }
    found
}

/// `true` when the word `unsafe` starts at `at` and is not part of a longer name.
///
/// `unsafe_code` is the name of a *lint*, not a use of `unsafe`, and a lint that a project denies
/// would otherwise count as the thing it forbids.
fn stands_alone(bytes: &[u8], at: usize) -> bool {
    if bytes.get(at..at + UNSAFE.len()) != Some(UNSAFE.as_bytes()) {
        return false;
    }
    let before = at > 0 && bytes.get(at - 1).copied().is_some_and(is_identifier_byte);
    let after = bytes
        .get(at + UNSAFE.len())
        .copied()
        .is_some_and(is_identifier_byte);
    !before && !after
}

/// `true` when the byte at `at` is code (never a literal, never a comment).
fn is_code(classes: &[Class], at: usize) -> bool {
    classes.get(at).copied().unwrap_or(Class::Code).is_code()
}

/// Whether a note sits `back` lines above line `index`.
fn earlier_note(notes: &[bool], index: usize, back: usize) -> bool {
    index
        .checked_sub(back)
        .and_then(|earlier| notes.get(earlier))
        .copied()
        .unwrap_or(false)
}

/// The byte ranges of `#[cfg(test)]`-gated items, which are test code even inside `src/`.
fn test_regions(text: &str, classes: &[Class]) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut regions = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if !is_code(classes, at) || bytes.get(at..at + 2) != Some(b"#[") {
            at += 1;
            continue;
        }
        let Some(end) = attribute_end(bytes, classes, at) else {
            at += 1;
            continue;
        };
        if !is_cfg_test(bytes, classes, at, end) {
            at = end;
            continue;
        }
        // The attribute is followed by the item it gates: a block, or a declaration that has no body
        // (`mod tests;`, `use …;`) and therefore no region to skip.
        if let Some(region) = item_body(bytes, classes, end) {
            regions.push(region);
        }
        at = end;
    }
    regions
}

/// `true` when the attribute spanning `at..end` is `#[cfg(… test …)]`.
///
/// Inside a string literal the word does not count, so `#[cfg(feature = "test-utils")]` is not a
/// test gate: masking the literals is what tells the two apart.
fn is_cfg_test(bytes: &[u8], classes: &[Class], at: usize, end: usize) -> bool {
    let Some(attribute) = bytes.get(at..end) else {
        return false;
    };
    let code: Vec<u8> = attribute
        .iter()
        .copied()
        .zip(classes.get(at..end).unwrap_or_default())
        .map(|(byte, class)| if class.is_code() { byte } else { b' ' })
        .collect();
    let has = |needle: &[u8]| code.windows(needle.len()).any(|window| window == needle);
    has(b"cfg(") && has(b"test")
}

/// End of the `#[…]` attribute starting at `at`.
fn attribute_end(bytes: &[u8], classes: &[Class], at: usize) -> Option<usize> {
    let mut scan = at + 1;
    while scan < bytes.len() {
        if is_code(classes, scan) && bytes.get(scan) == Some(&b']') {
            return Some(scan + 1);
        }
        scan += 1;
    }
    None
}

/// The `{ … }` body of the item after `at`, matched by braces.
///
/// A `;` before the first `{` means the item has no body here (`use …;`, `mod tests;`), and looking
/// further would swallow the *next* item — which would hide production code behind a test attribute.
fn item_body(bytes: &[u8], classes: &[Class], at: usize) -> Option<Range<usize>> {
    let mut scan = at;
    while scan < bytes.len() {
        if is_code(classes, scan) {
            match bytes.get(scan) {
                Some(b';') => return None,
                Some(b'{') => return brace_end(bytes, classes, scan).map(|end| scan..end),
                _ => {}
            }
        }
        scan += 1;
    }
    None
}

/// End of the block that `open` opens, or `None` when it never closes.
fn brace_end(bytes: &[u8], classes: &[Class], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut scan = open;
    while scan < bytes.len() {
        if is_code(classes, scan) {
            match bytes.get(scan) {
                Some(b'{') => depth += 1,
                Some(b'}') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(scan + 1);
                    }
                }
                _ => {}
            }
        }
        scan += 1;
    }
    None
}

/// Every `.rs` file below `src/`, as paths relative to the project, in path order.
fn production_sources(project: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    walk(project, &project.join("src"), &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(root: &Path, directory: &Path, found: &mut Vec<String>) -> Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(KilnError::Read {
                path: directory.to_path_buf(),
                source,
            });
        }
    };
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
        if path.is_dir() {
            walk(root, &path, found)?;
            continue;
        }
        if !path.extension().is_some_and(|extension| extension == "rs") {
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            KilnError::Invalid(format!(
                "{} is not below {}",
                path.display(),
                root.display()
            ))
        })?;
        found.push(relative.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

/// `true` when the project's manifest denies `unsafe_code`.
///
/// The project under `app/<track>` is its own cargo workspace (plan §3 D6), so both spellings of the
/// policy are read from the same file: `[lints.rust]` for the package and `[workspace.lints.rust]`
/// for the members it carries. Either `deny` or `forbid` counts — one is the other with the loosen
/// option taken away.
fn denies_unsafe_code(project: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(project.join("Cargo.toml")) else {
        return false;
    };
    let Ok(manifest) = toml::from_str::<Manifest>(&text) else {
        return false;
    };
    let package = manifest.lints.and_then(|lints| lints.rust);
    let workspace = manifest
        .workspace
        .and_then(|workspace| workspace.lints)
        .and_then(|lints| lints.rust);
    [package, workspace]
        .into_iter()
        .flatten()
        .filter_map(|lints| lints.unsafe_code)
        .any(|level| level.denies())
}

/// What a manifest says about one lint: `"deny"`, or `{ level = "deny" }` (both spellings are cargo
/// syntax, and a policy that is written down but not read is worse than no policy).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Level {
    /// `unsafe_code = "deny"`.
    Simple(String),
    /// `unsafe_code = { level = "deny" }`.
    Detailed {
        /// The level cargo would read.
        level: String,
    },
}

impl Level {
    fn denies(&self) -> bool {
        let level = match self {
            Level::Simple(level) | Level::Detailed { level } => level,
        };
        level == "deny" || level == "forbid"
    }
}

/// The two places a manifest may state lint policy.
#[derive(Debug, Deserialize)]
struct Manifest {
    /// `[lints]` of the package itself.
    #[serde(default)]
    lints: Option<Lints>,
    /// `[workspace]`, whose lints the members may enable.
    #[serde(default)]
    workspace: Option<Workspace>,
}

/// `[workspace]`.
#[derive(Debug, Deserialize)]
struct Workspace {
    /// `[workspace.lints]`.
    #[serde(default)]
    lints: Option<Lints>,
}

/// `[lints]`.
#[derive(Debug, Deserialize)]
struct Lints {
    /// `[lints.rust]`.
    #[serde(default)]
    rust: Option<RustLints>,
}

/// `[lints.rust]`.
#[derive(Debug, Deserialize)]
struct RustLints {
    /// The `unsafe_code` policy, when the manifest states one.
    #[serde(default)]
    unsafe_code: Option<Level>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn places(file: &str, text: &str) -> Vec<String> {
        occurrences(file, text)
            .iter()
            .map(Occurrence::place)
            .collect()
    }

    #[test]
    fn the_word_unsafe_is_counted_with_its_line() {
        let text = "fn a() {\n    unsafe { poke() };\n}\n\nunsafe impl Send for X {}\n";
        assert_eq!(places("src/lib.rs", text), ["src/lib.rs:2", "src/lib.rs:5"]);
    }

    #[test]
    fn the_name_of_the_lint_is_not_a_use_of_it() {
        let text = "#![forbid(unsafe_code)]\n\nfn a() -> u32 { 1 }\n";
        assert!(places("src/lib.rs", text).is_empty(), "{text}");
    }

    #[test]
    fn text_and_comments_are_not_code() {
        let text = "fn a() {\n    let s = \"unsafe\";\n    // unsafe is handled in the store\n    /* unsafe */\n}\n";
        assert!(places("src/lib.rs", text).is_empty(), "{text}");
    }

    #[test]
    fn a_safety_note_within_three_lines_is_what_makes_it_explained() {
        let documented = occurrences(
            "src/lib.rs",
            "fn a() {\n    // SAFETY: the pointer is the one we just allocated.\n    unsafe { poke() };\n}\n",
        );
        assert_eq!(documented.len(), 1);
        assert!(documented[0].documented, "{documented:?}");

        let too_far = occurrences(
            "src/lib.rs",
            "// SAFETY: it was true once.\n//\n//\n//\nunsafe { poke() }\n",
        );
        assert!(!too_far[0].documented, "{too_far:?}");

        let elsewhere = occurrences(
            "src/lib.rs",
            "// SAFETY: another block.\nunsafe { poke() }\n",
        );
        assert!(
            elsewhere[0].documented,
            "the nearest note above is the one that counts"
        );
    }

    #[test]
    fn a_safety_word_outside_a_comment_is_not_a_note() {
        let found = occurrences(
            "src/lib.rs",
            "fn a() {\n    let s = \"SAFETY: even better\";\n    unsafe { poke() };\n}\n",
        );
        assert!(!found[0].documented, "{found:?}");
    }

    #[test]
    fn test_code_inside_a_source_file_is_not_production_code() {
        let text = "fn a() { unsafe { poke() }; }\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    \
                    #[test]\n    fn it_works() {\n        unsafe { poke() };\n    }\n}\n\nfn b() {\n    \
                    unsafe { poke() };\n}\n";
        assert_eq!(
            places("src/lib.rs", text),
            ["src/lib.rs:1", "src/lib.rs:14"]
        );

        // `#[cfg(test)]` on something with no body does not hide the item after it.
        let declared = "#[cfg(test)]\nmod tests;\n\nfn a() { unsafe { poke() }; }\n";
        assert_eq!(places("src/lib.rs", declared), ["src/lib.rs:4"]);

        // The `all(test, …)` spelling of the same attribute.
        let nested = "#[cfg(all(test, feature = \"slow\"))]\nmod tests {\n    \
                        fn b() { unsafe { poke() }; }\n}\n";
        assert!(places("src/lib.rs", nested).is_empty(), "{nested}");
    }

    #[test]
    fn a_project_without_the_lint_is_not_denying_it() {
        let scratch = TempDir::new("unsafe-manifest");
        let project = scratch.path();
        scratch.write(
            "Cargo.toml",
            "[package]\nname = \"jobq\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n",
        );
        assert!(!denies_unsafe_code(project));

        scratch.write(
            "Cargo.toml",
            "[package]\nname = \"jobq\"\nversion = \"0.1.0\"\n\n[lints.rust]\nunsafe_code = \"warn\"\n",
        );
        assert!(!denies_unsafe_code(project), "`warn` is not a denial");

        scratch.write(
            "Cargo.toml",
            "[package]\nname = \"jobq\"\nversion = \"0.1.0\"\n\n[lints.rust]\nunsafe_code = \"deny\"\n",
        );
        assert!(denies_unsafe_code(project));

        scratch.write(
            "Cargo.toml",
            "[package]\nname = \"jobq\"\nversion = \"0.1.0\"\n\n[lints.rust]\n\
             unsafe_code = \"forbid\"\n",
        );
        assert!(denies_unsafe_code(project), "`forbid` is a denial too");

        scratch.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.lints.rust]\n\
             unsafe_code = { level = \"deny\" }\n",
        );
        assert!(
            denies_unsafe_code(project),
            "the detailed spelling is cargo too"
        );

        scratch.write("Cargo.toml", "[package]\nname = ");
        assert!(
            !denies_unsafe_code(project),
            "a manifest nobody can read denies nothing"
        );
        assert!(!denies_unsafe_code(&scratch.path().join("elsewhere")));
    }

    #[test]
    fn a_step_with_no_unsafe_does_not_claim_it_documented_any() {
        let manifest = Path::new("/tmp/project/Cargo.toml");
        let clean = passed_line(0, 1, 0, manifest, true);
        assert!(
            clean.contains("0 `unsafe` occurrence(s) in the 1 production file(s) this step writes"),
            "{clean}"
        );
        assert!(!clean.contains("each with a"), "{clean}");
        assert!(clean.contains("unsafe_code` is denied"), "{clean}");

        // With occurrences, the note clause is the point, and a step that allows some says so.
        let documented = passed_line(2, 5, 2, manifest, true);
        assert!(
            documented.contains("each with a `// SAFETY:` note"),
            "{documented}"
        );
        assert!(documented.contains("(this step allows 2)"), "{documented}");
    }

    #[test]
    fn the_deny_is_required_only_while_the_project_holds_no_unsafe() {
        assert!(
            deny_required(0, false, 0),
            "a project with no `unsafe` and no deny has lost the compile-time barrier"
        );
        assert!(
            !deny_required(0, true, 0),
            "a manifest that denies needs nothing from this rule"
        );
        assert!(
            !deny_required(0, false, 5),
            "a later step's documented `unsafe` lifts the deny, and an earlier step is not failed \
             for it"
        );
        assert!(
            !deny_required(5, false, 5),
            "a step that allows some `unsafe` never needed the deny"
        );
    }

    #[test]
    fn production_sources_are_the_rust_files_under_src() {
        let scratch = TempDir::new("unsafe-walk");
        let project = scratch.path();
        scratch.write("src/lib.rs", "");
        scratch.write("src/service/queue.rs", "");
        scratch.write("src/notes.md", "not rust\n");
        scratch.write("tests/spec_spec.rs", "");
        let found = production_sources(project).expect("the fixture walks");
        assert_eq!(found, ["src/lib.rs", "src/service/queue.rs"]);
    }
}
