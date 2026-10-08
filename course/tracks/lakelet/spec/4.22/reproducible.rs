//! Spec for `lakelet/4.22-reproducible-build`: the same source, the same bytes, and the supply chain
//! that decides what "the same source" means.
//!
//! `kiln run` copies this file into the project as `tests/spec_reproducible.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Sixteen cases, and the shape of them is the point:
//!
//! * **the reproducer runs.** One case starts `scripts/repro.sh` and keeps its output for every other
//!   case — two release builds, which is why this spec's deadline is what it is. Nothing here builds
//!   an image: the grading build is `container-repro`'s job, and a spec that built one would make
//!   every step of this track depend on a container engine;
//! * **the page is the run.** `docs/build.md` carries a fenced block, and case 3 re-runs the script
//!   and compares that block line for line (`docs/authoring.md` §7.4, handoff-m3 §1 rule 18). The
//!   block carries counts, names and a verdict and never a hash, a path or a time, because those are
//!   the numbers that move and a page carrying one goes stale in silence;
//! * **the comparison really happened.** The spec sets `LAKELET_REPRO_ROOT` and
//!   `LAKELET_REPRO_KEEP=1`, and case 4 looks *inside* the two trees for every artifact the block
//!   says it compared: a script that printed `artifacts_compared=4` without comparing anything fails
//!   here rather than passing as a well-formatted lie;
//! * **the image is two stages.** Case 5 reads the `Dockerfile` for the shape the 25 MB ceiling
//!   stands for, and case 6 asks the flag the gate asks (`--check-config`) of the daemon this tree
//!   builds. The gate itself is not run from here — it builds an image;
//! * **the policy is a file and a page that agree.** `deny.toml`'s four sections, and the lists in
//!   them, are read against this tree's own `Cargo.lock` (`cargo metadata`) and against
//!   `docs/supply-chain.md`, because a policy file whose lists nobody checked is a policy in the
//!   shape of one. `cargo-deny` is not installed here (`kiln doctor` says so), so the file is judged
//!   as a file — and the page is required to say so;
//! * **the pipeline is the course's pipeline.** `.github/workflows/ci.yml` is read as **text**: no
//!   YAML parser is a dependency of this project and one must not become one, so the cases assert the
//!   commands and their order — which is the part a reviewer argues about — and say in their own
//!   words that they are reading characters rather than a document.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The pin, as a file: the compiler is one of a build's inputs.
const TOOLCHAIN: &str = "rust-toolchain.toml";
/// The reproducer: two builds, two directories, compared byte for byte.
const REPRODUCER: &str = "scripts/repro.sh";
/// The page this step delivers, and the one that quotes a run.
const PAGE: &str = "docs/build.md";
/// The policy a reviewer reads.
const SUPPLY: &str = "docs/supply-chain.md";
/// The policy a tool would run.
const POLICY: &str = "deny.toml";
/// The image this project ships as.
const DOCKERFILE: &str = "Dockerfile";
/// The pipeline, as code.
const WORKFLOW: &str = ".github/workflows/ci.yml";
/// The dependency pin.
const LOCKFILE: &str = "Cargo.lock";
/// What the tree declares about itself.
const MANIFEST: &str = "Cargo.toml";

/// The two base images the step's metadata declares (`images = [...]` in `track.toml`), in the order
/// the `Dockerfile` names them. The `container-repro` gate resolves these defaults against the local
/// image store, so a literal `FROM` would break a machine behind a registry mirror.
const IMAGES: [&str; 2] = ["rust:1.98-alpine", "alpine:3.20"];

/// The keys the comparison's block carries, in the order the script prints them.
///
/// The order is part of the contract for the same reason the keys are: the block is quoted on a page
/// and compared line for line, so a block that grew a line in the middle is a page that stopped
/// being true. Every value here is a count, a name or a verdict — nothing that moves.
const BLOCK_KEYS: [&str; 12] = [
    "toolchain",
    "locked",
    "builds",
    "build_roots",
    "target_roots",
    "artifacts",
    "artifacts_compared",
    "artifacts_identical",
    "artifacts_differing",
    "difference_path",
    "difference_timestamp",
    "verdict",
];

/// The two differences two builds of one tree can have, as the block names them.
const DIFFERENCES: [&str; 2] = ["path", "timestamp"];

/// Keys whose *name* is the shape of a number that a wall clock produced. The script may measure one
/// (it compares the two copies' modification times), and a judged line may carry the *verdict* of
/// that measurement — never the number, because the number is different on every machine and the
/// page that quotes it would be true on exactly one of them (`lakelet/4.19-backup-restore` made the
/// same trade for a restore's RTO).
const TIME_KEYS: [&str; 6] = [
    "millis",
    "micros",
    "seconds",
    "elapsed",
    "duration",
    "wall_clock",
];

/// The cargo subcommands this project's own files may name.
///
/// The pinned toolchain's components are `rustfmt` and `clippy`, so the commands this project can run
/// are cargo's own plus those two: everything else — `cargo deny`, `cargo audit`, `cargo mutants`,
/// `cargo nextest`, `cargo llvm-cov` — is a cargo plugin somebody installed, and a check that names
/// one is a check that passes on that laptop and fails on the runner.
const CARGO_BUILTINS: [&str; 8] = [
    "build", "clippy", "doc", "fmt", "metadata", "run", "test", "tree",
];

