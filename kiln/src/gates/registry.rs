//! The gate catalogue: every name a `track.toml` may mention, and what runs it.
//!
//! The catalogue is the plan's gate table (§6) written down once. A gate is either *ready* (a
//! function exists) or *pending* (named so metadata can already declare it, with the milestone
//! that brings it). Metadata that names an unknown gate is a mistake and is reported as such —
//! silently ignoring it is how a catalogue rots.

use crate::gates::GateSpec;
use crate::gates::runner::GateContext;
use crate::gates::tool::Tool;

/// Severity of what a gate found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The gate ran and passed.
    Passed,
    /// The gate ran and failed: the step is not done.
    Failed,
    /// The gate could not run — missing tool, or the artifact it judges does not exist yet.
    Blocked,
    /// The gate is catalogued but not implemented yet (never counted as a pass).
    Pending,
}

impl Outcome {
    /// Marker used in reports.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            Outcome::Passed => "✓",
            Outcome::Failed => "✗",
            Outcome::Blocked => "⚠",
            Outcome::Pending => "·",
        }
    }

    /// One word for machine-readable output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::Blocked => "blocked",
            Outcome::Pending => "pending",
        }
    }
}

/// What one gate reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    /// Severity of the result.
    pub outcome: Outcome,
    /// One line explaining it, phrased so it can be acted on.
    pub detail: String,
}

impl GateOutcome {
    /// The gate passed.
    #[must_use]
    pub fn passed(detail: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Passed,
            detail: detail.into(),
        }
    }

    /// The gate ran and failed.
    #[must_use]
    pub fn failed(detail: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Failed,
            detail: detail.into(),
        }
    }

    /// The gate could not run for a reason outside the learner's code.
    #[must_use]
    pub fn blocked(detail: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Blocked,
            detail: detail.into(),
        }
    }
}

/// Whether a catalogued gate can run yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateStatus {
    /// Implemented.
    Ready,
    /// Named in metadata already; this says which milestone brings it.
    Pending(&'static str),
}

