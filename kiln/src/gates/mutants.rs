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
//!   positions, starting at the first one, at most [`MUTANT_CAP`] of them are judged and at most
//!   [`ATTEMPT_CAP`] sites are tried;
//! * **how** is a fixed table of textual rewrites ([`mutation`]), never applied inside a string
//!   literal, a character literal or a comment ([`crate::gates::lexer`]). The table holds rewrites
//!   whose result is a *change in behaviour*, because a rewrite that cannot build costs an attempt
//!   and buys nothing: three classes were removed after a dry run measured them (handoff M2 §4.3);
//! * **what counts** is the spec's verdict against one project copy that every mutant reuses, so
//!   cargo rebuilds incrementally: spec fails = killed, spec passes = survived, and a mutant that
//!   does not build or does not finish = skipped and excluded from the ratio, because a compile error
//!   is not evidence that the specs are good.
//!
//! Two clocks keep the walk from becoming the slowest thing in the course, and both are about
//! mutant *runs* rather than about this gate's own work ([`attempt_timeout`] and [`WALK_BUDGET`]):
//! a mutant can make a spec hang — one `&&` → `||` in `MemoryStore::claim` hands the same job out
//! forever, and the spec's drain loop stops only if a claim comes back empty — and waiting out the
//! runner's global deadline for every attempt is how one step's gate turns into an hour.
//!
//! The copy is made in a scratch directory, never in the learner's tree, and every file in it is
//! stamped ahead of the clock — [`sandbox::copy_project`], which is also what the `coverage` gate
//! builds in, and where the two reasons are written out: cargo keys a build slot by package *name
//! and version* rather than by the directory a package lives in, so a copied tree is one slot with
//! the tree it came from, and a build that is not preceded by a newer source is served the artifact
//! of that other tree. [`sandbox`] also carries the toolchain pin the project is built under, so a
//! mutant run compiles with the compiler the learner's own commands use.
//!
//! The same fact runs the other way round, and it is why the copy builds in a directory of its own
//! (`<cache>/kiln/mutants-target`, [`scratch::cache`]): cargo keys a slot by package name and version,
//! so a copy that built into the shared directory would leave a binary whose `CARGO_MANIFEST_DIR`
//! points at a copy that no longer exists — and the next gate to run the step's command reads that
//! path. Measured (handoff M2 §5.2): `slow:mutants@80` followed by `slow:conformance@2` made the
//! conformance run execute the *mutants copy's* binary, which then failed to open the project's own
//! page. With a directory of its own the copy cannot reach anything outside itself, so the walk needs
//! no cleanup pass after its last mutant.
//!
//! Before the mutants, the gate runs the spec once on the unmutated copy: a project whose own spec
//! already fails would otherwise report every mutant as killed, which is the one answer this gate must
//! never give by accident.

use std::ops::Range;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{KilnError, Result};
use crate::gates::lexer::{Class, classify, is_identifier_byte};
use crate::gates::runner::{GateContext, clip};
use crate::gates::{GateOutcome, GateSpec, sandbox, sources};
use crate::process::{self, Ran};
use crate::scratch;

/// Most mutants one step is judged on: rebuilding and re-running the spec six times is the price the
/// slow path pays for a spec that has been shown to be able to fail (handoff M2 §1).
///
/// `attempts`, not judgements: a mutation whose site does not compile scores nothing (see
/// [`Verdict::Skipped`]), and six of those in a row would leave the gate with no evidence at all
/// rather than with a pass. The walk stops as soon as [`MUTANT_CAP`] mutants have been *judged*, so
/// the healthy case still costs six runs and only a project whose operators are hard to reach pays
/// for more.
pub const MUTANT_CAP: usize = 6;

/// Candidate sites one step may try to reach [`MUTANT_CAP`] judgements.
///
/// Twice the cap, because a site that does not compile is common — a `>` that turns out to be part
/// of a type the angle-bracket rule below could not tell from a comparison, a constant that is also
/// an array length — and an attempt is cheaper than a step judged on four mutants.
const ATTEMPT_CAP: usize = MUTANT_CAP * 2;

