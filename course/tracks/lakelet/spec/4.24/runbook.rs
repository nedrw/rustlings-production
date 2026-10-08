//! Spec for `lakelet/4.24-runbook-drill`: the procedure, the number it protects, and the
//! instrument that runs the page.
//!
//! `kiln run` copies this file into the project as `tests/spec_runbook.rs`. Two subjects, and the
//! second one is what this step is about:
//!
//! * **the pages** — `docs/runbook.md` is the procedure and `docs/slo.md` the number it protects.
//!   Neither is judged as prose: the runbook is read as *stages* by the project's own instrument
//!   (`tests/runbook_harness.rs`, reached through `#[path]` below), and the SLO page is read as the
//!   table it is;
//! * **the instrument** — this file never runs the procedure itself. [`drill`] is the harness's,
//!   so a reader that waves a page through fails here even while the page passes: the gap cases
//!   below hand the reader pages *this* file writes, which is the only way to judge a reader.
//!
//! What this file does not judge: whether the acts themselves work. That is each step's own suite
//! (`lakelet/4.19-backup-restore`, `lakelet/4.21-cold-tiering`, `lakelet/4.23-secret-rotation`),
//! and a runbook is not the place a store is proved.

#[path = "runbook_harness.rs"]
mod harness;

use std::collections::BTreeSet;
use std::path::PathBuf;

use harness::{PARTS, PREFIX, PROGRAMS, Rollback, Runbook, drill};

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/runbook.md";

/// The page that carries the numbers the procedure protects.
const SLO: &str = "docs/slo.md";

/// The section of the SLO page that spends the budget.
const BUDGET: &str = "## The error budget";

/// The project this spec runs in.
fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// One file of the project.
fn read(name: &str) -> String {
    let path = project().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()))
}

/// The runbook, as the page has it.
fn page() -> String {
    read(PAGE)
}

/// What the instrument says a page owes.
fn gaps(text: &str) -> Vec<harness::Gap> {
    Runbook::of(text).gaps(&project())
}

/// A stage written the way a page writes them: a heading, one command, and the four answers.
///
/// The fixtures are built out of this rather than pasted, so a case about one part cannot go stale
/// the day another part is spelled differently.
fn stage_of(number: u32, title: &str, command: &str, state: &str) -> String {
    format!(
        "### {number}. {title}\n\n```sh\n{command}\n```\n\n\
         - signal: `tracked`, which the store's ledger counts ([the cardinality \
         page](cardinality.md))\n\
         - state: `{state}`\n\
         - rollback: nothing was taken\n\
         - audience: the on-call engineer\n\n"
    )
}

/// A stage of the reference shape: the store's own bill, read.
fn stage(number: u32, title: &str) -> String {
    stage_of(
        number,
        title,
        "lakeletq inspect series --store $LAKELET_STORE",
        "bill-adds-up",
    )
}

/// The same stage with one of its five parts taken out.
fn without(stage: &str, part: &str) -> String {
    let opening = format!("- {part}:");
    let mut lines: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut dropped_fence = false;
    for line in stage.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            if part == "command" && !dropped_fence {
                // The command is the fenced block: taking it out takes the block and its body.
                dropped_fence = in_fence;
                continue;
            }
            if part == "command" && dropped_fence && !in_fence {
                continue;
            }
        } else if part == "command" && dropped_fence {
            continue;
        }
        if line.starts_with(&opening) {
            continue;
        }
        lines.push(line.to_owned());
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The page with one stage's line rewritten: the stage is found by its number.
fn rewrite(text: &str, number: u32, prefix: &str, answer: &str) -> String {
    let heading = format!("### {number}. ");
    let mut inside = false;
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with("### ") {
            inside = line.starts_with(&heading);
        }
        if inside && line.starts_with(prefix) {
            lines.push(format!("{prefix}{answer}"));
            continue;
        }
        lines.push(line.to_owned());
    }
    let mut joined = lines.join("\n");
    joined.push('\n');
    joined
}

/// A page whose stages are what the case hands in, with the sections a page has around them.
fn fixture(steps: &str) -> String {
    format!("## Steps\n\n{steps}## Escalation\n\nnobody to escalate to\n")
}

/// The SLO page's table rows, split into cells, headings and alignment rows dropped.
fn table(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|line| line.trim_start().starts_with('|'))
        .map(|line| {
            line.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect::<Vec<String>>()
        })
        .filter(|cells| {
            // The table's heading row carries no backticked name and its alignment row is dashes:
            // a row of numbers is a row that names one.
            !cells.is_empty()
                && cells.iter().any(|cell| cell.contains('`'))
                && !cells
                    .iter()
                    .all(|cell| cell.chars().all(|byte| byte == '-' || byte == ':'))
        })
        .collect()
}