/// Implementation of a gate: one function, no state.
pub type GateFn = fn(&GateContext<'_>, &GateSpec) -> crate::error::Result<GateOutcome>;

/// One entry of the catalogue.
#[derive(Debug, Clone, Copy)]
pub struct GateDef {
    /// Gate name as `track.toml` writes it.
    pub name: &'static str,
    /// Tool the gate needs, probed before it runs.
    pub tool: Option<Tool>,
    /// Why the gate exists, for `kiln check` and the docs.
    pub purpose: &'static str,
    /// Implementation, when there is one.
    pub run: Option<GateFn>,
    /// Whether it can run today.
    pub status: GateStatus,
}

/// Every gate the course knows about, in the order `kiln check` reports them.
static CATALOGUE: &[GateDef] = &[
    GateDef {
        name: "spec-integrity",
        tool: None,
        purpose: "the spec files are the ones the course shipped (plan §3 D2)",
        run: Some(crate::gates::spec_integrity::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "caps",
        tool: None,
        purpose: "the step's capability declarations pass lint-caps",
        run: Some(crate::gates::caps::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "fmt",
        tool: Some(Tool::CargoFmt),
        purpose: "formatting is uniform and nobody argues about it in review",
        run: Some(crate::gates::fmt::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "clippy",
        tool: Some(Tool::CargoClippy),
        purpose: "the lint ladder, which only ever tightens (plan §6)",
        run: Some(crate::gates::clippy::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "no-todo",
        tool: Some(Tool::CargoClippy),
        purpose: "no `todo!`/`unimplemented!`/`dbg!` left behind",
        run: Some(crate::gates::no_todo::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "docs:public",
        tool: Some(Tool::CargoClippy),
        purpose: "public items carry documentation and error sections",
        run: Some(crate::gates::docs_public::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "exit-code",
        tool: None,
        purpose: "failure paths exit non-zero and explain themselves",
        run: Some(crate::gates::exit_code::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "i18n",
        tool: None,
        purpose: "translations keep the structure and hash of their source",
        run: Some(crate::gates::i18n::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "images",
        tool: None,
        purpose: "the images a step needs are already pulled",
        // The engine is probed by the gate itself (it is not a cargo tool), so `tool` stays empty
        // and a missing engine is reported as `blocked` rather than as a missing tool.
        run: Some(crate::gates::images::run),
        status: GateStatus::Ready,
    },
    GateDef {
        name: "container-repro",
        tool: None,
        purpose: "the image runs the same binary and fits its budget",
        run: None,
        status: GateStatus::Pending("arrives with T1 step 21's content"),
    },
    GateDef {
        name: "links",
        tool: None,
        purpose: "documentation links resolve",
        run: None,
        status: GateStatus::Pending("needs `lychee`; arrives with T1 step 22's content"),
    },
    GateDef {
        name: "timeout",
        tool: None,
        purpose: "every command the runner starts has a deadline",
        run: None,
        status: GateStatus::Pending(
            "enforced by the runner itself (`process::run`), not by a per-step gate",
        ),
    },
    GateDef {
        name: "conformance",
        tool: None,
        purpose: "one suite runs against every implementation of a port",
        run: None,
        status: GateStatus::Pending("T3"),
    },
    GateDef {
        name: "mutants",
        tool: Some(Tool::CargoMutants),
        purpose: "the specs are not vacuous",
        run: None,
        status: GateStatus::Pending("T3"),
    },
    GateDef {
        name: "coverage",
        tool: Some(Tool::CargoLlvmCov),
        purpose: "error branches are exercised",
        run: None,
        status: GateStatus::Pending("T3"),
    },
    GateDef {
        name: "bench-budget",
        tool: None,
        purpose: "performance stays within a budget of the baseline",
        run: None,
        status: GateStatus::Pending("T3"),
    },
    GateDef {
        name: "deny",
        tool: Some(Tool::CargoDeny),
        purpose: "licences, versions and sources follow policy",
        run: None,
        status: GateStatus::Pending("T2"),
    },
    GateDef {
        name: "unsafe-budget",
        tool: None,
        purpose: "no unexplained `unsafe`",
        run: None,
        status: GateStatus::Pending("T3"),
    },
    GateDef {
        name: "semver",
        tool: Some(Tool::CargoSemverChecks),
        purpose: "the public API stays compatible with its baseline",
        run: None,
        status: GateStatus::Pending("T3"),
    },
    GateDef {
        name: "sim",
        tool: None,
        purpose: "distributed behaviour is deterministic under a seed",
        run: None,
        status: GateStatus::Pending("T4/T5"),
    },
    GateDef {
        name: "soak",
        tool: None,
        purpose: "no leaks or pile-ups over a long run",
        run: None,
        status: GateStatus::Pending("T4"),
    },
];

/// Every catalogued gate.
#[must_use]
pub fn catalogue() -> &'static [GateDef] {
    CATALOGUE
}

/// Look a gate up by name.
#[must_use]
pub fn lookup(name: &str) -> Option<&'static GateDef> {
    CATALOGUE.iter().find(|gate| gate.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_unique_and_complete() {
        let mut names: Vec<&str> = CATALOGUE.iter().map(|gate| gate.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate gate name in the catalogue");

        for gate in CATALOGUE {
            assert!(!gate.purpose.is_empty(), "{} has no purpose", gate.name);
            match (gate.status, gate.run) {
                (GateStatus::Ready, Some(_)) => {}
                (GateStatus::Pending(reason), None) => {
                    assert!(
                        !reason.is_empty(),
                        "{} is pending without a reason",
                        gate.name
                    );
                }
                (status, run) => panic!("{}: status {status:?} contradicts run {run:?}", gate.name),
            }
        }
    }

    #[test]
    fn every_gate_a_mailer_step_mentions_exists() {
        // The three working gates plus the ones the T1 metadata already declares.
        for name in [
            "spec-integrity",
            "caps",
            "i18n",
            "fmt",
            "clippy",
            "exit-code",
            "no-todo",
            "docs:public",
            "images",
            "container-repro",
            "links",
        ] {
            assert!(lookup(name).is_some(), "`{name}` is used by track metadata");
        }
        assert!(lookup("nonsense").is_none());
    }

    #[test]
    fn every_ready_gate_has_an_implementation_and_a_name_a_step_can_use() {
        let ready: Vec<&str> = CATALOGUE
            .iter()
            .filter(|gate| gate.status == GateStatus::Ready)
            .map(|gate| gate.name)
            .collect();
        // The gates a T1 step declares must be runnable, and the ones that need a tool must say
        // which one: that pairing is what makes `kiln doctor` able to explain a blocked gate.
        // `exit-code` arrives with T1 step 4's content, `docs:public` with step 5's and `images`
        // with step 9's; the remaining names a step mentions (`container-repro`, `links`) still say
        // which milestone brings them.
        assert_eq!(
            ready,
            [
                "spec-integrity",
                "caps",
                "fmt",
                "clippy",
                "no-todo",
                "docs:public",
                "exit-code",
                "i18n",
                "images",
            ],
            "a gate moved between ready and pending"
        );
        for gate in CATALOGUE
            .iter()
            .filter(|gate| gate.status == GateStatus::Ready)
        {
            assert!(
                gate.run.is_some(),
                "{} is ready without a function",
                gate.name
            );
        }
    }
}
