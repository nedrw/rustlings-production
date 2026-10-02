//! `clippy@N` — the lint ladder (plan §6).
//!
//! The rungs are data, and the ladder only ever tightens:
//!
//! | rung | added over the previous one |
//! | --- | --- |
//! | `@0` | plain `cargo clippy` |
//! | `@1` | `-D warnings`, `todo`, `dbg_macro`, `print_stdout` in library code |
//! | `@2` | `unwrap_used`, `expect_used`, `indexing_slicing`, `panic` in library code |
//! | `@3` | `missing_errors_doc`, `disallowed_methods`, a pedantic whitelist |
//! | `@4` | no new lints: the slow gates (`mutants`, `deny`, `semver`, `bench-budget`) become must |
//!
//! Test targets are relaxed on purpose (authoring.md §7.4): the lints marked "library only" run
//! against `--lib --bins`, because asserting loudly in a test is how tests are written. Which
//! rung a step uses is metadata, so one rule covers every step instead of per-step `allow`s.

use std::fmt;

use crate::error::{KilnError, Result};
use crate::gates::runner::{GateContext, clip};
use crate::gates::tool::{self, Tool};
use crate::gates::{GateOutcome, GateSpec};
use crate::process;

/// Highest rung this build understands.
pub const MAX_RUNG: u32 = 4;

/// Pedantic lints with a track record of catching real problems rather than taste. The list grows
/// with the course; it is a whitelist precisely so `@3` stays reviewable.
const PEDANTIC_WHITELIST: &[&str] = &[
    "clippy::manual_let_else",
    "clippy::match_wildcard_for_single_variants",
    "clippy::semicolon_if_nothing_returned",
    "clippy::uninlined_format_args",
];

/// One rung of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rung {
    /// Rung number as metadata writes it (`clippy@2`).
    pub level: u32,
}

impl Rung {
    /// Parse the rung from a `clippy@N` parameter.
    pub fn from_spec(spec: &GateSpec) -> Result<Self> {
        let level = spec.numeric_param()?;
        if level > MAX_RUNG {
            return Err(KilnError::Invalid(format!(
                "clippy@{level}: the ladder ends at @{MAX_RUNG}"
            )));
        }
        Ok(Self { level })
    }

    /// Lints applied to every target (`cargo clippy --all-targets`).
    #[must_use]
    pub fn all_targets(self) -> Vec<&'static str> {
        let mut lints = Vec::new();
        if self.level >= 1 {
            lints.extend(["warnings", "clippy::todo", "clippy::dbg_macro"]);
        }
        if self.level >= 3 {
            lints.push("clippy::missing_errors_doc");
            lints.extend(PEDANTIC_WHITELIST);
        }
        lints
    }

    /// Lints that only have to hold for library and binary code, not for tests.
    #[must_use]
    pub fn production_only(self) -> Vec<&'static str> {
        let mut lints = Vec::new();
        if self.level >= 1 {
            lints.push("clippy::print_stdout");
        }
        if self.level >= 2 {
            lints.extend([
                "clippy::unwrap_used",
                "clippy::expect_used",
                "clippy::indexing_slicing",
                "clippy::panic",
            ]);
        }
        if self.level >= 3 {
            // Configured by the project's `clippy.toml` (business code may not read the clock or
            // the environment directly); without that file the lint simply never fires.
            lints.push("clippy::disallowed_methods");
        }
        lints
    }
}

impl fmt::Display for Rung {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.level)
    }
}

