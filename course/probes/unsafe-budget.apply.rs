//! probe = "unsafe/budget@apply"
//! title = "Deny by default, and write down every exception"
//! hint = "Count the word `unsafe` in production lines only, insist on a `// SAFETY:` comment with a reason on one of the three lines above the use (a marker on the same line does not count), and refuse a crate whose lint policy does not deny by default."
//!
//! Step `jobq/3.24-unsafe-budget` is a policy expressed as three facts, and the gate that checks them
//! is a text scan plus the compiler (handoff-m2 §1.3):
//!
//! 1. **`unsafe_code = "deny"` is the default, and it is a decision.** A crate that says nothing
//!    about `unsafe` has not decided anything; a crate that denies has decided that every use is an
//!    exception somebody has to argue for. [That switch is not a formality:] without it, the third
//!    fact below is a habit rather than a rule, and a habit is what the next contributor does not
//!    have;
//! 2. **the budget is a number.** `unsafe-budget@0` is what `jobq/3.24` ships: zero uses in
//!    production code, and allowing one costs a raised number and a review conversation;
//! 3. **every use carries a reason, on one of the three lines above it.** `// SAFETY:` followed by
//!    *what would break if it were wrong* — not "this is safe", not an empty marker. A marker on the
//!    use's own line does not count and a marker below never does, because the window is the lines
//!    *above* the use. The comment is the only place a future reader can find out why the `unsafe`
//!    is there, and a bare marker is a comment that looks like evidence and says nothing.
//!
//! The rule about *which* lines count is the one that makes the number mean something: `#[cfg(test)]`
//! modules are not production code, and an `unsafe` in a test is a different statement from an
//! `unsafe` in the library (the same split clippy's target selection uses). A count that included
//! tests would push a project to write worse tests.
//!
//! ## What you write
//!
//! * [`audit`] — one file: every `unsafe` in its production lines, each with whether it is
//!   documented and with what;
//! * [`check`] — the policy over a set of files: the deny-by-default rule, the documentation rule,
//!   and the budget.
//!
//! [`Occurrence`], [`Audit`] and [`Refused`] are given: the refusal vocabulary is what a report and
//! a reviewer read, and a probe that let you rename it would be checking a different gate.
//!
//! ## What the tests ask
//!
//! The count is the *word* `unsafe` — `unsafe_code` in a lint attribute is not a use of it. A
//! `// SAFETY:` comment on one of the three lines above the occurrence documents it; a marker on the
//! occurrence's own line does not, and neither does one four lines above. A bare `// SAFETY:` with no
//! reason is not documentation, and it does not borrow a reason from further up. A `#[cfg(test)]`
//! module is skipped wherever it is in the file, nested blocks included. Then the policy: a crate
//! that does not deny by default cannot pass on the strength of its comments, going over the budget
//! names the count, and the three refusals come in the order a reader fixes them — the policy, the
//! comment, the number.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no `cargo clippy` and no `Cargo.toml`**: the real gate reads the
//! workspace lint table and runs the compiler, and the "denies by default" flag arrives here as a
//! parameter because a probe has no crate to read. What is re-checked is the scan the gate does and
//! the order it refuses in — which is the half a learner writes, and the half that decides whether
//! the gate is a rule or a report.
//!
//! Two narrowings, stated rather than hidden: the scan is textual, so it counts the word inside a
//! string literal or a comment too (a parser would not, and the gate's own answer is that a
//! `// SAFETY:` above it settles the line either way); and `unsafe` in a `macro_rules!` body is
//! counted where it is written rather than where it expands, which is the reading a reviewer can
//! follow.

/// One place production code uses `unsafe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    /// Line number, counting from 1.
    pub line: u32,
    /// Whether a `// SAFETY:` comment with a reason is on one of the three lines above it. A marker
    /// on the occurrence's own line is not above it, and does not document the use.
    pub documented: bool,
    /// The reason, when there is one.
    pub safety: Option<String>,
}

/// What one file says about its own `unsafe`, and what the crate's policy is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audit {
    /// The file, as the report names it.
    pub file: String,
    /// Every use in its production lines, in line order.
    pub occurrences: Vec<Occurrence>,
    /// Whether the crate denies `unsafe_code` by default. Read from the workspace lint table by the
    /// real gate; a parameter here, because a probe has no manifest.
    pub denies_by_default: bool,
}

