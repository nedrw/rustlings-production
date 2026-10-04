//! `kiln lint-caps` — the rules that keep "no repeated teaching" honest (plan §3 D4).
//!
//! The rule set is the one catalogued in curriculum.md §2.3, plus R0 from the document-level
//! prototype (`tools/lint-caps-md.py`), which cross-checked the ledger against the step tables.
//! Here the step tables have been replaced by `track.toml`, so R0 is what keeps the two sources
//! of truth in agreement:
//!
//! | rule | what it stops |
//! | --- | --- |
//! | R0 | ledger and step metadata disagreeing about where a depth is established |
//! | R1 | teaching the same capability at the same depth twice inside a track |
//! | R2 | depth going backwards along the path (or sideways without `reteach`) |
//! | R3 | requiring a capability that is taught later, or never |
//! | R4 | "teaching" something with no gate and no spec to bind it to |
//! | R5 | prose that re-explains what a deepen/transfer step must not re-explain |
//! | R6 | a track whose declared exit capabilities it does not establish |
//!
//! R1-R4 and R6 are pure metadata checks and always run. R5 reads step bodies and reports itself
//! as skipped while they do not exist — the honest answer during M0, not a silent pass.
//!
//! One rule is shared by R0, R1 and R2: a step may re-establish a depth that the ledger already
//! pins elsewhere, but only by saying so (`reteach = "why"`), and every such declaration is
//! collected in the report so it can be reviewed instead of becoming the silent default.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::caps::{CapRequirement, CapabilityId, Level};
use crate::error::Result;
use crate::i18n::{Locale, read_step_body};
use crate::info::{Course, Origin, Step};

/// Rule a problem is attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// R0 — ledger and step metadata must agree slot by slot.
    LedgerConformance,
    /// R1 — no (capability, depth) taught twice inside one track.
    RepeatTeaching,
    /// R2 — depth strictly increases along the path unless `reteach` says otherwise.
    DepthOrder,
    /// R3 — everything a step requires is taught earlier.
    UnreachableRequires,
    /// R4 — every `teaches` entry is bound to a must gate or a spec file.
    UnboundTeaching,
    /// R5 — the prose template matches `origin`.
    ProseStructure,
    /// R6 — exit capabilities are established by the track that declares them.
    UncoveredExit,
}

impl Rule {
    /// Short id used in reports (`R3`).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Rule::LedgerConformance => "R0",
            Rule::RepeatTeaching => "R1",
            Rule::DepthOrder => "R2",
            Rule::UnreachableRequires => "R3",
            Rule::UnboundTeaching => "R4",
            Rule::ProseStructure => "R5",
            Rule::UncoveredExit => "R6",
        }
    }

    /// What the rule protects, for the human reading the report.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Rule::LedgerConformance => "ledger conformance",
            Rule::RepeatTeaching => "repeated teaching",
            Rule::DepthOrder => "depth order",
            Rule::UnreachableRequires => "unreachable requirement",
            Rule::UnboundTeaching => "unbound teaching",
            Rule::ProseStructure => "prose structure",
            Rule::UncoveredExit => "uncovered exit capability",
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id())
    }
}

/// One problem found by the linter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Issue {
    /// Rule that was broken.
    pub rule: Rule,
    /// Step the problem belongs to, when it belongs to one.
    ///
    /// Metadata rules that judge the ledger or a whole track rather than a single step leave it
    /// empty; the `caps` gate uses it to report only what the step being judged is responsible for.
    pub step: Option<String>,
    /// What is wrong, named so it can be fixed without reading the linter.
    pub message: String,
}

impl Issue {
    /// A problem one step is responsible for.
    fn about(rule: Rule, step: &Step, message: String) -> Self {
        Self {
            rule,
            step: Some(step.qualified_id()),
            message,
        }
    }

    /// A problem with the ledger or a whole track rather than a single step.
    fn course(rule: Rule, message: String) -> Self {
        Self {
            rule,
            step: None,
            message,
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.rule, self.message)
    }
}

