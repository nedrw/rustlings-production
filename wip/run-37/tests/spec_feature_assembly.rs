//! Spec for `jobq/3.4-feature-assembly`: one crate, assembled several ways.
//!
//! `kiln run` copies this file into the project as `tests/spec_feature_assembly.rs`; it is the
//! course's file, not the learner's (plan §3 D2). A feature is a *compile-time* fact, so the only
//! way to judge one is to build the crate the way it says and then run something in it:
//!
//! * **the assemblies are the crate's own.** The `[features]` table is the learner's edit, so this
//!   file reads the manifest and builds the combinations it finds there rather than importing a
//!   list: a learner who adds a feature gets it judged, and one who skips `postgres` is told which
//!   name the step is about. What it insists on is that the in-memory engine is the *floor* — the
//!   build that turns every feature off has to work, because that is the difference between a crate
//!   that assembles and a crate that has been forked.
//! * **every assembly builds, and its own tests pass.** `cargo build` for the library and the two
//!   binaries, `cargo test --lib` for the crate's unit tests. Not the whole test suite: the
//!   integration targets are the course's specs — this file among them, so a nested run of the whole
//!   suite would run this test inside itself — while what a feature combination has to hold to is
//!   the crate.
//! * **a caller runs a job in every assembly.** A second crate, written by this file and compiled
//!   against the project with the features the assembly names, submits a job to the in-memory engine
//!   and drains it. That is the behaviour that must hold everywhere, and it is judged from outside
//!   the crate because that is where a library's assemblies matter.
//! * **a build without the durable backend says so.** In an assembly without `postgres`, the same
//!   caller asks the service to open a durable store and reads the refusal: it has to fail, and it
//!   has to name the feature, because an operator with a database needs to know which flag to
//!   rebuild with. An assembly that has the feature is not asked: whether a durable store can be
//!   opened then is the subject of the steps after this one.
//!
//! Two notes on how this file fails. It is not written against an interface the step adds — the
//! manifest check and the refusal above are what fail on the tree from the previous step, and they
//! fail with a sentence rather than with a compile error. And the builds it starts are `--offline`:
//! the dependencies are already in the cache, and a check that can hang on a network is a check
//! nobody can trust.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::Deserialize;

/// The feature the step is about.
const POSTGRES: &str = "postgres";

/// The `[features]` table of the crate under test, as Cargo reads it.
///
/// Only the two things a build needs are named: an unknown section is not this file's business, and
/// a feature's own value (its dependency wiring) is Cargo's.
#[derive(Debug, Deserialize)]
struct Manifest {
    /// `[features]`, name to value. `default` is the one Cargo reads for a plain `cargo build`.
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

/// One way of assembling the crate: the features it gets, and the flags that spell them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Assembly {
    /// How a failure names this assembly.
    what: String,
    /// Feature names, sorted: what the probe asks for, spelled out with `default-features = false`.
    features: Vec<String>,
    /// Flags for a build of the project itself.
    flags: Vec<String>,
}

/// The project directory the specs run in.
fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The feature table the manifest declares.
fn feature_table() -> BTreeMap<String, Vec<String>> {
    let path = project().join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
    let manifest: Manifest = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not a manifest: {error}", path.display()));
    manifest.features
}

