//! The capability ledger, `course/capabilities.toml`.
//!
//! The ledger answers one question for the whole course: *where* is this (capability, depth)
//! pair established? It is the reference the step metadata is checked against, which is why
//! `kiln lint-caps` (rule R0) needs nothing but this file and `tracks/*/track.toml`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use super::{CapabilityId, Level};
use crate::error::{KilnError, Result};

/// Position of one slot: track plus the phase index of the step inside it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SlotPosition {
    /// Track id, e.g. `mailer`.
    pub track: String,
    /// Phase index as the curriculum writes it, e.g. `1.14`.
    pub number: String,
}

impl SlotPosition {
    /// Parse `<track>/<phase>.<index>`.
    pub fn parse(text: &str) -> Result<Self> {
        let (track, number) = text.split_once('/').ok_or_else(|| {
            KilnError::Invalid(format!(
                "`{text}` is not a ledger position: expected `<track>/<phase>.<index>`"
            ))
        })?;
        let well_formed = !track.is_empty()
            && track
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && matches!(number.split_once('.'), Some((phase, index))
                if !phase.is_empty()
                    && !index.is_empty()
                    && phase.bytes().all(|b| b.is_ascii_digit())
                    && index.bytes().all(|b| b.is_ascii_digit()));
        if !well_formed {
            return Err(KilnError::Invalid(format!(
                "`{text}` is not a ledger position: expected `<track>/<phase>.<index>`"
            )));
        }
        Ok(Self {
            track: track.to_owned(),
            number: number.to_owned(),
        })
    }
}

/// The shape of `course/capabilities.toml` on disk.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerFile {
    format_version: u32,
    #[serde(default)]
    capability: BTreeMap<CapabilityId, BTreeMap<Level, String>>,
}

/// Where every (capability, depth) pair of the course is established.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    slots: BTreeMap<CapabilityId, BTreeMap<Level, SlotPosition>>,
}

impl Ledger {
    /// Read and validate a ledger file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| KilnError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text).map_err(|error| match error {
            // Attach the file name so an invalid slot names the file it sits in.
            KilnError::Invalid(message) => {
                KilnError::Invalid(format!("{message} (in {})", path.display()))
            }
            other => other,
        })
    }

    /// Parse ledger text (the loader used by `load` and by tests).
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let file: LedgerFile = toml::from_str(text).map_err(|source| KilnError::Metadata {
            path: Path::new("capabilities.toml").to_path_buf(),
            source,
        })?;
        if file.format_version != crate::info::FORMAT_VERSION {
            return Err(KilnError::Invalid(format!(
                "capabilities.toml: unsupported format_version {}, expected {}",
                file.format_version,
                crate::info::FORMAT_VERSION
            )));
        }

        let mut slots: BTreeMap<CapabilityId, BTreeMap<Level, SlotPosition>> = BTreeMap::new();
        for (id, levels) in file.capability {
            let mut positions = BTreeMap::new();
            for (level, position) in levels {
                positions.insert(level, SlotPosition::parse(&position)?);
            }
            if positions.is_empty() {
                return Err(KilnError::Invalid(format!(
                    "capabilities.toml: `{id}` declares no depth, so it teaches nothing"
                )));
            }
            slots.insert(id, positions);
        }
        Ok(Self { slots })
    }

    /// Every capability, in id order.
    pub fn capabilities(&self) -> impl Iterator<Item = &CapabilityId> {
        self.slots.keys()
    }

    /// Number of registered capabilities.
    #[must_use]
    pub fn capability_count(&self) -> usize {
        self.slots.len()
    }

    /// Number of (capability, depth) slots — the "134" of the ledger.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.values().map(BTreeMap::len).sum()
    }

    /// Where the pair is established, if the course claims it at all.
    #[must_use]
    pub fn slot(&self, id: &CapabilityId, level: Level) -> Option<&SlotPosition> {
        self.slots.get(id).and_then(|levels| levels.get(&level))
    }

    /// Every depth of one capability, shallowest first.
    pub fn levels(&self, id: &CapabilityId) -> impl Iterator<Item = (Level, &SlotPosition)> {
        self.slots
            .get(id)
            .into_iter()
            .flat_map(|levels| levels.iter().map(|(level, position)| (*level, position)))
    }

    /// Every slot, in capability then depth order.
    pub fn slots(&self) -> impl Iterator<Item = (&CapabilityId, Level, &SlotPosition)> {
        self.slots.iter().flat_map(|(id, levels)| {
            levels
                .iter()
                .map(move |(level, position)| (id, *level, position))
        })
    }

    /// Distinct step positions the ledger pins — one entry per step of the curriculum.
    #[must_use]
    pub fn positions(&self) -> BTreeSet<(&str, &str)> {
        self.slots()
            .map(|(_, _, position)| (position.track.as_str(), position.number.as_str()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEDGER: &str = r#"
        format_version = 1

        [capability."async/graceful-shutdown"]
        apply = "mailer/1.14"
        design = "relay/2.15"

        [capability."obs/metrics-slo"]
        aware = "mailer/1.20"
    "#;

    #[test]
    fn counts_capabilities_and_slots_separately() {
        let ledger = Ledger::parse(LEDGER).unwrap();
        assert_eq!(ledger.capability_count(), 2);
        assert_eq!(ledger.slot_count(), 3);
        assert_eq!(ledger.positions().len(), 3);
    }

    #[test]
    fn slots_are_looked_up_by_capability_and_depth() {
        let ledger = Ledger::parse(LEDGER).unwrap();
        let id = CapabilityId::parse("async/graceful-shutdown").unwrap();
        assert_eq!(ledger.slot(&id, Level::Apply).unwrap().number, "1.14");
        assert!(ledger.slot(&id, Level::Operate).is_none());
        assert_eq!(ledger.levels(&id).count(), 2);
    }

    #[test]
    fn a_position_must_name_a_track_and_a_phase_index() {
        let bad = r#"
            format_version = 1
            [capability."async/backpressure"]
            aware = "mailer"
        "#;
        let error = Ledger::parse(bad).unwrap_err();
        assert!(
            error.to_string().contains("not a ledger position"),
            "{error}"
        );
    }

    #[test]
    fn an_unknown_depth_is_a_parse_error() {
        let bad = r#"
            format_version = 1
            [capability."async/backpressure"]
            expert = "mailer/1.14"
        "#;
        assert!(Ledger::parse(bad).is_err());
    }

    #[test]
    fn a_capability_without_any_depth_is_rejected() {
        let bad = r#"
            format_version = 1
            [capability."async/backpressure"]
        "#;
        let error = Ledger::parse(bad).unwrap_err();
        assert!(error.to_string().contains("teaches nothing"), "{error}");
    }
}