/// Run clippy at the rung the step declares.
pub fn run(ctx: &GateContext<'_>, spec: &GateSpec) -> Result<GateOutcome> {
    let rung = Rung::from_spec(spec)?;
    let probe = tool::probe(Tool::CargoClippy, ctx.probe_timeout());
    tool::require(&probe)?;

    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };

    let all_targets = rung.all_targets();
    if !all_targets.is_empty() {
        let args = clippy_args(&["--all-targets"], &all_targets);
        let ran = process::run("cargo", &args, project, ctx.timeout)?;
        if !ran.success() {
            return Ok(GateOutcome::failed(format!(
                "clippy{rung} failed on all targets:\n    {}",
                clip(&ran.stderr, 12)
            )));
        }
    }

    let production = rung.production_only();
    if !production.is_empty() {
        let args = clippy_args(&["--lib", "--bins"], &production);
        let ran = process::run("cargo", &args, project, ctx.timeout)?;
        if !ran.success() {
            return Ok(GateOutcome::failed(format!(
                "clippy{rung} failed for library code (tests are relaxed):\n    {}",
                clip(&ran.stderr, 12)
            )));
        }
    }

    let mut parts = vec![format!("{all_targets:?} lints on all targets").replace('"', "")];
    if !production.is_empty() {
        parts.push(format!("{production:?} on `--lib --bins`").replace('"', ""));
    }
    Ok(GateOutcome::passed(format!(
        "clippy{rung} is clean ({})",
        parts.join("; ")
    )))
}

/// Build `cargo clippy <targets> -- -D <lint> …`.
fn clippy_args<'a>(targets: &[&'a str], lints: &[&'a str]) -> Vec<&'a str> {
    let mut args: Vec<&str> = vec!["clippy"];
    args.extend(targets);
    args.push("--");
    for lint in lints {
        args.push("-D");
        args.push(lint);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::GateSpec;

    fn rung(text: &str) -> Rung {
        Rung::from_spec(&text.parse::<GateSpec>().unwrap()).unwrap()
    }

    #[test]
    fn the_ladder_only_tightens() {
        for level in 0..MAX_RUNG {
            let lower = rung(&format!("clippy@{level}"));
            let higher = rung(&format!("clippy@{}", level + 1));
            for lint in lower.all_targets() {
                assert!(
                    higher.all_targets().contains(&lint),
                    "{lint} disappears at clippy@{}",
                    level + 1
                );
            }
            for lint in lower.production_only() {
                assert!(
                    higher.production_only().contains(&lint),
                    "{lint} disappears at clippy@{}",
                    level + 1
                );
            }
        }
    }

    #[test]
    fn rung_zero_is_plain_clippy() {
        assert!(rung("clippy@0").all_targets().is_empty());
        assert!(rung("clippy@0").production_only().is_empty());
    }

    #[test]
    fn the_relaxation_for_test_targets_is_explicit() {
        // `unwrap` is fine in tests, so it may only be denied where tests are not compiled.
        let level_two = rung("clippy@2");
        assert!(level_two.production_only().contains(&"clippy::unwrap_used"));
        assert!(!level_two.all_targets().contains(&"clippy::unwrap_used"));
    }

    #[test]
    fn rung_four_adds_no_lints_but_names_the_slow_gates() {
        assert_eq!(
            rung("clippy@4").all_targets(),
            rung("clippy@3").all_targets()
        );
        assert_eq!(
            rung("clippy@4").production_only(),
            rung("clippy@3").production_only()
        );
    }

    #[test]
    fn a_rung_beyond_the_ladder_is_rejected() {
        let error = Rung::from_spec(&"clippy@9".parse::<GateSpec>().unwrap()).unwrap_err();
        assert!(
            error.to_string().contains("the ladder ends at @4"),
            "{error}"
        );
        let error = Rung::from_spec(&"clippy".parse::<GateSpec>().unwrap()).unwrap_err();
        assert!(
            error.to_string().contains("needs a numeric parameter"),
            "{error}"
        );
    }

    #[test]
    fn clippy_arguments_are_argv_not_a_shell_string() {
        assert_eq!(
            clippy_args(&["--all-targets"], &["warnings", "clippy::todo"]),
            [
                "clippy",
                "--all-targets",
                "--",
                "-D",
                "warnings",
                "-D",
                "clippy::todo"
            ]
        );
    }
}
