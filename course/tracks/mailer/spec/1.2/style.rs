//! Spec for `mailer/1.2-toolchain`: the toolchain, the MSRV and the lint policy are pinned in the
//! repository — and the policy actually bites.
//!
//! The last assertion is the interesting one. A `[workspace.lints]` table that nothing ever observes
//! is decoration; this compiles a fixture that trips a denied lint and requires the build to fail,
//! which proves the table is inherited by a real target of this crate (authoring.md §7.4: no `allow`
//! in the code, the rung in the ladder decides).
//!
//! `kiln run` copies this file into the project as `tests/spec_style.rs`.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The fixture that must trip the policy. Written into `tests/` for the length of one test.
const PROBE: &str = "\
//! Written by tests/spec_style.rs: this file has to trip a lint the policy denies.
#![allow(dead_code)]

/// Unfinished work, denied by `[workspace.lints.clippy]`.
pub fn unfinished() -> u32 {
    todo!()
}
";

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read a file of the project, saying which one is missing when it is.
fn read(relative: &str) -> String {
    let path = project().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{relative} must exist: the toolchain, the style settings and the lint policy live in \
             the repository, not in a reviewer's head ({error})"
        )
    })
}

/// The `key = "value"` line under `[table]` in a TOML file, if there is one.
///
/// A forty-line TOML parser is not what this step teaches, so this reads exactly the shape the
/// settings have and reports `None` for everything else.
fn toml_string(text: &str, table: &str, key: &str) -> Option<String> {
    let wanted = format!("[{table}]");
    let mut inside = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            inside = line == wanted;
            continue;
        }
        if !inside {
            continue;
        }
        if let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

fn cargo(args: &[&str]) -> Output {
    Command::new("cargo")
        .args(args)
        .env("CARGO_NET_OFFLINE", "true")
        .current_dir(project())
        .output()
        .expect("cargo must be on PATH; rust-toolchain.toml pins which one")
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

#[test]
fn the_toolchain_is_pinned_and_the_compiler_agrees() {
    let text = read("rust-toolchain.toml");
    let channel = toml_string(&text, "toolchain", "channel").unwrap_or_else(|| {
        panic!("rust-toolchain.toml must set `channel` under `[toolchain]`, e.g. 1.98.1")
    });
    let concrete = channel.split('.').count() == 3
        && channel
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    assert!(
        concrete,
        "`channel = \"{channel}\"` is not a pinned version. `stable` and `nightly` move under \
         your feet; a production service names the version it was built with."
    );

    for component in ["rustfmt", "clippy"] {
        assert!(
            text.contains(component),
            "the pinned toolchain must provide `{component}`, because the `fmt` and `clippy` gates \
             run it (rustup component add {component})"
        );
    }

    let output = Command::new("rustc")
        .arg("--version")
        .current_dir(project())
        .output()
        .expect("rustc must be on PATH");
    let reported = String::from_utf8_lossy(&output.stdout);
    let running = reported
        .split_whitespace()
        .nth(1)
        .unwrap_or("unknown")
        .to_owned();
    assert_eq!(
        running, channel,
        "rust-toolchain.toml pins {channel} but the compiler that runs this test is {running}: \
         the pin only bites through rustup (`rustup show`, `rustup toolchain install {channel}`)"
    );
}

#[test]
fn the_msrv_is_declared_and_the_pin_satisfies_it() {
    let channel = toml_string(&read("rust-toolchain.toml"), "toolchain", "channel")
        .expect("`channel` must be set in rust-toolchain.toml");
    let msrv = toml_string(&read("Cargo.toml"), "package", "rust-version").unwrap_or_else(|| {
        panic!(
            "Cargo.toml must declare `rust-version` under `[package]`: without it, a dependency \
             bump can quietly raise the compiler a user needs"
        )
    });
    assert!(
        channel.starts_with(&msrv),
        "the MSRV policy says the pinned toolchain has to satisfy the declared minimum, but \
         `rust-version = \"{msrv}\"` is newer than the pinned {channel}"
    );
    assert!(
        msrv.split('.').count() >= 2,
        "`rust-version = \"{msrv}\"` must name at least a major and a minor version"
    );
}

#[test]
fn the_lint_policy_bites() {
    let probe = Probe(project().join("tests/kiln_lint_probe.rs"));
    std::fs::write(&probe.0, PROBE)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", probe.0.display()));

    let output = cargo(&["clippy", "--tests"]);
    drop(probe);

    assert!(
        !output.status.success(),
        "the lint policy does not bite: a target containing `todo!()` compiled cleanly. \
         `[lints] workspace = true` must be declared in [package], and `[workspace.lints]` must deny \
         at least `clippy::todo`. cargo said: {}",
        said(&output)
    );
    let complaint = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        complaint.contains("todo"),
        "the policy rejected the fixture, but not because of `clippy::todo` — check that the lint \
         is denied rather than that something else is broken: {}",
        said(&output)
    );
}
