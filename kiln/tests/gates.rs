//! The three working gates, exercised against a real (tiny) cargo project.
//!
//! Unit tests can check the lint ladder as data; only a real `cargo fmt` / `cargo clippy` run shows
//! that a gate fails when it should, passes when it should, and respects the rule that test targets
//! are relaxed (plan §6, authoring.md §7.4). The fixture is a throwaway course plus a throwaway
//! crate in the system temporary directory.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kiln::gates::registry::Outcome;
use kiln::gates::{GateContext, GateResult, GateSpec, run_all};
use kiln::i18n::Locale;
use kiln::info::Course;
use kiln::runtime;

/// A directory under the system temporary directory, removed when dropped.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("kiln-it-{label}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch directory is creatable");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))
            .expect("scratch parents are creatable");
        std::fs::write(path, contents).expect("scratch file is writable");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A course with one step whose gates the caller names, plus a learner project holding `lib.rs`.
///
/// `lib_rs` is written with leading blank lines removed: the fixtures are raw strings indented for
/// readability, and `cargo fmt --check` would otherwise fail on the blank line alone.
fn course_with(scratch: &Scratch, gates: &[&str], lib_rs: &str) -> Course {
    course_with_images(scratch, gates, lib_rs, &[])
}

/// The same, with the step declaring container images (plan §9.3).
fn course_with_images(scratch: &Scratch, gates: &[&str], lib_rs: &str, images: &[&str]) -> Course {
    let gate_list = gates
        .iter()
        .map(|gate| format!("\"{gate}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let image_list = if images.is_empty() {
        String::new()
    } else {
        format!(
            "images = [{}]\n",
            images
                .iter()
                .map(|image| format!("\"{image}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    scratch.write(
        "course/course.toml",
        "format_version = 1\ndefault_locale = \"en\"\ndefault_track = \"mailer\"\n\
         welcome = \"welcome\"\nfinal_message = \"done\"\n",
    );
    scratch.write(
        "course/capabilities.toml",
        "format_version = 1\n\n[capability.\"repo/workspace-layout\"]\napply = \"mailer/1.1\"\n",
    );
    scratch.write(
        "course/tracks/mailer/track.toml",
        &format!(
            "format_version = 1\nid = \"mailer\"\nindex = 1\ntitle = \"Mailer\"\n\
             project = \"app/mailer\"\nexit = [\"repo/workspace-layout@apply\"]\n\n\
             [[step]]\nid = \"1.1-workspace\"\ntitle = \"Layout\"\nkind = \"milestone\"\n\
             origin = \"teach\"\nentry = \"docs/steps/en/mailer/1.1-workspace.md\"\n\
             time_hint = \"60m\"\nteaches = [\"repo/workspace-layout@apply\"]\n\
             spec = [\"spec/1.1/layout.rs\"]\ngates = [{gate_list}]\n{image_list}"
        ),
    );
    scratch.write(
        "app/mailer/Cargo.toml",
        "[package]\nname = \"mailer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n",
    );
    scratch.write("app/mailer/src/lib.rs", lib_rs.trim_start());
    Course::load(scratch.path()).expect("fixture course loads")
}

/// Run the named gates against the fixture project.
fn gates(scratch: &Scratch, gates: &[&str], lib_rs: &str) -> Vec<GateResult> {
    judge(&course_with(scratch, gates, lib_rs), gates)
}

/// The same, with a binary in the fixture: the ladder treats `--lib` and `--bins` differently.
fn gates_and_bin(
    scratch: &Scratch,
    gates: &[&str],
    lib_rs: &str,
    main_rs: &str,
) -> Vec<GateResult> {
    let course = course_with(scratch, gates, lib_rs);
    scratch.write("app/mailer/src/main.rs", main_rs);
    judge(&course, gates)
}

fn judge(course: &Course, gates: &[&str]) -> Vec<GateResult> {
    let track = course.default_track().unwrap();
    let step = track.step("1.1").unwrap();
    let specs: Vec<GateSpec> = gates.iter().map(|gate| gate.parse().unwrap()).collect();
    let context = GateContext {
        course,
        track,
        step,
        project_dir: track.project_dir(course.root()),
        timeout: Duration::from_secs(300),
        locale: &Locale::parse("en").expect("the fixture source locale parses"),
    };
    run_all(&context, &specs, false)
}

fn outcomes(results: &[GateResult]) -> Vec<Outcome> {
    results.iter().map(|result| result.outcome).collect()
}

fn report(results: &[GateResult]) -> String {
    results
        .iter()
        .map(GateResult::line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A crate that is unformatted, unfinished and prints to stdout from library code.
const MESSY: &str = r#"
pub fn add(a:u32,b:u32)->u32{ println!("adding"); a+b }
pub fn later() -> u32 { todo!() }
"#;

/// The same crate once the step is done.
const CLEAN: &str = r#"
/// Add two numbers.
///
/// # Errors
///
/// This function cannot fail; it exists so the fixture has a documented public item.
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::add;

    #[test]
    fn adds() {
        assert_eq!(add(1, 2), 3, "the fixture must stay trivially true");
    }
}
"#;

#[test]
fn fmt_fails_on_an_unformatted_project_and_passes_on_a_formatted_one() {
    let scratch = Scratch::new("fmt");
    let failures = gates(&scratch, &["fmt"], MESSY);
    assert_eq!(
        outcomes(&failures),
        [Outcome::Failed],
        "{}",
        report(&failures)
    );
    assert!(
        failures[0].detail.contains("cargo fmt --all"),
        "{}",
        failures[0].detail
    );

    let passes = gates(&scratch, &["fmt"], CLEAN);
    assert_eq!(outcomes(&passes), [Outcome::Passed], "{}", report(&passes));
}

#[test]
fn no_todo_fails_on_unfinished_work_and_passes_once_it_is_done() {
    let scratch = Scratch::new("no-todo");
    let failures = gates(&scratch, &["no-todo"], MESSY);
    assert_eq!(
        outcomes(&failures),
        [Outcome::Failed],
        "{}",
        report(&failures)
    );
    assert!(
        failures[0].detail.contains("unfinished work"),
        "{}",
        failures[0].detail
    );

    let passes = gates(&scratch, &["no-todo"], CLEAN);
    assert_eq!(outcomes(&passes), [Outcome::Passed], "{}", report(&passes));
}

#[test]
fn the_clippy_ladder_tightens_from_rung_one_to_rung_two() {
    let scratch = Scratch::new("clippy-ladder");
    // Library code that panics on an empty slice: fine at @1, denied at @2.
    const UNDER_THE_LADDER: &str = "/// First element.\n///\n/// # Panics\n///\n/// \
                                  Panics when the slice is empty.\npub fn first(v: &[u32]) -> u32 { v[0] }\n";

    let rung_one = gates(&scratch, &["clippy@1"], UNDER_THE_LADDER);
    assert_eq!(
        outcomes(&rung_one),
        [Outcome::Passed],
        "{}",
        report(&rung_one)
    );

    let rung_two = gates(&scratch, &["clippy@2"], UNDER_THE_LADDER);
    assert_eq!(
        outcomes(&rung_two),
        [Outcome::Failed],
        "{}",
        report(&rung_two)
    );
    assert!(
        rung_two[0].detail.contains("non-test targets"),
        "{}",
        rung_two[0].detail
    );
}

#[test]
fn print_stdout_is_a_library_lint_and_not_a_binary_one() {
    let scratch = Scratch::new("clippy-print-stdout");
    // A library that writes to stdout cannot be used from anything that is not a terminal, so rung
    // 1 refuses it — while the same `println!` in a binary is the binary's whole job.
    const PRINTING_LIBRARY: &str = r#"
/// Greets.
pub fn greet() -> u32 {
    println!("hello");
    0
}
"#;
    const QUIET_LIBRARY: &str = r#"
/// Greets nothing.
pub fn greet() -> u32 {
    0
}
"#;

    let refused = gates(&scratch, &["clippy@1"], PRINTING_LIBRARY);
    assert_eq!(
        outcomes(&refused),
        [Outcome::Failed],
        "{}",
        report(&refused)
    );
    assert!(
        refused[0].detail.contains("print_stdout"),
        "{}",
        refused[0].detail
    );

    let allowed = gates_and_bin(
        &scratch,
        &["clippy@1"],
        QUIET_LIBRARY,
        "fn main() {\n    println!(\"hello\");\n}\n",
    );
    assert_eq!(
        outcomes(&allowed),
        [Outcome::Passed],
        "{}",
        report(&allowed)
    );
}

#[test]
fn rung_two_relaxes_what_it_denies_inside_tests() {
    let scratch = Scratch::new("clippy-relaxed");
    // `unwrap` in a test is how tests are written; the ladder must not forbid it there.
    const TEST_ONLY: &str = r#"
/// Identity.
pub fn id(value: u32) -> u32 {
    value
}

#[cfg(test)]
mod tests {
    use super::id;

    #[test]
    fn ids() {
        assert_eq!(id(1).checked_add(0).unwrap(), 1);
    }
}
"#;
    let results = gates(&scratch, &["clippy@2"], TEST_ONLY);
    assert_eq!(
        outcomes(&results),
        [Outcome::Passed],
        "{}",
        report(&results)
    );
}

#[test]
fn a_gate_whose_tool_is_missing_reports_an_install_command() {
    let scratch = Scratch::new("missing-tool");
    // The defect is simulated where it can be: the catalogue must carry an install hint for every
    // gate that needs one, which is what turns a raw exit code into an instruction.
    let hint = kiln::gates::tool::Tool::CargoMutants.install_hint();
    assert!(hint.starts_with("cargo install"), "{hint}");

    // And a project that does not exist yet is blocked, not silently passed.
    let course = course_with(&scratch, &["fmt"], CLEAN);
    let track = course.default_track().unwrap();
    let step = track.step("1.1").unwrap();
    std::fs::remove_dir_all(scratch.path().join("app/mailer")).unwrap();
    let context = GateContext {
        course: &course,
        track,
        step,
        project_dir: track.project_dir(course.root()),
        timeout: Duration::from_secs(60),
        locale: &Locale::parse("en").expect("the fixture source locale parses"),
    };
    let results = run_all(&context, &["fmt".parse().unwrap()], false);
    assert_eq!(
        outcomes(&results),
        [Outcome::Blocked],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("not authored yet"),
        "{}",
        results[0].detail
    );
}

/// A binary that refuses what it does not understand — the failure path `exit-code` asks for.
const REFUSING_BIN: &str = r#"
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(flag) = args.first() {
        eprintln!("usage: fixture [OPTIONS]\n\nunknown argument `{flag}`");
        std::process::exit(2);
    }
}
"#;

/// A binary that ignores its arguments and exits successfully: the shape the gate exists to reject.
const IGNORING_BIN: &str = r#"
fn main() {
    println!("fixture: nothing to do");
}
"#;

#[test]
fn exit_code_passes_when_a_binary_refuses_an_argument_it_does_not_know() {
    let scratch = Scratch::new("exit-code-pass");
    let results = gates_and_bin(&scratch, &["exit-code"], CLEAN, REFUSING_BIN);
    assert_eq!(
        outcomes(&results),
        [Outcome::Passed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("mailer"),
        "{}",
        results[0].detail
    );
}

#[test]
fn exit_code_fails_when_a_binary_ignores_an_argument_it_does_not_know() {
    let scratch = Scratch::new("exit-code-ignore");
    let results = gates_and_bin(&scratch, &["exit-code"], CLEAN, IGNORING_BIN);
    assert_eq!(
        outcomes(&results),
        [Outcome::Failed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("exited 0"),
        "{}",
        results[0].detail
    );
}

#[test]
fn exit_code_probes_every_binary_the_project_builds() {
    let scratch = Scratch::new("exit-code-two-bins");
    // Two binaries: one that refuses what it does not understand, and one that ignores it. A gate
    // that stops at the first match would report a green step while half the project accepts
    // anything.
    let course = course_with(&scratch, &["exit-code"], CLEAN);
    scratch.write("app/mailer/src/main.rs", REFUSING_BIN.trim_start());
    scratch.write("app/mailer/src/bin/second.rs", IGNORING_BIN.trim_start());
    let results = judge(&course, &["exit-code"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Failed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("second"),
        "the gate has to keep probing after the first binary behaved: {}",
        results[0].detail
    );
}

#[test]
fn exit_code_passes_only_when_every_binary_names_what_it_refuses() {
    let scratch = Scratch::new("exit-code-two-good-bins");
    let course = course_with(&scratch, &["exit-code"], CLEAN);
    scratch.write("app/mailer/src/main.rs", REFUSING_BIN.trim_start());
    scratch.write("app/mailer/src/bin/second.rs", REFUSING_BIN.trim_start());
    let results = judge(&course, &["exit-code"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Passed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("mailer") && results[0].detail.contains("second"),
        "the report has to name every binary it probed: {}",
        results[0].detail
    );
}

#[test]
fn exit_code_is_blocked_when_the_project_ships_no_binary() {
    let scratch = Scratch::new("exit-code-no-bin");
    let results = gates(&scratch, &["exit-code"], CLEAN);
    assert_eq!(
        outcomes(&results),
        [Outcome::Blocked],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("builds no binary"),
        "{}",
        results[0].detail
    );
}

/// A library whose public function can fail but never says what it returns on the error path.
const UNDOCUMENTED_FALLIBLE_LIB: &str = r#"
//! A fixture crate.

/// Parses a port.
///
/// # Panics
///
/// Never: this one only fails.
pub fn parse(input: &str) -> Result<u16, std::num::ParseIntError> {
    input.parse()
}
"#;

/// The same function, documented the way the gate asks for.
const DOCUMENTED_FALLIBLE_LIB: &str = r#"
//! A fixture crate.

/// Parses a port.
///
/// # Errors
///
/// Returns `Err` when `input` is not a number in range.
pub fn parse(input: &str) -> Result<u16, std::num::ParseIntError> {
    input.parse()
}
"#;

#[test]
fn docs_public_wants_a_sentence_on_the_error_path() {
    let scratch = Scratch::new("docs-public");
    let undocumented = gates(&scratch, &["docs:public"], UNDOCUMENTED_FALLIBLE_LIB);
    assert_eq!(
        outcomes(&undocumented),
        [Outcome::Failed],
        "{}",
        report(&undocumented)
    );
    assert!(
        undocumented[0].detail.contains("missing_errors_doc"),
        "{}",
        undocumented[0].detail
    );

    let documented = gates(&scratch, &["docs:public"], DOCUMENTED_FALLIBLE_LIB);
    assert_eq!(
        outcomes(&documented),
        [Outcome::Passed],
        "{}",
        report(&documented)
    );
}

#[test]
fn docs_public_does_not_judge_test_code() {
    let scratch = Scratch::new("docs-public-tests");
    // A test that returns `Result` for `?` is not an API; demanding `# Errors` there would teach
    // the wrong reflex (authoring.md §7.4).
    const TEST_RETURNING_RESULT: &str = r#"
//! A fixture crate.

/// Identity.
pub fn id(value: u32) -> u32 {
    value
}

#[cfg(test)]
mod tests {
    use super::id;

    #[test]
    fn ids() -> Result<(), std::num::ParseIntError> {
        assert_eq!(id("1".parse()?), 1);
        Ok(())
    }
}
"#;
    let results = gates(&scratch, &["docs:public"], TEST_RETURNING_RESULT);
    assert_eq!(
        outcomes(&results),
        [Outcome::Passed],
        "{}",
        report(&results)
    );
}

#[test]
fn the_images_gate_judges_what_the_engine_has_and_names_the_pull_it_does_not() {
    // An engine is not part of every environment, and a missing one is not a failing test: the same
    // rule as the gates themselves (plan §10 — report what cannot be judged).
    let Some(engine) = runtime::first_engine(Duration::from_secs(30)) else {
        eprintln!("no container engine answered: skipping the live `images` check");
        return;
    };
    let local = runtime::local_images(engine, Duration::from_secs(30))
        .expect("the engine answers `images`");
    let Some(present) = local.first() else {
        eprintln!("the local image store is empty: skipping the live `images` check");
        return;
    };

    // An image the engine has — under whatever name this machine pulled it — is a pass.
    let scratch = Scratch::new("images-present");
    let course = course_with_images(&scratch, &["images"], CLEAN, &[present.as_str()]);
    let results = judge(&course, &["images"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Passed],
        "{}",
        report(&results)
    );

    // One it does not have is a failure whose detail is the command that fixes it.
    let scratch = Scratch::new("images-missing");
    let course = course_with_images(&scratch, &["images"], CLEAN, &["kiln-no-such-image:0"]);
    let results = judge(&course, &["images"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Failed],
        "{}",
        report(&results)
    );
    let detail = &results[0].detail;
    assert!(detail.contains("kiln-no-such-image:0"), "{detail}");
    assert!(
        detail.contains(&format!("{} pull", engine.name())),
        "{detail}"
    );
}
