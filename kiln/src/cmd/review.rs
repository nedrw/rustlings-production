//! `kiln review [--due [DAYS]]` — what this learner has proven, and what is getting stale.
//!
//! Plan §10: 118 steps span months, so early capabilities are guaranteed to go soft. The runner
//! keeps one record per capability (`level`, `last_verified`) and this command turns it into work:
//! the capabilities whose last proof is older than the window, oldest first, each with the probe
//! that would re-check it.
//!
//! Two honest limits show up in the output rather than in a footnote:
//!
//! * a capability no exercise can re-check gets that said (`no probe for this slot`), and one whose
//!   only exercise is shallower than the depth it reached gets the shallower one named as itself —
//!   never as a proof it is not;
//! * a capability proven by a step before this command existed has no date, and says so — "no date
//!   recorded" sorts with the overdue, because pretending it is fresh would be the wrong guess.

use std::collections::BTreeMap;

use crate::caps::Level;
use crate::cmd::Session;
use crate::error::{EXIT_OK, Result};
use crate::info::Course;
use crate::probe::{self, Exercise, Probe};
use crate::state::{State, days_between, unix_now};

/// Days after which a capability is worth re-checking (plan §10: 45).
///
/// `kiln review --due` uses it; `--due <DAYS>` overrides it for a learner who wants a tighter or a
/// looser loop.
pub const DUE_DAYS: u64 = 45;

/// What is known about one capability, for the report.
struct Row<'a> {
    capability: String,
    level: Level,
    evidence: String,
    age_days: Option<u64>,
    recheck: Exercise<'a>,
}

impl Row<'_> {
    /// `true` when the proof is older than the window, or has no date at all.
    fn due(&self, window: u64) -> bool {
        self.age_days.is_none_or(|days| days >= window)
    }
}

/// Print the review list.
pub fn run(session: &Session, due_days: Option<u64>) -> Result<u8> {
    let course = session.course();
    let state = session.state();
    let probes = probe::load(&course.dir())?;
    let due_days = due_days.unwrap_or(DUE_DAYS);
    let now = unix_now();

    let mut rows = collect(course, state, &probes, now);
    rows.sort_by(|left, right| {
        // Undated first (most likely forgotten), then oldest, then by capability name.
        let left_key = left.age_days.unwrap_or(u64::MAX);
        let right_key = right.age_days.unwrap_or(u64::MAX);
        right_key
            .cmp(&left_key)
            .then_with(|| left.capability.cmp(&right.capability))
    });

    let due: Vec<&Row<'_>> = rows.iter().filter(|row| row.due(due_days)).collect();
    println!(
        "review · {} capabilities have evidence · {} due (window {due_days} days)",
        rows.len(),
        due.len()
    );
    println!();
    if rows.is_empty() {
        println!(
            "nothing proven yet: the records are written when a step's gates pass and when a probe \
             passes. `kiln list` shows where to start."
        );
        return Ok(EXIT_OK);
    }

    if due.is_empty() {
        let newest = rows.first().and_then(|row| row.age_days).unwrap_or(0);
        println!("nothing is due — the newest proof is {newest} days old");
        println!("`kiln review` (no flag) lists every capability with its evidence");
        return Ok(EXIT_OK);
    }

    for row in &due {
        let age = match row.age_days {
            Some(days) => format!("{days} days ago"),
            None => "no date recorded".to_owned(),
        };
        println!(
            "  {:<32} {:<7} {} ({age})",
            row.capability, row.level, row.evidence
        );
        match row.recheck {
            Exercise::AtDepth(probe) => println!(
                "      re-check: `kiln probe {}` — {}",
                probe.slug(),
                probe.title()
            ),
            Exercise::Shallower(probe) => println!(
                "      no exercise at this depth; the closest is `kiln probe {}` ({}), which re-checks \
                 the habit — only redoing the step re-checks this one",
                probe.slug(),
                probe.requirement().level
            ),
            Exercise::Missing => println!(
                "      no probe for this slot (kiln/src/probe.rs says why some slots have none)"
            ),
        }
    }
    println!();
    println!(
        "a passed probe writes `last_verified`, which is what this list reads; \
         `kiln plan --fast` also accepts that record as proof you may skip the step"
    );
    Ok(EXIT_OK)
}

