//! `mutants@N` — the step's specs are not vacuous.
//!
//! A spec that passes is only worth something if it would fail when the behaviour it describes
//! changes. This gate proves the spec can tell: it makes one small deterministic change to the
//! production code the step is about and runs the step's own `run.cmd` against it. A spec that still
//! passes did not notice, and that is the defect the gate reports.
//!
//! Four rules keep it deterministic and free of external tools (handoff M2 §1):
//!
//! * **what** is mutated comes from the course's own metadata: the `.rs` files under `src/` that this
//!   step's reference-solution patch touches ([`sources::production_sources`]). A new step cannot
//!   forget to register its module, because the patch *is* the module;
//! * **where** is decided by source order: the mutants are taken evenly spaced over the candidate
//!   positions, starting at the first one, at most [`MUTANT_CAP`] of them;
//! * **how** is a fixed table of textual rewrites ([`mutation`]), never applied inside a string
//!   literal, a character literal or a comment ([`crate::gates::lexer`]);
//! * **what counts** is the spec's verdict against one project copy that every mutant reuses, so
//!   cargo rebuilds incrementally: spec fails = killed, spec passes = survived, and a mutant that
//!   does not build or does not finish = skipped and excluded from the ratio, because a compile error
//!   is not evidence that the specs are good.
//!
//! The copy is made in a scratch directory, never in the learner's tree, and every file in it is
//! stamped ahead of the clock. That is cargo's rule rather than this gate's: a build directory is
//! keyed by package *name and version*, not by the directory a package lives in, so a copy of the
//! project and the project itself are one build slot, and cargo decides whether a slot is up to date
//! by comparing the modification times of its sources with the build it already has. `fs::copy`
//! brings the original's timestamps along, which can make a fresh copy look *older* than an artifact
//! of the tree it came from — and a mutant run would then judge yesterday's binary. [`make_fresh`]
//! covers the copy and [`write_fresh`] every file this gate rewrites afterwards.
//!
//! The same fact runs the other way round, and the gate has to tidy up after it: the last thing a
//! mutant run leaves in that shared slot is a build of the *mutated* tree, and cargo would serve it to
//! a later build of the same tree — the learner's own `cargo test` would then run a mutant. So when the
//! copy builds somewhere other than its own directory, the gate runs the spec once more after the last
//! restore, which leaves the cache describing the code that is really there. That run is skipped in the
//! usual case, where the copy builds into its own `target/` and takes its slot with it when it goes.
//!
//! Before the mutants, the gate runs the spec once on the unmutated copy: a project whose own spec
//! already fails would otherwise report every mutant as killed, which is the one answer this gate must
//! never give by accident.

use std::ops::Range;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::error::{KilnError, Result};
use crate::gates::lexer::{Class, classify, is_identifier_byte};
use crate::gates::runner::{GateContext, clip};
use crate::gates::sources;
use crate::gates::{GateOutcome, GateSpec};
use crate::process::{self, Ran};
use crate::scratch;

/// Most mutants one step is judged on: rebuilding and re-running the spec six times is the price the
/// slow path pays for a spec that has been shown to be able to fail (handoff M2 §1).
pub const MUTANT_CAP: usize = 6;

/// Lines of a failing spec run the report keeps.
const DETAIL_LINES: usize = 8;

/// Whole words that become another whole word when they stand alone.
const WORDS: &[(&str, &str)] = &[("true", "false"), ("false", "true")];

/// Constructor variants: the `(` they end with is what makes them one token, so only the front of
/// the name has to be checked.
const VARIANTS: &[(&str, &str)] = &[
    ("Ok(", "Err("),
    ("Err(", "Ok("),
    ("Some(", "None("),
    ("None(", "Some("),
];

/// One textual change to one line of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mutant {
    /// File, relative to the project.
    path: String,
    /// 1-based line number, for the report and for the reader's editor.
    line: usize,
    /// Byte offset the line starts at, so a rewrite leaves every other byte of the file alone.
    start: usize,
    /// The line as it is now.
    before: String,
    /// The line the mutant run gets.
    after: String,
    /// The rewrite, as `` `==` → `!=` ``, for the report.
    edit: String,
}

