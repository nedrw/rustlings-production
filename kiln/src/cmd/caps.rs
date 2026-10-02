//! `kiln caps` — the capability ledger with evidence and status.
//!
//! This is the answer to "what does this course actually teach, and where?": every depth of every
//! capability, the step that establishes it (or the fact that the track is not authored yet), and
//! whether the learner has verified it. Deliberate re-teaches are listed with their reason, so the
//! escape hatch of plan §3 D4 stays visible instead of becoming a habit.

use crate::caps::CapabilityId;
use crate::caps::ledger::SlotPosition;
use crate::cmd::Session;
use crate::error::{EXIT_OK, Result};
use crate::info::Course;
use crate::state::{State, StepStatus};

/// Print the ledger, optionally restricted to one track.
pub fn run(session: &Session, track: Option<&str>) -> Result<u8> {
    let course = session.course();
    let ledger = course.ledger();
    if let Some(id) = track {
        // Validate before printing anything, so a typo cannot produce a half-empty listing.
        course.track(id)?;
    }

    println!(
        "capability ledger — {} capabilities, {} slots (docs/curriculum.md §3)",
        ledger.capability_count(),
        ledger.slot_count()
    );

    let mut shown = 0;
    let mut verified = 0;
    for capability in ledger.capabilities() {
        let slots: Vec<_> = ledger
            .levels(capability)
            .filter(|(_, position)| track.is_none_or(|id| position.track == id))
            .collect();
        if slots.is_empty() {
            continue;
        }

        println!();
        println!("{capability}");
        for (level, position) in slots {
            shown += 1;
            let evidence = evidence_for(course, session.state(), capability, position);
            if evidence.verified {
                verified += 1;
            }
            println!("  {level:<8} {:<32} {}", evidence.step, evidence.status);
            if let Some(reason) = evidence.reteach {
                println!("           reteach: {reason}");
            }
        }
    }

    println!();
    match track {
        Some(id) => println!("{shown} slots in {id} · {verified} verified"),
        None => println!(
            "{shown} slots shown · {verified} verified · tracks not authored yet show their ledger \
             position only"
        ),
    }
    Ok(EXIT_OK)
}

/// What the ledger knows about one slot, plus what progress says about it.
#[derive(Debug)]
struct Evidence {
    /// Step that establishes the slot, or the reason it cannot be resolved yet.
    step: String,
    /// Status of that step.
    status: String,
    /// Set when the step declares a deliberate re-teach.
    reteach: Option<String>,
    /// Whether the slot counts as verified by the learner.
    verified: bool,
}

fn evidence_for(
    course: &Course,
    state: &State,
    capability: &CapabilityId,
    position: &SlotPosition,
) -> Evidence {
    let Ok(track) = course.track(&position.track) else {
        return Evidence {
            step: format!(
                "{}/{} (track not authored yet)",
                position.track, position.number
            ),
            status: "not authored yet".into(),
            reteach: None,
            verified: false,
        };
    };
    let Ok(step) = track.step_by_number(&position.number) else {
        return Evidence {
            step: format!("{}/{} (no such step)", position.track, position.number),
            status: format!(
                "metadata error: the ledger points at step {} for {capability}",
                position.number
            ),
            reteach: None,
            verified: false,
        };
    };

    let status = state.status(&step.qualified_id());
    Evidence {
        status: status.label().to_owned(),
        verified: status == StepStatus::Done,
        reteach: step.reteach.clone(),
        step: step.qualified_id(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, ledger_toml, load_course, step_toml, track_toml};

    /// A ledger slot must degrade to a readable position when its track or step is not authored
    /// yet: the ledger covers all 118 planned steps, only 22 of which exist as metadata.
    #[test]
    fn slots_are_resolved_or_explained() {
        let temp = TempDir::new("cmd-caps-resolve");
        let course = load_course(
            temp.path(),
            &track_toml(
                &["repo/workspace-layout@apply"],
                &[step_toml("1.1-workspace", &["repo/workspace-layout@apply"])],
            ),
            &ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("http/service-skeleton", "apply", "relay/2.1"),
                ("async/backpressure", "aware", "mailer/9.9"),
            ]),
        );
        let state = State::default();
        let capability = |id: &str| CapabilityId::parse(id).unwrap();
        let position = |text: &str| SlotPosition::parse(text).unwrap();

        let authored = evidence_for(
            &course,
            &state,
            &capability("repo/workspace-layout"),
            &position("mailer/1.1"),
        );
        assert_eq!(authored.step, "mailer/1.1-workspace");
        assert_eq!(authored.status, "not verified");

        let unauthored = evidence_for(
            &course,
            &state,
            &capability("http/service-skeleton"),
            &position("relay/2.1"),
        );
        assert!(
            unauthored.step.contains("track not authored yet"),
            "{unauthored:?}"
        );
        assert!(!unauthored.verified);

        let broken = evidence_for(
            &course,
            &state,
            &capability("async/backpressure"),
            &position("mailer/9.9"),
        );
        assert!(broken.step.contains("no such step"), "{broken:?}");
        assert!(broken.status.contains("metadata error"), "{broken:?}");

        let mut verified = State::default();
        verified.record_spec("mailer/1.1-workspace", true);
        verified.record_gates("mailer/1.1-workspace", true);
        let done = evidence_for(
            &course,
            &verified,
            &capability("repo/workspace-layout"),
            &position("mailer/1.1"),
        );
        assert!(done.verified);
        assert_eq!(done.status, "verified");
    }
}
