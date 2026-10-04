//! `kiln review [--due [DAYS]]` — what this learner has proven, and what is getting stale.
//!
//! Plan §10: 118 steps span months, so early capabilities are guaranteed to go soft. The runner
//! keeps one record per capability (`level`, `last_verified`) and this command turns it into work:
//! the capabilities whose last proof is older than the window, oldest first, each with the probe
//! that would re-check it.
//!
//! Two honest limits show up in the output rather than in a footnote:
//!
//! * a capability with no probe gets the reason (`no probe: <why>`) instead of an invented quiz;
//! * a capability proven by a step before this command existed has no date, and says so — "no
//!   date recorded" sorts with the overdue, because pretending it is fresh would be the wrong guess.

use std::collections::BTreeMap;

use crate::caps::Level;
use crate::cmd::Session;
use crate::error::{EXIT_OK, Result};
use crate::info::Course;
use crate::probe::{self, Probe};
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
    probe: Option<&'a Probe>,
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
        match row.probe {
            Some(probe) => println!(
                "      re-check: `kiln probe {}` — {}",
                probe.slug(),
                probe.title()
            ),
            None => println!(
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
            let probe = probes.iter().find(|probe| {
                probe.requirement().id.as_str() == capability
                    && probe.requirement().level.rank() >= level.rank()
            });
            Row {
                capability,
                level,
                evidence: if evidence.is_empty() {
                    "no evidence recorded".to_owned()
                } else {
                    evidence
                },
                age_days: last_verified.map(|then| days_between(now, then)),
                probe,
            }
        })
        .collect()
}