impl Mutant {
    /// `src/queue.rs:42` — how a report names a mutant.
    fn place(&self) -> String {
        format!("{}:{}", self.path, self.line)
    }

    /// `src/queue.rs:42 `if pending == 0` (`==` → `!=`)` — what a learner needs to write a test for.
    fn described(&self) -> String {
        format!("{} `{}` ({})", self.place(), self.before.trim(), self.edit)
    }

    /// The file with this mutant applied.
    fn splice(&self, text: &str) -> Result<String> {
        let head = text.get(..self.start).ok_or_else(|| self.moved())?;
        let tail = text
            .get(self.start + self.before.len()..)
            .ok_or_else(|| self.moved())?;
        Ok(format!("{head}{}{tail}", self.after))
    }

    /// The file changed between planning and applying the mutation.
    fn moved(&self) -> KilnError {
        KilnError::Invalid(format!(
            "cannot mutate {}: the file is not the one the mutation was planned against",
            self.place()
        ))
    }
}

/// One possible rewrite of one line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    /// Byte range of the text the rewrite replaces.
    range: Range<usize>,
    /// What replaces it.
    replacement: String,
    /// `` `==` → `!=` ``, for the report.
    description: String,
}

/// What one mutant run showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The spec failed: it noticed the change.
    Killed,
    /// The spec passed: the change was invisible, which is the defect this gate is about.
    Survived,
    /// The mutant did not build or the run did not finish: it scores nothing either way.
    Skipped,
}

/// Run the step's specs against mutants of the code the step is about.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    let percent = spec.numeric_param()?;
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };
    let Some(command) = ctx.step.run.as_ref().map(|run| run.cmd.clone()) else {
        return Ok(GateOutcome::blocked(format!(
            "{} declares no `run.cmd`, so there is no spec to run against a mutant",
            ctx.step.qualified_id()
        )));
    };

    let track_dir = ctx.track.dir(ctx.course.root());
    let module = sources::production_sources(&track_dir, ctx.step)?;
    if module.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "{}'s reference solution touches no production source under `src/`, so the module this \
             gate mutates is not defined — `mutants` needs a `solution` patch that changes `src/`",
            ctx.step.qualified_id()
        )));
    }

    let candidates = candidates(project, &module)?;
    if candidates.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "no mutable operator in the step's module ({}), so there is nothing to mutate",
            module.join(", ")
        )));
    }
    let chosen = sample(&candidates, MUTANT_CAP);

    let scratch = scratch::directory("mutants")?;
    let copy = scratch.path().join("project");
    copy_project(project, &copy)?;

    let baseline = process::run_line(&command, &copy, ctx.timeout)?;
    if !baseline.success() {
        return Ok(GateOutcome::blocked(format!(
            "the spec does not pass on the project as it is, so no mutant can be judged — a mutant \
             is only evidence when the unmutated copy is green:\n    {}",
            output(&baseline)
        )));
    }

    let mut killed = 0usize;
    let mut survived: Vec<Mutant> = Vec::new();
    let mut skipped = 0usize;
    for mutant in &chosen {
        let path = copy.join(&mutant.path);
        let original = read(&path)?;
        write_fresh(&path, &mutant.splice(&original)?)?;
        let ran = process::run_line(&command, &copy, ctx.timeout)?;
        let verdict = verdict(&ran);
        // Put the file back before the next mutant: every mutant has to be measured against the same
        // tree, not against the one before it.
        write_fresh(&path, &original)?;
        match verdict {
            Verdict::Killed => killed += 1,
            Verdict::Survived => survived.push(mutant.clone()),
            Verdict::Skipped => skipped += 1,
        }
    }

    // The last build in a shared target directory belongs to whoever built last, and that was a
    // mutant. One more run of the spec over the restored copy leaves the cache describing the tree the
    // learner has — and proves the copy was put back the way it was found.
    if process::target_dir(project) != project.join("target") {
        let restored = process::run_line(&command, &copy, ctx.timeout)?;
        if !restored.success() {
            return Ok(GateOutcome::blocked(format!(
                "the copy of the project does not pass its own spec once the mutants are taken back \
                 out, so the results above cannot be trusted — this is a `kiln` fault, not yours:\n    {}",
                output(&restored)
            )));
        }
    }

    Ok(judge(percent, killed, &survived, skipped, chosen.len()))
}

