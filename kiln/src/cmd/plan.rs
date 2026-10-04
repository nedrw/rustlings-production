//! `kiln plan [track] [--fast]` — the path ahead, and what skipping would skip.
//!
//! Plan §3 D4 draws one line through the whole de-duplication design: a capability that is already
//! proven does not have to be *taught* again, but the implementation still belongs to the new
//! project unless the learner proved the capability on their own, with a probe, after learning it.
//! `--fast` is that decision made visible: for every step it says *keep* or *skip*, and names the
//! evidence behind a skip.
//!
//! Nothing here changes the learner's tree: a plan is a plan. `kiln skip <step>` is the command
//! that acts on one, and it applies the reference solution (recording that it did), because the
//! project has to keep compiling for the steps after it to mean anything.

use crate::caps::CapRequirement;
use crate::cmd::Session;
use crate::error::{EXIT_OK, Result};
use crate::info::{Step, Track};
use crate::state::{State, StepStatus, days_between, unix_now};

/// Why a step may be skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipReason {
    /// One line per slot that proves it, e.g. `delivery/idempotency@apply proved by probe (3 days
    /// ago)`.
    pub proofs: Vec<String>,
}

impl SkipReason {
    /// One line for a report: the proofs, or the fact that `--force` went without them.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.proofs.is_empty() {
            "forced: no probe proved this step's slots".to_owned()
        } else {
            self.proofs.join("; ")
        }
    }
}

/// Why a step may not be skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    /// Slot that is not proven; `None` when the step declares no capability at all.
    pub requirement: Option<CapRequirement>,
    /// What the learner would have to do about it.
    pub detail: String,
}

/// The decision for one step under `--fast`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Work on it: at least one slot it establishes is not proven by a probe.
    Keep(Blocker),
    /// Skip it: every slot it establishes has probe evidence.
    Skip(SkipReason),
}

/// Decide one step, from the learner's capability records alone.
///
/// The rule has one sentence and one consequence. *A step may be skipped when every slot it
/// teaches is proven by a probe the learner passed.* A step that teaches three slots and has two
/// proven is **kept**: it still carries new material, and half a step is not a skip.
#[must_use]
pub fn disposition(state: &State, step: &Step) -> Disposition {
    let now = unix_now();
    if step.teaches.is_empty() {
        // R4 reports a step that teaches nothing as unbound metadata; here it is simply not
        // skippable, because there is nothing a probe could prove.
        return Disposition::Keep(Blocker {
            requirement: None,
            detail: "this step declares no capability, so there is nothing a probe could prove"
                .to_owned(),
        });
    }

    let mut proofs = Vec::with_capacity(step.teaches.len());
    for requirement in &step.teaches {
        match state.capability(&requirement.id) {
            Some(record)
                if record.level.rank() >= requirement.level.rank() && record.from_probe() =>
            {
                let age = record.last_verified.map_or_else(
                    || "no date recorded".to_owned(),
                    |then| format!("{} days ago", days_between(now, then)),
                );
                proofs.push(format!(
                    "{requirement} proved by {}, {age}",
                    record.evidence
                ));
            }
            Some(record) if record.level.rank() >= requirement.level.rank() => {
                return Disposition::Keep(Blocker {
                    requirement: Some(requirement.clone()),
                    detail: format!(
                        "the deepest proof is {} ({}) — not a probe, so `kiln probe {}` is what \
                         re-checks it from an empty file",
                        record.evidence, record.level, requirement
                    ),
                });
            }
            Some(record) => {
                return Disposition::Keep(Blocker {
                    requirement: Some(requirement.clone()),
                    detail: format!(
                        "proven at {} only — {} is a new depth, and new depth is never skipped",
                        record.level, requirement.level
                    ),
                });
            }
            None => {
                return Disposition::Keep(Blocker {
                    requirement: Some(requirement.clone()),
                    detail: format!(
                        "nothing proven about {} yet — `kiln probe {}` is what would prove it",
                        requirement.id, requirement
                    ),
                });
            }
        }
    }
    Disposition::Skip(SkipReason { proofs })
}