/// How long the whole walk may take, however many attempts are left.
///
/// The slow path runs this gate for every step that declares it, and a gate whose cost is the
/// product of an unbounded number of attempts and an unbounded time each is a gate that stops being
/// run. Five minutes is roughly a hundred times what six mutant runs of this course's specs cost on
/// the machine it was measured on (handoff M2 §1.3); what reaches this budget is something stuck,
/// and the report says the walk was cut short rather than pretending it finished.
const WALK_BUDGET: Duration = Duration::from_secs(300);

/// Lines of a failing spec run the report keeps.
const DETAIL_LINES: usize = 8;

/// Whole words that become another whole word when they stand alone.
const WORDS: &[(&str, &str)] = &[("true", "false"), ("false", "true")];

/// The build directory a mutant walk keeps: the copy's own, claimed for as long as it walks.
///
/// Shared between runs because the dependencies in it are what a walk after it reuses ([`scratch::cache`]),
/// and claimed because a walk and any other build in one directory would be served each other's
/// artifacts ([`scratch::claim`]).
const TARGET_DIR: &str = "mutants-target";

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
    /// 1-based column of the rewrite within its line.
    ///
    /// A line can hold two of the same operator — `a && b && c` is two mutants of one line — and a
    /// report that names only the line names the same place twice. A column is also what a terminal
    /// turns into an editor link.
    column: usize,
}