/// Grade a run of the spec against one mutant.
fn verdict(ran: &Ran) -> Verdict {
    if ran.timed_out {
        return Verdict::Skipped;
    }
    if ran.success() {
        return Verdict::Survived;
    }
    if is_compile_failure(ran) {
        return Verdict::Skipped;
    }
    Verdict::Killed
}

/// `true` when the command refused to build the mutant instead of running it and failing.
///
/// Cargo says `could not compile …` exactly when compilation stopped, and a spec that ran and failed
/// never prints it. The distinction is what keeps a mutant that only breaks the build from looking
/// like a spec that noticed something.
fn is_compile_failure(ran: &Ran) -> bool {
    ran.stderr.contains("could not compile") || ran.stderr.contains("error[E")
}

/// What a spec run said, both streams together.
///
/// A failing `cargo test` splits its story: the assertion that failed is on *stdout* (that is where
/// the test harness prints the panic) and cargo's own `error: test failed` is on stderr. Quoting one
/// of the two would produce a report with no failure in it.
fn output(ran: &Ran) -> String {
    clip(&format!("{}{}", ran.stdout, ran.stderr), DETAIL_LINES)
}

/// The gate's answer, from what the runs showed.
fn judge(
    percent: u32,
    killed: usize,
    survived: &[Mutant],
    skipped: usize,
    chosen: usize,
) -> GateOutcome {
    let judged = killed + survived.len();
    if judged == 0 {
        return GateOutcome::blocked(format!(
            "none of the {chosen} mutant(s) could be judged (all of them failed to build or timed \
             out), so the specs were not tested — this is not a pass"
        ));
    }

    let killed_percent = killed * 100 / judged;
    let note = if skipped == 0 {
        String::new()
    } else {
        format!(" ({skipped} of {chosen} could not be judged: no build, or no finish)")
    };
    if killed * 100 >= percent as usize * judged {
        return GateOutcome::passed(format!(
            "{killed}/{judged} mutant(s) killed ({killed_percent}%, needs ≥{percent}%){note}"
        ));
    }
    let survivors = survived
        .iter()
        .map(Mutant::described)
        .collect::<Vec<String>>()
        .join("; ");
    GateOutcome::failed(format!(
        "{} of {judged} mutant(s) survived ({killed_percent}% killed, needs ≥{percent}%){note} — no \
         test noticed {survivors}",
        survived.len()
    ))
}

/// Every position that could be mutated, in source order: the files in the order the patch lists
/// them, then line by line, then left to right within a line.
fn candidates(project: &Path, module: &[String]) -> Result<Vec<Mutant>> {
    let mut all = Vec::new();
    for path in module {
        // A file the patch touches but the project does not have is not a mutant site: the spec run
        // will say what is missing, and mutating a file nobody wrote is not this gate's question.
        let Ok(text) = std::fs::read_to_string(project.join(path)) else {
            continue;
        };
        let classes = classify(&text);
        let mut start = 0usize;
        for (index, chunk) in text.split_inclusive('\n').enumerate() {
            let line = chunk.strip_suffix('\n').unwrap_or(chunk);
            let on_this_line = classes.get(start..start + line.len()).unwrap_or_default();
            for edit in edits(line, on_this_line) {
                let Some(after) = apply(line, &edit) else {
                    continue;
                };
                all.push(Mutant {
                    path: path.clone(),
                    line: index + 1,
                    start,
                    before: line.to_owned(),
                    after,
                    edit: edit.description,
                });
            }
            start += chunk.len();
        }
    }
    Ok(all)
}

/// The line with one rewrite applied.
fn apply(line: &str, edit: &Edit) -> Option<String> {
    let head = line.get(..edit.range.start)?;
    let tail = line.get(edit.range.end..)?;
    Some(format!("{head}{}{tail}", edit.replacement))
}

