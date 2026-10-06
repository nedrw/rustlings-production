//! The gates, exercised against a real (tiny) cargo project.
//!
//! Unit tests can check the lint ladder as data; only a real `cargo fmt` / `cargo clippy` / `cargo test`
//! run shows that a gate fails when it should, passes when it should, and respects the rule that test
//! targets are relaxed (plan §6, authoring.md §7.4). The same goes for the slow gates: `mutants` and
//! `conformance` are only convincing against a project whose specs actually run, and the point of each
//! of them is that a *deliberate degradation* turns it red while the good version stays green — which
//! is what every test below does, both ways. The fixture is a throwaway course plus a throwaway crate
//! in the system temporary directory.

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
///
/// The step also carries the shape the slow gates need — a `run.cmd` that starts its spec target, and
/// a reference solution that touches `src/lib.rs` — so that "this step's module" is defined the way
/// the course defines it (handoff M2 §1) without every test restating it.
fn course_with(scratch: &Scratch, gates: &[&str], lib_rs: &str) -> Course {
    course_with_images(scratch, gates, lib_rs, &[])
}

/// The reference solution the fixture's step declares.
///
/// Nothing here applies it: `mutants` and `coverage` read it for *which* files the step is about, and
/// an insertion with no context line is a patch that parses against any `lib.rs`, so the fixture does
/// not have to keep its text in step with the crate below.
const SOLUTION: &str = concat!(
    "--- a/src/lib.rs\n",
    "+++ b/src/lib.rs\n",
    "@@ -1,0 +2,1 @@\n",
    "+/// The reference solution adds this line.\n",
);

/// Write a fixture file, with `{crate}` standing for the fixture crate's (unique) name.
///
/// A spec or a harness has to name the crate it tests, and that name carries the fixture directory's
/// uniqueness (see [`crate_name`]), so the bodies are written as templates.
fn fixture_file(scratch: &Scratch, relative: &str, body: &str) {
    let name = crate_name(scratch, "mailer");
    scratch.write(relative, &body.replace("{crate}", &name));
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
             run = {{ cmd = \"cargo test -p {} --test spec_layout\" }}\n\
             spec = [\"spec/1.1/layout.rs\"]\n\
             solution = \"solutions/1.1-workspace.patch\"\ngates = [{gate_list}]\n{image_list}",
            crate_name(scratch, "mailer")
        ),
    );
    scratch.write(
        "course/tracks/mailer/solutions/1.1-workspace.patch",
        SOLUTION,
    );
    scratch.write(
        "app/mailer/Cargo.toml",
        &format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n",
            crate_name(scratch, "mailer")
        ),
    );
    scratch.write("app/mailer/src/lib.rs", lib_rs.trim_start());
    Course::load(scratch.path()).expect("fixture course loads")
}

/// Add a second binary to the fixture project and answer its (unique) name.
fn add_second_bin(scratch: &Scratch, body: &str) -> String {
    let name = crate_name(scratch, "second");
    scratch.write(&format!("app/mailer/src/bin/{name}.rs"), body.trim_start());
    name
}

/// A crate name that no other fixture in this file can collide with.
///
/// Cargo writes every workspace's output into `CARGO_TARGET_DIR` when the environment sets one, and
/// these fixtures are all called `mailer`: two tests running at the same time would overwrite each
/// other's `debug/mailer`, and `exit-code` probes whichever binary landed last — a green step
/// judged against the wrong program. The name carries the fixture directory's uniqueness, so the
/// path is unique without depending on the environment (which is what a test cannot control).
fn crate_name(scratch: &Scratch, label: &str) -> String {
    let unique = scratch
        .path()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    format!("{label}_{unique}")
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
    judge_all(course, gates, false)
}

/// The same, with the slow gates included — what `kiln verify --slow` and `selftest` do.
fn judge_slow(course: &Course, gates: &[&str]) -> Vec<GateResult> {
    judge_all(course, gates, true)
}

