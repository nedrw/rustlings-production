//! probe = "ops/runbook-drill@operate"
//! title = "The page is the data, and a page that cannot be run has not been written"
//! hint = "Read the five parts out of every stage, report every obligation no stage states — sorted, so two runs print the same lines — and name the first stage that is not runnable."
//!
//! Step `lakelet/4.24-runbook-drill` is the deepest rung of this track, and it is about the one
//! document a responder reads rather than a developer: **the procedure**. Twenty-three steps
//! produced acts, and each one shipped the page that explains its own act; *not one of them is the
//! procedure*. The instrument is what makes a procedure a procedure: the page is read as data,
//! walked stage by stage against a deployment, and a stage that cannot be run is a finding with its
//! number, its part and one sentence rather than a crash or a shrug. Every rule below is a trap
//! somebody wrote a page into:
//!
//! * **a stage owes five parts, and the two a hurried page drops are `rollback` and `audience`.**
//!   The command is one command, the signal is a name the deployment really reports, the state is a
//!   check made *after* the command ran, the rollback is what puts the deployment back (or `none`,
//!   or `nothing was taken`), and the audience is who is told. A stage missing any of them is a
//!   stage nobody finished, and the finding says which part and what the part is for;
//! * **the page is data, so what is judged is what it states rather than how it is laid out.** A
//!   `### <number>. <title>` heading is a stage, a `name: answer` line is one of the five parts, and
//!   any other line inside a stage is a line the format does not know — a *finding* rather than
//!   prose a reader skips, because a reader that skips lines is a reader that waves a page through;
//! * **a page that cannot be run is a page that has not been written.** The verdict names the
//!   *first* stage that owes something: the first excuse a reader meets is the one they fix, and a
//!   gap nobody meets is a gap nobody fixes. A page with no stage at all is not runnable either,
//!   and its finding is about the page, because there is no stage to name;
//! * **the report is compared line for line, so the reading has to be reproducible.** Two runs over
//!   one page print the same lines: the findings come out sorted, which is the promise that the
//!   order a page happens to write its parts in is not part of what the page says. It also means
//!   the numbers have to hold their places — a page numbered 1, 2, 4 is two documents.
//!
//! ## What you write
//!
//! * [`stages`] — the page as data: one [`Stage`] per heading, the parts it states in the order it
//!   writes them, and the lines the format does not know;
//! * [`gaps`] — every obligation no stage states, in the order the report prints them;
//! * [`runnable`] — whether the page can be run, and when it cannot, the first stage that says so.
//!
//! [`PARTS`], [`Stage`], [`Gap`], [`Verdict`], [`NUMBER`], [`LINE`], [`PAGE`] and [`owes`] are
//! given: the five names are the project's own, the parts of a finding are what a report branches
//! on, and the sentence a missing part gets is the instrument's rather than the page's.
//!
//! ## What the tests ask
//!
//! The page is read into three stages with their numbers, titles and parts in the order they are
//! written, and the prose above the first stage and the section after the last are nobody's body. A
//! page that states all five obligations owes nothing and is runnable with the number of stages the
//! summary line carries. A stage with a command and no rollback is a finding, and so is one nobody
//! is told about — the two parts a page leaves out — each with the part's own sentence. A line the
//! format does not know is carried as data and reported as a finding that names it. A stage that
//! does not hold the place its number says is a finding about the number, and the reading of the
//! stage is unaffected. Two spellings of one page — the unknown line written at the top of a stage
//! and at the bottom of it — owe the same two things in the same order, which is what sorted
//! findings buy. And the verdict is the first stage that owes something, or the count of the stages
//! when none does.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store, no binaries and no report**. The real instrument builds a
//! store through the project's own write path, starts `lakeletd` on it, runs each stage's command as
//! the program it names, and makes that stage's check against the world *afterwards* — which is what
//! turns "the page looks right" into evidence, and it is the half a probe cannot have. What this
//! file is, and what it is the whole of, is **the reading of the page**: the format, the parts a
//! stage owes, the findings and the verdict — the half the real instrument's own gaps are computed
//! from, and the half a page nobody has walked yet already fails.
//!
//! Missing on purpose, and each one because it needs a project on disk rather than a page in a
//! string: no signal is held against a counter this tree really prints, no rollback word is judged
//! against what the stage's act takes (`none` where an act writes records is the trap), no command
//! is held to a program's own grammar, and the report block a page quotes is not re-run. This file's
//! `command` is a `name: answer` line as well, where the real page writes it as a fenced `sh` block,
//! because a miniature that needed a Markdown *lexer* would be re-checking the wrong habit.