/// Every rewrite of one line, in source order, skipping everything that is not code.
fn edits(line: &str, classes: &[Class]) -> Vec<Edit> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if !classes.get(at).copied().unwrap_or(Class::Code).is_code() {
            at += 1;
            continue;
        }
        let Some((from, to)) = mutation(bytes, at) else {
            at += 1;
            continue;
        };
        found.push(Edit {
            range: at..at + from.len(),
            replacement: to.unwrap_or_default().to_owned(),
            description: format!("`{from}` → `{}`", to.unwrap_or("nothing")),
        });
        // Past the whole operator: `!=` must not also be read as `!`, and `<=` only once.
        at += from.len();
    }
    found
}

/// The rewrite that applies at `at`, if one does: the text it replaces and what replaces it.
///
/// `None` for the replacement means the text is dropped. Longer operators are tested before the ones
/// they start with, so `!=` is never read as a lone `!`; a `-` that opens an arrow is left alone,
/// because `->` is not arithmetic.
fn mutation(bytes: &[u8], at: usize) -> Option<(&'static str, Option<&'static str>)> {
    for (from, to) in WORDS {
        if stands_at(bytes, at, from, true) {
            return Some((from, Some(to)));
        }
    }
    for (from, to) in VARIANTS {
        if stands_at(bytes, at, from, false) {
            return Some((from, Some(to)));
        }
    }

    let byte = bytes.get(at).copied()?;
    let next = bytes.get(at + 1).copied();
    match byte {
        // An arrow is neither a subtraction nor a comparison, and both of its bytes have to be left
        // alone: `->` is far more common in real code than either of the rewrites it would attract.
        b'-' if next == Some(b'>') => None,
        b'>' if at > 0 && bytes.get(at - 1) == Some(&b'-') => None,
        b'!' if next == Some(b'=') => Some(("!=", Some("=="))),
        b'=' if next == Some(b'=') => Some(("==", Some("!="))),
        b'<' if next == Some(b'=') => Some(("<=", Some("<"))),
        b'<' => Some(("<", Some("<="))),
        b'>' if next == Some(b'=') => Some((">=", Some(">"))),
        b'>' => Some((">", Some(">="))),
        b'&' if next == Some(b'&') => Some(("&&", Some("||"))),
        b'|' if next == Some(b'|') => Some(("||", Some("&&"))),
        // A lone `!` is a negation, and negating nothing is a real bug: the rewrite drops it.
        b'!' => Some(("!", None)),
        b'+' => Some(("+", Some("-"))),
        b'-' => Some(("-", Some("+"))),
        b'0' if is_integer_constant(bytes, at) => Some(("0", Some("1"))),
        b'1' if is_integer_constant(bytes, at) => Some(("1", Some("0"))),
        _ => None,
    }
}

/// `true` when `text` stands at `at` and is not part of a longer name.
///
/// `whole` additionally requires the text to *end* at a boundary: `true` is only the boolean when no
/// identifier character follows it, while `Ok(` ends in punctuation and is one token either way.
fn stands_at(bytes: &[u8], at: usize, text: &str, whole: bool) -> bool {
    if bytes.get(at..at + text.len()) != Some(text.as_bytes()) {
        return false;
    }
    let before_is_identifier = at > 0 && bytes.get(at - 1).copied().is_some_and(is_identifier_byte);
    let after_is_identifier = whole
        && bytes
            .get(at + text.len())
            .copied()
            .is_some_and(is_identifier_byte);
    !before_is_identifier && !after_is_identifier
}

/// `true` when the digit at `at` is an integer constant: not part of a longer number (`10`, `1_000`,
/// `1u32`) and not the second half of something else (`1.5`, `x.0`).
fn is_integer_constant(bytes: &[u8], at: usize) -> bool {
    let joins_a_number = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.';
    let before = at > 0 && bytes.get(at - 1).copied().is_some_and(joins_a_number);
    let after = bytes.get(at + 1).copied().is_some_and(joins_a_number);
    !before && !after
}

