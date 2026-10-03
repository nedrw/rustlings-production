//! `exit-code` — a failure path exits non-zero and explains itself (plan §6).
//!
//! The gate is a subprocess test, not a grep: it builds every binary of the step's project and runs
//! it with one argument no program can understand ([`PROBE`]):
//!
//! * exit **non-zero** — a program that accepts what it does not understand starts with the wrong
//!   settings and never says so;
//! * a message on either stream that **names the argument** it refused — an exit code on its own is
//!   a number a learner has to go looking for;
//! * no panic — a panic is a stack trace, not an error message.
//!
//! Everything the gate cannot judge is reported as *blocked* with an instruction rather than as a
//! bare exit code: a missing `cargo`, a project that does not build, a hang, a target directory
//! nobody can find (authoring.md §7.2).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::{KilnError, Result};
use crate::gates::runner::{GateContext, clip};
use crate::gates::{GateOutcome, GateSpec};
use crate::process::{self, Ran};

/// The argument the gate hands every binary.
///
/// Deliberately not a realistic flag: a program that *implements* this one would hide the failure
/// path the gate is about.
pub const PROBE: &str = "--kiln-exit-code-probe";

/// How stdout/stderr says a Rust program panicked instead of deciding.
const PANIC: &str = "panicked at";

/// One binary target of the learner's project.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bin {
    /// Package that owns it (cargo needs it when two packages share a binary name).
    package: String,
    /// Target name, which is what `--bin` takes.
    target: String,
}

/// What listing the project's binaries found.
#[derive(Debug)]
enum LayoutOutcome {
    /// The binaries to probe and where cargo builds them.
    Known(Layout),
    /// Nothing could be listed, with the reason to report.
    Unknown(String),
}

/// Where the project builds and what it builds.
#[derive(Debug)]
struct Layout {
    /// Directory cargo builds into.
    target_dir: PathBuf,
    /// Binary targets, in package order.
    bins: Vec<Bin>,
}

/// Everything `cargo metadata --no-deps` says that this gate needs.
#[derive(Debug, Deserialize)]
struct Metadata {
    /// Directory cargo builds into.
    target_directory: PathBuf,
    /// Packages of the workspace, in the order cargo lists them.
    packages: Vec<Package>,
}

/// One package of the workspace.
#[derive(Debug, Deserialize)]
struct Package {
    /// Package name.
    name: String,
    /// Its targets.
    targets: Vec<Target>,
}

/// One target of a package.
#[derive(Debug, Deserialize)]
struct Target {
    /// Target name.
    name: String,
    /// Kinds cargo reports (`bin`, `lib`, `test`, …).
    kind: Vec<String>,
}

/// Run the failure path of every binary the project builds.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let Some(project) = ctx.existing_project() else {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    };
    if process::locate("cargo").is_none() {
        return Err(KilnError::MissingTool {
            tool: "cargo".to_owned(),
            hint: "install Rust with rustup (https://rustup.rs), then run `kiln doctor`".to_owned(),
        });
    }

    let layout = match layout(project, ctx.timeout)? {
        LayoutOutcome::Known(layout) => layout,
        LayoutOutcome::Unknown(reason) => return Ok(GateOutcome::blocked(reason)),
    };
    if layout.bins.is_empty() {
        return Ok(GateOutcome::blocked(format!(
            "{} builds no binary, so there is no failure path to judge — `exit-code` belongs on a \
             step that ships something a user starts",
            project.display()
        )));
    }

    let mut probed = Vec::new();
    for bin in &layout.bins {
        match probe(bin, &layout.target_dir, project, ctx.timeout)? {
            None => probed.push(bin.target.clone()),
            Some(outcome) => return Ok(outcome),
        }
    }

    Ok(GateOutcome::passed(format!(
        "{} exits non-zero on `{PROBE}` and names the argument it refused",
        probed.join(", ")
    )))
}

/// Build and run one binary with the probe argument.
///
/// `None` means it behaved; `Some(outcome)` is the report for the learner.
fn probe(
    bin: &Bin,
    target_dir: &Path,
    project: &Path,
    timeout: Duration,
) -> Result<Option<GateOutcome>> {
    let built = process::run(
        "cargo",
        &[
            "build",
            "--quiet",
            "--package",
            &bin.package,
            "--bin",
            &bin.target,
        ],
        project,
        timeout,
    )?;
    if built.timed_out {
        return Ok(Some(GateOutcome::blocked(format!(
            "`cargo build --bin {}` did not finish within {timeout:?}: the failure path cannot be \
             judged yet — build it yourself (`cargo build -p {}`), then re-run `kiln check`",
            bin.target, bin.package
        ))));
    }
    if !built.success() {
        return Ok(Some(GateOutcome::failed(format!(
            "`{}` does not build, so its failure path cannot be judged:\n    {}",
            bin.target,
            clip(&built.stderr, 8)
        ))));
    }

    let Some(executable) = executable(target_dir, &bin.target) else {
        return Ok(Some(GateOutcome::blocked(format!(
            "cargo built {} into a directory kiln cannot read: `kiln` looked for {} — if \
             `CARGO_TARGET_DIR` points elsewhere, unset it or run the project's build by hand",
            bin.target,
            target_dir.display()
        ))));
    };

    let ran = process::run(&executable.to_string_lossy(), &[PROBE], project, timeout)?;
    Ok(Some(judge(&bin.target, &ran)))
}

