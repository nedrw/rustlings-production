//! `course/capabilities.toml` must stay the transcription of `docs/curriculum.md` §3.
//!
//! `kiln lint-caps` (rule R0) cross-checks the ledger against the authored `track.toml` files, and
//! all five tracks are authored now — but only 119 of the ledger's 136 slots are claimed by a step.
//! A slot no step claims still shapes what later tracks may teach, so nothing else would notice if
//! the document and the ledger drifted apart there. This test closes that gap by re-deriving the
//! whole ledger from the document: the same comparison the document-level prototype
//! (`tools/lint-caps-md.py`) makes, done against the metadata instead of the tables.
//!
//! It is deliberately strict: the ledger is not allowed to gain a slot the curriculum does not
//! claim, or to lose one it does.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use kiln::caps::{CapabilityId, Level};
use kiln::info::Course;

/// Track labels used by the curriculum tables (`T1`) and the ids they stand for.
const TRACKS: [(u32, &str); 5] = [
    (1, "mailer"),
    (2, "relay"),
    (3, "jobq"),
    (4, "lakelet"),
    (5, "meshkv"),
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("kiln lives one directory below the repository root")
        .to_path_buf()
}

fn curriculum() -> String {
    let path = repository_root().join("docs/curriculum.md");
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("cannot read {path:?}: {error}"))
}

/// The text between `start` and `end`, both matched as prefixes of a heading line.
fn section<'a>(document: &'a str, start: &str, end: &str) -> &'a str {
    let after = document
        .split_once(start)
        .unwrap_or_else(|| panic!("docs/curriculum.md has no `{start}`"))
        .1;
    after.split_once(end).map_or(after, |(section, _)| section)
}

/// Split a markdown table row into its cells.
fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

/// (capability, depth) slots as §3 declares them.
fn slots_from_document() -> BTreeMap<String, BTreeMap<Level, String>> {
    let document = curriculum();
    let ledger = section(&document, "## 3. 能力台账", "## 4.");
    let mut slots: BTreeMap<String, BTreeMap<Level, String>> = BTreeMap::new();

    for line in ledger.lines() {
        if !line.starts_with('|') {
            continue;
        }
        let columns = cells(line);
        if columns.len() != 5 {
            continue;
        }
        let capability = columns[0].trim_matches('`');
        if !capability.contains('/') {
            continue;
        }
        let mut depths = BTreeMap::new();
        for (depth, column) in Level::ALL.iter().zip(&columns[1..]) {
            let cell = column.trim_matches('`').trim();
            let Some((track, number)) = cell.split_once('·') else {
                continue;
            };
            let index: u32 = track
                .trim_start_matches('T')
                .parse()
                .unwrap_or_else(|_| panic!("`{cell}` is not a T<n>·<phase>.<index> cell"));
            let id = TRACKS
                .iter()
                .find(|(position, _)| *position == index)
                .map(|(_, id)| *id)
                .unwrap_or_else(|| panic!("unknown track label T{index}"));
            depths.insert(*depth, format!("{id}/{number}"));
        }
        assert!(
            !depths.is_empty(),
            "§3 claims no depth for `{capability}`, so it should not be listed"
        );
        slots.insert(capability.to_owned(), depths);
    }
    slots
}

/// Step positions (`<track>/<phase>.<index>`) named by the per-track step tables (§6-§10).
fn positions_from_document() -> BTreeSet<String> {
    let document = curriculum();
    let bounds = [
        ("## 6. T1", "## 7.", "mailer"),
        ("## 7. T2", "## 8.", "relay"),
        ("## 8. T3", "## 9.", "jobq"),
        ("## 9. T4", "## 10.", "lakelet"),
        ("## 10. T5", "## 11.", "meshkv"),
    ];
    let mut positions = BTreeSet::new();
    for (start, end, track) in bounds {
        for line in section(&document, start, end).lines() {
            if !line.starts_with('|') {
                continue;
            }
            let columns = cells(line);
            let Some(first) = columns.first() else {
                continue;
            };
            let number = *first;
            let is_step = number.split_once('.').is_some_and(|(phase, index)| {
                !phase.is_empty()
                    && !index.is_empty()
                    && phase.bytes().all(|b| b.is_ascii_digit())
                    && index.bytes().all(|b| b.is_ascii_digit())
            });
            if is_step {
                positions.insert(format!("{track}/{number}"));
            }
        }
    }
    positions
}

#[test]
fn the_ledger_is_the_transcription_of_the_curriculum() {
    let course = Course::load(&repository_root()).expect("the repository course loads");
    let ledger = course.ledger();
    let document_slots = slots_from_document();

    // Same capabilities, same depths, same positions — in both directions.
    let from_ledger: BTreeMap<String, BTreeMap<Level, String>> =
        ledger
            .slots()
            .fold(BTreeMap::new(), |mut acc, (capability, level, position)| {
                acc.entry(capability.as_str().to_owned())
                    .or_default()
                    .insert(level, format!("{}/{}", position.track, position.number));
                acc
            });
    assert_eq!(
        from_ledger, document_slots,
        "capabilities.toml and docs/curriculum.md §3 disagree"
    );

    // The counts the prototype reports, now pinned against the metadata.
    assert_eq!(ledger.capability_count(), 97);
    assert_eq!(ledger.slot_count(), 136);

    // Every slot points at a step the curriculum actually lists, and every listed step has one.
    let document_positions = positions_from_document();
    let ledger_positions: BTreeSet<String> = ledger
        .positions()
        .into_iter()
        .map(|(track, number)| format!("{track}/{number}"))
        .collect();
    assert_eq!(
        document_positions.len(),
        119,
        "the curriculum has 119 steps"
    );
    assert_eq!(
        ledger_positions, document_positions,
        "the ledger pins a different set of steps than the curriculum lists"
    );

    // Ids and depths parse: a transcription typo would show up as a parse failure above, but the
    // explicit check below says which value is wrong when it happens.
    for (capability, levels) in &document_slots {
        CapabilityId::parse(capability).expect("§3 capabilities are valid ids");
        assert!(levels.len() <= Level::ALL.len());
    }
}