/// The features the default build turns on, as *features*.
///
/// `default` may list more than feature names — `dep:sqlx`, `serde?/derive` — and those are
/// dependency wiring rather than switches a build can be asked for, so they are left out of what the
/// probe spells.
fn default_features(table: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut features: Vec<String> = table
        .get("default")
        .map(|names| {
            names
                .iter()
                .filter(|name| table.contains_key(name.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    features.sort_unstable();
    features
}

/// The features an assembly really turns on, following what each one implies.
///
/// A feature's value is a list of other features and of dependency wiring; the wiring (`dep:sqlx`)
/// is Cargo's and is skipped here. Following the list matters for one decision below: `metrics =
/// ["postgres"]` turns the durable backend on, and a spec that only read back the names it asked for
/// would then ask a build that has the feature to refuse a durable store.
fn enabled_features(table: &BTreeMap<String, Vec<String>>, wanted: &[String]) -> BTreeSet<String> {
    let mut enabled = BTreeSet::new();
    let mut pending: Vec<String> = wanted.to_vec();
    while let Some(name) = pending.pop() {
        if !enabled.insert(name.clone()) {
            continue;
        }
        if let Some(implied) = table.get(&name) {
            pending.extend(
                implied
                    .iter()
                    .filter(|entry| table.contains_key(entry.as_str()))
                    .cloned(),
            );
        }
    }
    enabled
}

/// Every assembly this spec builds.
///
/// The three the step is about — nothing, the default build, everything — plus, for every feature
/// that is not on by default, the build that turns on that one alone. That last set is what
/// "additive" means in practice: a feature that cannot be enabled by itself is a feature that has
/// quietly become a fork.
fn assemblies(table: &BTreeMap<String, Vec<String>>) -> Vec<Assembly> {
    let default = default_features(table);
    // `default` is Cargo's switch for the default set rather than a feature of its own, so it is not
    // one of the things a build can be asked for.
    let all: Vec<String> = table
        .keys()
        .filter(|name| name.as_str() != "default")
        .cloned()
        .collect();
    let mut assemblies = vec![
        Assembly {
            what: "no features at all".to_owned(),
            features: Vec::new(),
            flags: vec!["--no-default-features".to_owned()],
        },
        Assembly {
            what: format!("the default build ({})", names(&default)),
            features: default.clone(),
            flags: Vec::new(),
        },
        Assembly {
            what: "every feature".to_owned(),
            features: all.clone(),
            flags: vec!["--all-features".to_owned()],
        },
    ];
    for feature in all.iter().filter(|name| !default.contains(name)) {
        assemblies.push(Assembly {
            what: format!("`{feature}` on its own"),
            features: vec![feature.clone()],
            flags: vec![
                "--no-default-features".to_owned(),
                "--features".to_owned(),
                feature.clone(),
            ],
        });
    }
    assemblies
}

/// A feature list for a message.
fn names(features: &[String]) -> String {
    if features.is_empty() {
        "none".to_owned()
    } else {
        features.join(", ")
    }
}

/// The manifest has to name the feature this step is about, and turn it on.
#[test]
fn the_manifest_declares_the_assemblies() {
    let table = feature_table();
    let default = default_features(&table);
    let declared = names_of(&table);

    assert!(
        table.contains_key(POSTGRES),
        "`Cargo.toml` declares no `{POSTGRES}` feature (it declares {declared}): the durable \
         backend of `jobq/3.5-store-port-conformance` lands behind that name, and the step is that \
         the crate assembles around it instead of being forked over it. Expected at least:\n\n  \
         [features]\n  default = [\"{POSTGRES}\"]\n  {POSTGRES} = []\n"
    );
    assert!(
        enabled_features(&table, &default).contains(POSTGRES),
        "`{POSTGRES}` is not on by default (default = {}): the plain `cargo build` — what a \
         dependent, a `cargo install` and every step of this track uses — has to be the full \
         engine, and the in-memory-only build is the one you ask for by name",
        names(&default)
    );
    assert!(
        !default.is_empty() || table.is_empty(),
        "the default build turns on nothing while the crate declares features: default = {}",
        names(&default)
    );
}

/// Every assembly builds, its unit tests pass, and a caller runs a job in it.
#[test]
fn every_assembly_builds_and_runs_a_job() {
    let table = feature_table();
    assert!(
        table.contains_key(POSTGRES),
        "`Cargo.toml` declares no `{POSTGRES}` feature (it declares {}): there is nothing to \
         assemble",
        names_of(&table)
    );

    let dir = Dir::new("jobq-features");
    for assembly in assemblies(&table) {
        // The library and the two binaries: a feature that only the library knows about leaves the
        // daemon failing to compile in the build nobody tried.
        built(
            &["build", "--offline"],
            &assembly,
            "`cargo build` has to succeed in every assembly: the daemon is built from the same \
             crate, and a `cfg` that is missing from its call site is a build that only works in \
             the combination its author happened to use",
        );
        // The crate's own unit tests.
        built(
            &["test", "--offline", "--lib"],
            &assembly,
            "the crate's own tests have to hold in every assembly: a test that only passes with \
             the durable backend is a test that has to say so with a `cfg` of its own",
        );
        // And a caller: another program, compiled with these features, running a job.
        let postgres = enabled_features(&table, &assembly.features).contains(POSTGRES);
        let printed = probe(&dir, &assembly, !postgres);
        assert!(
            printed.contains("in-memory job ran"),
            "the probe could not run a job in the assembly of {}: it said\n{printed}",
            assembly.what
        );
        if !postgres {
            assert!(
                printed.contains("durable store refused by name"),
                "in the assembly of {} there is no durable backend, and the refusal has to say so \
                 by name — this caller asked the service for one and got:\n{printed}",
                assembly.what
            );
        }
    }
}

/// The features the manifest declares, for a message.
fn names_of(table: &BTreeMap<String, Vec<String>>) -> String {
    names(
        &table
            .keys()
            .filter(|name| name.as_str() != "default")
            .cloned()
            .collect::<Vec<String>>(),
    )
}

/// Run one cargo command over the project in this assembly, and fail with its own words if it does.
fn built(args: &[&str], assembly: &Assembly, why: &str) {
    let mut command = Command::new("cargo");
    command
        .args(args)
        .args(&assembly.flags)
        .current_dir(project());
    inherited_target_dir(&mut command);
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("cargo has to be on PATH: {error}"));
    assert!(
        output.status.success(),
        "cargo {} {} failed in the assembly of {} — {why}:\n{}",
        args.join(" "),
        assembly.flags.join(" "),
        assembly.what,
        said(&output)
    );
}

/// Write the probe for this assembly, run it, and answer what it printed.
///
/// `durable_refusal` asks for the half that only a build without the durable backend can run.
fn probe(dir: &Dir, assembly: &Assembly, durable_refusal: bool) -> String {
    let features: Vec<String> = assembly
        .features
        .iter()
        .map(|feature| format!("\"{feature}\""))
        .collect();
    dir.write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"jobq-assembly-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
             [dependencies]\njobq = {{ path = \"{}\", default-features = false, features = [{}] }}\n\
             tokio = {{ version = \"1\", features = [\"macros\", \"rt\", \"time\"] }}\n\n\
             [workspace]\n",
            project().display(),
            features.join(", ")
        ),
    );
    let source = PROBE.replace(
        "    // The durable half.\n",
        if durable_refusal { PROBE_DURABLE } else { "" },
    );
    dir.write("src/main.rs", &source);

    let mut command = Command::new("cargo");
    command
        .args(["run", "--quiet", "--offline"])
        .current_dir(dir.path());
    inherited_target_dir(&mut command);
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("cargo has to be on PATH: {error}"));
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "the probe did not run in the assembly of {} (features: {}): a caller of this crate has to \
         be able to build against it and run an in-memory job:\n{}",
        assembly.what,
        names(&assembly.features),
        said(&output)
    );
    println!("{}: {}", assembly.what, printed.trim());
    printed
}

/// Point a cargo command at the build directory this run is using, when there is one.
///
/// The dependencies of the crate are already compiled there, so a probe that shares it costs
/// seconds rather than a compile of the world; a run without one gets cargo's own default.
fn inherited_target_dir(command: &mut Command) {
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        command.env("CARGO_TARGET_DIR", target_dir);
    }
}