/// The names between backticks, in the order they are written.
fn backticked(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            break;
        };
        if !after[..close].is_empty() {
            names.push(after[..close].to_owned());
        }
        rest = &after[close + 1..];
    }
    names
}

/// One `## heading` section's body, without the heading.
fn section(text: &str, heading: &str) -> Option<String> {
    let start = text.find(heading)? + heading.len();
    let rest = &text[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

/// The relative targets a markdown page links to, with their anchors.
///
/// Fenced blocks are skipped, because a page quotes output as often as it links, and the block the
/// procedure's report lives in carries no targets at all.
fn links(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut rest = line;
        while let Some(open) = rest.find("](") {
            let after = &rest[open + 2..];
            let Some(close) = after.find(')') else {
                break;
            };
            found.push(after[..close].to_owned());
            rest = &after[close + 1..];
        }
    }
    found
}

/// The stage a piece of text says protects a number: `stage 3`, and nothing else.
fn mentioned(text: &str) -> Option<u32> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.windows(2).find_map(|pair| {
        if pair[0].trim_matches(|byte: char| !byte.is_ascii_alphanumeric()) == "stage" {
            pair[1]
                .trim_matches(|byte: char| !byte.is_ascii_alphanumeric())
                .parse()
                .ok()
        } else {
            None
        }
    })
}

/// Everything one stage of a page says: its heading, its command and its four answers.
fn stage_text(runbook: &Runbook, number: u32) -> Option<String> {
    let stage = runbook
        .stages()
        .iter()
        .find(|stage| stage.number() == number)?;
    let mut text = stage.title().to_owned();
    text.push(' ');
    text.push_str(stage.command().unwrap_or_default());
    for part in PARTS {
        text.push(' ');
        text.push_str(stage.answer(part).unwrap_or_default());
    }
    Some(text)
}

/// The block a page quotes, from the fence before the instrument's first line to the fence after
/// it.
fn quoted(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let anchor = lines
        .iter()
        .position(|line| line.starts_with(PREFIX))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} does not quote the instrument's report: a reader has to be able to see \
                 what running the procedure prints, and no line of the page is one of its lines"
            )
        });
    let opening = lines[..anchor]
        .iter()
        .rposition(|line| line.trim_start().starts_with("```"))
        .unwrap_or_else(|| {
            panic!("{PAGE} has the instrument's report at line {anchor} outside a fenced block")
        });
    let closing = lines[anchor..]
        .iter()
        .position(|line| line.trim_start().starts_with("```"))
        .map(|offset| anchor + offset)
        .unwrap_or_else(|| panic!("{PAGE}'s fenced block never closes after line {anchor}"));
    lines[opening + 1..closing]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

/// The page is a procedure: its stages are numbered, its opening's count is the count it writes,
/// and every stage answers all five of its parts.
#[test]
fn the_page_is_a_procedure_and_every_stage_names_its_parts() {
    let text = page();
    let lowered = text.to_lowercase();
    for placeholder in ["todo", "tbd", "fixme", "placeholder"] {
        assert!(
            !lowered.contains(placeholder),
            "`{PAGE}` still says `{placeholder}`: a page with a blank in it is a page nobody has \
             written down, and the blank is the one thing a drill cannot fill"
        );
    }
    let runbook = Runbook::of(&text);
    assert!(
        !runbook.stages().is_empty(),
        "`{PAGE}` names no stage at all: a procedure is a list of numbered sections, and a heading \
         without a number is a heading where a stage belongs"
    );
    assert_eq!(
        runbook.claim(),
        Some(runbook.stages().len() as u64),
        "`{PAGE}` opens by claiming {:?} stage(s) and writes {}: the count in the opening line is \
         the first thing a reader checks this document against, and a page that disagrees with \
         itself is two documents",
        runbook.claim(),
        runbook.stages().len()
    );
    for stage in runbook.stages() {
        for part in PARTS {
            assert!(
                stage
                    .answer(part)
                    .is_some_and(|answer| !answer.trim().is_empty()),
                "stage {} does not say what its `{part}` is: the five parts are the whole of what a \
                 stage owes, and the one nobody writes is the one somebody needs at three in the \
                 morning",
                stage.number()
            );
        }
    }
}