/// What the linter saw, including the parts it could not check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Registered capabilities.
    pub capabilities: usize,
    /// (Capability, depth) slots the ledger declares.
    pub slots: usize,
    /// Step positions the ledger pins, authored or not.
    pub ledger_positions: usize,
    /// Steps that exist as metadata.
    pub authored_steps: usize,
    /// Step bodies found in the source locale.
    pub bodies: usize,
    /// Every problem, sorted by rule then message.
    pub issues: Vec<Issue>,
    /// Rules that could not run, with the reason.
    pub skipped: Vec<String>,
    /// Deliberate re-teaches, as `(step, reason)` — plan §12 wants these surfaced, not buried.
    pub reteaches: Vec<(String, String)>,
}

impl Report {
    /// Problems of one rule.
    pub fn issues_for(&self, rule: Rule) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |issue| issue.rule == rule)
    }

    /// Problems one step is responsible for, in report order.
    #[must_use]
    pub fn issues_for_step(&self, step: &Step) -> Vec<&Issue> {
        let id = step.qualified_id();
        self.issues
            .iter()
            .filter(|issue| issue.step.as_deref() == Some(id.as_str()))
            .collect()
    }

    /// `true` when nothing is wrong (skipped rules are not problems).
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Run every rule over a course.
pub fn lint(course: &Course) -> Result<Report> {
    let mut report = Report {
        capabilities: course.ledger().capability_count(),
        slots: course.ledger().slot_count(),
        ledger_positions: course.ledger().positions().len(),
        authored_steps: course.steps().count(),
        ..Report::default()
    };

    let authored_tracks: BTreeSet<&str> = course
        .tracks()
        .iter()
        .map(|track| track.id.as_str())
        .collect();

    check_ledger_conformance(course, &authored_tracks, &mut report);
    check_tracks(course, &mut report);
    check_path_order(course, &mut report);
    check_requires(course, &mut report);
    check_evidence(course, &mut report);
    check_exits(course, &mut report);
    check_prose(course, &mut report)?;

    report.issues.sort();
    report.reteaches.sort();
    report.reteaches.dedup();
    Ok(report)
}