impl Mutant {
    /// `src/queue.rs:42:17` — how a report names a mutant.
    fn place(&self) -> String {
        format!("{}:{}:{}", self.path, self.line, self.column)
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
    let attempts = sample(&candidates, ATTEMPT_CAP);

    let scratch = scratch::directory("mutants")?;
    let copy = scratch.path().join("project");
    sandbox::copy_project(project, &copy)?;

    // The copy's builds get a directory of their own, and this is a correctness rule rather than a
    // cache one: cargo keys a build slot by package name and version, not by the directory the
    // package lives in, so a copy that built into the shared directory leaves a binary whose
    // `CARGO_MANIFEST_DIR` points at a copy that no longer exists — and the *next* gate to run the
    // step's command (`slow:conformance@2` runs right after this one) then reads another tree's
    // files, or fails on a file it cannot open at all. `coverage` keeps a directory of its own for
    // the same reason (a different `RUSTFLAGS` is a different build); this is the same rule for a
    // different tree. The directory persists, so the baseline and the attempts after it reuse what
    // they compiled — and the copy is stamped fresh, so a build from an earlier run is never
    // mistaken for this one's.
    //
    // Persisting is also why it is *claimed* first, for the whole walk: two kiln processes building
    // in it at once would be served each other's artifacts, and the mutant verdicts would then be
    // about another tree (see [`scratch::claim`]).
    let Some(_claim) = scratch::claim(TARGET_DIR)? else {
        return Ok(GateOutcome::blocked(format!(
            "{} — two mutant walks in one build directory would serve each other's artifacts, so this \
             gate did not build there; wait for that run to finish, or delete the file if that run is \
             gone",
            scratch::contended(TARGET_DIR)
        )));
    };
    let target = scratch::cache(TARGET_DIR)?;
    let environment = [("CARGO_TARGET_DIR".to_owned(), target.display().to_string())];

    let baseline = process::run_line_with_env(&command, &copy, ctx.timeout, &environment)?;
    if !baseline.success() {
        return Ok(GateOutcome::blocked(format!(
            "the spec does not pass on the project as it is, so no mutant can be judged — a mutant \
             is only evidence when the unmutated copy is green:\n    {}",
            output(&baseline)
        )));
    }
    // One mutant differs from this run by one operator, so it cannot honestly take four times as
    // long: what is still going after that is stuck rather than slow.
    let per_attempt = attempt_timeout(baseline.elapsed, ctx.timeout);

    let walk_started = Instant::now();
    let mut killed = 0usize;
    let mut survived: Vec<Mutant> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for mutant in &attempts {
        // Enough evidence, or the budget for looking for it is spent: the rest of the sample is not
        // worth a build.
        if killed + survived.len() >= MUTANT_CAP || walk_started.elapsed() >= WALK_BUDGET {
            break;
        }
        let path = copy.join(&mutant.path);
        let original = read(&path)?;
        write_fresh(&path, &mutant.splice(&original)?)?;
        let ran = process::run_line_with_env(&command, &copy, per_attempt, &environment)?;
        let verdict = verdict(&ran);
        // Put the file back before the next mutant: every mutant has to be measured against the same
        // tree, not against the one before it.
        write_fresh(&path, &original)?;
        match verdict {
            Verdict::Killed => killed += 1,
            Verdict::Survived => survived.push(mutant.clone()),
            // Why a mutant was not judged is the one thing a reader of this gate cannot guess and
            // will need: a `skipped` count on its own leaves a blocked gate unexplained, and the
            // reason is already in hand.
            Verdict::Skipped => skipped.push(skipped_because(mutant, &ran, per_attempt)),
        }
    }
    let elapsed = walk_started.elapsed();
    let stopped_early = killed + survived.len() < MUTANT_CAP && elapsed >= WALK_BUDGET;

    Ok(judge(Tally {
        percent,
        killed,
        survived: &survived,
        skipped: &skipped,
        attempted: attempts.len(),
        elapsed,
        stopped_early,
    }))
}

/// How long one mutant run may take, given how long the unmutated suite took.
///
/// The runner's `--timeout` is the deadline for *a command the course asks for*; a mutant run is a
/// command this gate invented, and it is entitled to no more time than the suite it came from needs,
/// with room for a slower machine on a bad day. The floor keeps a fast suite (a memory-only spec
/// finishes in milliseconds) from timing out on a cold rebuild.
fn attempt_timeout(baseline: Duration, global: Duration) -> Duration {
    let generous = baseline
        .saturating_mul(4)
        .saturating_add(Duration::from_secs(15));
    if generous < global { generous } else { global }
}

/// One line saying which mutant was not judged and why.
///
/// A mutant that does not build scores nothing, which is the right call — but "all six failed to
/// build" is a gate nobody can act on. The first diagnostic cargo printed is what turns it back into
/// a sentence about this project.
fn skipped_because(mutant: &Mutant, ran: &Ran, timeout: Duration) -> String {
    let why = if ran.timed_out {
        format!("did not finish within {}s", timeout.as_secs())
    } else if let Some(diagnostic) = first_error(&ran.stderr) {
        format!("does not build: {diagnostic}")
    } else {
        "does not build".to_owned()
    };
    format!(
        "{}:{}:{} {} — {why}",
        mutant.path, mutant.line, mutant.column, mutant.edit
    )
}

/// The first line of a compiler's complaint, which is the one that names the problem.
fn first_error(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("error"))
        .map(str::to_owned)
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

/// What one walk over the attempts produced, as the report needs it.
struct Tally<'a> {
    /// The threshold the step declared.
    percent: u32,
    /// Mutants the spec noticed.
    killed: usize,
    /// Mutants the spec did not notice, in the order they were tried.
    survived: &'a [Mutant],
    /// One line per attempt that scored nothing, saying why.
    skipped: &'a [String],
    /// Sites tried, judged or not.
    attempted: usize,
    /// How long the walk took, for the report: a gate whose cost cannot be read off the result is a
    /// gate whose cost nobody can budget.
    elapsed: Duration,
    /// `true` when the walk stopped on [`WALK_BUDGET`] rather than on having enough evidence.
    stopped_early: bool,
}