/// Every command the page names is one of this deployment's, down to its verbs and flags.
#[test]
fn every_command_the_page_names_is_one_of_this_deployments() {
    let owed = gaps(&page());
    let bad: Vec<String> = owed
        .iter()
        .filter(|gap| gap.part() == "command")
        .map(ToString::to_string)
        .collect();
    assert!(
        bad.is_empty(),
        "`{PAGE}` names a command this deployment does not have — its programs are {}:\n    {}",
        PROGRAMS.join(", "),
        bad.join("\n    ")
    );

    // The plausible-looking command is the failure this case is for: a verb the program does not
    // declare is not a command, and it is a finding rather than a process that answers with
    // something else.
    let invented = page().replace("lakeletq inspect series", "lakeletq tier sweep");
    assert!(
        invented != page(),
        "the page no longer runs `lakeletq inspect series`, so this case cannot mutate it"
    );
    let found = gaps(&invented);
    assert!(
        found
            .iter()
            .any(|gap| gap.part() == "command" && gap.said().contains("`tier`")),
        "`lakeletq tier sweep` is not this project's grammar and the reader waved it through: \
         {found:?}"
    );

    let foreign = page().replace("lakeletq inspect series", "kubectl get segments");
    assert!(
        gaps(&foreign)
            .iter()
            .any(|gap| gap.part() == "command" && gap.said().contains("kubectl")),
        "a program nobody has is a finding: {:?}",
        gaps(&foreign)
    );
}

/// Every signal the page names is a number this project reports somewhere.
#[test]
fn every_signal_the_page_names_is_one_this_project_reports() {
    let owed = gaps(&page());
    let bad: Vec<String> = owed
        .iter()
        .filter(|gap| gap.part() == "signal")
        .map(ToString::to_string)
        .collect();
    assert!(
        bad.is_empty(),
        "`{PAGE}` watches something this deployment does not publish:\n    {}",
        bad.join("\n    ")
    );

    let invented = page().replace(
        "`tracked`, `series_admitted`",
        "`series_reported_total`, `series_admitted`",
    );
    assert!(
        invented != page(),
        "the page's ledger section no longer names `tracked`, so this case cannot mutate it"
    );
    let found = gaps(&invented);
    assert!(
        found
            .iter()
            .any(|gap| gap.part() == "signal" && gap.said().contains("series_reported_total")),
        "a signal nobody reports is the failure a runbook must not have: {found:?}"
    );
}

/// Every rollback is a command, `none`, or the words for a stage that changes nothing.
#[test]
fn every_rollback_is_a_command_or_says_why_it_cannot_be() {
    let text = page();
    let runbook = Runbook::of(&text);
    let owed = gaps(&text);
    let bad: Vec<String> = owed
        .iter()
        .filter(|gap| gap.part() == "rollback")
        .map(ToString::to_string)
        .collect();
    assert!(
        bad.is_empty(),
        "`{PAGE}` has a rollback that is not one of the three things one can be:\n    {}",
        bad.join("\n    ")
    );

    // A stage whose command writes what the deployment serves has a way back, or says it has none.
    for stage in runbook.stages() {
        let Some(check) = stage.check() else {
            continue;
        };
        if !check.takes {
            continue;
        }
        assert!(
            matches!(stage.rollback(), Rollback::Command(_) | Rollback::Undone),
            "stage {} runs `{}`, which writes what the deployment serves, and its rollback says \
             `{}`: an act that takes something says how it goes back, or says in the word for it \
             that it cannot",
            stage.number(),
            stage.command().unwrap_or_default(),
            stage.answer("rollback").unwrap_or_default()
        );
    }

    let mutating = runbook
        .stages()
        .iter()
        .find(|stage| stage.check().is_some_and(|check| check.takes))
        .map(|stage| stage.number())
        .expect("the procedure takes something at some stage");
    let weakened = rewrite(&text, mutating, "- rollback: ", "nothing was taken");
    assert!(
        gaps(&weakened)
            .iter()
            .any(|gap| gap.part() == "rollback" && gap.number() == mutating),
        "stage {mutating} puts records on the store and its rollback now says nothing was taken, \
         and the reader waved it through"
    );
}

