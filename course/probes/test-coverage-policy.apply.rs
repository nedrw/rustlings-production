//! probe = "test/coverage-policy@apply"
//! title = "A number and the policy around it"
//! hint = "A missing report is `blocked`, never a pass; the share is of this step's own modules and rounds down; and an error branch with no test walking it fails the gate at any percentage."
//!
//! Step `jobq/3.21-coverage-policy` is where coverage stops being a badge and becomes a rule. The
//! number is the easy half — a tool prints it — and the policy is the half that gets written down in
//! `docs/coverage.md`, because every one of these questions has two defensible answers and only one
//! of them can be enforced:
//!
//! * **what is the share *of*?** The step's own modules. A crate-wide percentage moves when somebody
//!   adds a well-tested file somewhere else, and a gate that goes green because of code this step did
//!   not touch is a gate with no owner;
//! * **what happens when the tool is not installed?** `Blocked`. Not a failure — that would turn a
//!   good machine red for a missing binary, which is the rule this course made about every `should`
//!   gate (handoff-m2 §1.1) — and not a pass, because a gate that passes when it could not run is
//!   worse than no gate at all;
//! * **is a number enough?** No. An *error branch* nobody walks is a failure at **any** percentage:
//!   the happy path is easy to cover and the branches that matter are the ones where the code says
//!   no. That is why the step ships an inventory of error branches, each naming the test that
//!   reaches it, and why the inventory is checked rather than trusted;
//! * **why is the threshold not 100%?** Because some branches cannot be walked — a `match` arm the
//!   compiler proves unreachable, a defensive `unwrap` on an invariant — and the honest way to say
//!   so is an exemption **with a written reason**, counted in the report. An exemption with no
//!   reason is a hole; a hole with a reason is a decision.
//!
//! ## What you write
//!
//! * [`check`] — the whole policy: the share, the threshold, the inventory, the exemptions and the
//!   four lists a learner has to act on.
//!
//! [`Line`] (what the tool exports), [`Branch`] (one entry of the inventory), [`Policy`] (the
//! report) and [`Verdict`] are given: the shapes are the step's, and a probe that let you rename
//! them would be checking a different policy.
//!
//! ## What the tests ask
//!
//! No report is `blocked` and never a pass. The share is of the step's modules — a file outside them
//! does not move it — and it rounds down, so a threshold is never flattered. A branch whose line has
//! no hits fails the gate while the overall number is a clean 100%. A justified exemption passes and
//! is counted; a blank justification is not an exemption. An inventory entry naming a test that does
//! not exist fails, because an inventory is a claim until it is checked. A module the report never
//! mentions fails rather than counting as fully covered. And a policy that is satisfied passes with
//! all four lists empty.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no `cargo-llvm-cov` and no profile data**: the report arrives as
//! data, because the real gate is a `should` gate precisely so that a machine without the tool is
//! not red (handoff-m2 §1.3). What is re-checked here is everything the gate does *after* the tool
//! ran — which modules the share is over, how it rounds, what happens with no report at all, and the
//! inventory rules — and that is the half a learner writes and defends.
//!
//! Also missing on purpose: no branch-level coverage data (this counts *lines*, like the gate; a
//! branch percentage is a different tool and a different argument), no per-function breakdown, and
//! no parsing of the tool's own output format — an exporter with a version to it is a dependency,
//! and a probe has none.

/// One line of the tool's report: a file, a line number and how many times it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The file, as the tool names it: relative to the project root.
    pub file: String,
    /// Line number, counting from 1.
    pub line: u32,
    /// How many times the line was executed. Zero means nobody walked it.
    pub hits: u64,
}

/// One entry of the error-branch inventory: a branch that has to be walked, and by what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// The file the branch is in.
    pub file: String,
    /// The line the branch is on.
    pub line: u32,
    /// Which branch, in a sentence a reviewer can match against the source.
    pub what: String,
    /// The test that walks it.
    pub test: String,
    /// The reason it is not walked at all, when it is not. An exemption, and the reason is what
    /// makes it one.
    pub justification: Option<String>,
}

/// What the policy decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The share reached the threshold and every branch the inventory names is accounted for.
    Pass,
    /// It did not, or the inventory has a hole. The lists say which.
    Fail,
    /// There was nothing to measure — the tool did not run. Never a pass, and never a failure of the
    /// learner's code.
    Blocked,
}