/// R0: every `teaches` has its ledger slot, and every authored track's slots are declared.
fn check_ledger_conformance(
    course: &Course,
    authored_tracks: &BTreeSet<&str>,
    report: &mut Report,
) {
    for step in course.steps() {
        for requirement in &step.teaches {
            match course.ledger().slot(&requirement.id, requirement.level) {
                None => report.issues.push(Issue::about(
                    Rule::LedgerConformance,
                    step,
                    format!(
                        "{} teaches {}@{} but the ledger has no slot there",
                        step.qualified_id(),
                        requirement.id,
                        requirement.level
                    ),
                )),
                Some(position)
                    if (position.track != step.track || position.number != step.number())
                        && step.reteach.is_none() =>
                {
                    report.issues.push(Issue::about(
                        Rule::LedgerConformance,
                        step,
                        format!(
                            "{} teaches {}@{} but the ledger puts it at {}/{}",
                            step.qualified_id(),
                            requirement.id,
                            requirement.level,
                            position.track,
                            position.number
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
    }

    for (capability, level, position) in course.ledger().slots() {
        if !authored_tracks.contains(position.track.as_str()) {
            continue;
        }
        let declares_it = match course.track(&position.track) {
            Ok(track) => track.step_by_number(&position.number).is_ok_and(|step| {
                step.teaches
                    .iter()
                    .any(|declared| &declared.id == capability && declared.level == level)
            }),
            Err(_) => false,
        };
        if !declares_it {
            report.issues.push(Issue::course(
                Rule::LedgerConformance,
                format!(
                    "the ledger puts {capability}@{level} at {}/{}, but no step there declares it",
                    position.track, position.number
                ),
            ));
        }
    }
}

/// R1 inside each track: the same (capability, depth) twice is a re-teach, and a deliberate one
/// has to say so.
fn check_tracks(course: &Course, report: &mut Report) {
    for track in course.tracks() {
        let mut positions: BTreeMap<&CapabilityId, &Step> = BTreeMap::new();
        for step in &track.step {
            for requirement in &step.teaches {
                if let Some(previous) = positions.insert(&requirement.id, step) {
                    match &step.reteach {
                        Some(reason) => {
                            report.reteaches.push((step.qualified_id(), reason.clone()))
                        }
                        None => report.issues.push(Issue::about(
                            Rule::RepeatTeaching,
                            step,
                            format!(
                                "{} and {} both teach {}@{}",
                                previous.qualified_id(),
                                step.qualified_id(),
                                requirement.id,
                                requirement.level
                            ),
                        )),
                    }
                }
            }
        }
    }
}

/// R2: walk the path and check that each capability only ever gets deeper (or is re-taught on
/// purpose, with a reason).
fn check_path_order(course: &Course, report: &mut Report) {
    let mut seen: BTreeMap<&CapabilityId, (Level, &Step)> = BTreeMap::new();
    for step in course.steps() {
        for requirement in &step.teaches {
            if let Some((previous_level, previous_step)) = seen.get(&requirement.id)
                && requirement.level.rank() <= previous_level.rank()
            {
                if let Some(reason) = &step.reteach {
                    report.reteaches.push((step.qualified_id(), reason.clone()));
                } else {
                    report.issues.push(Issue::about(
                        Rule::DepthOrder,
                        step,
                        format!(
                            "{} teaches {}@{} after {} taught it at {} — a deeper depth or \
                             `reteach = \"why\"` is required",
                            step.qualified_id(),
                            requirement.id,
                            requirement.level,
                            previous_step.qualified_id(),
                            previous_level
                        ),
                    ));
                }
            }
            seen.insert(&requirement.id, (requirement.level, step));
        }
    }
}

/// R3: a step may only require what the path already established, at that depth or deeper.
fn check_requires(course: &Course, report: &mut Report) {
    let mut covered: BTreeMap<&CapabilityId, (Level, &Step)> = BTreeMap::new();
    for step in course.steps() {
        for requirement in &step.requires {
            let satisfied = covered
                .get(&requirement.id)
                .is_some_and(|(level, _)| level.rank() >= requirement.level.rank());
            if !satisfied {
                let where_taught = covered.get(&requirement.id).map_or_else(
                    || "never taught before this step".to_owned(),
                    |(level, earlier)| {
                        format!("only taught at {level} by {}", earlier.qualified_id())
                    },
                );
                report.issues.push(Issue::about(
                    Rule::UnreachableRequires,
                    step,
                    format!(
                        "{} requires {}@{}: {where_taught}",
                        step.qualified_id(),
                        requirement.id,
                        requirement.level
                    ),
                ));
            }
        }
        for requirement in &step.teaches {
            covered.insert(&requirement.id, (requirement.level, step));
        }
    }
}

/// R4: teaching without a gate or a spec is a promise, not a step.
fn check_evidence(course: &Course, report: &mut Report) {
    for step in course.steps() {
        let has_evidence = step.must_gates().next().is_some() || !step.spec.is_empty();
        if has_evidence {
            continue;
        }
        report.issues.push(Issue::about(
            Rule::UnboundTeaching,
            step,
            format!(
                "{} teaches {} but declares no must gate and no spec file",
                step.qualified_id(),
                join(&step.teaches)
            ),
        ));
    }
}

/// R6: a track must establish the exit capabilities it declares.
fn check_exits(course: &Course, report: &mut Report) {
    for track in course.tracks() {
        for exit in &track.exit {
            let established = track.step.iter().any(|step| {
                step.teaches.iter().any(|teaches| {
                    teaches.id == exit.id && teaches.level.rank() >= exit.level.rank()
                })
            });
            if !established {
                report.issues.push(Issue::course(
                    Rule::UncoveredExit,
                    format!(
                        "{} declares {exit} as an exit capability, but no step establishes it",
                        track.id
                    ),
                ));
            }
        }
    }
}

/// R5: prose template versus `origin`, checked only where a step body exists.
///
/// Two halves. The first is structural — the template's sections, and no `WHY` on a step that must
/// not re-explain. The second checks that a `deepen`/`transfer` step's `RECALL` is *true* rather
/// than merely present: it has to name an earlier step of this course, name a depth, and point at a
/// step that actually established what this one builds on (authoring.md §4.3).
fn check_prose(course: &Course, report: &mut Report) -> Result<()> {
    let source = Locale::parse(&course.manifest().default_locale)?;
    let mut checked = 0usize;
    let mut missing = 0usize;

    // The path order `RECALL` is checked against: a step may only look backwards.
    let path: Vec<String> = course.steps().map(Step::qualified_id).collect();

    for step in course.steps() {
        let path_of = step.source_path(&course.dir());
        if !path_of.is_file() {
            missing += 1;
            continue;
        }
        checked += 1;
        report.bodies += 1;
        let body = match read_step_body(&path_of) {
            Ok(body) => body,
            Err(error) => {
                report
                    .issues
                    .push(Issue::about(Rule::ProseStructure, step, format!("{error}")));
                continue;
            }
        };
        let sections: BTreeSet<String> = body
            .headings
            .iter()
            .map(|heading| heading.trim_start_matches('#').trim().to_ascii_uppercase())
            .collect();

        // One definition of the templates, shared with the `i18n` gate (authoring.md §4).
        let wanted: &[&str] = crate::i18n::required_sections(step.origin);
        for section in wanted {
            if !sections.contains(*section) {
                report.issues.push(Issue::about(
                    Rule::ProseStructure,
                    step,
                    format!(
                        "{} (origin = {}) has no `## {section}` section",
                        step.qualified_id(),
                        step.origin
                    ),
                ));
            }
        }
        if step.origin != Origin::Teach && sections.contains("WHY") {
            report.issues.push(Issue::about(
                Rule::ProseStructure,
                step,
                format!(
                    "{} (origin = {}) must not re-explain with `## WHY` — use RECALL/DELTA",
                    step.qualified_id(),
                    step.origin
                ),
            ));
        }
        if step.origin != Origin::Teach {
            check_recall(course, step, &body, &path, report);
        }
    }

    if missing > 0 {
        report.skipped.push(format!(
            "R5 prose structure: checked {checked} of {} step bodies, {missing} not authored yet \
             (`{source}` is the source locale)",
            checked + missing
        ));
    }
    Ok(())
}

/// The `RECALL` half of R5: it has to name the step and the depth this one builds on.
fn check_recall(
    course: &Course,
    step: &Step,
    body: &crate::i18n::StepBody,
    path: &[String],
    report: &mut Report,
) {
    let recall = crate::i18n::section_text(&body.text, "RECALL");
    let issue = |report: &mut Report, message: String| {
        report
            .issues
            .push(Issue::about(Rule::ProseStructure, step, message));
    };
    if recall.trim().is_empty() {
        return; // The missing-section issue above already says it.
    }

    let here = path
        .iter()
        .position(|id| id == &step.qualified_id())
        .unwrap_or(0);
    let mut referenced: Vec<&Step> = Vec::new();
    for token in recall.split(|character: char| {
        character.is_whitespace()
            || matches!(character, '`' | '(' | ')' | ',' | ';' | '*' | '"' | '\'')
    }) {
        let token = token.trim_end_matches(['.', ':']);
        let Some((track, local)) = token.split_once('/') else {
            continue;
        };
        if !track
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || local.is_empty()
            || !local.starts_with(|c: char| c.is_ascii_digit())
        {
            continue;
        }
        match course.step(token) {
            Ok(earlier) => {
                let at = path
                    .iter()
                    .position(|id| id == &earlier.qualified_id())
                    .unwrap_or(usize::MAX);
                if at >= here {
                    issue(
                        report,
                        format!(
                            "{}: RECALL names `{token}`, which comes at or after this step — RECALL \
                             is for what you already learned",
                            step.qualified_id()
                        ),
                    );
                } else {
                    referenced.push(earlier);
                }
            }
            Err(_) => issue(
                report,
                format!(
                    "{}: RECALL names `{token}`, which is not a step of this course",
                    step.qualified_id()
                ),
            ),
        }
    }

    if referenced.is_empty() {
        issue(
            report,
            format!(
                "{} (origin = {}) has a RECALL that names no earlier step — write it as \
                 `<capability>@<depth> — <track>/<phase>.<index>: <what it established>`",
                step.qualified_id(),
                step.origin
            ),
        );
    }
    let depth_named = ["@aware", "@apply", "@design", "@operate"]
        .iter()
        .any(|depth| recall.contains(depth));
    if !depth_named {
        issue(
            report,
            format!(
                "{}: RECALL names no depth (`@apply`, `@design`, …) — the learner needs to know \
                 how deep the previous step went",
                step.qualified_id()
            ),
        );
    }

    // The reference has to be the right kind of step: it must have established something this step
    // builds on — one of its `requires` (what a transfer redoes) or a shallower depth of what a
    // `deepen` step takes further.
    let meaningful = referenced.iter().any(|earlier| {
        earlier.teaches.iter().any(|taught| {
            step.requires.iter().any(|required| {
                required.id == taught.id && taught.level.rank() >= required.level.rank()
            }) || step.teaches.iter().any(|deepening| {
                taught.id == deepening.id && taught.level.rank() < deepening.level.rank()
            })
        })
    });
    if !meaningful {
        issue(
            report,
            format!(
                "{} (origin = {}) builds on {} but its RECALL points at steps that never established \
                 them — name the step that did",
                step.qualified_id(),
                step.origin,
                join(&step.requires)
            ),
        );
    }
}

fn join(requirements: &[CapRequirement]) -> String {
    requirements
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Convenience for `xtask ci`, which lints the repository's own course.
pub fn lint_root(root: &std::path::Path) -> Result<Report> {
    lint(&Course::load(root)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        Step, TempDir, ledger_toml, load_course, one_track, step_toml, track_toml,
    };

    fn one_step_course(root: &std::path::Path) -> Course {
        load_course(
            root,
            &one_track(
                &[step_toml(
                    "mailer",
                    "1.1-workspace",
                    &["repo/workspace-layout@apply"],
                )],
                &["repo/workspace-layout@apply"],
            ),
            &ledger_toml(&[("repo/workspace-layout", "apply", "mailer/1.1")]),
        )
    }

    #[test]
    fn a_clean_course_reports_no_issues_and_full_counts() {
        let temp = TempDir::new("lint-clean");
        let course = one_step_course(temp.path());
        let report = lint(&course).unwrap();
        assert!(report.is_clean(), "{:?}", report.issues);
        assert_eq!(report.capabilities, 1);
        assert_eq!(report.slots, 1);
        assert_eq!(report.ledger_positions, 1);
        assert_eq!(report.authored_steps, 1);
        assert_eq!(report.bodies, 0);
        assert_eq!(report.skipped.len(), 1, "R5 says it could not run");
        assert!(report.skipped[0].contains("R5 prose structure"));
    }

    #[test]
    fn r0_catches_a_teaching_without_a_ledger_slot() {
        let temp = TempDir::new("lint-r0-missing-slot");
        let course = load_course(
            temp.path(),
            &one_track(
                &[step_toml(
                    "mailer",
                    "1.1-workspace",
                    &["repo/workspace-layout@apply"],
                )],
                &["repo/workspace-layout@apply"],
            ),
            // The ledger knows the capability but at another depth, i.e. no slot here.
            &ledger_toml(&[("repo/workspace-layout", "design", "mailer/1.1")]),
        );
        let report = lint(&course).unwrap();
        assert_eq!(
            report.issues_for(Rule::LedgerConformance).count(),
            2,
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn r0_catches_a_ledger_slot_no_step_declares() {
        let temp = TempDir::new("lint-r0-undeclared");
        let course = load_course(
            temp.path(),
            &one_track(
                &[step_toml(
                    "mailer",
                    "1.1-workspace",
                    &["repo/workspace-layout@apply"],
                )],
                &["repo/workspace-layout@apply"],
            ),
            &ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("ci/pipeline", "apply", "mailer/1.1"),
            ]),
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::LedgerConformance)
            .map(ToString::to_string)
            .collect();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].contains("no step there declares it"),
            "{issues:?}"
        );
    }

    #[test]
    fn r1_catches_the_same_depth_taught_twice_in_one_track() {
        let temp = TempDir::new("lint-r1");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    step_toml("mailer", "1.1-workspace", &["repo/workspace-layout@apply"]),
                    step_toml("mailer", "1.2-toolchain", &["repo/workspace-layout@apply"]),
                ],
                &["repo/workspace-layout@apply"],
            ),
            &ledger_toml(&[("repo/workspace-layout", "apply", "mailer/1.1")]),
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::RepeatTeaching)
            .chain(report.issues_for(Rule::DepthOrder))
            .map(ToString::to_string)
            .collect();
        assert!(
            issues.iter().any(|issue| issue.contains("both teach")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|issue| issue.contains("reteach")),
            "{issues:?}"
        );
    }

    #[test]
    fn r2_catches_a_level_regression_across_tracks() {
        let temp = TempDir::new("lint-r2");
        let course = load_course(
            temp.path(),
            &[
                (
                    "mailer",
                    1,
                    track_toml(
                        "mailer",
                        1,
                        &["async/graceful-shutdown@apply"],
                        &[step_toml(
                            "mailer",
                            "1.14-worker",
                            &["async/graceful-shutdown@apply"],
                        )],
                    ),
                ),
                (
                    "relay",
                    2,
                    track_toml(
                        "relay",
                        2,
                        &["async/graceful-shutdown@aware"],
                        &[step_toml(
                            "relay",
                            "2.1-shutdown",
                            &["async/graceful-shutdown@aware"],
                        )],
                    ),
                ),
            ],
            &ledger_toml(&[
                ("async/graceful-shutdown", "apply", "mailer/1.14"),
                ("async/graceful-shutdown", "aware", "relay/2.1"),
            ]),
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::DepthOrder)
            .map(ToString::to_string)
            .collect();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("relay/2.1-shutdown"), "{issues:?}");
        assert!(
            issues[0].contains("teaches async/graceful-shutdown@aware"),
            "{issues:?}"
        );
    }

    #[test]
    fn r2_accepts_a_deliberate_reteach_and_surfaces_its_reason() {
        let temp = TempDir::new("lint-r2-reteach");
        let course = load_course(
            temp.path(),
            &[
                (
                    "mailer",
                    1,
                    track_toml(
                        "mailer",
                        1,
                        &["async/graceful-shutdown@apply"],
                        &[step_toml(
                            "mailer",
                            "1.14-worker",
                            &["async/graceful-shutdown@apply"],
                        )],
                    ),
                ),
                (
                    "relay",
                    2,
                    track_toml(
                        "relay",
                        2,
                        &["async/graceful-shutdown@apply"],
                        &[
                            Step::new("relay", "2.1-shutdown", &["async/graceful-shutdown@apply"])
                                .reteach("the first run was measured, the second is a rehearsal")
                                .toml(),
                        ],
                    ),
                ),
            ],
            &ledger_toml(&[("async/graceful-shutdown", "apply", "mailer/1.14")]),
        );
        let report = lint(&course).unwrap();
        assert!(
            report.is_clean(),
            "an explicit re-teach is allowed: {:?}",
            report.issues
        );
        assert_eq!(report.issues_for(Rule::DepthOrder).count(), 0);
        assert_eq!(
            report.reteaches,
            [(
                "relay/2.1-shutdown".to_owned(),
                "the first run was measured, the second is a rehearsal".to_owned()
            )]
        );
    }

    #[test]
    fn r3_catches_a_requirement_taught_later() {
        let temp = TempDir::new("lint-r3-order");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    Step::new("mailer", "1.1-workspace", &["repo/workspace-layout@apply"])
                        .requires(&["ci/pipeline@apply"])
                        .toml(),
                    step_toml("mailer", "1.2-toolchain", &["ci/pipeline@apply"]),
                ],
                &["ci/pipeline@apply"],
            ),
            &ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("ci/pipeline", "apply", "mailer/1.2"),
            ]),
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::UnreachableRequires)
            .map(ToString::to_string)
            .collect();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].contains("never taught before this step"),
            "{issues:?}"
        );
    }

    #[test]
    fn r3_catches_a_requirement_that_is_taught_too_shallowly() {
        let temp = TempDir::new("lint-r3-depth");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    step_toml("mailer", "1.1-workspace", &["ci/pipeline@aware"]),
                    Step::new("mailer", "1.2-toolchain", &["repo/workspace-layout@apply"])
                        .requires(&["ci/pipeline@apply"])
                        .toml(),
                ],
                &["repo/workspace-layout@apply"],
            ),
            &ledger_toml(&[
                ("ci/pipeline", "aware", "mailer/1.1"),
                ("repo/workspace-layout", "apply", "mailer/1.2"),
            ]),
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::UnreachableRequires)
            .map(ToString::to_string)
            .collect();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("only taught at aware"), "{issues:?}");
    }

    #[test]
    fn r4_catches_teaching_without_a_gate_or_a_spec() {
        let temp = TempDir::new("lint-r4");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    Step::new("mailer", "1.1-workspace", &["repo/workspace-layout@apply"])
                        .spec(false)
                        .gates(&[])
                        .toml(),
                ],
                &["repo/workspace-layout@apply"],
            ),
            &ledger_toml(&[("repo/workspace-layout", "apply", "mailer/1.1")]),
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::UnboundTeaching)
            .map(ToString::to_string)
            .collect();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("no must gate"), "{issues:?}");
    }

    #[test]
    fn r6_catches_an_exit_capability_the_track_never_establishes() {
        let temp = TempDir::new("lint-r6");
        let course = load_course(
            temp.path(),
            &one_track(
                &[step_toml(
                    "mailer",
                    "1.1-workspace",
                    &["repo/workspace-layout@apply"],
                )],
                &["release/container-image@apply"],
            ),
            &ledger_toml(&[("repo/workspace-layout", "apply", "mailer/1.1")]),
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::UncoveredExit)
            .map(ToString::to_string)
            .collect();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].contains("release/container-image@apply as an exit capability"),
            "{issues:?}"
        );
    }

    #[test]
    fn r5_checks_prose_once_bodies_exist() {
        let temp = TempDir::new("lint-r5");
        let course = one_step_course(temp.path());
        // A `teach` step must have WHY; a body without it is a problem once the body exists.
        temp.write(
            "course/docs/steps/en/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\n+++\n\n# Title\n\n## TASK\n",
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::ProseStructure)
            .map(ToString::to_string)
            .collect();
        assert_eq!(report.bodies, 1);
        assert!(
            report.skipped.is_empty(),
            "nothing was skipped: {:?}",
            report.skipped
        );
        assert!(
            issues.iter().any(|issue| issue.contains("no `## WHY`")),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("no `## DONE WHEN`")),
            "{issues:?}"
        );
    }

    #[test]
    fn r5_accepts_a_teach_body_that_uses_the_template() {
        let temp = TempDir::new("lint-r5-teach-ok");
        let course = one_step_course(temp.path());
        temp.write(
            "course/docs/steps/en/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\n+++\n\n# Title\n\n## WHY\n\n## TASK\n\n\
             ## DONE WHEN\n\n## TRAP\n\n## FURTHER\n",
        );
        let report = lint(&course).unwrap();
        assert_eq!(report.bodies, 1);
        assert!(
            report.issues_for(Rule::ProseStructure).count() == 0,
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn r5_accepts_a_deepen_body_with_recall_and_delta() {
        let temp = TempDir::new("lint-r5-deepen-ok");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    step_toml("mailer", "1.1-workspace", &["ci/pipeline@aware"]),
                    Step::new("mailer", "1.2-toolchain", &["ci/pipeline@apply"])
                        .origin("deepen")
                        .toml(),
                ],
                &["ci/pipeline@apply"],
            ),
            &ledger_toml(&[
                ("ci/pipeline", "aware", "mailer/1.1"),
                ("ci/pipeline", "apply", "mailer/1.2"),
            ]),
        );
        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\n+++\n\n# Title\n\n## RECALL\n\n\
             `ci/pipeline@aware` — `mailer/1.1`: a task runner that runs the checks.\n\n\
             ## DELTA\n\n## TASK\n\n## DONE WHEN\n\n## TRAP\n",
        );
        let report = lint(&course).unwrap();
        assert!(
            report.issues_for(Rule::ProseStructure).count() == 0,
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn r5_catches_a_recall_that_does_not_say_where_it_comes_from() {
        let temp = TempDir::new("lint-r5-recall-empty");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    step_toml("mailer", "1.1-workspace", &["ci/pipeline@aware"]),
                    Step::new("mailer", "1.2-toolchain", &["ci/pipeline@apply"])
                        .origin("deepen")
                        .toml(),
                ],
                &["ci/pipeline@apply"],
            ),
            &ledger_toml(&[
                ("ci/pipeline", "aware", "mailer/1.1"),
                ("ci/pipeline", "apply", "mailer/1.2"),
            ]),
        );
        // An empty RECALL, a RECALL with a step that does not exist, and a RECALL that looks
        // forward are all "you have not said what you already know".
        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\n+++\n\n# Title\n\n## RECALL\n\nIt builds on the \
             `ci/pipeline@aware` you saw earlier.\n\n## DELTA\n\n## TASK\n\n## DONE WHEN\n\n## TRAP\n",
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::ProseStructure)
            .map(ToString::to_string)
            .collect();
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("names no earlier step")),
            "{issues:?}"
        );

        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\n+++\n\n# Title\n\n## RECALL\n\n\
             `ci/pipeline@apply` — `mailer/9.9`: a step that does not exist.\n\n## DELTA\n\n\
             ## TASK\n\n## DONE WHEN\n\n## TRAP\n",
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::ProseStructure)
            .map(ToString::to_string)
            .collect();
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("is not a step of this course")),
            "{issues:?}"
        );

        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\n+++\n\n# Title\n\n## RECALL\n\n\
             `ci/pipeline@apply` — `mailer/1.2`: this very step.\n\n## DELTA\n\n## TASK\n\n\
             ## DONE WHEN\n\n## TRAP\n",
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::ProseStructure)
            .map(ToString::to_string)
            .collect();
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("comes at or after this step")),
            "{issues:?}"
        );
    }

    #[test]
    fn r5_catches_a_deepen_that_recalls_the_wrong_step() {
        let temp = TempDir::new("lint-r5-recall-wrong-step");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    step_toml("mailer", "1.1-workspace", &["repo/workspace-layout@apply"]),
                    step_toml("mailer", "1.2-toolchain", &["ci/pipeline@aware"]),
                    Step::new("mailer", "1.3-ci", &["ci/pipeline@apply"])
                        .origin("deepen")
                        .toml(),
                ],
                &["ci/pipeline@apply"],
            ),
            &ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("ci/pipeline", "aware", "mailer/1.2"),
                ("ci/pipeline", "apply", "mailer/1.3"),
            ]),
        );
        temp.write(
            "course/docs/steps/en/mailer/1.3-ci.md",
            "+++\nid = \"mailer/1.3-ci\"\n+++\n\n# Title\n\n## RECALL\n\n\
             `ci/pipeline@aware` — `mailer/1.1`: the workspace split, which taught nothing about \
             pipelines.\n\n## DELTA\n\n## TASK\n\n## DONE WHEN\n\n## TRAP\n",
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::ProseStructure)
            .map(ToString::to_string)
            .collect();
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("never established")),
            "{issues:?}"
        );
    }

    #[test]
    fn r5_rejects_a_why_section_on_a_deepen_step() {
        let temp = TempDir::new("lint-r5-deepen");
        let course = load_course(
            temp.path(),
            &one_track(
                &[
                    step_toml("mailer", "1.1-workspace", &["repo/workspace-layout@apply"]),
                    Step::new("mailer", "1.2-toolchain", &["ci/pipeline@apply"])
                        .origin("deepen")
                        .toml(),
                ],
                &["ci/pipeline@apply"],
            ),
            &ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("ci/pipeline", "apply", "mailer/1.2"),
            ]),
        );
        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\n+++\n\n# Title\n\n## WHY\n\n## DELTA\n\n\
             ## TASK\n\n## DONE WHEN\n\n## TRAP\n",
        );
        let report = lint(&course).unwrap();
        let issues: Vec<String> = report
            .issues_for(Rule::ProseStructure)
            .map(ToString::to_string)
            .collect();
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("must not re-explain")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|issue| issue.contains("no `## RECALL`")),
            "{issues:?}"
        );
    }

    /// The acceptance check of this chunk: the repository's own metadata must be clean.
    #[test]
    fn the_repository_course_is_clean() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("kiln lives one directory below the repository root");
        let report = lint_root(root).unwrap();

        assert_eq!(report.capabilities, 97);
        assert_eq!(report.slots, 134);
        assert_eq!(report.ledger_positions, 118);
        // Every track the ledger plans has metadata now (T1 22 steps + T2 24); the bodies inside
        // those steps arrive one at a time, which is what the R5 notice below reports.
        assert_eq!(report.authored_steps, 46, "T1 and T2 metadata");
        assert!(report.is_clean(), "{:?}", report.issues);
        assert!(report.reteaches.is_empty());
        // R5 checks the bodies that exist and reports how many are still missing. Both numbers move
        // as the course gets authored, so the notice is checked against the report that produced it.
        match report.skipped.first() {
            Some(notice) => {
                assert!(
                    report.bodies < report.authored_steps,
                    "a skip notice with every body authored?"
                );
                assert!(
                    notice.contains(&format!(
                        "checked {} of {} step bodies",
                        report.bodies, report.authored_steps
                    )),
                    "{:?}",
                    report.skipped
                );
            }
            None => assert_eq!(
                report.bodies, report.authored_steps,
                "no skip notice means every body exists"
            ),
        }
    }
}