use std::fmt;

/// The five parts a stage owes, in the order a page writes them.
///
/// The project's own list (`runbook_harness::PARTS`), word for word: a part this file did not know
/// would be a finding on every page the project writes.
pub const PARTS: [&str; 5] = ["command", "signal", "state", "rollback", "audience"];

/// The part a finding about a heading that does not hold its place carries.
pub const NUMBER: &str = "number";

/// The part a finding about a line the format does not know carries.
pub const LINE: &str = "line";

/// The part a finding about the page as a whole carries. Its stage number is `0`.
pub const PAGE: &str = "page";

/// One stage of a page, read as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// The number the heading carries.
    pub number: usize,
    /// The title after the number.
    pub title: String,
    /// Every part the stage states, in the order the page writes them, as `(name, answer)`.
    pub parts: Vec<(String, String)>,
    /// Every body line that states no part, in the order the page writes them.
    ///
    /// Carried rather than dropped: a line nothing reads is a line nobody wrote a rule for, and
    /// [`gaps`] is where it becomes a finding.
    pub unknown: Vec<String>,
}

impl Stage {
    /// What this stage answers for `part`, or `None` when it does not name it.
    ///
    /// The first line wins when a stage names a part twice: a page is read the way it is written,
    /// and the instrument's own reading is what a page is held to.
    #[must_use]
    pub fn answer(&self, part: &str) -> Option<&str> {
        self.parts
            .iter()
            .find(|(name, _)| name == part)
            .map(|(_, answer)| answer.as_str())
    }
}

/// One obligation a stage did not meet: which stage, which part, and one sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// The number of the stage it is about, or `0` for a finding about the page as a whole.
    pub number: usize,
    /// Which part it is about: one of [`PARTS`], or [`NUMBER`], [`LINE`] or [`PAGE`].
    pub part: &'static str,
    /// One sentence saying what the stage owed, in the words a reader acts on.
    pub said: String,
}

/// A finding prints as the stage and the part it is about, so a report line names both: a sentence
/// without its stage is a sentence a responder cannot act on.
impl fmt::Display for Gap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.number == 0 {
            write!(formatter, "{}: {}", self.part, self.said)
        } else {
            write!(
                formatter,
                "stage {} does not meet `{}`: {}",
                self.number, self.part, self.said
            )
        }
    }
}

/// What a page comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every stage states all five parts, and every stage holds the place its number says.
    Runnable {
        /// How many stages the page has, which is what the report's summary line states.
        stages: usize,
    },
    /// The first stage that is not runnable, and the obligation it did not meet.
    NotRunnable {
        /// The number of the first stage that owes something, or `0` for the page itself.
        number: usize,
        /// The part it owes, or [`PAGE`] when the page states no stage at all.
        part: &'static str,
    },
}

impl fmt::Display for Verdict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runnable { stages } => write!(formatter, "the page holds: {stages} stage(s)"),
            Self::NotRunnable { part, number: 0 } => {
                write!(formatter, "the page owes its `{part}`")
            }
            Self::NotRunnable { number, part } => {
                write!(formatter, "the page owes stage {number} its `{part}`")
            }
        }
    }
}

/// What a part is for, in the sentence a stage that forgot it is answered with.
///
/// Given: the sentences are the instrument's rather than the page's, and they are the words the
/// project's own reader uses (`runbook_harness::owes`), with the one difference this file's format
/// has — a command is a `name: answer` line here rather than a fenced `sh` block.
#[must_use]
pub fn owes(part: &str) -> &'static str {
    match part {
        "command" => {
            "the command is the one part of a stage that is not a sentence: a stage a program cannot \
             be started for is a stage nobody has taken"
        }
        "signal" => {
            "an operator watching this stage needs a number to watch, and a number nobody publishes \
             is a reader sent to a dashboard that does not exist"
        }
        "state" => {
            "the state is the check that is made after the command ran, and \"it looked fine\" is \
             not an assertion"
        }
        "rollback" => {
            "the column a responder reads while the incident is still happening: what puts the \
             deployment back, or that nothing does"
        }
        _ => "who is told, by role: a stage nobody is told about is an act taken in private",
    }
}

