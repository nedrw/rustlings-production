//! Repository tasks.
//!
//! `cargo xtask ci` is the only thing the CI workflow calls, so local and CI checks cannot
//! drift (plan §10). Adding a check means adding it here, not in YAML.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

fn main() -> ExitCode {
    let task = match std::env::args().nth(1) {
        Some(task) => task,
        None => {
            eprintln!("usage: cargo xtask <task>\n\n  ci    run every check CI runs");
            return ExitCode::from(2);
        }
    };

    let result = match task.as_str() {
        "ci" => ci(),
        other => Err(format!("unknown task `{other}`: try `cargo xtask ci`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("\nxtask: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Every check CI runs, in the order that fails fastest.
fn ci() -> Result<(), String> {
    let root = workspace_root()?;
    run(&root, "cargo", &["fmt", "--all", "--check"])?;
    run(
        &root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run(&root, "cargo", &["test", "--workspace"])?;
    lint_caps(&root)?;
    println!("\nxtask ci: all checks passed");
    Ok(())
}

/// The course's own linter, called through the library the runner uses.
///
/// Deliberately not a nested `cargo run`: `xtask` and `kiln` must agree on what the metadata
/// rules are, and the only way to guarantee that is to call the same code (plan §10).
fn lint_caps(root: &Path) -> Result<(), String> {
    println!("xtask ci: kiln lint-caps (rules R0-R6 over course/)");
    let report = kiln::caps::lint::lint_root(root).map_err(|error| error.to_string())?;
    println!(
        "  {} capabilities · {} slots · {} ledger positions · {} authored steps",
        report.capabilities, report.slots, report.ledger_positions, report.authored_steps
    );
    for notice in &report.skipped {
        println!("  skipped: {notice}");
    }
    for (step, reason) in &report.reteaches {
        println!("  reteach {step}: {reason}");
    }
    if report.is_clean() {
        return Ok(());
    }
    for issue in &report.issues {
        println!("  {issue}");
    }
    Err(format!("{} lint-caps issue(s)", report.issues.len()))
}

/// Repository root, derived from this crate's location so the task works from anywhere.
fn workspace_root() -> Result<PathBuf, String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent directory", manifest.display()))
}

/// Run one check, streaming its output.
fn run(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    println!("xtask ci: {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("cannot run {program}: {err}"))?;
    if status.success() {
        return Ok(());
    }
    Err(format!("{program} {} failed ({status})", args.join(" ")))
}