fn judge_all(course: &Course, gates: &[&str], run_slow: bool) -> Vec<GateResult> {
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
    run_all(&context, &specs, run_slow)
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
    let second = add_second_bin(&scratch, IGNORING_BIN);
    let results = judge(&course, &["exit-code"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Failed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains(&second),
        "the gate has to keep probing after the first binary behaved: {}",
        results[0].detail
    );
}

#[test]
fn exit_code_passes_only_when_every_binary_names_what_it_refuses() {
    let scratch = Scratch::new("exit-code-two-good-bins");
    let course = course_with(&scratch, &["exit-code"], CLEAN);
    scratch.write("app/mailer/src/main.rs", REFUSING_BIN.trim_start());
    let second = add_second_bin(&scratch, REFUSING_BIN);
    let results = judge(&course, &["exit-code"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Passed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains(&crate_name(&scratch, "mailer"))
            && results[0].detail.contains(&second),
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

/// The `links` gate as the runner sees it: offline, over the project's own pages (plan §6).
#[test]
fn the_links_gate_judges_the_pages_of_the_project() {
    // A page whose targets resolve. The external URL is listed rather than fetched — this gate may
    // not touch the network, which is what makes the result the same on every machine.
    let scratch = Scratch::new("links-clean");
    scratch.write(
        "app/mailer/docs/runbook.md",
        "# Runbook\n\n## Steps\n\nRead [the code](../src/lib.rs) and \
         <https://example.com/docs>.\n\nBack to [the steps](#steps).\n",
    );
    let course = course_with(&scratch, &["links"], CLEAN);
    let results = judge(&course, &["links"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Passed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("not fetched"),
        "{}",
        results[0].detail
    );

    // What a half-written runbook looks like: a placeholder where a target should be, and a file
    // that was renamed. Both are what a learner sees before they finish the step.
    let scratch = Scratch::new("links-half-written");
    scratch.write(
        "app/mailer/docs/runbook.md",
        "# Runbook\n\nSee [the dashboard](TODO) and [the drill](tests/drill.rs).\n",
    );
    let course = course_with(&scratch, &["links"], CLEAN);
    let results = judge(&course, &["links"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Failed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("docs/runbook.md:3") && results[0].detail.contains("TODO"),
        "{}",
        results[0].detail
    );

    // A design step whose page was never written cannot pass a gate about its links.
    let scratch = Scratch::new("links-no-page");
    let course = course_with(&scratch, &["links"], CLEAN);
    let results = judge(&course, &["links"]);
    assert_eq!(
        outcomes(&results),
        [Outcome::Failed],
        "{}",
        report(&results)
    );
    assert!(
        results[0].detail.contains("no markdown"),
        "{}",
        results[0].detail
    );
}

/// A crate with three mutable sites on one line, and a spec that either notices them or does not.
const BUMP: &str = r#"
/// Doubles a positive value; zero is a case of its own.
pub fn bump(value: u32) -> u32 {
    if value > 0 { value + value } else { 100 }
}
"#;

/// The spec a learner writes when they only test the easy case.
///
/// Every site the gate mutates is on one line, so this spec's coverage of that line is exactly what
/// the gate measures: it runs `bump(2)`, and everything a mutation could do to `value > 0` or to
/// `+` is invisible from there.
const WEAK_SPEC: &str = r#"
#[test]
fn doubles() {
    assert_eq!({crate}::bump(2), 4);
}
"#;

/// The same spec once it covers the branch the mutations change.
const STRONG_SPEC: &str = r#"
#[test]
fn doubles() {
    assert_eq!({crate}::bump(2), 4);
    assert_eq!({crate}::bump(1), 2);
    assert_eq!({crate}::bump(0), 100);
}
"#;

#[test]
fn mutants_fail_a_spec_that_does_not_notice_a_change_and_pass_one_that_does() {
    let scratch = Scratch::new("mutants");
    let course = course_with(&scratch, &["mutants@80"], BUMP);

    // A spec that passes and would keep passing if the function changed: that is the defect, and the
    // report has to say which change it failed to notice.
    fixture_file(&scratch, "app/mailer/tests/spec_layout.rs", WEAK_SPEC);
    let weak = judge_slow(&course, &["slow:mutants@80"]);
    assert_eq!(outcomes(&weak), [Outcome::Failed], "{}", report(&weak));
    assert!(
        weak[0].detail.contains("src/lib.rs:3"),
        "{}",
        weak[0].detail
    );
    assert!(
        weak[0].detail.contains("if value > 0"),
        "{}",
        weak[0].detail
    );
    assert!(weak[0].detail.contains("needs ≥80%"), "{}", weak[0].detail);

    // The same code, the same gate, a spec that exercises the branch: nothing survives.
    fixture_file(&scratch, "app/mailer/tests/spec_layout.rs", STRONG_SPEC);
    let strong = judge_slow(&course, &["slow:mutants@80"]);
    assert_eq!(outcomes(&strong), [Outcome::Passed], "{}", report(&strong));
    assert!(strong[0].detail.contains("100%"), "{}", strong[0].detail);

    // And the gate has to leave the build cache describing the code that is really there: a shared
    // `CARGO_TARGET_DIR` (this test has one) keys a slot by package name, so a leftover build of a
    // mutant would be served to the learner's own next `cargo test` — which would then run a mutant.
    let own = kiln::process::run_line(
        &format!(
            "cargo test -p {} --test spec_layout",
            crate_name(&scratch, "mailer")
        ),
        &scratch.path().join("app/mailer"),
        Duration::from_secs(300),
    )
    .expect("cargo runs in the fixture project");
    assert!(
        own.success(),
        "the learner's own test run does not pass after the gate:\n{}",
        own.stdout + &own.stderr
    );
}

/// A lookup that halves the span: the count grows like log₂(n).
const BUDGET_LIB: &str = r#"
/// Cells a lookup examines in a list of `n` items.
pub fn cells_examined(n: u32) -> u32 {
    let mut cells = 0;
    let mut span = n;
    while span > 1 {
        cells += 1;
        span /= 2;
    }
    cells
}
"#;

/// The same lookup rewritten as a linear scan — the degradation the step is about.
const LINEAR_LIB: &str = r#"
/// Cells a lookup examines in a list of `n` items.
pub fn cells_examined(n: u32) -> u32 {
    let mut cells = 0;
    let mut left = n;
    while left > 0 {
        cells += 1;
        left -= 1;
    }
    cells
}
"#;

/// The harness the course scaffolds: counts, never times.
const BUDGET_HARNESS: &str = r#"
#[test]
fn prints_the_counts() {
    let cells = {crate}::cells_examined(1024);
    println!("jobq-budget scan_cells={cells}");
}
"#;

/// The course-side baseline: the command that prints the counts, next to what they have to stay near.
const BUDGET_BASELINE: &str = r#"
command = "cargo test -p {crate} --test budget -- --nocapture"

[metric]
scan_cells = 10
"#;

#[test]
fn bench_budget_fails_when_a_count_leaves_the_baseline_and_passes_when_it_stays() {
    let scratch = Scratch::new("bench-budget");
    let course = course_with(&scratch, &["bench-budget@10"], BUDGET_LIB);
    fixture_file(&scratch, "app/mailer/tests/budget.rs", BUDGET_HARNESS);
    fixture_file(
        &scratch,
        "course/tracks/mailer/budget/1.1-workspace.toml",
        BUDGET_BASELINE,
    );

    let inside = judge_slow(&course, &["slow:bench-budget@10"]);
    assert_eq!(outcomes(&inside), [Outcome::Passed], "{}", report(&inside));
    assert!(
        inside[0].detail.contains("scan_cells 10"),
        "{}",
        inside[0].detail
    );

    // The O(log n) lookup replaced by a linear scan: 1024 cells where the baseline is 10.
    scratch.write("app/mailer/src/lib.rs", LINEAR_LIB.trim_start());
    let outside = judge_slow(&course, &["slow:bench-budget@10"]);
    assert_eq!(
        outcomes(&outside),
        [Outcome::Failed],
        "{}",
        report(&outside)
    );
    assert!(
        outside[0].detail.contains("scan_cells is 1024"),
        "{}",
        outside[0].detail
    );
    assert!(
        outside[0].detail.contains("baseline 10"),
        "{}",
        outside[0].detail
    );

    // A harness that is not there is not the learner's code being wrong: nothing can be compared, so
    // the gate says what is missing instead of passing.
    std::fs::remove_file(scratch.path().join("app/mailer/tests/budget.rs"))
        .expect("the harness is in the fixture");
    let no_harness = judge_slow(&course, &["slow:bench-budget@10"]);
    assert_eq!(
        outcomes(&no_harness),
        [Outcome::Blocked],
        "{}",
        report(&no_harness)
    );
    assert!(
        no_harness[0].detail.contains("tests/budget.rs"),
        "{}",
        no_harness[0].detail
    );
}

/// The public item the course's witness pins.
const API_LIB: &str = r#"
/// A widget whose size the course pinned.
pub struct Widget;

impl Widget {
    /// Its size, in units.
    pub fn size(&self) -> u32 {
        3
    }
}
"#;

/// The same crate after a rename nobody announced.
const RENAMED_LIB: &str = r#"
/// A widget whose size the course pinned.
pub struct Widget;

impl Widget {
    /// Its size, in units.
    pub fn len(&self) -> u32 {
        3
    }
}
"#;

/// The course's witness: a form the compiler can judge, not a description of one.
const API_WITNESS: &str = r#"
//! The public API of this crate, as the course pinned it.
use {crate}::Widget;

#[test]
fn the_signature_is_still_there() {
    let _: fn(&Widget) -> u32 = Widget::size;
}
"#;

/// The witness a *later* step ships, written for an API this step does not have yet.
///
/// It is here to be refused: a step cannot be judged against a baseline that was established after it,
/// so a gate that picked the newest file in `api/` regardless of the step number would report this
/// compile error instead of the pass the step deserves.
const LATER_WITNESS: &str = r#"
//! The witness a later step ships.
use {crate}::Widget;

#[test]
fn the_signature_is_still_there() {
    let _: fn(&Widget) -> u32 = Widget::width;
}
"#;

#[test]
fn semver_judges_a_step_against_the_newest_witness_at_or_before_it() {
    let scratch = Scratch::new("semver");
    let course = course_with(&scratch, &["semver@baseline"], API_LIB);
    let witness = "app/mailer/tests/semver_baseline.rs";
    let earlier = "course/tracks/mailer/api/1.1-workspace.rs";
    fixture_file(&scratch, earlier, API_WITNESS);

    let holding = judge(&course, &["semver@baseline"]);
    assert_eq!(
        outcomes(&holding),
        [Outcome::Passed],
        "{}",
        report(&holding)
    );
    assert!(
        holding[0].detail.contains("api/1.1-workspace.rs"),
        "{}",
        holding[0].detail
    );
    // The witness is course-owned: the gate writes it inside the learner's tree and takes it back
    // out again, so the learner's next `cargo test` compiles what it compiled before.
    assert!(
        !scratch.path().join(witness).exists(),
        "{witness} was left behind"
    );

    // A witness shipped by a *later* step is not this step's baseline: the step is blocked rather
    // than judged against an API it was never asked to have.
    std::fs::remove_file(scratch.path().join(earlier)).expect("the witness is in the fixture");
    fixture_file(
        &scratch,
        "course/tracks/mailer/api/1.2-unix-sockets.rs",
        LATER_WITNESS,
    );
    let too_new = judge(&course, &["semver@baseline"]);
    assert_eq!(
        outcomes(&too_new),
        [Outcome::Blocked],
        "{}",
        report(&too_new)
    );
    assert!(
        too_new[0].detail.contains("no witness at or before 1.1"),
        "{}",
        too_new[0].detail
    );

    // With both shipped, the earlier one is still the one that decides.
    fixture_file(&scratch, earlier, API_WITNESS);
    let both = judge(&course, &["semver@baseline"]);
    assert_eq!(outcomes(&both), [Outcome::Passed], "{}", report(&both));

    // The item renamed: the witness stops compiling, and the report quotes the compiler.
    scratch.write("app/mailer/src/lib.rs", RENAMED_LIB.trim_start());
    let broken = judge(&course, &["semver@baseline"]);
    assert_eq!(outcomes(&broken), [Outcome::Failed], "{}", report(&broken));
    assert!(broken[0].detail.contains("size"), "{}", broken[0].detail);
    assert!(
        broken[0].detail.contains("public API")
            && broken[0].detail.contains("api/1.1-workspace.rs"),
        "{}",
        broken[0].detail
    );
    assert!(
        !scratch.path().join(witness).exists(),
        "{witness} was left behind"
    );
}

/// Production code with one `unsafe` block and the note that explains it.
const DOCUMENTED_UNSAFE: &str = r#"
/// Reads a byte through a raw pointer.
///
/// # Safety
///
/// The caller guarantees `pointer` is valid for a read.
pub fn peek(pointer: *const u8) -> u8 {
    // SAFETY: the caller's contract is the `# Safety` section above.
    unsafe { *pointer }
}
"#;

/// The same block with the note deleted: the promise is now unstated.
const UNDOCUMENTED_UNSAFE: &str = r#"
/// Reads a byte through a raw pointer.
pub fn peek(pointer: *const u8) -> u8 {
    unsafe { *pointer }
}
"#;

/// The manifest a step at `unsafe-budget@0` has to carry: the compiler holds the line, not the gate.
fn deny_unsafe_code(scratch: &Scratch) {
    scratch.write(
        "app/mailer/Cargo.toml",
        &format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [lints.rust]\nunsafe_code = \"deny\"\n",
            crate_name(scratch, "mailer")
        ),
    );
}

#[test]
fn unsafe_budget_wants_every_block_explained_and_the_lint_at_zero() {
    let scratch = Scratch::new("unsafe-budget");
    let course = course_with(&scratch, &["unsafe-budget@1"], DOCUMENTED_UNSAFE);

    let explained = judge(&course, &["unsafe-budget@1"]);
    assert_eq!(
        outcomes(&explained),
        [Outcome::Passed],
        "{}",
        report(&explained)
    );
    assert!(
        explained[0].detail.contains("1 `unsafe` occurrence"),
        "{}",
        explained[0].detail
    );

    // Within budget and still wrong: the note is what makes an `unsafe` explained.
    scratch.write("app/mailer/src/lib.rs", UNDOCUMENTED_UNSAFE.trim_start());
    let undocumented = judge(&course, &["unsafe-budget@1"]);
    assert_eq!(
        outcomes(&undocumented),
        [Outcome::Failed],
        "{}",
        report(&undocumented)
    );
    assert!(
        undocumented[0].detail.contains("src/lib.rs:3"),
        "{}",
        undocumented[0].detail
    );
    assert!(
        undocumented[0].detail.contains("SAFETY"),
        "{}",
        undocumented[0].detail
    );

    // Over the budget the report names every place, not just the first.
    scratch.write("app/mailer/src/lib.rs", DOCUMENTED_UNSAFE.trim_start());
    let over = judge(&course, &["unsafe-budget@0"]);
    assert_eq!(outcomes(&over), [Outcome::Failed], "{}", report(&over));
    assert!(
        over[0].detail.contains("src/lib.rs:8"),
        "{}",
        over[0].detail
    );

    // `@0` is the step that says "none at all", so the manifest has to deny the word as well: the
    // compiler then rejects the next block instead of this gate reporting it.
    scratch.write("app/mailer/src/lib.rs", CLEAN.trim_start());
    let undefended = judge(&course, &["unsafe-budget@0"]);
    assert_eq!(
        outcomes(&undefended),
        [Outcome::Failed],
        "{}",
        report(&undefended)
    );
    assert!(
        undefended[0].detail.contains("unsafe_code"),
        "{}",
        undefended[0].detail
    );

    deny_unsafe_code(&scratch);
    let defended = judge(&course, &["unsafe-budget@0"]);
    assert_eq!(
        outcomes(&defended),
        [Outcome::Passed],
        "{}",
        report(&defended)
    );
}

/// The engine's store, with the two implementations the suite has to agree about.
const CONFORMANCE_LIB: &str = r#"
/// The store the suite exercises, one implementation per `backend`.
pub struct Store {
    backend: String,
}

/// Opens the implementation `backend` names.
pub fn open(backend: &str) -> Store {
    Store { backend: backend.to_owned() }
}

impl Store {
    /// Writes a value and reads it back.
    pub fn round_trip(&self, value: u32) -> u32 {
        if self.backend == "postgres" {
            self.from_the_database(value)
        } else {
            value
        }
    }

    /// What the database implementation does with a value.
    fn from_the_database(&self, value: u32) -> u32 {
        value
    }
}
"#;

/// The same store with the second implementation quietly broken.
const DISAGREEING_LIB: &str = r#"
/// The store the suite exercises, one implementation per `backend`.
pub struct Store {
    backend: String,
}

/// Opens the implementation `backend` names.
pub fn open(backend: &str) -> Store {
    Store { backend: backend.to_owned() }
}

impl Store {
    /// Writes a value and reads it back.
    pub fn round_trip(&self, value: u32) -> u32 {
        if self.backend == "postgres" {
            self.from_the_database(value)
        } else {
            value
        }
    }

    /// What the database implementation does with a value.
    fn from_the_database(&self, _value: u32) -> u32 {
        0
    }
}
"#;

/// One suite, run once per backend the project declares.
const CONFORMANCE_SPEC: &str = r#"
#[test]
fn every_implementation_keeps_the_value() {
    let backend = std::env::var("KILN_BACKEND").unwrap_or_default();
    let store = {crate}::open(&backend);
    assert_eq!(store.round_trip(7), 7, "backend {backend}");
}
"#;

/// What the project declares: one suite, many implementations.
const CONFORMANCE_MANIFEST: &str = r#"
# One suite, many implementations: `kiln` re-runs this step's spec once per backend.
backends = ["memory", "postgres"]
"#;

#[test]
fn conformance_runs_the_suite_on_every_backend_and_catches_the_one_that_disagrees() {
    let scratch = Scratch::new("conformance");
    let course = course_with(&scratch, &["conformance@2"], CONFORMANCE_LIB);
    fixture_file(
        &scratch,
        "app/mailer/tests/spec_layout.rs",
        CONFORMANCE_SPEC,
    );
    fixture_file(
        &scratch,
        "app/mailer/conformance.toml",
        CONFORMANCE_MANIFEST,
    );
    let manifest = "app/mailer/conformance.toml";

    let agreeing = judge_slow(&course, &["slow:conformance@2"]);
    assert_eq!(
        outcomes(&agreeing),
        [Outcome::Passed],
        "{}",
        report(&agreeing)
    );
    assert!(
        agreeing[0].detail.contains("memory ✓") && agreeing[0].detail.contains("postgres ✓"),
        "{}",
        agreeing[0].detail
    );

    // One implementation disagrees with the other: the same suite, run twice, says which one.
    scratch.write("app/mailer/src/lib.rs", DISAGREEING_LIB.trim_start());
    let disagreeing = judge_slow(&course, &["slow:conformance@2"]);
    assert_eq!(
        outcomes(&disagreeing),
        [Outcome::Failed],
        "{}",
        report(&disagreeing)
    );
    assert!(
        disagreeing[0].detail.contains("memory ✓"),
        "{}",
        disagreeing[0].detail
    );
    assert!(
        disagreeing[0].detail.contains("postgres ✗"),
        "{}",
        disagreeing[0].detail
    );

    // A step about ports whose project declares fewer implementations than it claims has not
    // delivered the step, so the gate fails rather than blocking.
    let too_few = judge_slow(&course, &["slow:conformance@3"]);
    assert_eq!(
        outcomes(&too_few),
        [Outcome::Failed],
        "{}",
        report(&too_few)
    );
    assert!(
        too_few[0].detail.contains("declares 2 backend(s)"),
        "{}",
        too_few[0].detail
    );

    // And a project that never wrote the declaration has nothing to judge at all.
    std::fs::remove_file(scratch.path().join(manifest)).expect("the manifest is in the fixture");
    let missing = judge_slow(&course, &["slow:conformance@2"]);
    assert_eq!(
        outcomes(&missing),
        [Outcome::Failed],
        "{}",
        report(&missing)
    );
    assert!(
        missing[0].detail.contains("conformance.toml"),
        "{}",
        missing[0].detail
    );
}

#[test]
fn coverage_measures_the_step_module_or_names_the_component_to_install() {
    let scratch = Scratch::new("coverage");
    let course = course_with(&scratch, &["?coverage@1"], BUMP);
    fixture_file(&scratch, "app/mailer/tests/spec_layout.rs", STRONG_SPEC);

    let results = judge_slow(&course, &["slow:?coverage@1"]);

    // `coverage` is the one slow gate that is a `should`, and this is why: the LLVM tools come from a
    // rustup component rather than from the toolchain baseline, so a machine without them gets the
    // install line and no red step (plan §10).
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("kiln lives one directory below the repository root");
    if kiln::gates::coverage::tools(project, Duration::from_secs(30)).is_none() {
        assert_eq!(
            outcomes(&results),
            [Outcome::Blocked],
            "{}",
            report(&results)
        );
        assert!(
            results[0]
                .detail
                .contains("rustup component add llvm-tools-preview"),
            "{}",
            results[0].detail
        );
        return;
    }

    // With the tools, the gate measures: either the module is covered enough, or the report says by
    // how much it is not, or it says that the profile it read describes *another* tree — the fixture
    // shares the gate's build directory with every other fixture that ran before it, and a number
    // about another tree is the one answer this gate must never give.
    assert!(
        matches!(
            outcomes(&results)[0],
            Outcome::Passed | Outcome::Failed | Outcome::Blocked
        ),
        "{}",
        report(&results)
    );
    if outcomes(&results)[0] == Outcome::Blocked {
        assert!(
            results[0].detail.contains("different") && results[0].detail.contains("tree"),
            "a blocked answer with the tools present is only honest when it names the reason: {}",
            results[0].detail
        );
        return;
    }
    assert!(results[0].detail.contains('%'), "{}", results[0].detail);
}