/// The instrument runs the page's procedure against a store of its own, and nothing is owed.
#[test]
fn the_instrument_runs_every_stage_of_the_page_and_nothing_is_owed() {
    let text = page();
    let report = drill(&text, &project());
    let owed: Vec<String> = report.findings().iter().map(ToString::to_string).collect();
    assert!(
        report.holds_everything(),
        "the instrument ran {} of the page's {} stage(s) and {} held, so the procedure owes:\n    {}",
        report.ran(),
        report.stages(),
        report.holds(),
        owed.join("\n    ")
    );
    assert_eq!(
        report.stages(),
        Runbook::of(&text).stages().len(),
        "the report counts {} stage(s) and the reader parses {}: the two are one list",
        report.stages(),
        Runbook::of(&text).stages().len()
    );
    assert_eq!(
        report.ran(),
        report.stages(),
        "every stage's command runs: a stage the instrument cannot take is a stage nobody has run"
    );
    assert_eq!(
        report.holds(),
        report.stages(),
        "every stage's state holds, which is what makes the page evidence rather than a wish"
    );
}

/// The reader itself, judged on pages this file writes: a stage that forgets one part is a finding.
#[test]
fn a_stage_that_does_not_name_one_of_its_parts_is_a_finding() {
    for part in PARTS {
        let mutated = fixture(&without(&stage(1, "Read the store's bill"), part));
        let found = gaps(&mutated);
        assert!(
            found
                .iter()
                .any(|gap| gap.number() == 1 && gap.part() == part),
            "a stage that says nothing about its `{part}` is the gap a responder meets as a blank \
             line, and the reader reported {found:?} instead:\n{mutated}"
        );
        assert!(
            !found.iter().any(|gap| gap.part() == "page"),
            "the fixture is a page apart from the part this case took out: {found:?}"
        );
    }

    // And the same stage with all five parts holds, so the cases above are about the missing part
    // and not about the fixture.
    let complete = gaps(&fixture(&stage(1, "Read the store's bill")));
    assert!(complete.is_empty(), "{complete:?}");
}

/// A command that exits non-zero is the stage that failed, with what it printed.
#[test]
fn a_command_that_exits_non_zero_names_the_stage_that_failed() {
    // Restoring *from a store* is the mistake this case feeds the instrument: the source holds no
    // copy manifest, so the command refuses before it writes anything — which is exactly the
    // failure a drill has to report rather than swallow.
    let broken = fixture(&stage_of(
        1,
        "Restore from a directory that is not a copy",
        "lakeletq restore --from $LAKELET_STORE --to $LAKELET_RESTORED",
        "restore-holds-the-records",
    ));
    let report = drill(&broken, &project());
    let gap = report
        .findings()
        .iter()
        .find(|gap| gap.part() == "command")
        .unwrap_or_else(|| {
            panic!(
                "the instrument ran a command that cannot work and reported no finding: {:?}",
                report.findings()
            )
        });
    assert_eq!(gap.number(), 1, "the finding names the stage: {gap}");
    assert!(
        gap.said().contains("exited"),
        "the finding says what the command did: {gap}"
    );
    assert!(
        gap.said().contains("could not be restored"),
        "and it carries what the command printed, so a reader acts on the refusal rather than on \
         the number: {gap}"
    );
    let line = report
        .lines()
        .iter()
        .find(|line| line.contains("stage=1"))
        .unwrap_or_else(|| panic!("the report has a line for stage 1: {:?}", report.lines()));
    assert!(
        line.contains("verdict=failed"),
        "the stage's own line says it failed: {line}"
    );
    assert!(
        !report.holds_everything(),
        "a page whose command cannot run does not hold: {line}"
    );
}

/// The page's block is the instrument's report, re-run and compared line for line.
#[test]
fn the_page_carries_the_report_the_instrument_prints() {
    let text = page();
    let block = quoted(&text);
    let report = drill(&text, &project());
    assert_eq!(
        block,
        report.lines(),
        "`{PAGE}`'s block is not what the instrument prints: the page's numbers are a measurement, \
         and the way a page full of measurements stays true is that the command which produced them \
         is re-run and compared. Edit the page from a run, never from an intention."
    );
    assert!(
        block.iter().any(|line| line.contains("verdict=holds")),
        "the block does not carry a stage's verdict: {block:?}"
    );
    assert!(
        block.len() > 1,
        "the block is one line, so it is not a report per stage"
    );
}

