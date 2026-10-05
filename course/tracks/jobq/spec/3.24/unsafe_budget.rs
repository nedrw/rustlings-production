//! Spec for `jobq/3.24-unsafe-budget`: a policy with a default, a number, and a reason for every
//! exception.
//!
//! `kiln run` copies this file into the project as `tests/spec_unsafe_budget.rs`; it is the course's
//! file, not the learner's (plan §3 D2). The cases are about the decisions a text audit is made of,
//! and each one is a sentence somebody would otherwise have to trust:
//!
//! * **a comment is not a use, and neither is a literal.** The count is the *word* in code, which is
//!   not the same as the word in the file: this step's own module documents the rule at length, and a
//!   scan that read whole lines would report every explanation of it as a use of it. `unsafe_code` is
//!   the lint's name rather than a use of the keyword, which matters for the one spelling a crate
//!   root carries (`#![deny(unsafe_code)]`);
//! * **tests are not production code.** An item gated by `#[cfg(test)]` is skipped wherever it sits
//!   in the file and however deeply it nests, and the lines above and below it still count;
//! * **a reason is a comment line, in the window, above the use.** `WINDOW` is three: a bare marker
//!   is not a reason, it does not fall through to an older comment, a marker further up is about
//!   something else, and a marker *below* the block is a comment about code that has already been
//!   written;
//! * **the policy is a rule, and the refusals come in the order a reader fixes them.** The manifest
//!   denies the lint before any use can be an exception to it, both spellings of the level are
//!   cargo's, `forbid` counts and `warn` does not, and the three refusals arrive in the order the
//!   work is done: the policy, then the reasons, then the number.
//!
//! Two cases are about the project rather than about a fixture: the crate's own manifest denies the
//! lint, and the crate's own production code has no use in it. The second is the one that would
//! notice a scan that read prose as code — the file with the most to say about the keyword is this
//! step's own module — and it is why the walk is over `src/` rather than over a file this spec names.

use std::path::{Path, PathBuf};

use jobq::engine::{Audit, BUDGET, WINDOW, audit, check, counts, denies_unsafe_code};

/// A source where each of the rules above has something to say.
///
/// Line 1 is a reason and 2 the use it explains; 3 is a literal that contains a comment marker and
/// the word; 5 is a *bare* marker, which is not one; 9's use has a reason exactly `WINDOW` lines
/// above it, 14's reason is one line too far up to be about it, and 15's use sits after a division
/// sign, which is code rather than the start of a comment.
const MIXED: &str = r#"// SAFETY: the map does not move, so the pointer stays valid
unsafe { touch(); }
let before = "unsafe is a word in a literal // and this is not a comment either";
// SAFETY:
unsafe { bare(); }
// SAFETY: three lines above the use, which is the whole window
let padding = 1;
let more = 2;
unsafe { far(); }
// SAFETY: four lines above is a question about something else
let padding = 3;
let more = 4;
let last = 5;
unsafe { too_far(); }
let half = 1 / 2; unsafe { divide(); }
"#;

/// A file where the word appears in everything that is not code.
const TALK: &str = r#"//! A module whose own documentation mentions unsafe, and unsafe_code, and the keyword a fence
//! writes out: `unsafe { x(); }`.
/// An item's doc comment, saying unsafe once more.
const LINT: &str = "unsafe_code";
fn touch(line: &str) -> &str {
    let url = "https://example.invalid/unsafe";
    let text = "unsafe // not a comment, and not code either";
    let quoted = "an \"escaped\" quote, and the word unsafe after it";
    format!("{url}{text}{quoted}{LINT}")
}
"#;

/// A library with a test module in the middle of it, and code on both sides of it.
///
/// The two blocks around the module are several lines long on purpose: a region that started one
/// block too early or ended one block too late would skip the uses at lines 2 and 18, and a fixture
/// whose neighbouring blocks were a single line would hide exactly that.
const WITH_TESTS: &str = r#"fn build() {
    unsafe { inside_a_function(); }
}
unsafe { production(); }
#[cfg(test)]
mod tests {
    #[test]
    fn one() {
        unsafe { test_helper(); }
    }
    mod nested {
        fn two() {
            unsafe { nested_helper(); }
        }
    }
}
unsafe fn after_the_tests() {
    unsafe { last(); }
}
"#;

/// A crate root that denies the lint by attribute rather than by manifest.
const DENIED_AT_THE_ROOT: &str = r#"#![deny(unsafe_code)]
// SAFETY: the buffer is ours and nothing else writes to it
unsafe { write_it(); }
"#;

/// A manifest that denies, in the spelling a `Cargo.toml` uses most often.
const DENYING: &str = r#"[package]
name = "jobq"

[lints.rust]
unsafe_code = "deny"
"#;

