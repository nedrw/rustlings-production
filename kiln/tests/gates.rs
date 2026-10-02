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
    let gate_list = gates
        .iter()
        .map(|gate| format!("\"{gate}\""))
        .collect::<Vec<_>>()
        .join(", ");
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
             spec = [\"spec/1.1/layout.rs\"]\ngates = [{gate_list}]\n"
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