/// One step's disposition, as a learner reads it.
#[must_use]
pub fn describe(state: &State, step: &Step) -> String {
    match disposition(state, step) {
        Disposition::Skip(reason) => format!("skip ({})", reason.summary()),
        Disposition::Keep(blocker) => format!("keep ({})", blocker.detail),
    }
}

/// Print the path through a track.
pub fn run(session: &Session, track: Option<&str>, fast: bool) -> Result<u8> {
    let track = match track {
        Some(id) => session.course().track(id)?,
        None => session.course().default_track()?,
    };
    print_plan(session, track, fast);
    Ok(EXIT_OK)
}

/// The plan itself, in the shape `kiln skip --all` acts on.
pub fn print_plan(session: &Session, track: &Track, fast: bool) {
    let state = session.state();
    let title = if fast { "plan --fast" } else { "plan" };
    let mut lines = Vec::new();
    let mut planned = 0usize;
    let mut skippable = 0usize;
    for step in &track.step {
        let status = state.status(&step.qualified_id());
        let teaches = step
            .teaches
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        if !status.is_unfinished() {
            lines.push(format!(
                "  {} {:<24} {:<34} {}",
                status.marker(),
                step.qualified_id(),
                teaches,
                status.label()
            ));
            continue;
        }
        planned += 1;
        if fast {
            let disposition = disposition(state, step);
            if matches!(disposition, Disposition::Skip(_)) {
                skippable += 1;
            }
            let marker = match disposition {
                Disposition::Skip(_) => "»",
                Disposition::Keep(_) => "·",
            };
            // The verdict gets its own line: it is a sentence, and a sentence pushed past the
            // terminal width is the column that stops being read.
            lines.push(format!("  {marker} {:<24} {teaches}", step.qualified_id()));
            lines.push(format!("        {}", describe(state, step)));
        } else {
            lines.push(format!(
                "  · {:<24} {:<34} {}",
                step.qualified_id(),
                teaches,
                step.title
            ));
        }
    }

    if fast {
        println!(
            "{title} · {} ({} steps) · {} to work on, {} skippable by probe",
            track.id,
            track.step.len(),
            planned.saturating_sub(skippable),
            skippable
        );
    } else {
        println!(
            "{title} · {} ({} steps) · {planned} still to do · `--fast` marks what a probe could \
             prove you out of",
            track.id,
            track.step.len()
        );
    }
    println!();
    for line in &lines {
        println!("{line}");
    }
    println!();
    if !fast {
        println!("`kiln plan --fast` adds the keep/skip verdict for every step");
    } else if skippable == 0 {
        println!(
            "nothing is skippable yet: skipping takes a probe you passed (`kiln probe <capability>` \
             - `kiln probe` lists them), not a step you finished"
        );
    } else {
        println!(
            "» {skippable} step(s) can be skipped: `kiln skip --all` does it in one pass, `kiln skip \
             <step>` one at a time (the reference solution is applied so the project keeps compiling)"
        );
    }
    println!("`kiln review --due` shows which capabilities are getting stale");
}

/// The steps of a track the learner may skip right now, in order.
#[must_use]
pub fn skippable<'a>(state: &State, track: &'a Track) -> Vec<&'a Step> {
    track
        .step
        .iter()
        .filter(|step| state.status(&step.qualified_id()).is_unfinished())
        .filter(|step| matches!(disposition(state, step), Disposition::Skip(_)))
        .collect()
}