/// Why the budget was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The crate does not deny `unsafe_code` by default: there is no policy for the exceptions to be
    /// exceptions *to*.
    NotDenied {
        /// Which file said nothing.
        file: String,
    },
    /// A use with no `// SAFETY:` reason.
    Undocumented {
        /// Which file.
        file: String,
        /// Which line.
        line: u32,
    },
    /// More uses than the budget allows.
    Over {
        /// How many were found.
        count: usize,
        /// How many are allowed.
        budget: u32,
    },
}

/// Audit one file: every `unsafe` in its production lines, and whether each is documented.
///
/// "Production lines" means everything outside a `#[cfg(test)]` module, wherever that module sits in
/// the file. An occurrence is documented when the nearest `// SAFETY:` among the three lines above it
/// carries a reason: the occurrence's own line is not above it, a bare marker is not a reason, and it
/// does not fall through to an older comment, because the line a reader looks at first is the line
/// immediately above.
#[must_use]
pub fn audit(file: &str, source: &str, denies_by_default: bool) -> Audit {
    todo!()
}

/// Apply the policy to a set of files.
///
/// The order the three rules are checked in is the order a reader fixes them, and it is part of the
/// answer: **the policy first** (a crate that does not deny has no rule for the exceptions to be
/// exceptions to), **then the comments** (a line to write), **then the number** (a block to remove,
/// or a budget to raise on purpose). Refusals name the file and the line, because "unsafe budget
/// exceeded" sends a learner to grep.
///
/// # Errors
///
/// [`Refused::NotDenied`], [`Refused::Undocumented`] or [`Refused::Over`], in that order.
pub fn check(audits: &[Audit], budget: u32) -> Result<usize, Refused> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with one documented use, one bare marker, and one use with a comment too far above.
    const MIXED: &str = "\
// SAFETY: the map will not move, so the pointer stays valid
unsafe { touch(); }
let x = 1;
// SAFETY:
unsafe { bare(); }
// SAFETY: four lines up and out of the window


