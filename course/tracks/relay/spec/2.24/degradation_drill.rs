//! Spec for `relay/2.24-degradation-drill`: three ways the service degrades, one promise per way.
//!
//! `kiln run` copies this file into the project as `tests/spec_degradation_drill.rs`.
//!
//! This step delivers two things that have to agree, and neither can be judged by the other:
//!
//! * **the drills** (`tests/support/drill.rs`): one per degradation mode, each replaying a promise
//!   the runbook makes — nothing this service accepted is still owed when the drill stops. Each one
//!   is run **twice** here (the same call, a second database), because a drill is a command somebody
//!   types more than once: before a change and after it;
//! * **the page** (`docs/runbook.md`): what a person reads at three in the morning. The `links` gate
//!   checks that every target in it resolves; this file checks that it has the sections an incident
//!   needs, that it names a command a responder can type *per drill*, and that it names the signals
//!   those commands are read against.
//!
//! And **the third thing, which is the one that makes the others worth having**: every drill is run
//! again with the broken thing never coming back, and each of those runs has to report the loss. A
//! drill that says "nothing was lost" about every world is a green light wired to nothing — it is
//! the most common shape of a written drill and it is worse than none, because it turns an incident
//! into "the drill is green, so it must be something else". The `Never` cases are the drill's own
//! falsification, and they are why the promise above means anything.

#[path = "support/drill.rs"]
mod drill;

use std::path::PathBuf;

use drill::{Recovery, Report, Scenario};

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

/// The command that replays one drill, as the page has to write it.
fn command(drill: &str) -> String {
    format!("cargo test -p relay --test spec_degradation_drill {drill}")
}

/// The five sections, and the three drills, each one replayed twice.
#[test]
fn the_database_away_drill_keeps_the_promise_and_can_be_replayed() {
    let first = drill::run(Scenario::DatabaseAway);
    assert_promise_kept(&first);
    let again = drill::run(Scenario::DatabaseAway);
    assert_promise_kept(&again);
}

#[test]
fn the_slow_subscriber_drill_keeps_the_promise_and_can_be_replayed() {
    let first = drill::run(Scenario::SlowSubscriber);
    assert_promise_kept(&first);
    let again = drill::run(Scenario::SlowSubscriber);
    assert_promise_kept(&again);
}

#[test]
fn the_tenant_storm_drill_keeps_the_promise_and_can_be_replayed() {
    let first = drill::run(Scenario::TenantStorm);
    assert_promise_kept(&first);
    let again = drill::run(Scenario::TenantStorm);
    assert_promise_kept(&again);
}

#[test]
fn the_database_away_drill_reports_the_loss_when_nothing_recovers() {
    assert_loss_reported(drill::run_with(Scenario::DatabaseAway, Recovery::Never));
}

#[test]
fn the_slow_subscriber_drill_reports_the_loss_when_nothing_recovers() {
    assert_loss_reported(drill::run_with(Scenario::SlowSubscriber, Recovery::Never));
}

#[test]
fn the_tenant_storm_drill_reports_the_loss_when_nothing_recovers() {
    assert_loss_reported(drill::run_with(Scenario::TenantStorm, Recovery::Never));
}

#[test]
fn the_runbook_names_its_sections_its_drills_and_the_signals_they_are_read_against() {
    let page = page();
    for section in SECTIONS {
        assert!(
            page.contains(section),
            "the runbook has no `{section}` section: an incident page that skips a question leaves \
             it to be invented at three in the morning"
        );
    }
    for drill in [
        "the_database_away_drill",
        "the_slow_subscriber_drill",
        "the_tenant_storm_drill",
    ] {
        assert!(
            page.contains(&command(drill)),
            "the runbook has to give the exact command that replays `{drill}` — a page whose drills \
             are described rather than runnable is a page nobody runs"
        );
    }
    assert!(
        page.contains("relay_backlog_depth"),
        "the runbook has to name the signal that says the queue is growing \
         (`relay_backlog_depth`, step `relay/2.18-delivery-metrics`)"
    );
    assert!(
        page.contains("relay_deliveries_total") && page.contains("dead"),
        "and the signal that says deliveries are being given up on \
         (`relay_deliveries_total{{outcome=\"dead\"}}`)"
    );
    assert!(
        !page.contains("TODO"),
        "every placeholder is filled: a runbook with a hole in it is one nobody trusts on the day \
         it matters"
    );
}

/// The promise a drill makes when the failure ends: nothing is still owed.
fn assert_promise_kept(report: &Report) {
    assert!(
        report.accepted > 0,
        "the drill accepted no event, so it proves nothing: {report}"
    );
    assert!(
        report.delivered > 0,
        "the drill delivered nothing, so a queue that stayed full would look the same as one that \
         moved: {report}"
    );
    assert!(
        report.nothing_was_lost(),
        "{} deliveries are still owed after the failure ended — the number that matters is not how \
         many arrived but whether anything was left behind: {report}",
        report.outstanding
    );
}

/// The falsification: the same drill, with the broken thing never coming back, has to say so.
fn assert_loss_reported(report: Report) {
    assert!(
        report.accepted > 0,
        "the drill accepted no event, so it proves nothing: {report}"
    );
    assert!(
        !report.nothing_was_lost(),
        "the drill reported nothing lost while the failure never ended. Either the drill is not \
         measuring the service (`Report` decided in the file rather than read from the rows) or the \
         failure is not real (nothing was ever in flight), and either way the green runs above are \
         worth nothing: {report}"
    );
    assert!(
        report.outstanding > 0,
        "and it has to say how much is owed: a loss with no number is a rumour: {report}"
    );
}

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