/// The window's worth of uses, audited once.
fn mixed() -> Audit {
    audit("src/mixed.rs", MIXED, true)
}

/// The uses of an audit, as line numbers.
fn lines(report: &Audit) -> Vec<u32> {
    report
        .occurrences()
        .iter()
        .map(|occurrence| occurrence.line())
        .collect()
}

/// One file, read from the project it is part of.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

/// Every `.rs` file below `root`, in path order.
fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{} could not be listed: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("the directory is readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_count_is_the_word_in_the_code_and_not_the_text_around_it() {
    let report = audit("src/talk.rs", TALK, true);
    assert_eq!(
        report.count(),
        0,
        "every mention in that file is a comment, a doc comment or a string literal, and none of \
         them is a use: {:?}",
        report.occurrences()
    );

    let denied = audit("src/lib.rs", DENIED_AT_THE_ROOT, true);
    assert_eq!(
        lines(&denied),
        vec![3],
        "the attribute names the lint and the block uses the keyword, and only the second one is a \
         use — a lint that a crate denies would otherwise count as the thing it forbids"
    );
    assert_eq!(
        denied.occurrences()[0].safety(),
        Some("the buffer is ours and nothing else writes to it"),
        "and the reason is carried, because a report that only said `documented` sends a reviewer \
         to open the file"
    );
}

#[test]
fn a_reason_in_the_window_above_is_what_documents_a_use() {
    let report = mixed();
    assert_eq!(
        lines(&report),
        vec![2, 5, 9, 14, 15],
        "five uses, and the lines are the real ones so that a report can be acted on"
    );
    let documented: Vec<(u32, bool)> = report
        .occurrences()
        .iter()
        .map(|occurrence| (occurrence.line(), occurrence.documented()))
        .collect();
    assert_eq!(
        documented,
        vec![(2, true), (5, false), (9, true), (14, false), (15, false)],
        "line 2's reason is one line above it, line 9's is exactly {WINDOW} lines above, line 5's \
         marker carries no reason, and line 14's reason is one line too far up"
    );
    assert_eq!(
        report.undocumented(),
        3,
        "the number a budget is written in: a use with no reason is the one thing a reviewer has to \
         be told about, and three of these five uses have none"
    );
    assert_eq!(
        report.occurrences()[1].safety(),
        None,
        "`// SAFETY:` with nothing after it looks like evidence and says nothing — and it does not \
         borrow the reason from the marker above it, because the line a reader looks at first is the \
         one immediately above the block"
    );
}

#[test]
fn tests_are_not_production_code() {
    let report = audit("src/lib.rs", WITH_TESTS, true);
    assert_eq!(
        lines(&report),
        vec![2, 4, 17, 18],
        "the two uses before the test module and the two after it; the two inside it (lines 9 and 13, \
         the nested one included) are tests, and a count that included them would push a project to \
         write worse tests"
    );

    let only_tests =
        "unsafe { above(); }\n#[cfg(test)]\nmod tests {\n    unsafe { inside(); }\n}\n";
    let report = audit("tests/support/ptr.rs", only_tests, true);
    assert_eq!(
        lines(&report),
        vec![1],
        "a test module does not reach the line above it: the region starts where the gated item \
         opens"
    );
}

#[test]
fn the_budget_is_a_number_and_going_over_names_the_count() {
    let audits = [
        audit("src/lib.rs", "// SAFETY: why\nunsafe { a(); }\n", true),
        audit(
            "src/sched.rs",
            "// SAFETY: why one\nunsafe { b(); }\n// SAFETY: why two\nunsafe { c(); }\n",
            true,
        ),
    ];
    assert_eq!(
        check(&audits, 3),
        Ok(3),
        "three uses across two files and a budget of three: what it answers is what a harness prints"
    );
    assert_eq!(BUDGET, 0, "and zero is the budget this step ships");
    assert_eq!(
        check(&audits, BUDGET),
        Err(jobq::engine::Refused::Over {
            count: 3,
            budget: 0
        }),
        "zero is the number the course's baseline holds, so a tree with three uses is a failure with \
         a count rather than a quiet surprise"
    );
    assert_eq!(check(&[], 0), Ok(0), "no files is no uses");
}