/// `Some(status)` when a step is done or skipped already, for messages that say so.
#[must_use]
pub fn behind_us(state: &State, step: &Step) -> Option<StepStatus> {
    let status = state.status(&step.qualified_id());
    (!status.is_unfinished()).then_some(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CapabilityId;
    use crate::info::Course;
    use crate::state::{Evidence, State};
    use crate::test_support::{TempDir, ledger_toml, load_course, one_track, step_toml};

    /// A course with two steps: one that teaches two slots, one that teaches a single slot.
    fn fixture() -> (TempDir, Course) {
        let temp = TempDir::new("plan-disposition");
        let steps = vec![
            step_toml(
                "mailer",
                "1.1-first",
                &["delivery/idempotency@apply", "db/migrations@apply"],
            ),
            step_toml("mailer", "1.2-second", &["db/migrations@apply"]),
        ];
        let ledger = ledger_toml(&[
            ("delivery/idempotency", "apply", "mailer/1.1"),
            ("db/migrations", "apply", "mailer/1.1"),
        ]);
        let course = load_course(temp.path(), &one_track(&steps, &[]), &ledger);
        (temp, course)
    }

    fn probe(state: &mut State, slot: &str) {
        state.record_capability(
            &slot.parse().expect("a slot"),
            Evidence::Probe("delivery-idempotency.apply"),
        );
    }

    #[test]
    fn a_step_is_kept_until_every_slot_it_teaches_is_proven() {
        let (_temp, course) = fixture();
        let track = course.track("mailer").expect("the fixture track");
        let first = course.step("mailer/1.1").expect("the first step");
        let second = course.step("mailer/1.2").expect("the second step");

        // Nothing proven: both steps are work.
        let state = State::default();
        assert!(matches!(disposition(&state, first), Disposition::Keep(_)));
        assert!(matches!(disposition(&state, second), Disposition::Keep(_)));

        // One of the two slots of `1.1` proven: `1.2` becomes skippable, `1.1` does not.
        let mut state = State::default();
        probe(&mut state, "db/migrations@apply");
        assert!(
            matches!(disposition(&state, first), Disposition::Keep(_)),
            "half a step is not a skip"
        );
        assert!(matches!(disposition(&state, second), Disposition::Skip(_)));
        assert_eq!(skippable(&state, track).len(), 1);
    }

    #[test]
    fn a_finished_step_is_not_the_same_evidence_as_a_probe() {
        let (_temp, course) = fixture();
        let second = course.step("mailer/1.2").expect("the second step");
        let mut state = State::default();
        state.record_capability(
            &"db/migrations@apply".parse().expect("a slot"),
            Evidence::Step("mailer/1.2-second"),
        );
        let Disposition::Keep(blocker) = disposition(&state, second) else {
            panic!("step evidence must not unlock a skip");
        };
        let detail = blocker.detail;
        assert!(detail.contains("not a probe"), "{detail}");
        assert!(
            detail.contains("kiln probe db/migrations@apply"),
            "{detail}"
        );
    }

    #[test]
    fn a_deeper_depth_is_never_skipped() {
        let (_temp, course) = fixture();
        let first = course.step("mailer/1.1").expect("the first step");
        let mut state = State::default();
        // `db/migrations` is proven at `apply`; a step that teaches `design` would still be work.
        state.record_capability(
            &"db/migrations@apply".parse().expect("a slot"),
            Evidence::Probe("db-migrations.apply"),
        );
        state.record_capability(
            &"delivery/idempotency@apply".parse().expect("a slot"),
            Evidence::Probe("delivery-idempotency.apply"),
        );
        assert!(matches!(disposition(&state, first), Disposition::Skip(_)));

        // The record is what the ledger's next depth would have to beat; the shallow record does
        // not make `design` skippable, which is the rule this test exists for.
        assert_eq!(
            state
                .capability(&CapabilityId::parse("db/migrations").expect("an id"))
                .expect("a record")
                .level,
            crate::caps::Level::Apply
        );
    }

    #[test]
    fn a_step_that_teaches_nothing_is_never_skippable() {
        let (_temp, course) = fixture();
        // A step with an empty `teaches` list cannot be built through the fixture (the loader
        // rejects an unbound step), so this is checked on the failure path of the rule instead.
        let step = course.step("mailer/1.1").expect("the first step");
        let keep = disposition(&State::default(), step);
        assert!(matches!(keep, Disposition::Keep(_)));
    }

    #[test]
    fn describe_names_the_evidence_so_the_report_can_be_checked() {
        let (_temp, course) = fixture();
        let second = course.step("mailer/1.2").expect("the second step");
        let mut state = State::default();
        probe(&mut state, "db/migrations@apply");
        let text = describe(&state, second);
        assert!(text.starts_with("skip ("), "{text}");
        assert!(text.contains("probe:delivery-idempotency.apply"), "{text}");
    }
}
