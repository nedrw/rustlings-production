//! Repository tasks.
//!
//! `cargo xtask ci` is the only thing the CI workflow calls, so local and CI checks cannot drift
//! (plan §10). Adding a check means adding it here, not in YAML.

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
    spec_integrity(&root)?;
    lint_caps(&root)?;
    // The course's own anti-rot check (plan §10). It is the command a learner can run too, so CI
    // and a human have one implementation between them, not two.
    run(
        &root,
        "cargo",
        &["run", "--quiet", "--package", "kiln", "--", "selftest"],
    )?;
    println!("\nxtask ci: all checks passed");
    Ok(())
}

/// Both directions of the spec hash check, plus the metadata that names the spec files.
///
/// Delegates to the library for the same reason `lint_caps` does: `kiln` and `xtask` must agree on
/// what "the specs are unchanged" means, and the only way to guarantee that is one implementation.
fn spec_integrity(root: &Path) -> Result<(), String> {
    println!("xtask ci: spec integrity (spec/** against spec/manifest.toml)");
    let audit = kiln::spec::audit(root).map_err(|error| error.to_string())?;
    println!(
        "  {} spec file(s) pinned, {} step(s) declare their own",
        audit.files, audit.steps
    );
    if audit.is_clean() {
        return Ok(());
    }
    for problem in &audit.problems {
        println!("  {problem}");
    }
    Err(format!(
        "{} spec integrity problem(s)",
        audit.problems.len()
    ))
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
