//! `kiln lint-caps` — the course's own linter, run over its metadata.
//!
//! This is the command the goal of "complex projects never re-teach the simple ones" rests on, so
//! it prints what it could *not* check as loudly as what it checked: a rule that could not run
//! must never look like a rule that passed.

use crate::caps::lint::{self, Issue, Report};
use crate::cmd::Session;
use crate::error::{EXIT_FAILED, EXIT_OK, Result};

/// Lint the loaded course.
pub fn run(session: &Session) -> Result<u8> {
    let report = lint::lint(session.course())?;
    print_report(&report);
    if report.is_clean() {
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_FAILED)
    }
}

fn print_report(report: &Report) {
    println!(
        "{} capabilities · {} slots · {} ledger positions · {} authored steps",
        report.capabilities, report.slots, report.ledger_positions, report.authored_steps
    );

    if !report.skipped.is_empty() {
        println!();
        for notice in &report.skipped {
            println!("skipped: {notice}");
        }
    }

    if !report.reteaches.is_empty() {
        println!();
        println!("deliberate re-teaches ({}):", report.reteaches.len());
        for (step, reason) in &report.reteaches {
            println!("  {step}: {reason}");
        }
    }

    println!();
    if report.is_clean() {
        println!("no issues.");
        return;
    }
    for issue in &report.issues {
        print_issue(issue);
    }
    println!();
    println!("{} issue(s).", report.issues.len());
}

fn print_issue(issue: &Issue) {
    println!("[{}] {}", issue.rule.id(), issue.message);
}