/// Up to `cap` mutants, evenly spaced over the candidates, starting at the first one.
///
/// Even spacing — rather than the first `cap` positions, which would all sit in the first function
/// of the first file — is what makes six mutants say something about a whole module. The indices are
/// strictly increasing (the step is `(len - 1) / (cap - 1) ≥ 1`) and in range by construction.
fn sample(candidates: &[Mutant], cap: usize) -> Vec<Mutant> {
    let count = candidates.len().min(cap);
    if count < 2 || count == candidates.len() {
        return candidates.iter().take(count).cloned().collect();
    }
    let spread = candidates.len() - 1;
    let steps = count - 1;
    (0..count)
        .filter_map(|step| candidates.get(step * spread / steps).cloned())
        .collect()
}

/// Copy a project into a sandbox, leaving build output and version control behind.
///
/// `target/` and `.git/` are the two directories that are large and regenerable; everything else is
/// content a mutant run needs, `Cargo.lock` included, so that the copy neither re-resolves
/// dependencies nor wants a network.
fn copy_project(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|source| KilnError::Write {
        path: to.to_path_buf(),
        source,
    })?;
    let entries = std::fs::read_dir(from).map_err(|source| KilnError::Read {
        path: from.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| KilnError::Read {
            path: from.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some("target" | ".git")) {
            continue;
        }
        let source_path = entry.path();
        let target_path = to.join(&name);
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            copy_project(&source_path, &target_path)?;
        } else if kind.is_file() {
            // Byte for byte: a project may hold images, fixtures or a lock file, and none of them is
            // text this gate has an opinion about.
            std::fs::copy(&source_path, &target_path).map_err(|source| KilnError::Write {
                path: target_path.clone(),
                source,
            })?;
            make_fresh(&target_path)?;
        }
    }
    Ok(())
}