/// The page's stages, read as data.
///
/// The reading is of what the page states: a heading is a stage when it carries a number and a
/// title, a `name: answer` line whose name is one of [`PARTS`] is a part, and anything else inside a
/// stage's body is carried as a line this format does not know.
#[must_use]
pub fn stages(page: &str) -> Vec<Stage> {
    let mut stages: Vec<Stage> = Vec::new();
    let mut open = false;
    for line in page.lines() {
        let text = line.trim();
        let heading = text.strip_prefix("### ").and_then(|rest| {
            let (number, title) = rest.split_once('.')?;
            let title = title.trim();
            let number = number.trim().parse::<usize>().ok()?;
            (!title.is_empty()).then(|| (number, title.to_owned()))
        });
        if let Some((number, title)) = heading {
            stages.push(Stage {
                number,
                title,
                parts: Vec::new(),
                unknown: Vec::new(),
            });
            open = true;
            continue;
        }
        // Any other heading ends the last stage: what follows it is the page's own prose, and the
        // section a procedure ends with is not a stage's body.
        if text.starts_with('#') {
            open = false;
            continue;
        }
        if !open || text.is_empty() {
            continue;
        }
        let Some(stage) = stages.last_mut() else {
            continue;
        };
        match text.split_once(':') {
            Some((name, answer)) if PARTS.contains(&name.trim()) => {
                stage
                    .parts
                    .push((name.trim().to_owned(), answer.trim().to_owned()));
            }
            // A line the format does not know is data about the page rather than prose this reader
            // skips: a reader that skips lines is a reader that waves a page through.
            _ => stage.unknown.push(text.to_owned()),
        }
    }
    stages
}

/// Every obligation the page does not meet, in a deterministic order.
///
/// The findings come out sorted by the stage and then by the part's name, because the block a page
/// quotes is compared line for line: the order a page happens to write its parts in is not part of
/// what the page says.
#[must_use]
pub fn gaps(page: &str) -> Vec<Gap> {
    let read = stages(page);
    let mut findings: Vec<Gap> = Vec::new();
    if read.is_empty() {
        findings.push(Gap {
            number: 0,
            part: PAGE,
            said: "this page names no stage: a `### <number>. <title>` heading is one, and a page \
                   without one states no procedure to run"
                .to_owned(),
        });
    }
    for (index, stage) in read.iter().enumerate() {
        // The numbers are the places a reader takes the procedure in, so a page numbered 1, 2, 4 is
        // two documents: the stage the reader opens it for is not where it says it is.
        if stage.number != index + 1 {
            findings.push(Gap {
                number: stage.number,
                part: NUMBER,
                said: format!(
                    "this stage is the {}th of the page and is numbered {}: the numbers are the \
                     places an operator takes a procedure in",
                    index + 1,
                    stage.number
                ),
            });
        }
        for part in PARTS {
            match stage.answer(part) {
                None => findings.push(Gap {
                    number: stage.number,
                    part,
                    said: owes(part).to_owned(),
                }),
                Some(answer) if answer.trim().is_empty() => findings.push(Gap {
                    number: stage.number,
                    part,
                    said: format!(
                        "this stage names its `{part}` and answers nothing: \"it looked fine\" \
                         is not what a stage says"
                    ),
                }),
                Some(_) => {}
            }
        }
        for line in &stage.unknown {
            findings.push(Gap {
                number: stage.number,
                part: LINE,
                said: format!(
                    "`{line}` is not a part of a stage: the parts are {}, and a line this reading \
                     does not know is a finding rather than prose it skips",
                    PARTS.map(|part| format!("`{part}`")).join(", ")
                ),
            });
        }
    }
    findings.sort_by(|one, other| (one.number, one.part).cmp(&(other.number, other.part)));
    findings
}

