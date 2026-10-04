//! Probing the external tools the course depends on.
//!
//! Every gate asks this module first (plan §10, authoring.md §7): a missing tool must produce a
//! copy-pasteable install command, never a raw exit code. The same list backs `kiln doctor`, so
//! "what is missing on this machine" has exactly one answer.

use std::time::Duration;

use crate::error::{KilnError, Result};
use crate::process;

/// A tool outside this repository that a gate, a track or `doctor` may need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tool {
    /// `cargo fmt`, from the `rustfmt` component.
    CargoFmt,
    /// `cargo clippy`, from the `clippy` component.
    CargoClippy,
    /// `cargo watch`, used by `kiln watch` (M0 chunk 2).
    CargoWatch,
    /// `cargo expand`, handy when a macro obscures the types.
    CargoExpand,
    /// `cargo nextest`, the faster runner used by later tracks.
    CargoNextest,
    /// `cargo llvm-cov`, the coverage gate of T3.
    CargoLlvmCov,
    /// `cargo mutants`, the mutation gate of T3.
    CargoMutants,
    /// `cargo deny`, the supply-chain policy of T2.
    CargoDeny,
    /// `cargo semver-checks`, the public API baseline of T3.
    CargoSemverChecks,
    /// `cargo fuzz`, the parser gate of T4.
    CargoFuzz,
}

impl Tool {
    /// Name a learner types.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Tool::CargoFmt => "cargo fmt",
            Tool::CargoClippy => "cargo clippy",
            Tool::CargoWatch => "cargo watch",
            Tool::CargoExpand => "cargo expand",
            Tool::CargoNextest => "cargo nextest",
            Tool::CargoLlvmCov => "cargo llvm-cov",
            Tool::CargoMutants => "cargo mutants",
            Tool::CargoDeny => "cargo deny",
            Tool::CargoSemverChecks => "cargo semver-checks",
            Tool::CargoFuzz => "cargo fuzz",
        }
    }

    /// Command that identifies the tool and prints its version.
    #[must_use]
    pub fn argv(self) -> &'static [&'static str] {
        match self {
            Tool::CargoFmt => &["cargo", "fmt", "--version"],
            Tool::CargoClippy => &["cargo", "clippy", "--version"],
            Tool::CargoWatch => &["cargo", "watch", "--version"],
            Tool::CargoExpand => &["cargo", "expand", "--version"],
            Tool::CargoNextest => &["cargo", "nextest", "--version"],
            Tool::CargoLlvmCov => &["cargo", "llvm-cov", "--version"],
            Tool::CargoMutants => &["cargo", "mutants", "--version"],
            Tool::CargoDeny => &["cargo", "deny", "--version"],
            Tool::CargoSemverChecks => &["cargo", "semver-checks", "--version"],
            Tool::CargoFuzz => &["cargo", "fuzz", "--version"],
        }
    }

    /// Copy-pasteable way to get the tool.
    #[must_use]
    pub fn install_hint(self) -> &'static str {
        match self {
            Tool::CargoFmt => "rustup component add rustfmt",
            Tool::CargoClippy => "rustup component add clippy",
            Tool::CargoWatch => "cargo install cargo-watch --locked",
            Tool::CargoExpand => "cargo install cargo-expand --locked",
            Tool::CargoNextest => "cargo install cargo-nextest --locked",
            Tool::CargoLlvmCov => "cargo install cargo-llvm-cov --locked",
            Tool::CargoMutants => "cargo install cargo-mutants --locked",
            Tool::CargoDeny => "cargo install cargo-deny --locked",
            Tool::CargoSemverChecks => "cargo install cargo-semver-checks --locked",
            Tool::CargoFuzz => "cargo install cargo-fuzz --locked",
        }
    }

    /// Why the course wants it, for `kiln doctor`.
    ///
    /// The three tools the slow gates were once written around say what they are *now*: a tool that a
    /// gate no longer needs is still worth reporting, but `doctor` may not claim a step depends on it
    /// (handoff M2 §1 — a `must` gate is judged by `cargo`, `rustc` and the course's own files).
    #[must_use]
    pub fn purpose(self) -> &'static str {
        match self {
            Tool::CargoFmt => "the `fmt` gate from T1 step 2",
            Tool::CargoClippy => "the `clippy@N` and `no-todo` gates",
            Tool::CargoWatch => "`kiln watch`",
            Tool::CargoExpand => "inspecting macro output while debugging",
            Tool::CargoNextest => "the faster test runner used from T3 on",
            Tool::CargoLlvmCov => {
                "measuring coverage outside the course: the `coverage` gate uses the toolchain's own \
                 `llvm-tools-preview` component"
            }
            Tool::CargoMutants => {
                "mutation testing outside the course: the `mutants` gate is implemented in `kiln` and \
                 needs no tool"
            }
            Tool::CargoDeny => "hand-checking licences and sources: `deny` stays a `should` gate",
            Tool::CargoSemverChecks => {
                "checking a public API outside the course: the `semver` gate builds the course's own \
                 witness with `cargo test`"
            }
            Tool::CargoFuzz => "the `fuzz` work of T4",
        }
    }

    /// Every tool this module knows about, in the order `doctor` prints them.
    #[must_use]
    pub fn all() -> &'static [Tool] {
        &[
            Tool::CargoFmt,
            Tool::CargoClippy,
            Tool::CargoWatch,
            Tool::CargoExpand,
            Tool::CargoNextest,
            Tool::CargoLlvmCov,
            Tool::CargoMutants,
            Tool::CargoDeny,
            Tool::CargoSemverChecks,
            Tool::CargoFuzz,
        ]
    }
}