/// Make a file in the sandbox newer than any build that came before this gate.
///
/// Cargo keeps a build when no source is *newer* than it, and it identifies a package by name and
/// version rather than by the directory the package lives in — so a copy of `jobq` in a sandbox and
/// the learner's own `jobq` are one build slot in a shared `CARGO_TARGET_DIR`. `fs::copy` preserves
/// timestamps, which lets a fresh copy look *older* than an artifact of the tree it came from: cargo
/// calls the artifact current and the mutants are judged against the wrong binary (which is how this
/// gate behaved before it stamped its copy). A second ahead of now is unambiguously newer, and it also
/// clears the case a file system with second granularity would otherwise hide — two writes inside one
/// second look equal there.
fn make_fresh(path: &Path) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|source| KilnError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.set_modified(SystemTime::now() + Duration::from_secs(1))
        .map_err(|source| KilnError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

/// Write a file of the sandbox copy, stamped the way [`make_fresh`] stamps a copied one.
///
/// Every mutant is one rewrite of one file, so it is this call that has to make cargo notice: a mutant
/// that is never rebuilt is a mutant that is never killed, which is the opposite of what this gate is
/// for.
fn write_fresh(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|source| KilnError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    make_fresh(path)
}

/// Read a file of the sandbox copy.
fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| KilnError::Read {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(text: &str) -> Vec<Edit> {
        edits(text, &classify(text))
    }

    fn rewrites(text: &str) -> Vec<String> {
        only(text)
            .iter()
            .map(|edit| apply(text, edit).expect("the edit fits the line it came from"))
            .collect()
    }

    #[test]
    fn comparison_and_logic_operators_are_rewritten_both_ways() {
        assert_eq!(rewrites("if a == b {"), ["if a != b {"]);
        assert_eq!(rewrites("if a != b {"), ["if a == b {"]);
        assert_eq!(rewrites("if a < b {"), ["if a <= b {"]);
        assert_eq!(rewrites("if a <= b {"), ["if a < b {"]);
        assert_eq!(rewrites("if a > b {"), ["if a >= b {"]);
        assert_eq!(rewrites("if a >= b {"), ["if a > b {"]);
        assert_eq!(rewrites("if a && b {"), ["if a || b {"]);
        assert_eq!(rewrites("if a || b {"), ["if a && b {"]);
        assert_eq!(rewrites("let ok = !done;"), ["let ok = done;"]);
        assert_eq!(rewrites("let n = a + b;"), ["let n = a - b;"]);
        assert_eq!(rewrites("let n = a - b;"), ["let n = a + b;"]);
        assert_eq!(rewrites("let z = 0;"), ["let z = 1;"]);
        assert_eq!(rewrites("let o = 1;"), ["let o = 0;"]);
        assert_eq!(rewrites("let t = true;"), ["let t = false;"]);
        assert_eq!(rewrites("let f = false;"), ["let f = true;"]);
        assert_eq!(rewrites("return Ok(x);"), ["return Err(x);"]);
        assert_eq!(rewrites("return Err(x);"), ["return Ok(x);"]);
        assert_eq!(rewrites("return Some(x);"), ["return None(x);"]);
        assert_eq!(rewrites("return None(x);"), ["return Some(x);"]);
    }

    #[test]
    fn an_operator_inside_text_or_a_comment_is_not_a_mutant_site() {
        assert!(only("let s = \"a + b\";").is_empty());
        assert!(only("// a + b").is_empty());
        assert!(only("/* a + b */").is_empty());
        assert!(only("let c = '+'; not_code();").is_empty());
        assert!(only("/// A `\n/// # Examples\n/// ```\n/// x == y\n/// ```\n").is_empty());
    }

    #[test]
    fn one_operator_gives_one_mutant() {
        // `<=` is one rewrite, not a `<` and a `=`; `!=` is not a dropped `!`.
        assert_eq!(only("a <= b").len(), 1);
        assert_eq!(only("a != b").len(), 1);
        assert_eq!(only("a && b").len(), 1);
        assert_eq!(only("return Some(0);").len(), 2, "the `Some(` and the `0`");
    }

    #[test]
    fn a_number_that_is_not_an_integer_constant_is_left_alone() {
        assert!(only("let x = 10;").is_empty());
        assert!(only("let x = 1_000;").is_empty());
        assert!(only("let x = 1.5;").is_empty());
        assert!(only("let x = value.0;").is_empty());
        assert!(only("let x = 1u32;").is_empty());
    }

    #[test]
    fn an_arrow_is_not_a_subtraction() {
        assert!(only("fn f() -> u32 {").is_empty());
        assert_eq!(rewrites("let n = a - 1;").len(), 2, "the `-` and the `1`");
    }

    #[test]
    fn a_name_that_merely_contains_an_operator_is_not_rewritten() {
        assert!(only("let not_some = value;").is_empty());
        assert!(only("let x = foo_Someth;").is_empty());
        assert!(only("let true_story = value;").is_empty());
        assert_eq!(rewrites("let x = true;"), ["let x = false;"]);
    }

    #[test]
    fn mutants_are_taken_evenly_and_start_at_the_first_candidate() {
        let candidates: Vec<Mutant> = (0..10)
            .map(|index| Mutant {
                path: "src/lib.rs".to_owned(),
                line: index,
                start: 0,
                before: String::new(),
                after: String::new(),
                edit: String::new(),
            })
            .collect();
        let lines = |chosen: Vec<Mutant>| chosen.iter().map(|it| it.line).collect::<Vec<usize>>();
        assert_eq!(lines(sample(&candidates, 6)), [0, 1, 3, 5, 7, 9]);
        assert_eq!(lines(sample(&candidates, 2)), [0, 9]);
        assert_eq!(lines(sample(&candidates, 1)), [0]);
        assert_eq!(
            lines(sample(&candidates, 99)),
            (0..10).collect::<Vec<usize>>()
        );
        assert_eq!(lines(sample(&candidates[..3], MUTANT_CAP)), [0, 1, 2]);
    }

    #[test]
    fn a_mutant_replaces_only_its_own_line() {
        let scratch = crate::test_support::TempDir::new("mutants-splice");
        let text = "fn a() { 1 }\nfn b() { 1 + 1 }\nfn c() { 1 }\n";
        scratch.write("src/lib.rs", text);
        let module = ["src/lib.rs".to_owned()];
        let found = candidates(scratch.path(), &module).expect("the fixture is readable");

        // Five sites: the `1` of `a`, then `1`, `+` and `1` of `b`, then the `1` of `c`.
        assert_eq!(found.len(), 5, "{found:?}");
        assert_eq!(found[1].line, 2);
        let spliced = found[1].splice(text).expect("the splice fits the file");
        assert_eq!(spliced, "fn a() { 1 }\nfn b() { 0 + 1 }\nfn c() { 1 }\n");

        // Only the byte range of that line changed, and the file still ends the way it did.
        let plus = found[2].splice(text).expect("the splice fits the file");
        assert_eq!(plus, "fn a() { 1 }\nfn b() { 1 - 1 }\nfn c() { 1 }\n");
    }

    #[test]
    fn a_file_the_solution_touches_but_the_project_lacks_is_not_a_site() {
        let scratch = crate::test_support::TempDir::new("mutants-missing");
        let module = ["src/gone.rs".to_owned()];
        let found = candidates(scratch.path(), &module).expect("a missing file is not an error");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_written_mutant_is_newer_than_any_build_before_it() {
        // Cargo keeps a build when no source is *newer* than it, and two writes inside one second
        // look equal on a file system with second granularity. A mutant that is never rebuilt is a
        // mutant that is never killed.
        let scratch = crate::test_support::TempDir::new("mutants-fresh");
        let path = scratch.write("src/lib.rs", "fn a() {}\n");
        write_fresh(&path, "fn b() {}\n").expect("the copy is writable");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn b() {}\n");
        let written = std::fs::metadata(&path)
            .unwrap()
            .modified()
            .expect("the fixture file has a timestamp");
        assert!(
            written > SystemTime::now(),
            "{written:?} is not ahead of now"
        );
    }

    #[test]
    fn a_verdict_separates_a_broken_build_from_a_failed_spec() {
        let ran = |code: Option<i32>, stderr: &str| Ran {
            code,
            stdout: String::new(),
            stderr: stderr.to_owned(),
            timed_out: false,
            elapsed: std::time::Duration::ZERO,
        };
        assert_eq!(verdict(&ran(Some(0), "")), Verdict::Survived);
        assert_eq!(
            verdict(&ran(Some(101), "assertion failed")),
            Verdict::Killed
        );
        assert_eq!(
            verdict(&ran(
                Some(101),
                "error: could not compile `jobq` (test \"spec_queue\") due to 1 previous error"
            )),
            Verdict::Skipped
        );
    }

    #[test]
    fn a_threshold_names_every_surviving_mutant() {
        let survivor = Mutant {
            path: "src/queue.rs".to_owned(),
            line: 42,
            start: 0,
            before: "if pending == 0 {".to_owned(),
            after: "if pending != 0 {".to_owned(),
            edit: "`==` → `!=`".to_owned(),
        };
        let failed = judge(80, 1, std::slice::from_ref(&survivor), 0, 2);
        assert_eq!(failed.outcome, crate::gates::Outcome::Failed);
        assert!(
            failed.detail.contains("src/queue.rs:42"),
            "{}",
            failed.detail
        );
        assert!(
            failed.detail.contains("if pending == 0 {"),
            "{}",
            failed.detail
        );
        assert!(failed.detail.contains("50% killed"), "{}", failed.detail);
        assert!(failed.detail.contains("needs ≥80%"), "{}", failed.detail);

        // A denominator of zero is not a pass, however good the ratio would look.
        let blocked = judge(80, 0, &[], 6, 6);
        assert_eq!(blocked.outcome, crate::gates::Outcome::Blocked);
        assert!(blocked.detail.contains("not a pass"), "{}", blocked.detail);

        let passed = judge(80, 5, std::slice::from_ref(&survivor), 1, 6);
        assert_eq!(passed.outcome, crate::gates::Outcome::Passed);
        assert!(passed.detail.contains("83%"), "{}", passed.detail);
        assert!(
            passed.detail.contains("could not be judged"),
            "{}",
            passed.detail
        );
    }
}