/// Whether the page can be run, and when it cannot, the first stage that says so.
#[must_use]
pub fn runnable(page: &str) -> Verdict {
    match gaps(page).first() {
        // Nothing owed: the page holds, and the count is the stages the summary line states.
        None => Verdict::Runnable {
            stages: stages(page).len(),
        },
        // The first excuse a reader meets is the one they fix, and sorted findings are what make
        // "first" mean the same stage on every run.
        Some(first) => Verdict::NotRunnable {
            number: first.number,
            part: first.part,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page the drill was written for: three stages, five parts each, and prose above and below
    /// that is nobody's stage.
    const RUNBOOK: &str = "\
# A procedure for a store that is filling up

Something a responder reads before the first act, while something is wrong.

### 1. Read what this deployment is
command: lakeletd --check-config --store $LAKELET_STORE
signal: `settings` on the daemon's start line
state: settings-print
rollback: nothing was taken
audience: nobody

### 2. Take a copy
command: lakeletq backup --store $LAKELET_STORE --to $LAKELET_COPY
signal: `copy-holds-the-store` in the instrument's own report
state: copy-holds-the-store
rollback: lakeletq restore --store $LAKELET_STORE --from $LAKELET_COPY
audience: the on-call engineer

### 3. Retire a key
command: lakeletq rotate --store $LAKELET_STORE --retire sunset
signal: `window-respected` in the instrument's own report
state: window-respected
rollback: none
audience: the security owner

## What this page does not do

Nothing here sweeps or hydrates: that is the engine's, and the page this one is shortened from \
says so in its seventh stage.
";

    /// The `(stage, part)` of every finding, which is what a report line names.
    fn owed(page: &str) -> Vec<(usize, &'static str)> {
        gaps(page)
            .iter()
            .map(|gap| (gap.number, gap.part))
            .collect()
    }

    /// A heading is a stage, a part is a line, and the page's own prose is neither.
    #[test]
    fn a_page_is_read_as_stages_with_their_numbers_titles_and_parts() {
        let read = stages(RUNBOOK);
        assert_eq!(
            read.iter().map(|stage| stage.number).collect::<Vec<_>>(),
            [1, 2, 3],
            "a `### <number>. <title>` heading is a stage, and the prose above the first one and \
             the section after the last one are the page's rather than a stage's body"
        );
        assert_eq!(
            read[1].title, "Take a copy",
            "the title is what follows the number"
        );
        assert_eq!(
            read[1].answer("command"),
            Some("lakeletq backup --store $LAKELET_STORE --to $LAKELET_COPY"),
            "the command is one of the five parts, and it arrives as data"
        );
        assert_eq!(
            read[0]
                .parts
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            PARTS,
            "the parts are kept in the order the page writes them, which is the order the columns \
             of a stage are read in"
        );
        assert!(
            read.iter().all(|stage| stage.unknown.is_empty()),
            "and nothing in these bodies is a line the format does not know: {:?}",
            read.iter().map(|stage| &stage.unknown).collect::<Vec<_>>()
        );
        assert_eq!(
            read[2].answer("rollback"),
            Some("none"),
            "the one act on this page that cannot be undone answers `none`, which is an obligation \
             met rather than a gap"
        );
    }

    /// Five obligations, and a page that states all of them owes nothing.
    #[test]
    fn a_page_that_states_all_five_obligations_owes_nothing_and_is_runnable() {
        assert_eq!(
            PARTS.len(),
            5,
            "five parts is the number this reading is measured against"
        );
        let findings = gaps(RUNBOOK);
        assert!(
            findings.is_empty(),
            "every stage of this page names all five parts, so it owes nothing: {findings:?}"
        );
        assert_eq!(
            runnable(RUNBOOK),
            Verdict::Runnable { stages: 3 },
            "and a page that owes nothing is runnable, with the number of stages the report's own \
             summary line states"
        );
    }

    /// The two parts a hurried page leaves out.
    #[test]
    fn a_stage_with_a_command_but_no_rollback_is_a_gap_and_so_is_one_with_no_audience() {
        let no_rollback = RUNBOOK.replace("rollback: nothing was taken\n", "");
        let findings = gaps(&no_rollback);
        assert_eq!(
            owed(&no_rollback),
            [(1, "rollback")],
            "a stage with a command and no rollback is an act with no way back, and it is the part \
             a page leaves out first"
        );
        assert_eq!(
            findings[0].said,
            owes("rollback"),
            "and the sentence is the instrument's own: what the part is for"
        );
        assert_eq!(
            runnable(&no_rollback),
            Verdict::NotRunnable {
                number: 1,
                part: "rollback"
            },
            "a page with an act that has no way back is not one somebody has written"
        );

        let no_audience = RUNBOOK.replace("audience: the security owner\n", "");
        assert_eq!(
            owed(&no_audience),
            [(3, "audience")],
            "and a stage nobody is told about is a finding too: it is the other part a page leaves \
             out, because the act still looks finished without it"
        );
        assert_eq!(
            gaps(&no_audience)[0].said,
            owes("audience"),
            "with the sentence that says who a stage is for"
        );
    }

    /// A line the format does not know is a finding, and it names the line.
    #[test]
    fn a_line_the_format_does_not_know_is_a_finding_and_not_a_silent_skip() {
        let page = RUNBOOK.replace(
            "state: window-respected\n",
            "state: window-respected\nverdict: holds\n",
        );
        assert_eq!(
            stages(&page)[2].unknown,
            ["verdict: holds"],
            "a line nothing in this format reads is carried as data rather than dropped"
        );
        assert_eq!(
            owed(&page),
            [(3, LINE)],
            "one line, one finding: a line this reading skips is a page this reading waves through"
        );
        assert!(
            gaps(&page)[0].said.contains("verdict"),
            "and the finding names the key nobody wrote a rule for: {}",
            gaps(&page)[0].said
        );
    }

    /// The numbers are places, and the reading is not the judgement.
    #[test]
    fn a_stage_that_does_not_hold_the_place_its_number_says_is_a_finding() {
        let page = RUNBOOK.replace("### 3. Retire a key", "### 4. Retire a key");
        assert_eq!(
            stages(&page)[2].number,
            4,
            "the stage is still read as data: what a page states and what it owes are two readings"
        );
        assert_eq!(
            owed(&page),
            [(4, NUMBER)],
            "an operator reads the numbers as places, and a gap in them is a stage somebody forgot"
        );
        assert_eq!(
            runnable(&page),
            Verdict::NotRunnable {
                number: 4,
                part: NUMBER
            },
            "and the verdict names the stage whose number moved"
        );
    }

    /// Two runs of one page print the same lines, whatever order the page writes them in.
    #[test]
    fn two_spellings_of_one_page_owe_the_same_things_in_the_same_order() {
        // One page, one unknown line and one missing `audience`, with the unknown line at the top
        // of the stage's body in one spelling and at the bottom of it in the other.
        let base = RUNBOOK.replace("audience: the on-call engineer\n", "");
        let early = base.replace(
            "command: lakeletq backup --store $LAKELET_STORE --to $LAKELET_COPY\n",
            "verdict: holds\ncommand: lakeletq backup --store $LAKELET_STORE --to $LAKELET_COPY\n",
        );
        let late = base.replace(
            "rollback: lakeletq restore --store $LAKELET_STORE --from $LAKELET_COPY\n",
            "rollback: lakeletq restore --store $LAKELET_STORE --from $LAKELET_COPY\nverdict: holds\n",
        );
        for page in [&early, &late] {
            assert_eq!(
                owed(page),
                [(2, "audience"), (2, LINE)],
                "one stage owes its `audience` and carries one line the format does not know, and \
                 where a page writes that line is not something a reader was promised: the \
                 findings are sorted, so two runs over one page print the same lines"
            );
        }
    }

    /// The verdict is the first stage that owes something, in both directions.
    #[test]
    fn the_verdict_names_the_first_stage_that_is_not_runnable() {
        let two_broken = RUNBOOK
            .replace("rollback: nothing was taken\n", "")
            .replace("audience: the security owner\n", "");
        assert_eq!(
            owed(&two_broken),
            [(1, "rollback"), (3, "audience")],
            "both obligations are owed, and both are reported: a reader who fixes one and finds \
             another red line stops reading the report"
        );
        assert_eq!(
            runnable(&two_broken),
            Verdict::NotRunnable {
                number: 1,
                part: "rollback"
            },
            "and the verdict is the first of them rather than the count: the first excuse a reader \
             meets is the one they fix"
        );

        let prose = "A page somebody wrote before they got to the stages.\n";
        assert_eq!(
            owed(prose),
            [(0, PAGE)],
            "a page with no stage in it states no procedure at all, and the finding is about the \
             page, because there is no stage to name"
        );
        assert_eq!(
            runnable(prose),
            Verdict::NotRunnable {
                number: 0,
                part: PAGE
            },
            "so it is not answered as a page whose stages all ran"
        );
        assert_eq!(
            runnable(RUNBOOK),
            Verdict::Runnable { stages: 3 },
            "and the other direction holds: a page that owes nothing is runnable"
        );
    }

    /// A part a stage names and answers nothing is still an obligation it did not meet.
    #[test]
    fn a_part_a_stage_names_and_answers_nothing_is_a_gap() {
        let page = RUNBOOK.replace("state: settings-print\n", "state:\n");
        assert_eq!(
            stages(&page)[0].answer("state"),
            Some(""),
            "the stage names the part, so it is read as a part"
        );
        assert_eq!(
            owed(&page),
            [(1, "state")],
            "and it owes one: a line that names a part and answers nothing is the check nobody can \
             make"
        );
        assert!(
            gaps(&page)[0].said.contains("state"),
            "and the sentence it is answered with says which part answered nothing: {}",
            gaps(&page)[0].said
        );
    }
}