/// The gate's answer, from what the runs showed.
fn judge(tally: Tally<'_>) -> GateOutcome {
    let Tally {
        percent,
        killed,
        survived,
        skipped,
        attempted,
        elapsed,
        stopped_early,
    } = tally;
    let judged = killed + survived.len();
    let spent = format!(
        "the walk took {}s{}",
        elapsed.as_secs(),
        if stopped_early {
            " and hit its budget before it had enough evidence"
        } else {
            ""
        }
    );
    if judged == 0 {
        return GateOutcome::blocked(format!(
            "none of the {attempted} mutant(s) could be judged (all of them failed to build or did \
             not finish), so the specs were not tested — this is not a pass ({}):\n    {}",
            spent,
            clip(&skipped.join("\n    "), DETAIL_LINES)
        ));
    }

    let killed_percent = killed * 100 / judged;
    let note = if skipped.is_empty() {
        format!(" ({spent})")
    } else {
        format!(
            " ({} of {attempted} attempts could not be judged: no build, or no finish; {spent})",
            skipped.len()
        )
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
    // A ratio computed from three mutants is not the same evidence as one computed from six, and
    // the reader is the one who has to decide whether to act on it — so the reasons the other
    // attempts were dropped travel with the verdict.
    let dropped = if skipped.is_empty() {
        String::new()
    } else {
        format!("\n    {}", clip(&skipped.join("\n    "), DETAIL_LINES))
    };
    GateOutcome::failed(format!(
        "{} of {judged} mutant(s) survived ({killed_percent}% killed, needs ≥{percent}%){note} — no \
         test noticed {survivors}{dropped}",
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
                let column = edit.range.start + 1;
                all.push(Mutant {
                    path: path.clone(),
                    line: index + 1,
                    start,
                    before: line.to_owned(),
                    after,
                    edit: edit.description,
                    column,
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
/// because `->` is not arithmetic, and `>` that closes one is left alone with it.
///
/// Three shapes are not mutation sites at all, and the rule for each is about what the *text* says
/// rather than about what the compiler would say, because this gate reads lines and not trees:
///
/// * **arrows** — `->` and `=>` are spelled with the same characters as a comparison and a shift,
///   and both are far more common in this code than either;
/// * **unspaced angle brackets** — in rustfmt-shaped code (and `fmt` is a gate of every step that
///   declares this one) a generic list is written tight (`Vec<Job>`, `impl<S: Store>`) while a
///   comparison has a space on each side (`attempts < max`). Reading the whitespace is what keeps
///   `Vec<JobRow>` out of the sample: every one of those rewrites fails to build, and a gate that
///   spends its six runs on them reports nothing about the spec. An unformatted comparison in a
///   learner's tree costs the gate one candidate, never a false verdict;
/// * **a bang that belongs to a macro** — `format!`, `vec!`, `assert_eq!` all carry the character
///   this table reads as negation. A negation has no name to its left, so a `!` directly after an
///   identifier byte is a macro invocation and not a site.
///
/// [The constructor rewrites (`Ok(` ↔ `Err(`, `Some(` ↔ `None(`) are deliberately *not* in the
/// table. They are type-changing rather than behaviour-changing: `None(x)` is not callable at all
/// (`E0532`), and `Ok(`/`Err(` only pass each other's types when a function is polymorphic in a way
/// this course's code is not. The first dry run measured it: three of twelve attempts, none of them
/// judged.]
fn mutation(bytes: &[u8], at: usize) -> Option<(&'static str, Option<&'static str>)> {
    for (from, to) in WORDS {
        if stands_at(bytes, at, from, true) {
            return Some((from, Some(to)));
        }
    }

    let byte = bytes.get(at).copied()?;
    let next = bytes.get(at + 1).copied();
    let before = at
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .copied();
    match byte {
        // An arrow is neither a subtraction nor a comparison, and both of its bytes have to be left
        // alone: `->` is far more common in real code than either of the rewrites it would attract.
        b'-' if next == Some(b'>') => None,
        b'>' if before == Some(b'-') || before == Some(b'=') => None,
        b'!' if next == Some(b'=') => Some(("!=", Some("=="))),
        b'=' if next == Some(b'=') => Some(("==", Some("!="))),
        b'<' if next == Some(b'=') => Some(("<=", Some("<"))),
        b'>' if next == Some(b'=') => Some((">=", Some(">"))),
        b'<' if is_operator_gap(next) && is_operator_gap(before) => Some(("<", Some("<="))),
        b'>' if is_operator_gap(next) && is_operator_gap(before) => Some((">", Some(">="))),
        // A logical operator has an operand on its left; a closure's `||` and a double reference's
        // `&&` have a bracket, an `=` or an operator there instead.
        b'&' if next == Some(b'&') && ends_an_operand(bytes, at) => Some(("&&", Some("||"))),
        b'|' if next == Some(b'|') && ends_an_operand(bytes, at) => Some(("||", Some("&&"))),
        // A lone `!` is a negation, and negating nothing is a real bug: the rewrite drops it. A `!`
        // with a name in front of it is a macro invocation, which is not a site at all.
        b'!' if !before.is_some_and(is_identifier_byte) => Some(("!", None)),
        b'+' => Some(("+", Some("-"))),
        b'-' => Some(("-", Some("+"))),
        b'0' if is_integer_constant(bytes, at) => Some(("0", Some("1"))),
        b'1' if is_integer_constant(bytes, at) => Some(("1", Some("0"))),
        _ => None,
    }
}

/// `true` when an angle bracket may be read as a comparison here: whitespace on both sides.
///
/// `None` (the start or the end of the line) does not count — a type or a shift may open a line, a
/// comparison written against the edge of one is not the shape this gate is looking for.
fn is_operator_gap(byte: Option<u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_whitespace())
}

/// `true` when the last thing before `at` can be the left side of a two-operand operator.
///
/// Whitespace is skipped, because rustfmt puts one on each side of `&&` and `||`; what has to be
/// there underneath is the end of a value — a name, a number, a closing bracket or a string — and not
/// a `(`, a `,` or an `=`, which is what a closure written as `|| …` follows.
fn ends_an_operand(bytes: &[u8], at: usize) -> bool {
    let mut index = at;
    while index > 0 {
        index -= 1;
        let Some(byte) = bytes.get(index).copied() else {
            return false;
        };
        if byte.is_ascii_whitespace() {
            continue;
        }
        return byte.is_ascii_alphanumeric()
            || byte == b'_'
            || byte == b')'
            || byte == b']'
            || byte == b'"';
    }
    false
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

/// Write a file of the sandbox copy, stamped the way [`sandbox::make_fresh`] stamps a copied one.
///
/// Every mutant is one rewrite of one file, so it is this call that has to make cargo notice: a mutant
/// that is never rebuilt is a mutant that is never killed, which is the opposite of what this gate is
/// for.
fn write_fresh(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|source| KilnError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    sandbox::make_fresh(path)
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
        assert_eq!(rewrites("let x = !done;"), ["let x = done;"]);
    }

    #[test]
    fn a_constructor_is_not_rewritten_into_another_type() {
        // `Ok(` ↔ `Err(` and `Some(` ↔ `None(` were in the table and are not any more: `None(x)` is
        // not callable (`E0532`) and the two `Result` sides rarely share a type, so the rewrites only
        // ever produced builds that failed. Measured in the dry run of handoff M2 §4.3: three of
        // twelve attempts, none of them judged.
        assert!(rewrites("return Some(x);").is_empty());
        assert!(rewrites("    Some(other) => open_durable(other),").is_empty());
        assert!(rewrites("    Err(refused)?;").is_empty());
        assert!(only("let job = self.store.enqueue(request).await?;").is_empty());
        assert!(
            only("return Ok(());").is_empty(),
            "`Ok(` is not a site and `()` holds nothing to rewrite"
        );
    }

    #[test]
    fn a_closure_is_not_a_logical_or_and_a_macro_is_not_a_negation() {
        // Both shapes cost the gate a whole attempt in the first dry run: `.then_with(|| …)` was read
        // as `||`, and `format!(` as a negation.
        assert!(only("    .then_with(|| left.id.cmp(&right.id))").is_empty());
        assert_eq!(
            rewrites("    let f = || 1;"),
            ["    let f = || 0;"],
            "the closure stays a closure; the `1` is still a site"
        );
        assert!(only("    detail: format!(").is_empty());
        assert!(only("    assert_eq!(a, b);").is_empty());
        assert!(only("    log::info!(\"ready\");").is_empty());
        assert_eq!(rewrites("if a || b {"), ["if a && b {"]);
        assert_eq!(rewrites("if done() && ready {"), ["if done() || ready {"]);
        assert_eq!(rewrites("if !ready {"), ["if ready {"]);
        assert_eq!(rewrites("let x = !done;"), ["let x = done;"]);
        assert!(only("    let s: &&str = &&value;").is_empty());
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
        assert_eq!(
            only("let x = Some(0);").len(),
            1,
            "the `0` is the site; `Some(` is not one"
        );
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
        // A match arm's arrow is the same `>` as `>=`, and a `select!` arm is where an unspaced
        // one was found costing this gate a whole run (`service/mod.rs:159`).
        assert!(only("        () = terminate => {}").is_empty());
        // A match arm's `Some(` is not a site either — nothing in it is.
        assert!(only("    Some(x) => use_it(x),").is_empty());
        assert_eq!(rewrites("let n = a - 1;").len(), 2, "the `-` and the `1`");
    }

    #[test]
    fn an_angle_bracket_that_closes_a_type_is_not_a_comparison() {
        // The rule the first dry run of this gate produced (handoff M2 §4.3): twenty-four of the
        // six sampled mutants were type positions, every one of them failed to build, and the gate
        // reported nothing about the spec. In rustfmt-shaped code — and `fmt` is a gate of every
        // step that declares this one — a type is tight and a comparison has spaces.
        assert!(only("            jobs: Mutex<BTreeMap<JobId, Job>>,").is_empty());
        assert!(only("        ) -> BoxFuture<'a, Result<(), Error>> {").is_empty());
        assert!(only("            fn claim<'a>(").is_empty());
        assert!(only("                    let rows: Vec<JobRow> = query();").is_empty());
        assert!(only("        last_error: Option<String>,").is_empty());
        assert!(only("    pub use postgres::PostgresStore;").is_empty());
        assert!(only("    }) -> Result<Vec<Job>, Error> {").is_empty());
        assert!(only("    pub fn get(&self, id: &JobId) -> Option<Job> {").is_empty());
        // A comparison still is one, wherever it sits on the line.
        assert_eq!(
            rewrites("if attempts < max_attempts {"),
            ["if attempts <= max_attempts {"]
        );
        assert_eq!(rewrites("while seen < limit {"), ["while seen <= limit {"]);
        assert_eq!(
            rewrites("    let n = if a > b { 1 } else { 2 };"),
            [
                "    let n = if a >= b { 1 } else { 2 };",
                "    let n = if a > b { 0 } else { 2 };",
            ]
        );
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
                column: 1,
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
            written > std::time::SystemTime::now(),
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
            column: 17,
        };
        // A bounded drain: a mutant can make a spec hang — one `&&` → `||` in `MemoryStore::claim`
        // hands the same job out for ever — and the walk's clocks are what stop that from costing
        // the runner's whole deadline per attempt. The verdicts themselves do not change.
        let tally = |killed: usize, survived: &[Mutant], skipped: &[String], attempted: usize| {
            judge(Tally {
                percent: 80,
                killed,
                survived,
                skipped,
                attempted,
                elapsed: Duration::from_secs(9),
                stopped_early: false,
            })
        };
        let failed = tally(1, std::slice::from_ref(&survivor), &[], 2);
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
        assert!(
            failed.detail.contains("the walk took 9s"),
            "{}",
            failed.detail
        );

        // A denominator of zero is not a pass, however good the ratio would look — and the reason
        // for every skipped attempt is what turns a blocked gate into something its reader can act
        // on, so it has to survive into the message.
        let why = vec![
            "src/queue.rs:25 `>` → `>=` — does not build: error: expected `,`, or `}`".to_owned(),
        ];
        let blocked = tally(0, &[], &why, 6);
        assert_eq!(blocked.outcome, crate::gates::Outcome::Blocked);
        assert!(blocked.detail.contains("not a pass"), "{}", blocked.detail);
        assert!(
            blocked.detail.contains("src/queue.rs:25"),
            "{}",
            blocked.detail
        );
        assert!(
            blocked.detail.contains("does not build: error: expected"),
            "{}",
            blocked.detail
        );

        let passed = tally(5, std::slice::from_ref(&survivor), &why, 6);
        assert_eq!(passed.outcome, crate::gates::Outcome::Passed);
        assert!(passed.detail.contains("83%"), "{}", passed.detail);
        assert!(
            passed
                .detail
                .contains("1 of 6 attempts could not be judged"),
            "{}",
            passed.detail
        );
    }

    #[test]
    fn a_mutant_run_gets_four_times_the_baseline_and_not_the_runner_deadline() {
        // The whole point of the walk's clock: a hand-written spec takes two seconds here, a mutant
        // of it that has not finished in eight is stuck, and the runner's five minutes are for a
        // command the *course* asked for.
        assert_eq!(
            attempt_timeout(Duration::from_secs(2), Duration::from_secs(300)),
            Duration::from_secs(23)
        );
        assert_eq!(
            attempt_timeout(Duration::from_millis(50), Duration::from_secs(300)),
            Duration::from_millis(15_200),
            "a fast suite still gets the floor, so a cold rebuild is not read as a stuck mutant"
        );
        assert_eq!(
            attempt_timeout(Duration::from_secs(120), Duration::from_secs(30)),
            Duration::from_secs(30),
            "the runner's own deadline is still a ceiling"
        );
        assert_eq!(
            attempt_timeout(Duration::MAX, Duration::from_secs(300)),
            Duration::from_secs(300),
            "an absurd baseline cannot overflow into a panic"
        );
    }

    #[test]
    fn a_walk_that_hits_its_budget_says_so() {
        let survivor = Mutant {
            path: "src/queue.rs".to_owned(),
            line: 42,
            start: 0,
            before: "if pending == 0 {".to_owned(),
            after: "if pending != 0 {".to_owned(),
            edit: "`==` → `!=`".to_owned(),
            column: 17,
        };
        let outcome = judge(Tally {
            percent: 80,
            killed: 1,
            survived: std::slice::from_ref(&survivor),
            skipped: &[],
            attempted: 4,
            elapsed: WALK_BUDGET,
            stopped_early: true,
        });
        assert_eq!(outcome.outcome, crate::gates::Outcome::Failed);
        assert!(
            outcome
                .detail
                .contains("hit its budget before it had enough evidence"),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn an_attempt_that_does_not_build_costs_an_attempt_and_not_a_slot() {
        // The gate samples twice its cap, and stops once it has judged enough mutants: a site that
        // does not compile must not be able to leave a step measured on two mutants, and the
        // healthy project must still pay six runs rather than twelve.
        let candidates: Vec<Mutant> = (0..40)
            .map(|index| Mutant {
                path: "src/lib.rs".to_owned(),
                line: index,
                start: 0,
                before: String::new(),
                after: String::new(),
                edit: String::new(),
                column: 1,
            })
            .collect();
        let attempts = sample(&candidates, ATTEMPT_CAP);
        assert_eq!(attempts.len(), MUTANT_CAP * 2);
        assert!(
            attempts.len() <= candidates.len(),
            "a sample never invents sites"
        );
    }
}