/// One path inside the project, from the project's root.
fn project(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// A delivered file, as text — the failure every case starts with.
fn text(relative: &str) -> String {
    std::fs::read_to_string(project(relative)).unwrap_or_else(|error| {
        panic!("{relative} cannot be read: {error} — this step delivers it")
    })
}

/// A delivered file, parsed as TOML.
fn parsed(relative: &str) -> toml::Value {
    let source = text(relative);
    toml::from_str(&source).unwrap_or_else(|error| panic!("{relative} is not TOML: {error}"))
}

/// The lines of a string, without their endings.
fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// One run of the reproducer, and where it built.
struct Ran {
    /// Exit code, or `-1` when the script was killed by a signal.
    code: i32,
    /// What it printed on stdout: the block, and nothing else.
    stdout: String,
    /// What it printed on stderr: the two builds' progress.
    stderr: String,
    /// The root it built the two copies under, still there (`LAKELET_REPRO_KEEP=1`).
    root: PathBuf,
}

/// The reproducer's run, once per test binary.
///
/// Two release builds of this project live behind this call, and every case that needs the block
/// reads this one run: running the script once per case would make this spec minutes-long for no
/// question it asks differently. The two knobs the script documents for exactly this are set here —
/// the root is a directory this process owns, and `LAKELET_REPRO_KEEP` leaves the two trees behind so
/// that case 4 can look at what the block says was compared.
fn ran() -> &'static Ran {
    static RUN: OnceLock<Ran> = OnceLock::new();
    RUN.get_or_init(|| {
        let script = project(REPRODUCER);
        let root = std::env::temp_dir().join(format!("lakelet-repro-spec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Through the shell, which is how the pipeline runs it: the file arrives as a patch, and a
        // patch carries content rather than a mode bit, so the executable bit is not something this
        // step can deliver — and a command that only works when somebody remembered `chmod` is not
        // a command. `sh` does the same thing either way.
        let output = Command::new("sh")
            .arg(&script)
            .env("LAKELET_REPRO_ROOT", &root)
            .env("LAKELET_REPRO_KEEP", "1")
            // The script decides the flags its two builds use; an environment that also sets
            // `RUSTFLAGS` would be a second opinion about that, and the comparison would be of
            // something nobody wrote down.
            .env_remove("RUSTFLAGS")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap_or_else(|error| {
                panic!(
                    "`sh {REPRODUCER}` could not be started: {error} — the step's own command has to \
                     run in the project whoever cloned it"
                )
            });
        Ran {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            root,
        }
    })
}

/// The block the reproducer printed, as `name -> value`.
///
/// Parsing it strictly is a claim of this spec rather than a convenience: one `name=value` per line
/// is what makes a block quotable on a page and comparable by a person, and a line that is prose
/// fails here.
fn block() -> BTreeMap<String, String> {
    let run = ran();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in run.stdout.lines() {
        assert!(
            !line.trim().is_empty(),
            "{REPRODUCER} printed a blank line on stdout: the block is one `name=value` line per \
             fact and nothing else, because a page quotes it"
        );
        let Some((name, value)) = line.split_once('=') else {
            panic!(
                "{REPRODUCER} printed `{line}` on stdout: every line of the block is one \
                 `name=value` pair (rule 16), or the page that quotes it cannot be checked"
            );
        };
        assert!(
            !name.contains(' ') && !value.is_empty(),
            "{REPRODUCER} printed `{line}`: a key is one word and a value is never empty"
        );
        map.insert(name.to_owned(), value.to_owned());
    }
    map
}

/// One key of the block, or a sentence naming what is missing.
fn value(map: &BTreeMap<String, String>, key: &str) -> String {
    map.get(key).cloned().unwrap_or_else(|| {
        panic!(
            "{REPRODUCER}'s block has no `{key}=` line: the block carries {} and that one says \
             what a reader cannot guess",
            BLOCK_KEYS.join(", ")
        )
    })
}

/// The fenced block a page quotes: the one whose first line is the block's first key.
///
/// Identified by its content rather than by a heading, because what makes it the run's output is that
/// it *is* the run's output — a page may quote a fragment of the same block elsewhere without that
/// fragment being the thing this spec compares.
fn quoted(page: &str) -> Vec<String> {
    let lines = lines(page);
    let anchor = lines
        .iter()
        .position(|line| line.starts_with(&format!("{}=", BLOCK_KEYS[0])))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} does not quote the comparison: a reader has to be able to see the run this \
                 step is about, and no line of the page starts at `{}=`",
                BLOCK_KEYS[0]
            )
        });
    let opening = lines[..anchor]
        .iter()
        .rposition(|line| line.trim_start().starts_with("```"))
        .unwrap_or_else(|| {
            panic!(
                "{PAGE} has the block's first line outside a fenced block: it has to be copyable"
            )
        });
    let closing = anchor
        + lines[anchor..]
            .iter()
            .position(|line| line.trim_start().starts_with("```"))
            .unwrap_or_else(|| panic!("{PAGE} opens a fenced block it never closes"));
    lines[opening + 1..closing].to_vec()
}

/// The `run:` lines of the workflow, in the order they are written.
///
/// The file is read as **text**: this project has no YAML parser and does not want one, so what these
/// cases judge is the commands a reviewer reads in the diff — not indentation, and not a document
/// model that would have to be a dependency of a store that stores telemetry.
fn workflow_runs() -> Vec<String> {
    let file = text(WORKFLOW);
    let mut runs: Vec<String> = Vec::new();
    for line in lines(&file) {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("- run:") else {
            continue;
        };
        runs.push(rest.trim().to_owned());
    }
    runs
}

/// A piece of text with its comment lines removed.
///
/// A comment may name a command this project does not have — `docs/supply-chain.md` and the
/// workflow's own comments both name `cargo deny` while saying that it is deliberately not run — and
/// prose is not a claim that a command *exists*. What this spec reads is what runs.
fn code(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<&str>>()
        .join("\n")
}

/// Every `cargo <subcommand>` a piece of text names, as `(subcommand, where)`.
fn cargo_subcommands(text: &str, where_: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        while let Some(at) = rest.find("cargo ") {
            rest = &rest[at + "cargo ".len()..];
            let sub: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if sub.is_empty() {
                break;
            }
            found.push((sub.clone(), where_.to_owned()));
            rest = &rest[sub.len()..];
        }
    }
    found
}