/// The SLO page names every number, and a file of this project each one comes from.
#[test]
fn the_slo_page_names_each_number_and_where_it_comes_from() {
    let text = read(SLO);
    let lowered = text.to_lowercase();
    assert!(
        !lowered.contains("todo") && !lowered.contains("tbd"),
        "`{SLO}` is still a stub: the numbers this deployment promises are what the procedure \
         protects, and a page of placeholders promises nothing"
    );
    let rows = table(&text);
    assert!(
        rows.len() >= 8,
        "`{SLO}` carries {} row(s): the procedure has a number behind each of its stages, and a \
         page that names fewer numbers than the runbook has stages protects less than it claims",
        rows.len()
    );
    for row in &rows {
        let names = backticked(&row[0]);
        assert!(!names.is_empty(), "a row names no number at all: {row:?}");
        let cells = row.join(" ");
        let sources: Vec<String> = backticked(&cells)
            .into_iter()
            .filter(|name| {
                name.starts_with("src/")
                    || name.starts_with("docs/")
                    || name.starts_with("tests/")
                    || name.starts_with("course/")
            })
            .collect();
        assert!(
            !sources.is_empty(),
            "the row for {names:?} names no file its number comes from: a number nobody can \
             re-measure is a number somebody made up — {cells}"
        );
        for source in sources {
            assert!(
                source.starts_with("course/") || project().join(&source).exists(),
                "the row for {names:?} says its number comes from `{source}`, which is not a file \
                 this project keeps"
            );
        }
    }
}

/// The two pages are one document read from both ends: every number names its stage, and every
/// stage is named by a number.
#[test]
fn the_two_pages_cross_reference_each_other() {
    let runbook = Runbook::of(&page());
    let rows = table(&read(SLO));
    let mut named: BTreeSet<u32> = BTreeSet::new();
    for row in &rows {
        let cells = row.join(" ");
        let Some(number) = mentioned(&cells) else {
            panic!(
                "the row for {:?} names no runbook stage: an SLO whose numbers nobody protects is \
                 a list of wishes",
                row[0]
            );
        };
        named.insert(number);
        let text = stage_text(&runbook, number).unwrap_or_else(|| {
            panic!("`{SLO}` says stage {number} protects a number, and `{PAGE}` writes no stage {number}")
        });
        for name in backticked(&row[0]) {
            assert!(
                text.contains(&name),
                "`{SLO}` says stage {number} protects `{name}`, and stage {number} of `{PAGE}` does \
                 not name it: the two pages are read against each other, and a stage that does not \
                 name the number it protects is a stage watching something else"
            );
        }
    }
    for stage in runbook.stages() {
        assert!(
            named.contains(&stage.number()),
            "no number on `{SLO}` names stage {} (`{}`): every stage protects something, and the \
             page that says what is the SLO page",
            stage.number(),
            stage.title()
        );
    }
}

/// The error budget is in the units this deployment's failures are paid in, and it is made of the
/// numbers the page already carries.
#[test]
fn the_error_budget_is_counted_in_the_units_this_course_uses() {
    let text = read(SLO);
    let budget = section(&text, BUDGET)
        .unwrap_or_else(|| panic!("`{SLO}` has no `{BUDGET}` section: a budget is what an SLO is"));
    let lowered = budget.to_lowercase();
    for unit in ["refusal", "lost", "minutes"] {
        assert!(
            lowered.contains(unit),
            "the budget says nothing about a `{unit}`: this deployment's failures are counted in \
             refusals, records lost and minutes to restore, and a budget in another unit is a \
             budget nobody spends"
        );
    }
    let carries: BTreeSet<String> = table(&text)
        .into_iter()
        .flat_map(|row| backticked(&row[0]))
        .collect();
    for name in backticked(&budget) {
        assert!(
            carries.iter().any(|known| name.contains(known.as_str())),
            "the budget mentions `{name}`, which the table above does not carry: a budget may only \
             be spent against a number this page names, or it is a new promise nobody agreed to — \
             the table carries {carries:?}"
        );
    }
    let spent = section(&text, "## What happens when a budget is spent")
        .unwrap_or_else(|| panic!("`{SLO}` does not say what happens when a budget is spent"));
    assert!(
        spent.trim().len() > 80,
        "and it says it in more than a sentence: {spent:?}"
    );
}

