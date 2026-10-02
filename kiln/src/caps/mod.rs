//! The capability model: what the course claims to teach, at which depth (plan §3 D4).
//!
//! Capabilities are the currency of the course. A step declares `teaches = ["cap@depth"]`, and
//! the ledger in `course/capabilities.toml` records the single position where each
//! (capability, depth) pair is established. Nothing here talks to the file system; that is
//! [`ledger`]'s job.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::KilnError;

pub mod ledger;

pub use ledger::Ledger;

/// Depth at which a capability is established (curriculum.md §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Read and modify existing code, and know why it exists.
    Aware,
    /// Implement it from scratch without guidance.
    Apply,
    /// Explain the trade-offs, choose, and write the decision record.
    Design,
    /// Debug, tune and drill it in production.
    Operate,
}

impl Level {
    /// The four depths, shallowest first — the column order of the ledger.
    pub const ALL: [Level; 4] = [Level::Aware, Level::Apply, Level::Design, Level::Operate];

    /// Rank used to compare depths: `aware` = 0 … `operate` = 3.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Level::Aware => 0,
            Level::Apply => 1,
            Level::Design => 2,
            Level::Operate => 3,
        }
    }

    /// Lower-case name as it appears in metadata.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Aware => "aware",
            Level::Apply => "apply",
            Level::Design => "design",
            Level::Operate => "operate",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Capability id, `<domain>/<name>` in lower case (`async/graceful-shutdown`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Validate and wrap an id (authoring.md §1: lower case, hyphenated, one slash).
    pub fn parse(text: &str) -> Result<Self, KilnError> {
        let invalid = || {
            KilnError::Invalid(format!(
                "`{text}` is not a capability id: expected `<domain>/<name>`, lower case with hyphens"
            ))
        };
        let (domain, name) = text.split_once('/').ok_or_else(invalid)?;
        if !is_slug(domain) || !is_slug(name) {
            return Err(invalid());
        }
        Ok(Self(text.to_owned()))
    }

    /// Id as written in metadata.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Domain part, the substring before the slash (`async`).
    #[must_use]
    pub fn domain(&self) -> &str {
        self.0
            .split_once('/')
            .map_or(self.0.as_str(), |(domain, _)| domain)
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CapabilityId {
    type Err = KilnError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// A capability at a depth, as written in `teaches` / `requires` / `exit`:
/// `async/graceful-shutdown@apply`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct CapRequirement {
    /// Capability being established.
    pub id: CapabilityId,
    /// Depth it is established at.
    pub level: Level,
}

impl CapRequirement {
    /// Build a requirement from its two parts.
    #[must_use]
    pub fn new(id: CapabilityId, level: Level) -> Self {
        Self { id, level }
    }
}

impl fmt::Display for CapRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.id, self.level)
    }
}

impl FromStr for CapRequirement {
    type Err = KilnError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (id, level) = text.split_once('@').ok_or_else(|| {
            KilnError::Invalid(format!(
                "`{text}` is missing a depth: expected `<capability>@<aware|apply|design|operate>`"
            ))
        })?;
        let level = match level {
            "aware" => Level::Aware,
            "apply" => Level::Apply,
            "design" => Level::Design,
            "operate" => Level::Operate,
            other => {
                return Err(KilnError::Invalid(format!(
                    "`{other}` is not a depth: expected aware, apply, design or operate"
                )));
            }
        };
        Ok(Self {
            id: CapabilityId::parse(id)?,
            level,
        })
    }
}

impl TryFrom<String> for CapRequirement {
    type Error = KilnError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<CapRequirement> for String {
    fn from(requirement: CapRequirement) -> Self {
        requirement.to_string()
    }
}

impl TryFrom<String> for CapabilityId {
    type Error = KilnError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<CapabilityId> for String {
    fn from(id: CapabilityId) -> Self {
        id.0
    }
}

/// `true` for a lower-case hyphenated slug with no separators of its own.
fn is_slug(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered_shallowest_first() {
        assert!(Level::Aware < Level::Apply);
        assert!(Level::Apply < Level::Design);
        assert!(Level::Design < Level::Operate);
        assert_eq!(Level::ALL.map(Level::rank), [0, 1, 2, 3]);
    }

    #[test]
    fn requirements_round_trip_through_their_text_form() {
        let requirement: CapRequirement = "async/graceful-shutdown@operate".parse().unwrap();
        assert_eq!(requirement.id.as_str(), "async/graceful-shutdown");
        assert_eq!(requirement.level, Level::Operate);
        assert_eq!(requirement.to_string(), "async/graceful-shutdown@operate");
    }

    #[test]
    fn malformed_requirements_are_rejected_with_a_hint() {
        let missing_depth = "async/graceful-shutdown"
            .parse::<CapRequirement>()
            .unwrap_err();
        assert!(missing_depth.to_string().contains("missing a depth"));

        let bad_depth = "async/graceful-shutdown@expert"
            .parse::<CapRequirement>()
            .unwrap_err();
        assert!(bad_depth.to_string().contains("not a depth"));

        for bad in [
            "graceful-shutdown",
            "async//x",
            "Async/X",
            "async/graceful shutdown",
        ] {
            assert!(
                CapabilityId::parse(bad).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn domain_is_available_for_grouping() {
        let id = CapabilityId::parse("obs/metrics-slo").unwrap();
        assert_eq!(id.domain(), "obs");
    }
}