/// The fenced blocks of a page that are shell transcripts.
fn shell_blocks(page: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in page.lines() {
        let trimmed = line.trim_start();
        match &mut current {
            None => {
                if let Some(language) = trimmed.strip_prefix("```") {
                    let language = language.trim();
                    if matches!(language, "sh" | "bash" | "console" | "shell") {
                        current = Some(String::new());
                    }
                }
            }
            Some(body) => {
                if trimmed.starts_with("```") {
                    blocks.push(body.clone());
                    current = None;
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }
    blocks
}

/// Whether `expression` is satisfied by an allow-list, with `excepted` holding one crate's own set.
///
/// Deliberately the simple half of SPDX: `AND` needs both sides, `OR` needs one, and parentheses are
/// stripped. Every expression this lockfile declares is of that shape, and a case that guessed at the
/// rest of the grammar would be a parser rather than a check.
fn licensed(
    expression: &str,
    allow: &BTreeSet<String>,
    excepted: Option<&BTreeSet<String>>,
) -> bool {
    expression.split(" AND ").all(|conjunct| {
        let conjunct = conjunct
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')');
        conjunct.split(" OR ").map(str::trim).any(|alternative| {
            allow.contains(alternative) || excepted.is_some_and(|set| set.contains(alternative))
        })
    })
}

/// What this tree's own lockfile declares, as `(crate, licence or None)` and the duplicated names.
///
/// `cargo metadata` is the authority on both questions and it is the reason the policy file can be
/// judged against the tree rather than against a comment: the licences a `[licenses]` list has to
/// admit, and the crates a `[bans]` rule has to make an exception for, are the answers it gives.
fn lockfile_licences() -> (Vec<(String, Option<String>)>, BTreeSet<String>) {
    let manifest = project(MANIFEST);
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|error| panic!("`cargo metadata` could not be started: {error}"));
    assert!(
        output.status.success(),
        "`cargo metadata --locked` failed, so this tree's dependency graph cannot be read — which is \
         the same failure a `--locked` build would have: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("`cargo metadata` did not answer JSON: {error}"));
    let own = parsed["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| {
                    package["manifest_path"]
                        .as_str()
                        .is_some_and(|path| Path::new(path) == manifest.as_path())
                })
                .and_then(|package| package["name"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut licences: Vec<(String, Option<String>)> = Vec::new();
    for package in parsed["packages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        if name == own {
            continue;
        }
        let version = package["version"].as_str().unwrap_or_default().to_owned();
        seen.entry(name.to_owned()).or_default().insert(version);
        licences.push((
            name.to_owned(),
            package["license"].as_str().map(str::to_owned),
        ));
    }
    let duplicated: BTreeSet<String> = seen
        .into_iter()
        .filter(|(_, versions)| versions.len() > 1)
        .map(|(name, _)| name)
        .collect();
    (licences, duplicated)
}

#[test]
fn the_pin_is_a_file_and_the_compiler_answering_is_the_one_it_names() {
    let source = text(TOOLCHAIN);
    let parsed: toml::Value =
        toml::from_str(&source).unwrap_or_else(|error| panic!("{TOOLCHAIN} is not TOML: {error}"));
    let channel = parsed
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("channel"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| {
            panic!("{TOOLCHAIN} declares no `[toolchain] channel`, so nothing is pinned")
        });
    let parts: Vec<&str> = channel.split('.').collect();
    assert!(
        parts.len() == 3 && parts.iter().all(|part| part.parse::<u64>().is_ok()),
        "`channel = \"{channel}\"` is not a pinned version: `stable` and `nightly` move under a \
         build that changed nothing, and a channel that moves is the one difference this step's \
         whole page is about. Name the three numbers."
    );
    let components: Vec<String> = parsed["toolchain"]["components"]
        .as_array()
        .map(|array| {
            array
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for needed in ["rustfmt", "clippy"] {
        assert!(
            components
                .iter()
                .any(|component| component.as_str() == needed),
            "{TOOLCHAIN} does not name `{needed}` among its components: this project's gates run \
             it (`fmt`, `clippy@4`, `no-todo`), and a toolchain without it turns a lint into a \
             missing command on somebody else's machine"
        );
    }
    // The half that makes the file real rather than a document: the compiler that runs this case is
    // the one the file names. rustup honours the file because it sits in the project, so a project
    // whose pin is `stable` answers with whatever the day's Rust is — which is what this compares.
    let answered = Command::new("rustc")
        .arg("--version")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("`rustc` is on PATH; rust-toolchain.toml decides which one");
    let reported = String::from_utf8_lossy(&answered.stdout).into_owned();
    assert!(
        reported.contains(channel),
        "{TOOLCHAIN} pins {channel} and the compiler running this case reports `{}`: a pin only \
         bites through rustup, so either the channel is not installed (`rustup toolchain install \
         {channel}`) or this project is being built by something else",
        reported.trim()
    );
}

#[test]
fn the_pin_is_this_course_pin_and_not_a_second_opinion() {
    // The course's own file, found by walking up from the project: in the course's checkout the
    // project is `<repository>/app/lakelet` and the file is at the root, and in the sandbox `kiln
    // selftest` builds the file sits one level higher still — so the walk asks the tree rather than
    // counting directory names.
    let pin = parsed(TOOLCHAIN);
    let channel = pin["toolchain"]["channel"].as_str().unwrap_or_default();
    let mut found: Option<PathBuf> = None;
    let mut at = project("").parent().map(Path::to_path_buf);
    for _ in 0..5 {
        let Some(directory) = at else { break };
        let candidate = directory.join(TOOLCHAIN);
        if candidate.is_file() {
            found = Some(candidate);
            break;
        }
        at = directory.parent().map(Path::to_path_buf);
    }
    let Some(course) = found else {
        // A project copied out of the course is a project whose earlier steps do not exist either;
        // there is nothing above it to disagree with, and this case says so instead of passing
        // silently on a question it could not ask.
        eprintln!(
            "no `{TOOLCHAIN}` above this project: this tree is not inside a course checkout, so \
             there is no course pin to compare with — the channel this project pins is {channel}"
        );
        return;
    };
    let outside = std::fs::read_to_string(&course)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", course.display()));
    let outside: toml::Value = toml::from_str(&outside)
        .unwrap_or_else(|error| panic!("{} is not TOML: {error}", course.display()));
    let theirs = outside["toolchain"]["channel"].as_str().unwrap_or_default();
    assert_eq!(
        channel, theirs,
        "this project pins {channel} and the course pins {theirs}: this step adds a project's own \
         pin rather than a second opinion about which Rust the course uses, and two pins are two \
         answers the day they disagree — a difference that would show up in every earlier step's \
         build"
    );
}

#[test]
fn the_reproducer_answers_a_run_as_the_pipeline_runs_it() {
    let source = text(REPRODUCER);
    let first = source.lines().next().unwrap_or_default();
    assert!(
        first.starts_with("#!"),
        "{REPRODUCER} does not start with a `#!` line: the step's reproducer is a command that \
         `sh` and the pipeline both run, and a file without an interpreter line is a file somebody \
         has to read to find out what runs it"
    );
    // The executable bit is deliberately *not* judged, and this is the reason rather than an
    // oversight: the step's files arrive as a patch, and this course's patch applier writes content
    // (`kiln/src/patch.rs` accepts `new mode` lines and ignores them), so a mode bit set in the
    // patch is a promise the applier does not keep. What the spec and the pipeline both run is
    // `sh scripts/repro.sh`, which works with the bit and without it; `chmod +x` in the project
    // makes it a command a person can run directly, and the page says so.
    let mode = std::fs::metadata(project(REPRODUCER))
        .unwrap_or_else(|error| panic!("{REPRODUCER} cannot be read: {error}"))
        .permissions()
        .mode();
    assert!(
        mode & 0o644 == 0o644,
        "{REPRODUCER} is not readable (mode {:o})",
        mode & 0o777
    );
    let run = ran();
    assert_eq!(
        run.code, 0,
        "`sh {REPRODUCER}` exited {}: the two builds it compares did not come out the same, or they \
         did not come out at all.\nstdout:\n{}\nstderr (the last lines of two builds):\n{}",
        run.code, run.stdout, run.stderr
    );
    let map = block();
    assert_eq!(
        map.len(),
        BLOCK_KEYS.len(),
        "the block has {} line(s) and this step's block is {}: a block that grew or lost a line is \
         a page that stopped being what the script prints",
        map.len(),
        BLOCK_KEYS.len()
    );
    for key in BLOCK_KEYS {
        value(&map, key);
    }
    assert_eq!(
        value(&map, "verdict"),
        "reproducible",
        "the run says `verdict={}`: a script that exits 0 has to say so in its own block, or the \
         exit code is the whole verdict and the page carrying it says nothing",
        value(&map, "verdict")
    );
}

#[test]
fn the_page_carries_the_block_the_reproducer_prints() {
    let page = text(PAGE);
    let printed = lines(&ran().stdout);
    let quoted = quoted(&page);
    assert_eq!(
        quoted, printed,
        "{PAGE}'s block is not what {REPRODUCER} prints: the page's numbers are a measurement, and \
         the way a page full of measurements stays true is that the command which produced them is \
         re-run and compared (handoff-m3 §1 rule 18). Edit the block from a run, never from an \
         intention."
    );
    assert!(
        page.contains("stderr"),
        "{PAGE} does not say that the block is stdout and that progress goes to stderr: a reader \
         who runs the script and sees twenty `Compiling` lines has to know which of the two streams \
         the page is quoting"
    );
}

#[test]
fn the_run_compared_files_that_exist_in_both_builds() {
    let map = block();
    let compared: u32 = value(&map, "artifacts_compared")
        .parse()
        .unwrap_or_else(|_| panic!("`artifacts_compared` is not a count"));
    assert!(
        compared >= 2,
        "the run compared {compared} artifact(s): a comparison that is one file long is a \
         comparison that cannot tell a build from a copy of itself, and this step's claim is about \
         two builds"
    );
    let named: Vec<String> = value(&map, "artifacts")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    assert_eq!(
        named.len() as u32,
        compared,
        "the block names {} artifact(s) and counts {compared}: the two lines are the same fact said \
         twice, which is what makes a block readable by a person and checkable by this case",
        named.len()
    );

    let run = ran();
    for name in &named {
        let mut found: Vec<PathBuf> = Vec::new();
        collect(&run.root, name, &mut found);
        assert_eq!(
            found.len(),
            2,
            "{name} appears {} time(s) under the two trees: the block says it compared this \
             artifact in two builds, and this case looks for it rather than believing the line",
            found.len()
        );
        let left = std::fs::read(&found[0])
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", found[0].display()));
        let right = std::fs::read(&found[1])
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", found[1].display()));
        assert_eq!(
            left.len(),
            right.len(),
            "the two copies of {name} are {} and {} bytes: the block says every compared artifact \
             was identical",
            left.len(),
            right.len()
        );
        assert!(
            left == right,
            "the two copies of {name} differ, and the block says they do not — a comparison whose \
             verdict and whose files disagree is the one thing this whole step refuses"
        );
        // Where each copy was built: the artifact's path without the target directory it came out of.
        for (file, bytes) in [(&found[0], &left), (&found[1], &right)] {
            let location = file
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .unwrap_or_else(|| panic!("{} is not inside a target directory", file.display()));
            let needle = location.as_os_str().as_encoded_bytes();
            assert!(
                !contains(bytes, needle),
                "{} names the directory it was built in ({}): `--remap-path-prefix` did not cover \
                 the build location, so the two copies are equal for a reason nobody wrote down — \
                 and on two machines they would not be",
                file.display(),
                location.display()
            );
        }
        assert_ne!(
            found[0].parent(),
            found[1].parent(),
            "the two copies of {name} are in the same directory: a comparison of one build with \
             itself proves nothing"
        );
    }
    // The two target directories are what the 25 MB image and the whole comparison hang off, and
    // they are also hundreds of megabytes. Nothing here compares them again, so the spec takes them
    // away rather than leaving a gigabyte in a temporary directory nobody looks at.
    let _ = std::fs::remove_dir_all(&run.root);
}

/// Every file named `name` under `root`, for the case that looks at what the block compared.
fn collect(root: &Path, name: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, name, found);
        } else if path.file_name().is_some_and(|file| file == name) {
            found.push(path);
        }
    }
}

/// Whether a byte string occurs in another, the check a path-remap claim needs.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[test]
fn the_dockerfile_is_two_stages_and_the_runtime_stage_is_not_the_builder() {
    let file = text(DOCKERFILE);
    let mut defaults: BTreeMap<String, String> = BTreeMap::new();
    let mut stages: Vec<(String, String)> = Vec::new();
    let mut runtime_lines: Vec<String> = Vec::new();
    let mut in_runtime = false;
    for line in lines(&file) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("ARG ")
            && let Some((name, value)) = rest.split_once('=')
        {
            defaults.insert(name.trim().to_owned(), value.trim().to_owned());
        }
        if let Some(rest) = trimmed.strip_prefix("FROM ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            let image = parts.first().copied().unwrap_or_default().to_owned();
            // `FROM <image> AS <name>`, and a `FROM` without a name is a stage named after its
            // image — which is why an unnamed second stage is caught below rather than accepted.
            let stage = match parts.as_slice() {
                [_, keyword, name, ..] if keyword.eq_ignore_ascii_case("AS") => (*name).to_owned(),
                _ => image.clone(),
            };
            stages.push((image, stage));
            in_runtime = stages.len() == 2;
            continue;
        }
        if in_runtime && !trimmed.starts_with('#') && !trimmed.is_empty() {
            runtime_lines.push(trimmed.to_owned());
        }
    }
    assert_eq!(
        stages.len(),
        2,
        "{DOCKERFILE} has {} `FROM` line(s): this step's image is two stages — one that has the \
         toolchain and disappears, and one that has a binary — and a one-stage image ships the \
         compiler, the sources and a build environment's whole surface",
        stages.len()
    );
    // The base images are the step's own metadata, named as `ARG` defaults so that a machine with a
    // registry mirror can resolve them: the gate reads exactly these lines.
    assert_eq!(
        defaults.get("BUILDER").map(String::as_str),
        Some(IMAGES[0]),
        "{DOCKERFILE} does not default `BUILDER` to `{}`: the builder is one of the two images this \
         step declares, and the `container-repro` gate resolves this default against the local image \
         store instead of letting a `FROM` reach a registry",
        IMAGES[0]
    );
    assert_eq!(
        defaults.get("RUNTIME").map(String::as_str),
        Some(IMAGES[1]),
        "{DOCKERFILE} does not default `RUNTIME` to `{}`: the runtime stage is what the 25 MB \
         ceiling judges, and it is the second image the step declares",
        IMAGES[1]
    );
    assert_eq!(
        stages[0].0, "${BUILDER}",
        "{DOCKERFILE}'s builder stage does not start from the `BUILDER` argument: a literal image \
         name is the one part of a build a mirror cannot resolve"
    );
    assert_eq!(
        stages[1].0, "${RUNTIME}",
        "{DOCKERFILE}'s runtime stage does not start from the `RUNTIME` argument"
    );
    assert!(
        stages[0].1 != stages[1].1,
        "{DOCKERFILE} names both stages `{}`: the runtime stage has to be a stage of its own, or \
         `COPY --from` copies from the build environment into the shipped image",
        stages[0].1
    );
    assert!(
        runtime_lines
            .iter()
            .any(|line| line.starts_with("COPY --from=")),
        "{DOCKERFILE}'s runtime stage never copies anything out of the builder: the image has to \
         carry the binary this tree builds"
    );
    for forbidden in ["cargo build", "cargo install", "rustc", "apk add"] {
        assert!(
            !runtime_lines.iter().any(|line| line.contains(forbidden)),
            "{DOCKERFILE}'s runtime stage runs `{forbidden}`: that stage is the shipped image, and \
             the 25 MB ceiling is the assertion that it is a runtime rather than a build \
             environment — the toolchain belongs to the stage that disappears"
        );
    }
    let carries_target = runtime_lines.iter().any(|line| {
        line.contains("--from=builder /src") || line.contains("--from=builder /build/")
    });
    assert!(
        !carries_target,
        "{DOCKERFILE}'s runtime stage copies the builder's whole workspace: what crosses over is \
         the binary, and a stage that carries `src/` is a stage that carries a build environment"
    );
}

