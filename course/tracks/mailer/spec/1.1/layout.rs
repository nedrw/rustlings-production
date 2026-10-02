//! Spec for `mailer/1.1-workspace`: the service is a library, and the binary is a shell around it.
//!
//! `kiln run` copies this file into the project as `tests/spec_layout.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Everything here is a fact about the *built crate* rather than
//! about a file that happens to exist:
//!
//! * `cargo check --lib` accepts the package as a library — the compiler's own answer to "is there a
//!   library target?", and the failure message names the package;
//! * the binary still starts and answers `--help`, `--version` and an unknown flag the same way;
//! * `src/main.rs` is a shell: at most [`BIN_LINE_BUDGET`] lines that are neither blank nor comments.
//!
//! The third one is a proxy, and the step body says why: a black-box spec cannot call into a library
//! that may not exist yet, so "the binary is thin" is measured rather than invoked. The first two are
//! behavioural, and they are what the step is really about.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Lines of code (not blank, not a comment) `src/main.rs` may keep.
///
/// Generous on purpose: the point is "a shell, not the program", not a formatting opinion.
const BIN_LINE_BUDGET: usize = 20;

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run `cargo` in the project, offline, and return everything it said.
///
/// `CARGO_NET_OFFLINE` keeps the step fast and deterministic (plan §9.6): the crate has no
/// dependencies to fetch, and a spec that reaches for the network is a spec that fails on a train.
fn cargo(args: &[&str]) -> Output {
    Command::new("cargo")
        .args(args)
        .env("CARGO_NET_OFFLINE", "true")
        .current_dir(project())
        .output()
        .expect("cargo must be on PATH; rust-toolchain.toml pins which one")
}

/// The first lines of a command's complaint, for an assertion message a human can act on.
fn said(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    text.lines().take(8).collect::<Vec<_>>().join("\n      ")
}

/// Everything a command printed, lower-cased and joined, so `--help` may go to either stream.
fn text_of(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_lowercase()
}

fn run(binary: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{} could not be started: {error}", binary.display()))
}

#[test]
fn the_package_exposes_a_library_target() {
    let output = cargo(&["check", "--lib"]);
    assert!(
        output.status.success(),
        "`cargo check --lib` does not accept this package as a library, so nothing but the binary \
         can use the service (cargo said: {}).\n      Move the service into `src/lib.rs` and leave \
         `main.rs` calling it.",
        said(&output)
    );
}

#[test]
fn the_binary_answers_help_version_and_an_unknown_flag() {
    let binary = Path::new(env!("CARGO_BIN_EXE_mailer"));

    let help = run(binary, &["--help"]);
    assert!(
        help.status.success(),
        "`--help` must exit 0, it exited {:?}",
        help.status.code()
    );
    assert!(
        text_of(&help).contains("usage"),
        "`--help` must print how to use the service, it printed: {}",
        said(&help)
    );

    let version = run(binary, &["--version"]);
    assert!(version.status.success(), "`--version` must exit 0");
    assert!(
        text_of(&version).contains(env!("CARGO_PKG_VERSION")),
        "`--version` must print the crate version ({}), it printed: {}",
        env!("CARGO_PKG_VERSION"),
        said(&version)
    );

    let unknown = run(binary, &["--definitely-not-a-flag"]);
    assert!(
        !unknown.status.success(),
        "an unknown flag must fail, this one exited 0 — a service that ignores what it does not \
         understand starts with the wrong settings and says nothing"
    );
    assert!(
        text_of(&unknown).contains("--definitely-not-a-flag"),
        "the complaint must name the flag it did not understand, it said: {}",
        said(&unknown)
    );
}

#[test]
fn the_binary_is_a_shell_not_the_program() {
    let path = project().join("src/main.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must exist and be readable: {error}", path.display()));
    let code = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .count();
    assert!(
        code <= BIN_LINE_BUDGET,
        "`src/main.rs` has {code} lines of code (budget: {BIN_LINE_BUDGET}). A binary is a shell: \
         argument handling and everything it decides belong to the library, where they can be \
         tested without starting a process."
    );
}
