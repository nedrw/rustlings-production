//! Spec for `mailer/1.3-ci`: one command runs every check, and CI runs that command.
//!
//! The contract is the interesting part of this step, so it is *executed* rather than grepped: the
//! spec runs the project's own `cargo xtask ci` twice — once on a clean tree, which must be green,
//! and once with a deliberately broken target in place, which must be red. If the task runs only
//! some of the checks, one of the two runs says so.
//!
//! Two consequences of that, both of them lessons the step is about:
//!
//! * `cargo xtask ci` runs `cargo test`, which runs this spec again. The nested run is detected with
//!   an environment variable and stops immediately — a check that calls itself is a hang, and a
//!   task runner is exactly where that bites;
//! * the broken fixture is added to `tests/` and removed again, so the tree is left as it was found.
//!
//! `kiln run` copies this file into the project as `tests/spec_ci.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Environment variable that marks the nested `cargo test` started by `cargo xtask ci`.
const NESTED: &str = "KILN_CI_NESTED";

/// The fixture that has to make `cargo xtask ci` red, whichever checks it runs.
///
/// It breaks all three at once — a lint, the formatting and a test — because the spec cannot know in
/// which order the task runs them, and a task that runs one of them is a task that has to be told.
const PROBE: &str = r#"
//! Written by tests/spec_ci.rs: this file has to make `cargo xtask ci` fail.
#![allow(dead_code)]

/// Unfinished, denied by the lint policy, and misformatted all at once.
pub  fn unfinished()  ->  u32 { todo!() }

#[test]
fn deliberately_fails() {
    assert!(false, "spec_ci's fixture must make `cargo test` fail");
}
"#;

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run `cargo xtask ci` in the project, marking the nested test run and staying off the network.
fn xtask_ci() -> Output {
    Command::new("cargo")
        .args(["xtask", "ci"])
        .env(NESTED, "1")
        .env("CARGO_NET_OFFLINE", "true")
        .current_dir(project())
        .output()
        .expect("cargo must be on PATH, and `.cargo/config.toml` defines the `xtask` alias")
}

fn said(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    text.lines().take(12).collect::<Vec<_>>().join("\n      ")
}

/// Removes the fixture even when an assertion above it panics.
struct Probe(PathBuf);

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The workflow files of the project; `.github/workflows/` is where GitHub looks.
fn workflows() -> Vec<PathBuf> {
    let dir = project().join(".github/workflows");
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
        panic!(
            "{} must exist and hold the CI definition: a check nobody runs is not a check ({error})",
            dir.display()
        )
    });
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.expect("readable workflow directory").path();
        let extension = path.extension().and_then(std::ffi::OsStr::to_str);
        if matches!(extension, Some("yml" | "yaml")) {
            files.push(path);
        }
    }
    assert!(
        !files.is_empty(),
        "{} holds no `.yml`/`.yaml` workflow",
        dir.display()
    );
    files
}

/// The `cargo …` command lines of a workflow file.
fn cargo_lines(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
    text.lines()
        .map(|line| {
            line.trim()
                .strip_prefix("- run:")
                .unwrap_or(line.trim())
                .trim()
                .to_owned()
        })
        .filter(|line| line.starts_with("cargo "))
        .collect()
}

#[test]
fn the_ci_task_is_green_here_and_red_when_the_tree_is_broken() {
    if std::env::var_os(NESTED).is_some() {
        // We were started by `cargo xtask ci`, which was started by this test: return at once.
        return;
    }

    let clean = xtask_ci();
    assert!(
        clean.status.success(),
        "`cargo xtask ci` must be green on a clean tree, it exited {:?}. A task runner reports the \
         first check that fails, so this is that check: {}",
        clean.status.code(),
        said(&clean)
    );

    let probe = Probe(project().join("tests/kiln_ci_probe.rs"));
    std::fs::write(&probe.0, PROBE)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", probe.0.display()));
    let broken = xtask_ci();
    drop(probe);

    assert!(
        !broken.status.success(),
        "`cargo xtask ci` stayed green with a file that is unformatted, unfinished and fails a test. \
         One command has to run every check CI runs (fmt, clippy, test), or the local run and the CI \
         run disagree: {}",
        said(&broken)
    );
}

#[test]
fn ci_runs_the_same_entry_point_and_nothing_else() {
    let mut calls_xtask = false;
    let mut reimplemented = Vec::new();
    for path in workflows() {
        for line in cargo_lines(&path) {
            if line.starts_with("cargo xtask ci") {
                calls_xtask = true;
            } else {
                reimplemented.push(format!("{} runs `{line}`", path.display()));
            }
        }
    }
    assert!(
        reimplemented.is_empty(),
        "CI has to call the same entry point you run locally (`cargo xtask ci`); a workflow that \
         re-implements the checks is how local-green/CI-red starts: {}",
        reimplemented.join("; ")
    );
    assert!(
        calls_xtask,
        "no workflow runs `cargo xtask ci`: add a job step that calls it"
    );
}
