//! The gate catalogue and its string DSL.
//!
//! A step lists gates as strings (plan §4, authoring.md §7):
//!
//! ```text
//! spec-integrity      must, no parameter
//! clippy@1            must, parameter `1` (the lint ladder rung)
//! mutants@80          must, parameter `80` (a threshold)
//! docs:public         must, namespaced name - the `:` is *not* a parameter here
//! ?pedantic           should: reported, never blocking
//! slow:mutants        slow: only run by `kiln verify --slow` and `selftest`
//! ```
//!
//! Grammar, applied in this order:
//!
//! 1. an optional `slow:` prefix,
//! 2. an optional `?` marking the gate as *should*,
//! 3. a name, then an optional parameter after `@`, or after `:` **when it is numeric** —
//!    that last rule is what keeps namespaced names such as `docs:public` readable.
//!
//! This module owns the grammar and the catalogue; running a gate belongs to the modules that
//! implement one ([`fmt`], [`clippy`], [`mutants`], [`budget`] …), the little bit of source reading
//! the textual gates share lives in [`lexer`] and [`sources`], the counter harnesses the slow gates
//! judge (their line format, their course-side parameter files) share [`harness`], the copy of the
//! project a gate that builds in one starts from lives in [`sandbox`], and probing a tool belongs to
//! [`tool`] — which now answers for the few tools a gate still needs, since the slow gates judge with
//! `cargo`, `rustc` and the course's own files (handoff M2 §1). `kiln lint-caps` solves the same
//! problem for capability metadata instead of code.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::KilnError;

pub mod budget;
pub mod caps;
pub mod clippy;
pub mod conformance;
pub mod container_repro;
pub mod coverage;
pub mod docs_public;
pub mod exit_code;
pub mod fmt;
pub mod fuzz;
pub mod harness;
pub mod i18n;
pub mod images;
pub mod lexer;
pub mod links;
pub mod mutants;
pub mod no_todo;
pub mod registry;
pub mod runner;
pub mod sandbox;
pub mod semver;
pub mod sim;
pub mod soak;
pub mod sources;
pub mod spec_integrity;
pub mod tool;
pub mod unsafe_budget;

pub use registry::{GateDef, GateOutcome, GateStatus, Outcome, catalogue, lookup};
pub use runner::{GateContext, GateResult, all_must_passed, run_all};

/// Whether a failing gate blocks the step (plan §6: `must` versus `should`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The step is not done until this passes (the default).
    Must,
    /// Advisory: reported as a warning, never blocking (`?name`).
    Should,
}

/// One entry of a step's `gates` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSpec {
    /// Gate name — the key the registry is looked up by.
    pub name: String,
    /// Parameter after `@` (or after a numeric `:`), e.g. the rung of `clippy@1`.
    pub param: Option<String>,
    /// Must or should.
    pub tier: Tier,
    /// Only run by the slow path (`slow:name`).
    pub slow: bool,
}

impl GateSpec {
    /// Build a plain must gate, used by the registry for gates with a fixed shape.
    #[must_use]
    pub fn must(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            param: None,
            tier: Tier::Must,
            slow: false,
        }
    }

    /// Parameter as written, if any.
    #[must_use]
    pub fn param(&self) -> Option<&str> {
        self.param.as_deref()
    }

    /// Parameter as a number, or an actionable error naming the gate.
    pub fn numeric_param(&self) -> Result<u32, KilnError> {
        match self.param.as_deref() {
            None => Err(KilnError::Invalid(format!(
                "gate `{}` needs a numeric parameter, e.g. `{}@1`",
                self.name, self.name
            ))),
            Some(text) => text.parse::<u32>().map_err(|_| {
                KilnError::Invalid(format!(
                    "gate `{}`: `{text}` is not a number, e.g. `{}@1`",
                    self.name, self.name
                ))
            }),
        }
    }
}

impl std::fmt::Display for GateSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.slow {
            f.write_str("slow:")?;
        }
        if self.tier == Tier::Should {
            f.write_str("?")?;
        }
        f.write_str(&self.name)?;
        if let Some(param) = &self.param {
            // Canonical form always uses `@`; `name:80` is accepted as an alias when parsing.
            write!(f, "@{param}")?;
        }
        Ok(())
    }
}

impl FromStr for GateSpec {
    type Err = KilnError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid =
            |reason: &str| KilnError::Invalid(format!("`{text}` is not a gate: {reason}"));