/// What a command said, for a failure message.
fn said(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stderr.trim().is_empty() {
        stdout.into_owned()
    } else {
        stderr.into_owned()
    }
}

/// A directory of this test's own, removed when the test ends.
struct Dir {
    /// Where the probe lives.
    path: PathBuf,
}

impl Dir {
    /// An empty directory named after the test that owns it.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("jobq-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory is writable");
        Self { path }
    }

    /// Where the probe lives.
    fn path(&self) -> &Path {
        &self.path
    }

    /// Write one file of the probe, creating the directories it needs.
    fn write(&self, relative: &str, text: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a directory of the scratch tree is writable");
        }
        std::fs::write(&path, text)
            .unwrap_or_else(|error| panic!("{} could not be written: {error}", path.display()));
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A crate that uses `jobq` the way a caller does: through the public surface, in a build whose
/// features this spec chose.
const PROBE: &str = r#"
//! Written by tests/spec_feature_assembly.rs: a caller of `jobq`, in one assembly of it.
//!
//! Nothing here is optional: the in-memory engine is what every assembly of this crate promises, and
//! the job below runs through exactly the four calls a producer and a worker use.

use jobq::engine::{Engine, JobState, MemoryStore, Queue, SubmitRequest, WorkerId};

/// One job, from submitted to succeeded, with the in-memory engine alone.
async fn run_one_job() -> JobState {
    let engine = Engine::new(MemoryStore::new());
    let queue = Queue::parse("emails").expect("`emails` is a name the engine accepts");
    let worker = WorkerId::new("probe");
    let id = engine
        .submit(SubmitRequest::new(queue.clone(), Default::default()))
        .await
        .expect("submitting to an empty memory store cannot fail");
    let claimed = engine
        .claim(&worker, &queue, 1)
        .await
        .expect("claiming from a memory store cannot fail");
    assert_eq!(claimed.len(), 1, "the job was due, so it is claimed");
    engine
        .complete(&worker, &id)
        .await
        .expect("completing a claimed job cannot fail");
    engine
        .inspect(&id)
        .await
        .expect("the job is still there")
        .state()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let state = run_one_job().await;
    assert_eq!(
        state,
        JobState::Succeeded,
        "the in-memory engine has to run a job in every assembly of this crate"
    );
    println!("in-memory job ran");

    // The durable half.
}
"#;

/// The half of the probe that only exists in an assembly without the `postgres` feature: the same
/// service, asked for a store this build does not have.
const PROBE_DURABLE: &str = r#"    // No `postgres` feature in this build: the durable backend is not here, and the operator has to
    // be told *which* build this is rather than "use memory".
    let flags = jobq::service::Flags::default();
    let url = "postgres://jobq@127.0.0.1:5432/jobq";
    let config = jobq::service::Config::load(
        &flags,
        &[("JOBQ__STORE__URL".to_owned(), url.to_owned())],
    )
    .expect("a durable connection string is a valid setting for any build of this program");
    let served = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        jobq::service::serve(config, jobq::service::RunId::of_this_process()),
    )
    .await
    .expect(
        "a build without the `postgres` feature has to refuse a durable store rather than serve it \
         or wait for a signal",
    );
    let error = served.expect_err(
        "a build without the `postgres` feature cannot open a durable store",
    );
    let message = error.to_string();
    // `postgres` on its own is not enough: the connection string contains it, so a message that
    // only echoes the URL back would pass — and it would leave an operator with a database no idea
    // what to do. What is asked for is the *feature*, by name.
    let named = message.replace(url, "");
    assert!(
        named.contains("postgres") && named.contains("feature"),
        "the refusal has to name the missing feature (`postgres`) as a feature, because the \
         operator reading it has a database and needs to know which flag to rebuild with: \
         {message}"
    );
    println!("durable store refused by name");
"#;
