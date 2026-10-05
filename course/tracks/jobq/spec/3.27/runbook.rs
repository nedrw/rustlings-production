//! Spec for `jobq/3.27-runbook-drill`: the procedure, and the drill that finds the stage it is
//! missing.
//!
//! `kiln run` copies this file into the project as `tests/spec_runbook.rs`. Three subjects, and each
//! one is judged by something the others cannot check:
//!
//! * **the page** (`docs/runbook.md`) is the deliverable, and its shape is judged through the
//!   crate's own reader: [`Runbook::of`] parses the stages and [`Runbook::gaps`] names every stage
//!   that is not runnable. The four parts are the whole of the rule (`command`, `signal`, `refusal`,
//!   `rollback`), and the cases below judge the *reader* on pages this file writes — so a reader that
//!   waves a page through fails here even while the page passes;
//! * **the drill** (`tests/support/drill.rs`) walks the page's stages in order and takes the acts
//!   they name, so the page and the drill are compared in both directions: an act the page names and
//!   the drill cannot take is a stage nobody has run, and an act the drill needs and the page does
//!   not name is the stage this step exists to find (`World::Refused`);
//! * **the worlds** are what makes either of those evidence. `World::Recovers` is the promise,
//!   `World::Never` is the same procedure with nobody left to take the queue on — a drill that
//!   reports the promise held in both is a green light wired to nothing — and `World::Refused`
//!   replays the one act that cannot be undone, taken before the stage that says which rows it is
//!   for.
//!
//! What this file does **not** judge: whether the acts work. That is the engine's own suite, and a
//! page is not the place it is proved.

#[path = "support/drill.rs"]
mod drill;

use std::path::PathBuf;

use jobq::engine::{Gap, PARTS, Runbook};

use drill::{Report, World};

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/runbook.md";

/// The sections an incident page needs, in the order a responder reads them (`mailer/1.22-runbook`).
const SECTIONS: [&str; 5] = [
    "## Symptoms",
    "## Signals",
    "## Steps",
    "## Verification",
    "## Escalation",
];

/// The signals this page is read against: the gauge that means "held", and the two counters that
/// say whether the queue is moving (`jobq/3.17-error-budget-alerts`).
const SIGNALS: [&str; 3] = [
    "jobq_running",
    "jobq_succeeded_total",
    "jobq_reclaimed_total",
];

/// The fewest stages a procedure here can have: one per act the deployment offers, and the track
/// has been accumulating acts for twenty-six steps.
const STAGES: usize = 5;

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is the page this step delivers, and it cannot be read: {error}",
            path.display()
        )
    })
}

/// The page, as the crate's own reader sees it.
fn runbook() -> Runbook {
    Runbook::of(&page())
}

/// One stage of the reference shape: a numbered heading and four lines, one per part.
///
/// The fixtures below are written out of these rather than pasted, so a case that is about one part
/// cannot go stale the day another part is spelled differently.
fn stage(number: u32, title: &str) -> String {
    format!(
        "### {number}. {title}\n- command: `jobqc usage`\n- signal: the numbers move\n\
         - refusal: exit `4`\n- rollback: nothing was taken\n\n"
    )
}