/// Both pages are pages of this project, they link to each other, and their targets resolve.
#[test]
fn both_pages_link_to_each_other_and_their_targets_exist() {
    let runbook = page();
    let slo = read(SLO);
    for (page, text, other) in [(PAGE, &runbook, "slo.md"), (SLO, &slo, "runbook.md")] {
        let targets = links(text);
        assert!(
            targets.iter().any(|target| target == other),
            "`{page}` does not link to `{other}`: the procedure and the number it protects are one \
             document read from both ends, and a reader who lands on either has to reach the other"
        );
        for target in targets {
            if target.starts_with("http://") || target.starts_with("https://") {
                continue;
            }
            let (path, anchor) = match target.split_once('#') {
                Some((path, anchor)) => (path, anchor),
                None => (target.as_str(), ""),
            };
            if path.is_empty() {
                continue;
            }
            let file = project().join("docs").join(path);
            assert!(
                file.exists(),
                "`{page}` links to `{target}`, which is not a file this project keeps: the `links` \
                 gate is offline because a runbook that points at something nobody can open is \
                 abandoned at the first incident"
            );
            if anchor.is_empty() {
                continue;
            }
            let headings: Vec<String> = std::fs::read_to_string(&file)
                .unwrap_or_default()
                .lines()
                .filter(|line| line.starts_with('#'))
                .map(anchor_of)
                .collect();
            assert!(
                headings.contains(&anchor.to_owned()),
                "`{page}` links to `{target}` and that page has no such heading: {headings:?}"
            );
        }
    }
}

/// What a heading's anchor is: lowercase, spaces to hyphens, punctuation dropped.
fn anchor_of(heading: &str) -> String {
    heading
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|byte| match byte {
            ' ' => Some('-'),
            'a'..='z' | '0'..='9' | '-' | '_' => Some(byte),
            _ => None,
        })
        .collect()
}

/// The instrument reads no wall clock into what it compares, and it runs the project's own
/// binaries rather than `cargo`.
#[test]
fn the_instrument_reads_no_wall_clock_and_runs_no_cargo() {
    let source = read("tests/runbook_harness.rs");
    for forbidden in ["SystemTime", "chrono", "Utc::now", "Local::now"] {
        assert!(
            !source.contains(forbidden),
            "the instrument names `{forbidden}`: a wall clock in the instrument is a report that \
             says something different on a loaded machine, and the page's block is compared line \
             for line"
        );
    }
    assert!(
        !source.contains("Command::new(\"cargo\")") && !source.contains("\"cargo\""),
        "the instrument spawns `cargo`: the suite that runs it holds the target directory's lock, \
         so a child cargo waits for its parent and the step hangs rather than fails. The instrument \
         runs the project's own binaries (`CARGO_BIN_EXE_…`) and scripts"
    );
    assert!(
        source.contains("CARGO_BIN_EXE"),
        "the instrument does not run the project's own binaries at all, so the commands a page \
         names are not what it judges"
    );
    for call in ["backup::copy", "backup::restore"] {
        assert!(
            !source.contains(call),
            "the instrument calls `{call}` instead of running the command that does it: the acts a \
             page names are judged as commands, and an instrument that calls the library beside \
             them measures something else"
        );
    }

    // And the same page drilled twice is the same bytes, which is what lets a page carry the block.
    let first = drill(&page(), &project());
    let second = drill(&page(), &project());
    assert_eq!(
        first.lines(),
        second.lines(),
        "two drills of one page printed different reports: a report that moves on its own cannot be \
         quoted by a page, and the wall clock is the usual reason"
    );
}

/// The report says how many stages ran and which ones roll back with a command.
#[test]
fn the_report_says_how_many_stages_ran_and_which_ones_roll_back() {
    let runbook = Runbook::of(&page());
    let report = drill(&page(), &project());
    let commands = runbook
        .stages()
        .iter()
        .filter(|stage| matches!(stage.rollback(), Rollback::Command(_)))
        .count();
    let undone = runbook
        .stages()
        .iter()
        .filter(|stage| matches!(stage.rollback(), Rollback::Undone))
        .count();
    assert_eq!(
        report.rolled_back(),
        commands,
        "the report says {} stage(s) roll back with a command and the page writes {commands}",
        report.rolled_back()
    );
    assert_eq!(
        report.undone(),
        undone,
        "the report says {undone} stage(s) cannot be undone"
    );
    assert_eq!(report.ran(), report.stages());
    let header = report
        .lines()
        .first()
        .unwrap_or_else(|| panic!("the report has no first line"));
    for (what, value) in [
        ("stages", report.stages()),
        ("ran", report.ran()),
        ("holds", report.holds()),
        ("rolled_back", commands),
        ("undone", undone),
        ("findings", 0),
    ] {
        assert!(
            header.contains(&format!("{what}={value}")),
            "the report's own summary line states `{what}={value}`: {header}"
        );
    }
    assert_eq!(
        report
            .lines()
            .iter()
            .filter(|line| line.contains("rollback=command"))
            .count(),
        commands,
        "every stage that rolls back with a command says so on its own line: {:?}",
        report.lines()
    );
}