/// Grade one probe run.
fn judge(target: &str, ran: &Ran) -> GateOutcome {
    let output = format!("{}{}", ran.stdout, ran.stderr);
    if ran.timed_out {
        return GateOutcome::failed(format!(
            "`{target} {PROBE}` was still running after {:?}: an argument the program does not \
             understand must end in a usage error before anything else starts",
            ran.elapsed
        ));
    }
    if ran.success() {
        return GateOutcome::failed(format!(
            "`{target} {PROBE}` exited 0: a program that accepts an argument it does not \
             understand starts with the wrong settings and says nothing"
        ));
    }
    if let Some(line) = output.lines().find(|line| line.contains(PANIC)) {
        return GateOutcome::failed(format!(
            "`{target} {PROBE}` panicked instead of reporting: {} — a panic is a stack trace, not \
             an error message",
            line.trim()
        ));
    }
    if !output.contains(PROBE) {
        return GateOutcome::failed(format!(
            "`{target} {PROBE}` failed with exit code {:?} but never says which argument it \
             refused:\n    {}",
            ran.code,
            clip(&output, 6)
        ));
    }
    GateOutcome::passed(format!(
        "{target} exits non-zero and names the refused argument"
    ))
}

/// List the project's binaries and target directory, via `cargo metadata`.
fn layout(project: &Path, timeout: Duration) -> Result<LayoutOutcome> {
    let ran = process::run(
        "cargo",
        &["metadata", "--no-deps", "--format-version", "1"],
        project,
        timeout,
    )?;
    if ran.timed_out {
        return Ok(LayoutOutcome::Unknown(format!(
            "`cargo metadata` in {} did not finish within {timeout:?}",
            project.display()
        )));
    }
    if !ran.success() {
        return Ok(LayoutOutcome::Unknown(format!(
            "cannot list the binaries of {} — `cargo metadata` failed:\n    {}",
            project.display(),
            clip(&ran.stderr, 6)
        )));
    }
    let (target_dir, bins) = parse_metadata(&ran.stdout)?;
    Ok(LayoutOutcome::Known(Layout { target_dir, bins }))
}
/// Binaries and target directory out of a `cargo metadata` document.
fn parse_metadata(json: &str) -> Result<(PathBuf, Vec<Bin>)> {
    let metadata: Metadata = serde_json::from_str(json).map_err(|source| {
        KilnError::Invalid(format!(
            "`cargo metadata` answered with something this build cannot read: {source}"
        ))
    })?;
    let mut bins = Vec::new();
    for package in &metadata.packages {
        for target in &package.targets {
            if target.kind.iter().any(|kind| kind == "bin") {
                bins.push(Bin {
                    package: package.name.clone(),
                    target: target.name.clone(),
                });
            }
        }
    }
    Ok((metadata.target_directory, bins))
}

/// The executable cargo built for a target name, if it is there.
fn executable(target_dir: &Path, name: &str) -> Option<PathBuf> {
    let candidate = target_dir.join("debug").join(name);
    if candidate.is_file() {
        return Some(candidate);
    }
    let windows = target_dir.join("debug").join(format!("{name}.exe"));
    windows.is_file().then_some(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `cargo metadata` document with the two shapes this gate has to tell apart.
    const METADATA: &str = r#"
    {
      "target_directory": "/tmp/mailer/target",
      "packages": [
        {
          "name": "mailer",
          "targets": [
            { "name": "mailer", "kind": ["bin"] },
            { "name": "mailer", "kind": ["lib"] }
          ]
        },
        {
          "name": "tools",
          "targets": [{ "name": "tools", "kind": ["example"] }]
        }
      ]
    }
    "#;

    fn ran(code: Option<i32>, stdout: &str, stderr: &str) -> Ran {
        Ran {
            code,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            timed_out: false,
            elapsed: Duration::from_millis(5),
        }
    }

    #[test]
    fn only_binaries_are_probed_and_the_package_travels_along() {
        let (target_dir, bins) = parse_metadata(METADATA).expect("the fixture is valid metadata");
        assert_eq!(target_dir, PathBuf::from("/tmp/mailer/target"));
        assert_eq!(
            bins,
            [Bin {
                package: "mailer".to_owned(),
                target: "mailer".to_owned()
            }]
        );
    }

    #[test]
    fn metadata_this_build_cannot_read_is_an_error_not_a_pass() {
        let error = parse_metadata("not json").expect_err("garbage must be rejected");
        assert!(error.to_string().contains("cargo metadata"), "{error}");
    }

    #[test]
    fn a_binary_that_ignores_the_probe_fails_with_the_reason() {
        let outcome = judge("mailer", &ran(Some(0), "nothing to do\n", ""));
        assert_eq!(outcome.outcome, crate::gates::Outcome::Failed);
        assert!(outcome.detail.contains("exited 0"), "{}", outcome.detail);
    }

    #[test]
    fn a_panic_is_not_an_error_message() {
        let outcome = judge(
            "mailer",
            &ran(
                Some(101),
                "",
                "thread 'main' panicked at src/main.rs:4:5:\ncalled `Option::unwrap()`",
            ),
        );
        assert_eq!(outcome.outcome, crate::gates::Outcome::Failed);
        assert!(outcome.detail.contains("panicked"), "{}", outcome.detail);
    }

    #[test]
    fn an_exit_code_without_a_message_is_not_a_readable_failure() {
        let outcome = judge("mailer", &ran(Some(2), "", ""));
        assert_eq!(outcome.outcome, crate::gates::Outcome::Failed);
        assert!(
            outcome.detail.contains("never says which argument"),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn naming_the_argument_it_refused_is_what_passes() {
        let outcome = judge(
            "mailer",
            &ran(
                Some(2),
                "",
                "usage: mailer [OPTIONS]\n\nunknown argument `--kiln-exit-code-probe`",
            ),
        );
        assert_eq!(outcome.outcome, crate::gates::Outcome::Passed);
        assert!(outcome.detail.contains("mailer"), "{}", outcome.detail);
    }
}
