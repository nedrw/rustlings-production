//! Spec for `mailer/1.22-runbook`: the drill and the page it belongs to.
//!
//! A runbook is two things that have to agree, and neither can be judged by the other:
//!
//! * **the drill** (`tests/support/drill.rs`): it has to replay the promise the page makes — no
//!   message is lost while the mail server is away — *and* it has to be able to report the
//!   opposite. The same replay with the server never coming back is the case where the promise is
//!   broken, and a drill that cannot say so is decoration. The second test is what says so;
//! * **the page** (`docs/runbook.md`): what a person reads at three in the morning — what to look
//!   at, what to run, what "recovered" means. The `links` gate checks that its targets resolve;
//!   this file checks that it has the sections an incident needs and names the drill and the signal.
//!
//! `cargo test -p mailer --test spec_runbook_drill` is the command the runbook tells a responder to
//! replay, so the two halves are literally the same thing run from two directions.

#[path = "support/drill.rs"]
mod drill;

use std::path::PathBuf;

use drill::{Recovery, Report};

/// The page this step delivers, relative to the project.
const RUNBOOK: &str = "docs/runbook.md";

/// The headings an incident page needs, in the order a responder reads them.
const SECTIONS: [&str; 5] = [
    "## Symptoms",
    "## Signals",
    "## Steps",
    "## Verification",
    "## Escalation",
];

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RUNBOOK);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is the page this step delivers, and it cannot be read: {error}",
            path.display()
        )
    })
}

#[test]
fn the_drill_reports_the_promise_kept_when_the_server_comes_back() {
    let report: Report = drill::replay(Recovery::ComesBack);
    assert!(
        report.accepted > 0,
        "the drill accepted no message, so it proves nothing: {report:?}"
    );
    assert!(
        report.nothing_was_lost(),
        "a message was lost although the mail server came back: {report:?}"
    );
}

#[test]
fn the_drill_reports_the_loss_when_the_server_never_comes_back() {
    let report = drill::replay(Recovery::Never);
    assert!(
        report.accepted > 0,
        "the drill accepted no message, so it proves nothing: {report:?}"
    );
    assert!(
        !report.nothing_was_lost(),
        "the drill reported the promise kept while the mail server never came back: {report:?}"
    );
    assert!(
        report.given_up > 0,
        "the service gave up on nothing, which is not what an outage that outlasts the retry budget \
         looks like: {report:?}"
    );
}

#[test]
fn the_runbook_names_its_sections_the_drill_and_the_signal() {
    let page = page();
    for section in SECTIONS {
        assert!(
            page.contains(section),
            "the runbook has no `{section}` section: an incident page that skips a question leaves \
             it to be invented at three in the morning"
        );
    }
    assert!(
        page.contains("spec_runbook_drill"),
        "the runbook has to name the drill a responder replays — otherwise the page and the evidence \
         are two documents that never meet"
    );
    assert!(
        page.contains("deliveries_total") && page.contains("given_up"),
        "the runbook has to name the signal that says mail is being given up on \
         (`deliveries_total{{outcome=\"given_up\"}}`, step `mailer/1.20-metrics`)"
    );
}