/// The same stage with one of its four parts taken out.
fn without(stage: &str, part: &str) -> String {
    let opening = format!("- {part}:");
    stage
        .lines()
        .filter(|line| !line.starts_with(&opening))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// A page whose stages are what the case hands in, with the sections around them a page has.
fn fixture(steps: &str) -> String {
    format!("## Symptoms\n\nsomething is wrong\n\n## Steps\n\n{steps}## Escalation\n\nnobody\n")
}

/// The page is the procedure: every stage names the four things a responder is owed.
#[test]
fn the_page_is_a_procedure_whose_stages_all_hold() {
    let text = page();
    assert!(
        !text.contains("TODO"),
        "`{PAGE}` still has a placeholder in it: the page is the deliverable, and a stage nobody \
         finished is a stage the responder finishes at three in the morning"
    );

    let runbook = runbook();
    assert!(
        runbook.stages().len() >= STAGES,
        "`{PAGE}` names {} stage(s), and a procedure for this deployment has at least {STAGES}: \
         every act the responder takes has to be a stage, or the act is taken from memory",
        runbook.stages().len()
    );

    let gaps: Vec<String> = runbook.gaps().iter().map(ToString::to_string).collect();
    assert!(
        gaps.is_empty(),
        "`{PAGE}` is not runnable:\n    {}",
        gaps.join("\n    ")
    );

    for stage in runbook.stages() {
        for part in PARTS {
            assert!(
                stage
                    .answer(part)
                    .is_some_and(|value| !value.trim().is_empty()),
                "stage {} does not say what its `{part}` is",
                stage.number()
            );
        }
    }
}

/// The page is for a person, and it is read against the signals of `jobq/3.17`.
#[test]
fn the_page_names_its_sections_the_drill_and_the_signals() {
    let text = page();
    for section in SECTIONS {
        assert!(
            text.contains(section),
            "`{PAGE}` has no `{section}` section: an incident page that skips a question leaves it \
             to be invented under pressure"
        );
    }
    assert!(
        text.contains("spec_runbook"),
        "`{PAGE}` has to name the drill a responder replays — otherwise the page and the evidence \
         are two documents that never meet"
    );
    for signal in SIGNALS {
        assert!(
            text.contains(signal),
            "`{PAGE}` has to name `{signal}`: a procedure written against numbers the deployment \
             does not publish is a procedure about nothing"
        );
    }
    for class in ["`2`", "`3`", "`4`"] {
        assert!(
            text.contains(class),
            "and the classes a script branches on are on it, {class} included: this project answers \
             a class rather than a sentence, and a refusal column that does not name one is a column \
             the responder has to look up"
        );
    }
}

/// The drill and the page are one list, read from both ends — and the promise the last stage makes
/// holds when a worker takes the queue on.
#[test]
fn the_drill_runs_every_stage_the_page_names() {
    let runbook = runbook();
    let report: Report = drill::replay(World::Recovers);

    assert!(
        report.unnamed.is_empty(),
        "the page names no stage for {}: the drill takes these acts and the page leaves them out, \
         which is the stage this step is about finding",
        report.unnamed.join(", ")
    );
    assert_eq!(
        report.stages,
        runbook.stages().len(),
        "the drill walked {} stage(s) and the page names {}: a stage the drill cannot take is a \
         stage nobody has run, and the two have drifted",
        report.stages,
        runbook.stages().len()
    );
    assert!(
        report.promise_held(),
        "the promise the page's last stage makes — nothing the deployment accepted is still owed \
         — did not hold: {report:?}"
    );
}

/// A drill that reports the promise held in every world is decoration: this is the world where it
/// cannot, and the number it reports is the one the page's last stage promises is zero.
#[test]
fn the_drill_reports_the_work_that_is_still_owed() {
    let recovers = drill::replay(World::Recovers);
    let never = drill::replay(World::Never);

    assert_eq!(
        never.stages, recovers.stages,
        "the two worlds walk the same page — what changed is whether anybody comes back, and a \
         drill whose worlds differ in the procedure is two drills: {recovers:?} against {never:?}"
    );
    assert_eq!(
        recovers.outstanding, 0,
        "with a worker back on the queue nothing is owed: {recovers:?}"
    );
    assert!(
        never.outstanding > 0,
        "with nobody back the jobs the procedure put in the queue are still owed, and a drill that \
         reports zero here is a drill that cannot fail: {never:?}"
    );
    assert!(
        !never.promise_held(),
        "and the promise is broken in that world, which is what makes it evidence: {never:?}"
    );
}

/// The refusal is a stage and not a sentence: the drill takes the act that cannot be undone before
/// the stage that says which rows it is for, and the page has to say what the door leaves behind.
#[test]
fn the_drill_replays_the_refusal_and_the_page_says_what_it_leaves() {
    let runbook = runbook();
    let report = drill::replay(World::Refused);

    assert!(
        report.unnamed.is_empty(),
        "the page names no stage for {}: the act the door refuses is the act a responder is most \
         likely to get wrong, and it has to be on the page",
        report.unnamed.join(", ")
    );

    let number = report.refused.unwrap_or_else(|| {
        panic!(
            "the drill took the act the door refuses, and did not walk a stage whose command is \
             that act: {report:?} — a refusal the page does not name is a refusal the responder \
             meets for the first time during an incident"
        )
    });
    let stage = runbook
        .stages()
        .iter()
        .find(|stage| stage.number() == number)
        .unwrap_or_else(|| panic!("the drill walked stage {number}, which is not on the page"));

    assert!(
        stage.command().contains("discard"),
        "the act that cannot be undone is the stage the door refused, and this one runs `{}`",
        stage.command()
    );
    assert!(
        stage.refusal().contains('2'),
        "and its refusal column names the class the door answers with — `2`, the call was wrong: \
         `{}`",
        stage.refusal()
    );
    assert!(
        stage.rollback().to_lowercase().contains("none"),
        "the one irreversible act has to say so in its rollback column, in the word the page uses \
         for it (`none`): `{}`",
        stage.rollback()
    );

    // The refusal is one act taken too early and not a different procedure: everything else about
    // the two reports has to be the same, or the page has two procedures in it.
    let recovers = drill::replay(World::Recovers);
    assert_eq!(
        report.stages, recovers.stages,
        "the refused world walks the same page: {report:?} against {recovers:?}"
    );
    assert_eq!(
        report.unnamed, recovers.unnamed,
        "and the page names the same acts in both: {report:?} against {recovers:?}"
    );
    assert_eq!(
        report.outstanding, recovers.outstanding,
        "a refused act moves nothing, so what is left owed is what the recovering world owed: \
         {report:?} against {recovers:?}"
    );
    assert!(
        report.promise_held(),
        "and the procedure still ends with nothing owed, which is what a refused act leaves: \
         {report:?}"
    );
}

/// The reader itself, judged on pages this file writes: a stage that forgets one of the four parts
/// is not runnable.
#[test]
fn a_stage_that_does_not_name_its_four_parts_is_a_gap() {
    for part in PARTS {
        let page = fixture(&without(&stage(1, "Read the queue"), part));
        let runbook = Runbook::of(&page);
        assert_eq!(
            runbook.gaps(),
            vec![Gap::Missing { number: 1, part }],
            "a stage that says nothing about its `{part}` is the gap a responder meets as a blank \
             line: {page}"
        );
        assert!(!runbook.holds(), "and the page does not hold: {page}");
        assert_eq!(
            runbook.stages().first().map(|s| s.missing()),
            Some(Some(part))
        );
    }

    // Naming a part with nothing after it is the same gap: `- rollback:` is what a responder who
    // was about to think about it writes down.
    let empty = stage(1, "Read the queue").replace("- rollback: nothing was taken", "- rollback:");
    assert_eq!(
        Runbook::of(&fixture(&empty)).gaps(),
        vec![Gap::Missing {
            number: 1,
            part: "rollback",
        }],
        "`- rollback:` names the part and answers nothing"
    );

    // And the same stage with all four parts holds, so the cases above are about the missing part
    // and not about the fixture.
    let complete = Runbook::of(&fixture(&stage(1, "Read the queue")));
    assert!(complete.holds(), "{:?}", complete.gaps());
    assert_eq!(complete.stages().first().map(|s| s.missing()), Some(None));
}

/// The order is part of the procedure: a stage holds the place its number says, and both a repeated
/// number and a skipped one are gaps.
#[test]
fn a_stage_holds_the_place_its_number_says() {
    let in_order = Runbook::of(&fixture(&format!(
        "{}{}{}",
        stage(1, "Read the queue"),
        stage(2, "Find the rows"),
        stage(3, "Look at the dead letters")
    )));
    assert!(
        in_order.gaps().is_empty(),
        "1, 2, 3 is a procedure: {:?}",
        in_order.gaps()
    );

    let repeated = Runbook::of(&fixture(&format!(
        "{}{}",
        stage(1, "Read the queue"),
        stage(1, "Read the queue again")
    )));
    assert_eq!(
        repeated.gaps(),
        vec![Gap::OutOfOrder {
            number: 1,
            expected: 2,
        }],
        "the second stage claims a place the first already holds"
    );

    let skipped = Runbook::of(&fixture(&format!(
        "{}{}",
        stage(1, "Read the queue"),
        stage(3, "Look at the dead letters")
    )));
    assert_eq!(
        skipped.gaps(),
        vec![Gap::OutOfOrder {
            number: 3,
            expected: 2,
        }],
        "a stage is missing from the page, and the number after it says so"
    );

    let late = Runbook::of(&fixture(&stage(2, "Find the rows")));
    assert_eq!(
        late.gaps(),
        vec![Gap::OutOfOrder {
            number: 2,
            expected: 1,
        }],
        "a procedure that starts at 2 is missing what it says came first"
    );

    // Out of order *and* incomplete is two repairs, and the report names both.
    let both = Runbook::of(&fixture(&format!(
        "{}{}",
        stage(1, "Read the queue"),
        without(&stage(3, "Look at the dead letters"), "signal")
    )));
    assert_eq!(
        both.gaps(),
        vec![
            Gap::OutOfOrder {
                number: 3,
                expected: 2,
            },
            Gap::Missing {
                number: 3,
                part: "signal",
            },
        ],
        "{:?}",
        both.gaps()
    );
}

/// A page with no numbered stage is prose, and a `###` heading without a number is a heading where
/// a stage belongs — the acts it was meant to name are the ones the drill reports as unnamed.
#[test]
fn a_page_that_names_no_stage_is_prose() {
    let prose = Runbook::of("# Runbook\n\nWe look at the numbers and then we act.\n");
    assert_eq!(
        prose.gaps(),
        vec![Gap::Empty],
        "a page that names no stage is the prose a responder reads instead of following"
    );
    assert!(prose.stages().is_empty());

    let unnumbered = Runbook::of("### Look at the numbers\n- command: `jobqc usage`\n");
    assert!(
        unnumbered.stages().is_empty(),
        "a stage heading carries a number: a `###` heading without one is not a stage, and what it \
         should have named is what the drill reports as unnamed"
    );
    assert_eq!(unnumbered.gaps(), vec![Gap::Empty]);

    // A deeper heading is prose inside a stage rather than a stage of its own.
    let deeper = Runbook::of(&fixture(&format!(
        "{}#### what this means\n\nand why it is here\n\n",
        stage(1, "Read the queue")
    )));
    assert_eq!(
        deeper.stages().len(),
        1,
        "`####` is a heading inside a stage: {:?}",
        deeper.stages()
    );
    assert!(deeper.holds(), "{:?}", deeper.gaps());
}

/// A part is one line and everything wrapped under it: a page is written at a readable width, and a
/// reader that stopped at the newline would shorten every command it read.
#[test]
fn a_part_wrapped_onto_the_next_line_is_read_whole() {
    let wrapped = fixture(
        "### 1. Read the queue\n- command: `jobqc usage`\n- signal: the queue is stuck when\n  \
         `running` stays at the batch size\n- refusal: exit `4`\n- rollback: nothing was taken\n",
    );
    let runbook = Runbook::of(&wrapped);
    assert!(runbook.holds(), "{:?}", runbook.gaps());
    assert_eq!(
        runbook.stages().first().map(|s| s.signal()),
        Some("the queue is stuck when `running` stays at the batch size"),
        "a wrapped part is one part"
    );

    // A blank line ends the part, so a paragraph after a stage is not part of its last answer.
    let stopped = fixture(
        "### 1. Read the queue\n- command: `jobqc usage`\n- signal: the first line\n\n  a \
         paragraph nobody put in a part\n- refusal: exit `4`\n- rollback: none\n",
    );
    assert_eq!(
        Runbook::of(&stopped).stages().first().map(|s| s.signal()),
        Some("the first line"),
        "a blank line ends what a part says"
    );
}