/// The report a learner reads: the number, the verdict, and one list per thing to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// The verdict.
    pub verdict: Verdict,
    /// The share of the step's own lines that were walked, rounded down.
    pub percent: u32,
    /// Inventory entries whose line nobody walked, as `file:line: what`.
    pub unwalked: Vec<String>,
    /// Inventory entries whose justification is empty or only whitespace, as `file:line: what`.
    pub unjustified: Vec<String>,
    /// Inventory entries naming a test that does not exist.
    pub missing_tests: Vec<String>,
    /// Modules of this step the report does not mention at all.
    pub unknown_files: Vec<String>,
    /// How many branches were exempted with a written reason. Reported, because "why is this not
    /// 100%" is a question the number should answer.
    pub exemptions: usize,
}

/// Apply the policy to one coverage run.
///
/// `modules` are the files this step added or changed — the share is over their lines only.
/// `report` is `None` when the tool could not run at all. `inventory` is the error-branch list,
/// `tests` are the test names that exist, and `threshold` is the percentage the step promised.
#[must_use]
pub fn check(
    modules: &[&str],
    report: Option<&[Line]>,
    inventory: &[Branch],
    tests: &[&str],
    threshold: u32,
) -> Policy {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULES: [&str; 2] = ["src/store.rs", "src/claims.rs"];
    const TESTS: [&str; 3] = [
        "store_keeps_the_row",
        "a_refused_claim_changes_nothing",
        "the_lease",
    ];

    fn line(file: &str, number: u32, hits: u64) -> Line {
        Line {
            file: file.to_owned(),
            line: number,
            hits,
        }
    }

    fn branch(file: &str, number: u32, test: &str) -> Branch {
        Branch {
            file: file.to_owned(),
            line: number,
            what: "the store refused the write".to_owned(),
            test: test.to_owned(),
            justification: None,
        }
    }

    /// A report where both modules are walked, plus one line in the modules nobody watched.
    fn healthy_report() -> Vec<Line> {
        vec![
            line("src/store.rs", 10, 4),
            line("src/store.rs", 11, 0),
            line("src/claims.rs", 20, 7),
            line("src/main.rs", 1, 0),
        ]
    }

    #[test]
    fn a_report_the_tool_could_not_produce_is_blocked_and_never_green() {
        let policy = check(
            &MODULES,
            None,
            &[branch("src/store.rs", 10, "store_keeps_the_row")],
            &TESTS,
            0,
        );
        assert_eq!(
            policy.verdict,
            Verdict::Blocked,
            "a missing tool is not a failure of the learner's code, and a threshold of zero does not \
             make it a pass"
        );
        assert_eq!(policy.percent, 0);
        assert!(
            policy.unwalked.is_empty() && policy.missing_tests.is_empty(),
            "nothing is blamed on a report that does not exist: {policy:?}"
        );
    }

    #[test]
    fn the_share_is_of_this_steps_modules_and_rounds_down() {
        let report = healthy_report();
        // Two of the three lines in our modules were walked: the untouched file is not counted and
        // does not dilute the answer.
        let policy = check(&MODULES, Some(&report), &[], &TESTS, 66);
        assert_eq!(policy.percent, 66, "two of three, rounded down");
        assert_eq!(policy.verdict, Verdict::Pass);
        assert!(policy.unknown_files.is_empty());

        assert_eq!(
            check(&MODULES, Some(&report), &[], &TESTS, 67).verdict,
            Verdict::Fail,
            "66.6 percent does not meet a threshold of 67, which is the whole point of rounding down"
        );

        // A report that covered a module with no lines at all is a report that measured nothing.
        let empty: Vec<Line> = Vec::new();
        let policy = check(&MODULES, Some(&empty), &[], &TESTS, 0);
        assert_eq!(policy.percent, 0);
        assert_eq!(
            policy.unknown_files,
            vec!["src/store.rs".to_owned(), "src/claims.rs".to_owned()],
            "no line for a module means the report does not know about it"
        );
        assert_eq!(policy.verdict, Verdict::Fail);
    }

    #[test]
    fn an_error_branch_nobody_walked_fails_the_gate_at_any_percentage() {
        let mut report = healthy_report();
        // Make the overall number perfect: every line in the step's modules was walked.
        for entry in &mut report {
            if entry.file.starts_with("src/store") || entry.file.starts_with("src/claims") {
                entry.hits = 1;
            }
        }
        let inventory = [branch(
            "src/claims.rs",
            99,
            "a_refused_claim_changes_nothing",
        )];

        let policy = check(&MODULES, Some(&report), &inventory, &TESTS, 100);
        assert_eq!(policy.percent, 100, "the number is a clean hundred");
        assert_eq!(
            policy.verdict,
            Verdict::Fail,
            "and the gate still fails: the branch that matters is the one where the code says no, \
             and a percentage is not a substitute for walking it: {policy:?}"
        );
        assert_eq!(
            policy.unwalked,
            vec!["src/claims.rs:99: the store refused the write".to_owned()]
        );

        // Walking it is what turns the gate green.
        let walked = [branch("src/store.rs", 10, "store_keeps_the_row")];
        assert_eq!(
            check(&MODULES, Some(&report), &walked, &TESTS, 100).verdict,
            Verdict::Pass
        );
    }

    #[test]
    fn a_branch_with_a_written_reason_is_an_exemption_and_is_counted() {
        let report = healthy_report();
        let mut exempt = branch("src/claims.rs", 99, "the_lease");
        exempt.justification = Some(
            "the compiler proves the slice is non-empty, so the guard cannot be taken".to_owned(),
        );
        let policy = check(&MODULES, Some(&report), &[exempt.clone()], &TESTS, 60);
        assert_eq!(policy.verdict, Verdict::Pass);
        assert_eq!(
            policy.exemptions, 1,
            "an exemption is counted, because the number is what answers `why is this not 100%`"
        );
        assert!(policy.unwalked.is_empty() && policy.unjustified.is_empty());

        // Whitespace is not a reason. An exemption without one is a hole, and the policy says so
        // rather than rounding it away.
        exempt.justification = Some("   ".to_owned());
        let policy = check(&MODULES, Some(&report), &[exempt], &TESTS, 60);
        assert_eq!(policy.verdict, Verdict::Fail);
        assert_eq!(policy.exemptions, 0);
        assert_eq!(
            policy.unjustified,
            vec!["src/claims.rs:99: the store refused the write".to_owned()]
        );
        assert!(
            policy.unwalked.is_empty(),
            "it is not reported twice: the entry is exempt-shaped with no reason, and that is the \
             one thing wrong with it"
        );
    }

    #[test]
    fn an_inventory_that_names_a_test_nobody_has_is_a_claim_not_a_check() {
        let report = healthy_report();
        let inventory = [
            branch("src/store.rs", 10, "store_keeps_the_row"),
            branch("src/claims.rs", 20, "the_test_that_was_never_written"),
        ];
        let policy = check(&MODULES, Some(&report), &inventory, &TESTS, 50);
        assert_eq!(policy.verdict, Verdict::Fail);
        assert_eq!(
            policy.missing_tests,
            vec!["src/claims.rs:20: the store refused the write".to_owned()],
            "an inventory entry pointing at a test that does not exist is worse than a missing \
             entry: it reads like coverage"
        );
        assert!(
            policy.unwalked.is_empty(),
            "and the entry is not also called unwalked — one entry, one problem"
        );
    }

    #[test]
    fn a_module_the_report_forgot_cannot_be_silently_full() {
        let report = vec![line("src/store.rs", 10, 9), line("src/store.rs", 11, 9)];
        let policy = check(&MODULES, Some(&report), &[], &TESTS, 100);
        assert_eq!(
            policy.percent, 100,
            "the lines the report does have are all walked"
        );
        assert_eq!(
            policy.unknown_files,
            vec!["src/claims.rs".to_owned()],
            "and a module the report never mentions is a fail rather than an implicit hundred"
        );
        assert_eq!(policy.verdict, Verdict::Fail);
    }

    #[test]
    fn a_policy_that_is_satisfied_passes_with_nothing_to_do() {
        let report = healthy_report();
        let inventory = [
            branch("src/store.rs", 10, "store_keeps_the_row"),
            branch("src/claims.rs", 20, "a_refused_claim_changes_nothing"),
        ];
        let policy = check(&MODULES, Some(&report), &inventory, &TESTS, 66);
        assert_eq!(
            policy,
            Policy {
                verdict: Verdict::Pass,
                percent: 66,
                unwalked: Vec::new(),
                unjustified: Vec::new(),
                missing_tests: Vec::new(),
                unknown_files: Vec::new(),
                exemptions: 0,
            },
            "a pass is a report with nothing in it: four empty lists are what `green` means here"
        );
    }
}