let y = 2;
unsafe { undocumented(); }
";

    /// A library with a test module in the middle and one at the end.
    const WITH_TESTS: &str = "\
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
unsafe_code = \"deny\";
unsafe fn after_the_tests() {}
";

    #[test]
    fn the_count_is_the_word_unsafe_and_not_the_policy() {
        let report = audit("src/lib.rs", WITH_TESTS, true);
        assert_eq!(
            report.occurrences.len(),
            2,
            "one use before the test module and one after it; `unsafe_code` is not a use of \
             `unsafe`: {:?}",
            report.occurrences
        );
        assert_eq!(
            report
                .occurrences
                .iter()
                .map(|occurrence| occurrence.line)
                .collect::<Vec<u32>>(),
            vec![1, 15],
            "and the lines are the real ones, so a report can be acted on"
        );
    }

    #[test]
    fn tests_are_not_production_code() {
        let report = audit("src/lib.rs", WITH_TESTS, true);
        let lines: Vec<u32> = report
            .occurrences
            .iter()
            .map(|occurrence| occurrence.line)
            .collect();
        assert_eq!(
            lines,
            vec![1, 15],
            "the two production lines, and not the two inside the test module (6 and 10): an \
             `unsafe` in a test is a different statement from one in the library, and a count that \
             included them would push a project to write worse tests"
        );

        // A whole file of tests has no production uses at all, and the policy is happy with it.
        let tests_only =
            "unsafe { production_looking(); }\n#[cfg(test)]\nmod tests {\n    unsafe { x(); }\n}\n";
        let support = audit("tests/support/ptr.rs", tests_only, true);
        assert_eq!(
            support.occurrences.len(),
            1,
            "the line above the module still counts"
        );
        assert_eq!(support.occurrences[0].line, 1);
    }

    #[test]
    fn a_comment_in_the_three_lines_above_is_what_documents_a_use() {
        let report = audit("src/lib.rs", MIXED, true);
        assert_eq!(
            report
                .occurrences
                .iter()
                .map(|occurrence| (occurrence.line, occurrence.documented))
                .collect::<Vec<(u32, bool)>>(),
            vec![(2, true), (5, false), (10, false)],
            "the reason is found on one of the three lines above: {:?}",
            report.occurrences
        );
        assert_eq!(
            report.occurrences[0].safety.as_deref(),
            Some("the map will not move, so the pointer stays valid"),
            "and the reason is carried, because a report that only says `documented` sends a \
             reviewer to open the file"
        );
    }

    #[test]
    fn a_bare_marker_is_not_a_reason() {
        let report = audit("src/lib.rs", MIXED, true);
        assert_eq!(
            report.occurrences[1].safety, None,
            "`// SAFETY:` with nothing after it looks like evidence and says nothing"
        );
        assert!(!report.occurrences[1].documented);
        assert_eq!(
            report.occurrences[2].safety, None,
            "and it does not borrow the reason from four lines up: the window is three"
        );
    }

    #[test]
    fn the_budget_is_a_number_and_going_over_names_the_count() {
        let audits = [
            audit("src/lib.rs", "// SAFETY: why\nunsafe { a(); }\n", true),
            audit(
                "src/sched.rs",
                "// SAFETY: why\nunsafe { b(); }\nlet x = 1;\nlet y = 2;\nlet z = 3;\nunsafe { c(); }\n",
                true,
            ),
        ];
        assert_eq!(
            check(&audits, 0),
            Err(Refused::Undocumented {
                file: "src/sched.rs".to_owned(),
                line: 6
            }),
            "the second use is more than three lines below the reason, so it has none of its own: \
             the undocumented use comes before the count, because it is the comment to write first"
        );

        let documented = [
            audit("src/lib.rs", "// SAFETY: why\nunsafe { a(); }\n", true),
            audit(
                "src/sched.rs",
                "// SAFETY: why one\nunsafe { b(); }\n// SAFETY: why two\nunsafe { c(); }\n",
                true,
            ),
        ];
        assert_eq!(
            check(&documented, 3),
            Ok(3),
            "three uses across two files, a budget of three: the number the gate prints is what it \
             counted, and it counts every file it was given"
        );
        assert_eq!(
            check(&documented, 0),
            Err(Refused::Over {
                count: 3,
                budget: 0
            }),
            "and zero is the budget the step ships with"
        );
        assert_eq!(check(&[], 0), Ok(0), "no files is no uses");
    }

    #[test]
    fn a_crate_that_does_not_deny_by_default_cannot_pass_on_its_comments() {
        let audits = [audit(
            "src/lib.rs",
            "// SAFETY: why\nunsafe { a(); }\n",
            false,
        )];
        assert_eq!(
            check(&audits, 1),
            Err(Refused::NotDenied {
                file: "src/lib.rs".to_owned()
            }),
            "the exceptions are exceptions to a rule, and the rule is `unsafe_code = \"deny\"`: \
             without it this is a habit, and a habit is what the next contributor does not have"
        );
        assert_eq!(
            check(&audits, 0),
            Err(Refused::NotDenied {
                file: "src/lib.rs".to_owned()
            })
        );
    }

    #[test]
    fn the_refusals_come_in_the_order_a_reader_fixes_them() {
        // One file that breaks all three rules at once.
        let broken = [audit("src/lib.rs", "unsafe { a(); }\n", false)];
        assert_eq!(
            check(&broken, 0),
            Err(Refused::NotDenied {
                file: "src/lib.rs".to_owned()
            }),
            "the policy first"
        );

        let commented = [audit(
            "src/lib.rs",
            "unsafe { a(); }\n// SAFETY: too late\n",
            true,
        )];
        assert_eq!(
            check(&commented, 0),
            Err(Refused::Undocumented {
                file: "src/lib.rs".to_owned(),
                line: 1
            }),
            "then the comment: a `// SAFETY:` below the line it is about is not above it"
        );

        // And the other end of the same window: a marker documents the lines *above* it, so one
        // written on the occurrence's own line documents nothing. The scan counts the word wherever
        // it is written — in the marker line itself too, which the module's stated narrowing allows
        // — so this file holds one occurrence and the only marker anywhere near it, and the answer
        // has to be "not documented".
        let same_line = [audit(
            "src/lib.rs",
            "// SAFETY: the unsafe call below is sound\n",
            true,
        )];
        assert!(
            !same_line[0].occurrences[0].documented,
            "the marker is on the occurrence's own line, which is not one of the three lines above \
             it: {:?}",
            same_line[0].occurrences
        );
        assert_eq!(
            check(&same_line, 0),
            Err(Refused::Undocumented {
                file: "src/lib.rs".to_owned(),
                line: 1
            }),
            "so the use comes back undocumented, and the refusal names the use rather than the \
             comment"
        );

        let written = [audit(
            "src/lib.rs",
            "// SAFETY: why\nunsafe { a(); }\n",
            true,
        )];
        assert_eq!(
            check(&written, 0),
            Err(Refused::Over {
                count: 1,
                budget: 0
            }),
            "and the number last, because removing a block or raising the budget are both \
             decisions somebody makes on purpose"
        );
        assert_eq!(check(&written, 1), Ok(1));
    }
}