#[test]
fn the_image_answers_the_configuration_question_the_gate_asks() {
    let file = text(DOCKERFILE);
    let entry = lines(&file)
        .into_iter()
        .find(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("ENTRYPOINT") || trimmed.starts_with("CMD")
        })
        .unwrap_or_else(|| {
            panic!(
                "{DOCKERFILE} has neither `ENTRYPOINT` nor `CMD`: `docker run <tag> --check-config` \
                 in the `container-repro` gate runs the image's entry point with that argument, and \
                 an image whose entry point is a shell answers `--check-config: not found`"
            )
        });
    assert!(
        entry.contains("lakeletd"),
        "{DOCKERFILE}'s entry point is `{entry}` and this project's daemon is `lakeletd`: the gate \
         asks the image the same question it asks this tree's binary, so the entry point has to be \
         the binary that answers it"
    );
    // The question, asked of the answerer directly. The gate asks the *host* through `cargo run
    // --package lakelet -- --check-config`, which runs the binary the project names as its
    // `default-run`; this case runs that same daemon out of `cargo test`'s build of it, so the
    // question is judged without depending on the manifest line — the case beside this one is
    // what holds that line to the daemon.
    let answered = Command::new(env!("CARGO_BIN_EXE_lakeletd"))
        .arg("--check-config")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("`cargo test` builds this project's binaries, and this one is the daemon");
    assert!(
        answered.status.success(),
        "`lakeletd --check-config` exited {:?}: this is the question the `container-repro` gate \
         asks the image and the host, and it has to have an answer: {}",
        answered.status.code(),
        String::from_utf8_lossy(&answered.stderr).trim()
    );
    let report = String::from_utf8_lossy(&answered.stdout).into_owned();
    let printed: Vec<&str> = report
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        printed.len() >= 4,
        "`lakeletd --check-config` printed {} line(s): the report is the four-layer configuration, \
         and the gate compares its lines one by one between the image and this host",
        printed.len()
    );
    for layer in [
        "listen.addr = ",
        "store.dir = ",
        "log.level = ",
        "log.format = ",
    ] {
        assert!(
            report.contains(layer),
            "`lakeletd --check-config` does not report `{layer}`: a report the gate compares has to \
             name the settings a deployment would otherwise have to guess"
        );
    }
}