/// Every capability the learner has evidence for: the records, plus done or skipped steps.
///
/// The second half matters: a state file written before [`crate::state::CapabilityState`] existed
/// still says which steps are done, and those steps are evidence. Recomputing them here means the
/// review list is complete on the first run after an upgrade instead of only from then on.
fn collect<'a>(course: &Course, state: &State, probes: &'a [Probe], now: u64) -> Vec<Row<'a>> {
    // Capability → deepest (level, evidence, timestamp).
    let mut found: BTreeMap<String, (Level, String, Option<u64>)> = BTreeMap::new();
    for step in course.steps() {
        if state.status(&step.qualified_id()).is_unfinished() {
            continue;
        }
        for requirement in &step.teaches {
            let entry = found.entry(requirement.id.to_string()).or_insert((
                requirement.level,
                String::new(),
                None,
            ));
            if requirement.level.rank() >= entry.0.rank() {
                entry.0 = requirement.level;
            }
            if entry.1.is_empty() {
                entry.1 = step.qualified_id();
            }
        }
    }
    for (id, record) in &state.capability {
        let entry = found.entry(id.clone()).or_insert((
            record.level,
            record.evidence.clone(),
            record.last_verified,
        ));
        if record.level.rank() >= entry.0.rank() {
            entry.0 = record.level;
        }
        entry.1 = record.evidence.clone();
        entry.2 = record.last_verified;
    }

    found
        .into_iter()
        .map(|(capability, (level, evidence, last_verified))| {
            let recheck = probe::exercise(probes, &capability, level);
            Row {
                capability,
                level,
                evidence: if evidence.is_empty() {
                    "no evidence recorded".to_owned()
                } else {
                    evidence
                },
                age_days: last_verified.map(|then| days_between(now, then)),
                recheck,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Evidence;
    use crate::test_support::{TempDir, ledger_toml, load_course, one_track, step_toml};

    /// A course whose one step goes deeper than the only exercise the catalogue holds.
    fn fixture() -> (TempDir, Course) {
        let temp = TempDir::new("review-recheck");
        let steps = vec![step_toml(
            "mailer",
            "1.1-audit",
            &["sec/authz-tenant-isolation@design"],
        )];
        let ledger = ledger_toml(&[("sec/authz-tenant-isolation", "design", "mailer/1.1")]);
        let course = load_course(temp.path(), &one_track(&steps, &[]), &ledger);
        temp.write(
            "course/probes/sec-authz-tenant-isolation.apply.rs",
            "//! probe = \"sec/authz-tenant-isolation@apply\"\n//! title = \"Who may do what\"\n\
             //! hint = \"Start from the matrix.\"\n\npub fn allow() -> bool { todo!() }\n",
        );
        (temp, course)
    }

    /// One row per recorded proof, with the catalogue's answer attached.
    fn rows_of<'a>(course: &Course, probes: &'a [Probe], slot: &str) -> Vec<Row<'a>> {
        let mut state = State::default();
        state.record_capability(
            &slot.parse().expect("a slot"),
            Evidence::Step("mailer/1.1-audit"),
        );
        let rows = collect(course, &state, probes, unix_now());
        assert_eq!(rows.len(), 1, "the record is the only evidence");
        rows
    }

    #[test]
    fn a_shallower_exercise_is_offered_as_a_shallower_re_check() {
        let (_temp, course) = fixture();
        let probes = probe::load(&course.dir()).expect("the catalogue should load");
        let rows = rows_of(&course, &probes, "sec/authz-tenant-isolation@design");
        assert_eq!(rows[0].level, Level::Design);
        match rows[0].recheck {
            Exercise::Shallower(probe) => {
                assert_eq!(probe.slug(), "sec-authz-tenant-isolation.apply");
            }
            ref other => panic!("a shallower exercise is not a proof of `design`: {other:?}"),
        }
    }

    #[test]
    fn an_exercise_at_the_depth_is_the_re_check() {
        let (_temp, course) = fixture();
        let probes = probe::load(&course.dir()).expect("the catalogue should load");
        let rows = rows_of(&course, &probes, "sec/authz-tenant-isolation@apply");
        match rows[0].recheck {
            Exercise::AtDepth(probe) => {
                assert_eq!(probe.slug(), "sec-authz-tenant-isolation.apply");
            }
            ref other => panic!("an exercise at this depth re-checks the slot itself: {other:?}"),
        }
    }

    #[test]
    fn a_capability_no_exercise_answers_is_marked_missing() {
        let (_temp, course) = fixture();
        let probes = probe::load(&course.dir()).expect("the catalogue should load");
        let rows = rows_of(&course, &probes, "delivery/idempotency@apply");
        assert_eq!(rows[0].recheck, Exercise::Missing);
    }
}