/// What probing one tool found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// Tool that was probed.
    pub tool: Tool,
    /// `true` when its identifying command succeeded.
    pub available: bool,
    /// Version line, when the tool answered.
    pub version: Option<String>,
    /// Why it is not usable, when it is not.
    pub detail: Option<String>,
}

impl Probe {
    /// `✓ cargo clippy 0.1.98` or `✗ cargo mutants — cargo install cargo-mutants --locked`.
    #[must_use]
    pub fn line(&self) -> String {
        let name = self.tool.name();
        if self.available {
            let version = self.version.as_deref().unwrap_or("");
            return format!("✓ {name} {version}").trim_end().to_owned();
        }
        match &self.detail {
            Some(detail) => format!("✗ {name} — {detail} ({})", self.tool.install_hint()),
            None => format!("✗ {name} — {}", self.tool.install_hint()),
        }
    }
}

/// Probe one tool by running its identifying command.
pub fn probe(tool: Tool, timeout: Duration) -> Probe {
    let dir = std::env::temp_dir();
    let argv = tool.argv();
    let Some((program, args)) = argv.split_first() else {
        return Probe {
            tool,
            available: false,
            version: None,
            detail: Some("no probe command is defined".to_owned()),
        };
    };

    match process::run(program, args, &dir, timeout) {
        Ok(ran) if ran.success() => {
            let version = ran
                .stdout
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.chars().take(80).collect::<String>());
            Probe {
                tool,
                available: true,
                version,
                detail: None,
            }
        }
        Ok(ran) => Probe {
            tool,
            available: false,
            version: None,
            detail: Some(ran.summary()),
        },
        Err(error) => Probe {
            tool,
            available: false,
            version: None,
            detail: Some(error.to_string()),
        },
    }
}

/// Probe several tools.
#[must_use]
pub fn probe_all(tools: &[Tool], timeout: Duration) -> Vec<Probe> {
    tools.iter().map(|tool| probe(*tool, timeout)).collect()
}

/// Turn a failed probe into the gate's actionable error.
pub fn require(probe: &Probe) -> Result<()> {
    if probe.available {
        return Ok(());
    }
    Err(KilnError::MissingTool {
        tool: probe.tool.name().to_owned(),
        hint: probe.tool.install_hint().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_complete_and_documented() {
        for tool in Tool::all() {
            assert!(!tool.name().is_empty());
            assert!(
                !tool.install_hint().is_empty(),
                "{} has no install hint",
                tool.name()
            );
            assert!(
                tool.argv().len() >= 2,
                "{} must be probed with a subcommand",
                tool.name()
            );
            assert!(!tool.purpose().is_empty());
        }
    }

    /// `cargo` itself is always present in a Rust build, so it stands in for "a tool that is
    /// there" — the probe must report its version rather than just success.
    #[cfg(unix)]
    #[test]
    fn probing_reports_the_version_of_an_installed_tool() {
        let probe = probe(Tool::CargoFmt, Duration::from_secs(30));
        assert!(probe.available, "{:?}", probe.detail);
        assert!(probe.line().starts_with("✓ cargo fmt"), "{}", probe.line());
    }

    #[test]
    fn a_missing_tool_reports_its_install_command() {
        let probe = Probe {
            tool: Tool::CargoMutants,
            available: false,
            version: None,
            detail: Some("no such command".to_owned()),
        };
        assert!(
            probe
                .line()
                .contains("cargo install cargo-mutants --locked")
        );
        let error = require(&probe).unwrap_err();
        assert!(matches!(error, KilnError::MissingTool { .. }), "{error}");
        assert!(
            error.to_string().contains("cargo install cargo-mutants"),
            "{error}"
        );
    }

    #[test]
    fn a_present_tool_passes_the_requirement() {
        let probe = Probe {
            tool: Tool::CargoClippy,
            available: true,
            version: Some("clippy 0.1.98".to_owned()),
            detail: None,
        };
        assert!(require(&probe).is_ok());
        assert_eq!(probe.line(), "✓ cargo clippy clippy 0.1.98");
    }
}