        let rest = text.trim();
        let (slow, rest) = match rest.strip_prefix("slow:") {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let (tier, rest) = match rest.strip_prefix('?') {
            Some(rest) => (Tier::Should, rest),
            None => (Tier::Must, rest),
        };

        let (name, param) = match split_param(rest) {
            Some((name, param)) => (name, Some(param)),
            None => (rest, None),
        };
        if name.is_empty() {
            return Err(invalid("it has no name"));
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b':')
        {
            return Err(invalid(
                "names are lower case; parameters go after `@` (or a numeric `:`)",
            ));
        }
        if param.is_some_and(str::is_empty) {
            return Err(invalid("it has an empty parameter"));
        }
        if let Some(param) = param
            && !param
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(invalid(
                "parameters are alphanumeric; quoting is never needed (and never parsed)",
            ));
        }

        Ok(Self {
            name: name.to_owned(),
            param: param.map(str::to_owned),
            tier,
            slow,
        })
    }
}

/// Split `name@param`, or `name:param` when the parameter is numeric.
fn split_param(text: &str) -> Option<(&str, &str)> {
    if let Some((name, param)) = text.split_once('@') {
        return Some((name, param));
    }
    let (name, param) = text.split_once(':')?;
    let numeric = !param.is_empty() && param.bytes().all(|b| b.is_ascii_digit());
    numeric.then_some((name, param))
}

impl TryFrom<String> for GateSpec {
    type Error = KilnError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl Serialize for GateSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for GateSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> GateSpec {
        text.parse().expect("gate should parse")
    }

    #[test]
    fn a_bare_name_is_a_must_gate() {
        let gate = parsed("spec-integrity");
        assert_eq!(gate.name, "spec-integrity");
        assert_eq!(gate.tier, Tier::Must);
        assert!(!gate.slow);
        assert_eq!(gate.param(), None);
    }

    #[test]
    fn a_question_mark_makes_it_advisory() {
        assert_eq!(parsed("?pedantic").tier, Tier::Should);
    }

    #[test]
    fn the_ladder_rung_uses_the_at_sign() {
        let gate = parsed("clippy@2");
        assert_eq!(gate.name, "clippy");
        assert_eq!(gate.numeric_param().unwrap(), 2);
    }

    #[test]
    fn a_numeric_colon_is_a_parameter() {
        let gate = parsed("mutants:80");
        assert_eq!(gate.name, "mutants");
        assert_eq!(gate.numeric_param().unwrap(), 80);
    }

    #[test]
    fn a_namespaced_name_keeps_its_colon() {
        let gate = parsed("docs:public");
        assert_eq!(gate.name, "docs:public");
        assert_eq!(gate.param(), None);
    }

    #[test]
    fn a_symbolic_parameter_is_kept_as_written() {
        let gate = parsed("sim@seed");
        assert_eq!(gate.name, "sim");
        assert_eq!(gate.param(), Some("seed"));
    }

    #[test]
    fn slow_and_should_combine() {
        let gate = parsed("slow:?mutants@80");
        assert!(gate.slow);
        assert_eq!(gate.tier, Tier::Should);
        assert_eq!(gate.name, "mutants");
        assert_eq!(gate.param(), Some("80"));
    }

    #[test]
    fn specs_round_trip_through_their_canonical_text_form() {
        for text in [
            "spec-integrity",
            "?pedantic",
            "clippy@2",
            "mutants@80",
            "docs:public",
            "slow:mutants",
        ] {
            assert_eq!(parsed(text).to_string(), text);
        }
    }

    #[test]
    fn a_numeric_colon_is_accepted_and_normalised_to_the_at_sign() {
        assert_eq!(parsed("mutants:80").to_string(), "mutants@80");
        assert_eq!(parsed("mutants:80"), parsed("mutants@80"));
    }

    #[test]
    fn malformed_specs_are_rejected() {
        for bad in [
            "",
            "?",
            "slow:",
            "clippy@",
            "UPPER",
            "clippy@1@2",
            "clippy@a b",
        ] {
            assert!(bad.parse::<GateSpec>().is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn a_gate_without_a_parameter_says_so_when_one_is_required() {
        let error = parsed("clippy").numeric_param().unwrap_err();
        assert!(
            error.to_string().contains("needs a numeric parameter"),
            "{error}"
        );
    }
}
