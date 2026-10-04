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
    // The files cargo cannot see: the specs and the probe exercises live under `course/` and only
    // ever reach a compiler as a copy, but the course asks its learners for formatted code
    // (authoring.md §6.7), so its own material is held to the same rule here.
    format_course_rust(&root)?;
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
    probe_check(&root)?;
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

/// `kiln probe --check`: every probe reference solution compiles and passes.
///
/// Called through the binary rather than the library because it starts `cargo` in a scratch crate:
/// one implementation, the one a learner can run (`kiln probe --check`), and the reason this check
/// is worth its seconds is that a probe which cannot pass sends people to a broken exercise.
fn probe_check(root: &Path) -> Result<(), String> {
    println!("xtask ci: kiln probe --check (every reference solution passes)");
    run_allow_not_authored(
        root,
        "cargo",
        &[
            "run",
            "--quiet",
            "--package",
            "kiln",
            "--",
            "probe",
            "--check",
        ],
    )
}

/// Format the Rust the course ships that cargo cannot reach: specs and probe exercises.
///
/// They live under `course/` and are copied into a project (or a scratch crate) before anything
/// compiles them, so `cargo fmt --all` never sees them. One `rustfmt` invocation over every file:
/// the command a maintainer runs by hand (authoring.md §6.7) is the command CI runs.
fn format_course_rust(root: &Path) -> Result<(), String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rust(&root.join("course/tracks"), &mut files)?;
    collect_rust(&root.join("course/probes"), &mut files)?;
    files.sort();
    if files.is_empty() {
        return Ok(());
    }
    println!(
        "xtask ci: rustfmt --check on {} course file(s) under course/",
        files.len()
    );
    let mut command = Command::new("rustfmt");
    command
        .args([
            "--edition",
            "2024",
            "--config-path",
            "rustfmt.toml",
            // Every file is named on the command line, so rustfmt must not follow `mod`
            // declarations: a spec's `mod support;` only resolves after a scaffold has copied the
            // fixtures into a project (which is where the `fmt` gate sees them again).
            "--config",
            "skip_children=true",
            "--check",
        ])
        .args(&files)
        .current_dir(root)
        .stdin(Stdio::null());
    let status = command
        .status()
        .map_err(|err| format!("cannot run rustfmt: {err} — `rustup component add rustfmt`"))?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "rustfmt reports files under course/ that are not formatted ({status})"
    ))
}

/// Every `.rs` file below `dir`, recursively. A missing directory is not an error.
fn collect_rust(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
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

/// Run one check that is allowed to answer "not authored yet" (exit code 3).
///
/// The runner's exit codes are part of its contract (`kiln/src/cmd/mod.rs`): 3 means nothing was
/// judged because the content does not exist. For a check over content that is still being written
/// (probes arrive with M1), that is not a CI failure — but it must be *said*, not silently allowed.
fn run_allow_not_authored(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
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
    if status.code() == Some(3) {
        println!("  (exit 3: nothing authored yet — not a failure)");
        return Ok(());
    }
    Err(format!("{program} {} failed ({status})", args.join(" ")))
}