#[test]
fn the_project_names_the_binary_the_gate_asks_as_its_default() {
    let manifest = parsed(MANIFEST);
    let named = manifest
        .get("package")
        .and_then(|package| package.get("default-run"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "`{MANIFEST}` declares no `package.default-run` and this project has three \
                 binaries, so `cargo run --package lakelet -- --check-config` — the question \
                 the `container-repro` gate asks this tree — ends in cargo refusing to guess \
                 which of them to run, and the gate never gets to judge an answer. Name the \
                 daemon."
            )
        });
    assert_eq!(
        named, "lakeletd",
        "`default-run = \"{named}\"` is not the binary the image runs: `{DOCKERFILE}`'s entry \
         point is the daemon and it is the process that answers `--check-config`, so the two \
         files have to name the same one"
    );
}

#[test]
fn the_policy_file_states_four_answers_that_fit_this_tree() {
    let policy = parsed(POLICY);
    for section in ["advisories", "licenses", "bans", "sources"] {
        let table = policy.get(section).unwrap_or_else(|| {
            panic!("{POLICY} has no `[{section}]` section: the policy answers four questions — what \
                    is vulnerable, what may be built from, what may be here twice, and where a crate \
                    may come from — and a section nobody wrote is a question nobody answered")
        });
        let entries = table.as_table().map(|table| table.len()).unwrap_or(0);
        assert!(
            entries > 0,
            "{POLICY}'s `[{section}]` section is empty: `cargo-deny` reads a section with no keys as \
             a section with no policy, so an empty one is a comment with brackets around it"
        );
    }
    let (licences, duplicated) = lockfile_licences();
    let allow: BTreeSet<String> = policy["licenses"]["allow"]
        .as_array()
        .map(|array| {
            array
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| {
            panic!(
                "{POLICY}'s `[licenses]` has no `allow` list: an empty allow-list refuses every \
                    dependency this tree has, which is a policy nobody can run"
            )
        });
    let mut excepted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for exception in policy["licenses"]["exceptions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(name) = exception["name"].as_str() else {
            continue;
        };
        let set: BTreeSet<String> = exception["allow"]
            .as_array()
            .map(|array| {
                array
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        excepted.insert(name.to_owned(), set);
    }
    for (name, licence) in &licences {
        let Some(expression) = licence else {
            panic!(
                "{name} declares no licence in `cargo metadata`: a policy that watches licences \
                 cannot see a crate that states none, and this tree's policy file says which \
                 packages it means and which it does not"
            );
        };
        assert!(
            licensed(expression, &allow, excepted.get(name)),
            "{name} is licensed `{expression}`, which `{POLICY}` does not admit: the allow-list is \
             this tree's own licence census, so either the crate leaves, or the licence is added to \
             `[licenses] allow`, or the crate is named in `[licenses] exceptions` with the reason \
             written next to it — the three are different decisions and this case is where they get \
             made"
        );
    }
    let skipped: BTreeSet<String> = policy["bans"]["skip"]
        .as_array()
        .map(|array| {
            array
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|| {
            panic!(
                "{POLICY}'s `[bans]` has no `skip` list, and this lockfile holds {} crate(s) twice: \
                 a rule that refuses every duplicate refuses this tree's own build",
                duplicated.len()
            )
        });
    for name in &duplicated {
        assert!(
            skipped.contains(name),
            "`{name}` appears in this lockfile at more than one version and `{POLICY}` does not \
             name it in `[bans] skip`: a duplicate is either refused or written down with the reason \
             it is here, because the failure mode of the other choice is a second copy nobody \
             audits"
        );
    }
}

#[test]
fn the_policy_and_the_page_state_the_same_thing() {
    let policy = parsed(POLICY);
    let page = text(SUPPLY);
    // Every key the file sets. A page that promises a policy the file does not state, or a file that
    // refuses something the page never mentions, is the drift this pair exists to catch — so the
    // page has to name each key it is describing, and the names below are read out of the file
    // rather than listed here.
    let mut keys: Vec<String> = Vec::new();
    for section in ["advisories", "licenses", "bans", "sources"] {
        if let Some(table) = policy.get(section).and_then(toml::Value::as_table) {
            for key in table.keys() {
                keys.push(key.clone());
            }
        }
    }
    for key in &keys {
        assert!(
            page.contains(key.as_str()),
            "{SUPPLY} never names `{key}`, which `{POLICY}` sets: the file is the policy a tool \
             would run and the page is the policy a reviewer reads, and a reader who cannot find a \
             key on the page has to read TOML to find out what the project does"
        );
    }
    // The lists, item by item: the licences this file admits, the crates it excepts, the duplicates
    // it tolerates and the sources it trusts.
    let mut named: Vec<String> = Vec::new();
    for licence in policy["licenses"]["allow"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(licence) = licence.as_str() {
            named.push(licence.to_owned());
        }
    }
    for exception in policy["licenses"]["exceptions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(name) = exception["name"].as_str() {
            named.push(name.to_owned());
        }
        for licence in exception["allow"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            if let Some(licence) = licence.as_str() {
                named.push(licence.to_owned());
            }
        }
    }
    for crate_ in policy["bans"]["skip"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(name) = crate_.as_str() {
            named.push(name.to_owned());
        }
    }
    for source in policy["sources"]["allow-registry"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(url) = source.as_str() {
            named.push(url.to_owned());
        }
    }
    for url in policy["advisories"]["db-urls"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(url) = url.as_str() {
            named.push(url.to_owned());
        }
    }
    for entry in &named {
        assert!(
            page.contains(entry.as_str()),
            "{SUPPLY} does not name `{entry}`, which `{POLICY}` lists: the two files are one policy, \
             and a list only one of them carries is a policy the other one contradicts by silence"
        );
    }
    assert!(
        page.to_ascii_lowercase().contains("cargo-deny")
            || page.to_ascii_lowercase().contains("cargo deny"),
        "{SUPPLY} never names the tool the policy file is written for: a reader has to know that \
         `{POLICY}` is data for a program, and that the program is not installed here"
    );
}

#[test]
fn the_workflow_runs_the_course_commands_in_the_course_order() {
    // Read as text, not parsed: this project has no YAML parser and must not grow one (see the
    // module comment). What is judged is the commands and their order.
    let runs = workflow_runs();
    assert!(!runs.is_empty(), "{WORKFLOW} has no `run:` step");
    let checks: Vec<&String> = runs
        .iter()
        .filter(|run| run.starts_with("cargo ") || run.contains(REPRODUCER))
        .collect();
    // Four, in this order: the format check, clippy, the tests, the reproducer.
    let expected = [
        "cargo fmt --all --check",
        "cargo clippy --workspace --all-targets",
        "cargo test --workspace",
        REPRODUCER,
    ];
    assert_eq!(
        checks.len(),
        expected.len(),
        "{WORKFLOW} runs {} check(s) and this project's pipeline is {}: the commands a reviewer \
         runs are the commands CI runs, in the order that fails cheapest first",
        checks.len(),
        expected.len()
    );
    for (step, want) in checks.iter().zip(expected) {
        assert!(
            step.contains(want),
            "{WORKFLOW} runs `{step}` where this pipeline's step is `{want}`: the pipeline is code \
             a reviewer reads, and a step that drifted from the others is the drift `mailer/1.3-ci` \
             argued against"
        );
    }
    assert!(
        checks[1].contains("-D warnings"),
        "{WORKFLOW} runs `{}`: a lint that is a warning today is an error in a later toolchain, and \
         `-D warnings` is what makes the ladder a rung rather than a habit",
        checks[1]
    );
    // The two steps that resolve the dependency graph, and the flag that keeps that resolution the
    // lockfile's: a pipeline without it can go green on a graph nobody committed.
    for step in [&checks[1], &checks[2]] {
        assert!(
            step.contains("--locked"),
            "{WORKFLOW} runs `{step}` without `--locked`: a check that resolves the graph freely \
             can pass on dependencies this project never pinned, which is the half of ``the same \
             source'' that a lockfile is for"
        );
    }
    let file = text(WORKFLOW);
    for forbidden in ["1.98.1", "stable", "nightly", "dtolnay/rust-toolchain"] {
        assert!(
            !file.contains(forbidden),
            "{WORKFLOW} names `{forbidden}`: the channel is decided by `{TOOLCHAIN}` in the \
             repository, and a second pin is a second answer to which Rust this project uses"
        );
    }
    assert!(
        file.contains("rustup"),
        "{WORKFLOW} never runs `rustup`: the runner's toolchain comes from `{TOOLCHAIN}` in the \
             checkout, and a job that never prints which toolchain it selected cannot be read when \
             the file above is the thing that changed"
    );
}

#[test]
fn no_file_of_this_step_names_a_command_this_project_does_not_have() {
    // The workflow first, because that is where a command nobody has gets *executed*.
    for run in workflow_runs() {
        let program = run.split_whitespace().next().unwrap_or_default();
        assert!(
            matches!(program, "cargo" | "rustup" | "sh") || program.ends_with(REPRODUCER),
            "{WORKFLOW} runs `{run}`: a step of this pipeline runs `cargo` or `rustup` (the tools \
             the pinned toolchain installs), `sh {REPRODUCER}` (the step's own command, run through \
             the interpreter because a patch does not carry a mode bit), and nothing else — a \
             command from somewhere else is a check that passes on the laptop that has it"
        );
    }
    // Then every `cargo <subcommand>` the project's own files name, wherever they name it. The
    // pinned toolchain's components are `rustfmt` and `clippy`, so the commands this project can run
    // are cargo's own plus those two: `cargo deny`, `cargo audit`, `cargo mutants` and `cargo
    // nextest` are plugins somebody installed, and `deny.toml` is judged as a file on this machine
    // precisely because the tool that would read it is not in the pin.
    let mut named = cargo_subcommands(&code(&text(WORKFLOW)), WORKFLOW);
    for page in [PAGE, SUPPLY] {
        for block in shell_blocks(&text(page)) {
            named.extend(cargo_subcommands(&block, page));
        }
    }
    for (sub, where_) in &named {
        assert!(
            CARGO_BUILTINS.contains(&sub.as_str()),
            "`{where_}` runs `cargo {sub}`, which the pinned toolchain does not provide: this \
             project's own commands are cargo's {}, and a plugin is an installation rather than a \
             pin — a check that needs one is a check that fails on the runner",
            CARGO_BUILTINS.join(", ")
        );
    }
}

#[test]
fn the_lockfile_pins_every_dependency_the_manifest_declares() {
    let manifest = parsed(MANIFEST);
    let lock = text(LOCKFILE);
    let pinned: Vec<(String, String)> = lock
        .lines()
        .collect::<Vec<&str>>()
        .windows(2)
        .filter_map(|pair| {
            let name = pair[0].strip_prefix("name = \"")?.strip_suffix('"')?;
            let version = pair[1].strip_prefix("version = \"")?.strip_suffix('"')?;
            Some((name.to_owned(), version.to_owned()))
        })
        .collect();
    let mut checked = 0usize;
    for table in ["dependencies", "dev-dependencies"] {
        let Some(entries) = manifest.get(table).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, requirement) in entries {
            let wanted = match requirement {
                toml::Value::String(version) => version.clone(),
                toml::Value::Table(options) => options
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                _ => continue,
            };
            let found = pinned.iter().find(|(crate_, _)| crate_ == name);
            let Some((_, version)) = found else {
                panic!(
                    "`{name}` is a dependency of this project in `{MANIFEST}` and `{LOCKFILE}` does \
                     not pin it: a build with `--locked` refuses rather than resolving, so a \
                     lockfile that lost a dependency is a build that does not start"
                );
            };
            let wanted_numbers: Vec<&str> = wanted.split('.').collect();
            assert!(
                version.starts_with(&wanted_numbers[..wanted_numbers.len().min(2)].join(".")),
                "`{MANIFEST}` asks for `{name}` at {wanted} and `{LOCKFILE}` pins {version}: the \
                 lockfile is the versions this tree was tested with, and a lockfile that drifted \
                 from the manifest is a build nobody reproduced"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 10,
        "only {checked} dependency(ies) of `{MANIFEST}` were checked against `{LOCKFILE}`: this \
         project's dependencies are what its builds are reproducible *against*, and a case that \
         found almost none of them is a case that stopped reading"
    );
}

#[test]
fn the_page_names_the_differences_the_run_had() {
    let page = text(PAGE);
    let map = block();
    for difference in DIFFERENCES {
        let key = format!("difference_{difference}");
        assert!(
            map.contains_key(&key),
            "{REPRODUCER}'s block has no `{key}=` line: the two differences two builds of one tree \
             can have are the path and the timestamp, and a comparison that does not name what it \
             did with each is a comparison nobody can audit"
        );
        assert!(
            page.contains(&key),
            "{PAGE} does not name `{key}`, which the run's block carries: the page has to explain \
             each named difference, or the reader is left with a key and no sentence"
        );
    }
    // The other direction, and it is the half that catches a page written from an intention: a page
    // that explains a difference the run did not have is a page about a different build.
    let mut at = 0;
    while let Some(found) = page[at..].find("difference_") {
        let start = at + found + "difference_".len();
        let name: String = page[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        at = start;
        assert!(
            DIFFERENCES.contains(&name.as_str()),
            "{PAGE} names the difference `difference_{name}`, which the run did not produce: the \
             differences a page explains are the ones the block above it reports, and a third one \
             is a page that has drifted from the build it describes"
        );
    }
    assert!(
        page.contains("--remap-path-prefix"),
        "{PAGE} names the path difference without naming the mechanism that handles it: a reader \
         who cannot see how the path stops being a difference has a claim rather than a procedure"
    );
    assert!(
        page.contains("modification time"),
        "{PAGE} names the timestamp difference without naming what it is: the difference is the \
         copies' modification times, and nothing in the artifacts records a time — a reader who \
         thinks the artifact carries one will look for a flag that does not exist"
    );
}

#[test]
fn the_supply_chain_page_names_the_four_sections_and_the_pin() {
    let page = text(SUPPLY);
    for section in ["advisories", "licenses", "bans", "sources"] {
        assert!(
            page.contains(section),
            "{SUPPLY} does not name the `[{section}]` section of `{POLICY}`: the page is where a \
             reviewer learns what the project refuses, and it refuses four kinds of thing"
        );
    }
    assert!(
        page.contains(TOOLCHAIN) || page.contains("1.98.1"),
        "{SUPPLY} does not name the compiler's pin: what is pinned is half of a supply-chain page, \
         and the pin is a file rather than a sentence"
    );
    let lowered = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !lowered.contains(placeholder),
            "{SUPPLY} still says `{placeholder}`: a page with a blank in it is a page nobody has \
             written down"
        );
    }
    // The step page links this one and the `links` gate resolves that link; a spec inside the
    // project cannot read the course's own page, so what is checked here is that this page is
    // written as a destination: it names what it is, in its first paragraph.
    assert!(
        page.lines().count() > 40,
        "{SUPPLY} is {} line(s) long: the policy a reviewer reads is the half of this step that \
         states why each refusal exists, and a stub is not that",
        page.lines().count()
    );
}

#[test]
fn nothing_on_this_steps_judged_side_reads_a_wall_clock_into_a_number() {
    let map = block();
    for (key, value) in &map {
        for bad in TIME_KEYS {
            assert!(
                !key.contains(bad),
                "{REPRODUCER}'s block carries `{key}=`: a line the page quotes and this spec \
                 compares cannot carry a number a clock produced, because that number is different \
                 on every machine and the block would be true on exactly one of them \
                 (`lakelet/4.19-backup-restore`'s rule)"
            );
        }
        assert!(
            !value.contains(':'),
            "`{key}={value}` looks like a timestamp: the block names what a difference *is*, never \
             when it happened"
        );
    }
    // A claim a page makes is a claim the files have to keep: the page says the two builds are
    // remapped and locked, and both of those are properties of the script, not of the page.
    let page = text(PAGE);
    let script = text(REPRODUCER);
    if page.contains("--remap-path-prefix") {
        assert!(
            script.contains("--remap-path-prefix"),
            "{PAGE} promises that the two builds are remapped and {REPRODUCER} does not pass \
             `--remap-path-prefix`: a page describing a mechanism no file implements is the \
             cheapest kind of lie, and the one nobody catches"
        );
    }
    if page.contains("--locked") {
        for file in [REPRODUCER, WORKFLOW] {
            assert!(
                text(file).contains("--locked"),
                "{PAGE} promises a locked build and `{file}` does not pass `--locked`: the lockfile \
                 is only a pin in the builds that ask for it"
            );
        }
    }
}