#[test]
fn the_refusals_come_in_the_order_a_reader_fixes_them() {
    let broken = [audit("src/lib.rs", "unsafe { a(); }\n", false)];
    assert_eq!(
        check(&broken, BUDGET),
        Err(jobq::engine::Refused::NotDenied {
            file: "src/lib.rs".to_owned()
        }),
        "the policy first: a crate that does not deny has no rule for the uses to be exceptions to, \
         so the comments are not the thing to write yet"
    );

    let commented = [audit(
        "src/lib.rs",
        "unsafe { a(); }\n// SAFETY: too late\n",
        true,
    )];
    assert_eq!(
        check(&commented, BUDGET),
        Err(jobq::engine::Refused::Undocumented {
            file: "src/lib.rs".to_owned(),
            line: 1
        }),
        "then the reason: a `// SAFETY:` below the line it is about is not above it"
    );

    let written = [audit(
        "src/lib.rs",
        "// SAFETY: why\nunsafe { a(); }\n",
        true,
    )];
    assert_eq!(
        check(&written, BUDGET),
        Err(jobq::engine::Refused::Over {
            count: 1,
            budget: 0
        }),
        "and the number last, because removing a block and raising the budget are both decisions \
         somebody makes on purpose"
    );
    assert_eq!(check(&written, 1), Ok(1));
}

#[test]
fn the_policy_is_the_manifest_and_both_spellings_say_it() {
    assert!(
        denies_unsafe_code(DENYING),
        "`unsafe_code = \"deny\"` in the lint table is what makes the default a rule"
    );
    assert!(
        denies_unsafe_code("[lints.rust]\nunsafe_code = { level = \"deny\" }\n"),
        "and cargo has two spellings of a level: a policy written down in the second one is a policy"
    );
    assert!(
        denies_unsafe_code("[workspace.lints.rust]\nunsafe_code = \"forbid\"\n"),
        "`forbid` counts, because it is `deny` with the loosen option taken away, and the \
         workspace's table is where a project that is its own workspace may put it"
    );

    for level in ["warn", "allow"] {
        assert!(
            !denies_unsafe_code(&format!("[lints.rust]\nunsafe_code = \"{level}\"\n")),
            "`{level}` is not a denial: a lint that prints a note is what a policy looks like when \
             nobody has decided anything"
        );
    }
    assert!(
        !denies_unsafe_code("[lints.rust]\n"),
        "a manifest with no policy in it does not deny"
    );
    assert!(
        !denies_unsafe_code("this is not a manifest at all"),
        "and neither does one this reader cannot parse: the honest reading of a policy that cannot \
         be read is that it is not there, which is a refusal rather than a pass"
    );
}

#[test]
fn the_crate_holds_its_own_budget() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = read(&root.join("Cargo.toml"));
    assert!(
        denies_unsafe_code(&manifest),
        "the crate's own manifest does not deny `unsafe_code`: the lint table is where this step's \
         policy lands, and every use below is judged against it"
    );

    let files = rust_files(&root.join("src"));
    assert!(
        files.len() >= 20,
        "the walk found {} file(s) under `src/`: the audit is about the crate's production code, and \
         a walk that read two of them would measure nothing",
        files.len()
    );
    let audits: Vec<Audit> = files
        .iter()
        .map(|path| {
            let file = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            audit(&file, &read(path), true)
        })
        .collect();

    let uses: usize = audits.iter().map(Audit::count).sum();
    assert_eq!(
        uses, 0,
        "the crate's production code uses the keyword {uses} time(s): this step ships a budget of \
         zero, and the file with the most to say about the rule is the one it is written in"
    );
    assert_eq!(
        check(&audits, BUDGET),
        Ok(0),
        "and the policy over the whole crate answers the same thing the budget does"
    );
    assert!(
        counts(&audits).contains(&"jobq-budget unsafe_occurrences=0".to_owned()),
        "the lines a harness prints are about the crate this case just read: {:?}",
        counts(&audits)
    );
}

#[test]
fn the_report_is_the_three_facts_a_harness_prints() {
    let audits = [
        mixed(),
        audit("src/other.rs", "// SAFETY: why\nunsafe { b(); }\n", true),
        audit("src/policy.rs", "unsafe { a(); }\n", false),
    ];
    assert_eq!(
        counts(&audits),
        [
            "jobq-budget unsafe_occurrences=7".to_owned(),
            "jobq-budget undocumented_max=4".to_owned(),
            "jobq-budget unguarded_max=1".to_owned(),
        ],
        "one metric per line, in the names the course's baseline is written in, and the file whose \
         crate does not deny is the one the third number counts"
    );
    assert_eq!(
        jobq::engine::UNSAFE_METRICS,
        ["unsafe_occurrences", "undocumented_max", "unguarded_max"],
        "the names a harness prints and a baseline holds, in one list read from both sides"
    );
    assert_eq!(
        counts(&[]),
        [
            "jobq-budget unsafe_occurrences=0".to_owned(),
            "jobq-budget undocumented_max=0".to_owned(),
            "jobq-budget unguarded_max=0".to_owned(),
        ],
        "and all three lines are printed even for nothing at all, because the gate that reads them \
         treats a missing metric as `blocked` rather than as a pass"
    );
}
