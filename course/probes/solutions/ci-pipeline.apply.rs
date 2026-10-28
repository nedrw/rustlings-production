//! probe = "ci/pipeline@apply"
//! title = "One list of checks, called from two places"
//! hint = "The workflow calls the task runner; a second list of checks in YAML is the list that drifts."
//!
//! Step `mailer/1.3-ci` puts every check in the task runner (`cargo xtask ci`) and makes the
//! workflow call exactly that. The failure it prevents is not a red build — it is a pipeline that
//! is green for a different reason than the one a human runs, and the difference is discovered on
//! the day it matters.
//!
//! The workflow here is a string, because a probe runs offline and cannot start GitHub. What is
//! re-checked is the rule: one entry point, called from CI, with no second copy of the checks.
//!
//! ## What you write
//!
//! [`audit`] — every finding, in this order:
//!
//! * [`Finding::NoTaskRunner`] when no line runs `cargo xtask ci`;
//! * one [`Finding::Reimplemented`] per line that runs a check the task already runs, with its
//!   one-based line number and the check that matched.
//!
//! A `#` starts a comment, as it does in the workflow itself: what is after it is not a command.
//! A line that runs the task runner and *mentions* a check in a comment is therefore not a second
//! list. When a line matches more than one check, the first match in the order given is reported —
//! one line is one decision.
//!
//! ## What the tests ask
//!
//! A workflow that only calls the task; one that adds `cargo test` (named, with its line); a
//! commented command that is not a command; a workflow with no task runner; two extra checks on
//! two lines, in order; and a check mentioned in a comment on the task line.

/// One thing wrong with the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// No line runs the task runner.
    NoTaskRunner,
    /// A line runs a check the task runner already runs.
    Reimplemented {
        /// One-based line number.
        line: usize,
        /// The check that matched.
        check: String,
    },
}

/// Every finding about `workflow`, given the checks the task runner runs.
///
/// Each check is matched as a substring of the command part of a line.
#[must_use]
pub fn audit(workflow: &str, checks: &[&str]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut runs_the_task = false;
    for (offset, raw) in workflow.lines().enumerate() {
        // A `#` starts a comment, as in the workflow itself: what follows is prose, not a command.
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.contains("cargo xtask ci") {
            runs_the_task = true;
        }
        if let Some(check) = checks.iter().find(|check| line.contains(*check)) {
            findings.push(Finding::Reimplemented {
                line: offset + 1,
                check: (*check).to_owned(),
            });
        }
    }
    let mut all = Vec::new();
    if !runs_the_task {
        all.push(Finding::NoTaskRunner);
    }
    all.extend(findings);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKS: &[&str] = &["cargo fmt", "cargo clippy", "cargo test"];

    fn reimplemented(line: usize, check: &str) -> Finding {
        Finding::Reimplemented {
            line,
            check: check.to_owned(),
        }
    }

    #[test]
    fn a_workflow_that_only_calls_the_task_is_clean() {
        let workflow = "\
name: ci
jobs:
  ci:
    steps:
      - uses: actions/checkout@v4
      - run: rustup show
      - run: cargo xtask ci
";
        assert_eq!(audit(workflow, CHECKS), []);
    }

    #[test]
    fn a_second_copy_of_a_check_is_named_with_its_line() {
        let workflow = "\
steps:
  - run: cargo xtask ci
  - run: cargo test --workspace
";
        assert_eq!(
            audit(workflow, CHECKS),
            [reimplemented(3, "cargo test")]
        );
    }

    #[test]
    fn a_comment_is_not_a_command() {
        let workflow = "\
steps:
  - run: cargo xtask ci
  # - run: cargo test --workspace
";
        assert_eq!(audit(workflow, CHECKS), []);
    }

    #[test]
    fn no_task_runner_is_its_own_finding_and_comes_first() {
        let workflow = "\
steps:
  - run: cargo fmt --all --check
  - run: cargo test --workspace
";
        assert_eq!(
            audit(workflow, CHECKS),
            [
                Finding::NoTaskRunner,
                reimplemented(2, "cargo fmt"),
                reimplemented(3, "cargo test"),
            ]
        );
    }

    #[test]
    fn two_extra_checks_arrive_in_line_order() {
        let workflow = "\
steps:
  - run: cargo clippy --workspace
  - run: cargo xtask ci
  - run: cargo fmt --all
";
        assert_eq!(
            audit(workflow, CHECKS),
            [reimplemented(2, "cargo clippy"), reimplemented(4, "cargo fmt")]
        );
    }

    #[test]
    fn a_check_named_in_a_comment_on_the_task_line_is_still_a_comment() {
        let workflow = "steps:\n  - run: cargo xtask ci # cargo test lives inside the task\n";
        assert_eq!(audit(workflow, CHECKS), []);
    }

    #[test]
    fn a_check_the_task_does_not_run_is_not_a_duplicate() {
        let workflow = "steps:\n  - run: cargo xtask ci\n  - run: cargo deny check\n";
        assert_eq!(audit(workflow, CHECKS), []);
    }
}
