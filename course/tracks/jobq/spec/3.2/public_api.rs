//! Spec for `jobq/3.2-public-api-baseline`: the engine's public surface as an interface, judged from
//! outside it.
//!
//! `kiln run` copies this file into the project as `tests/spec_public_api.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Four claims:
//!
//! * **the surface is usable through the re-exports** — a job is submitted, claimed, failed and read
//!   back with every name taken from `jobq::engine` and nothing else. The `use` at the top is the
//!   first half of that claim: if a re-export disappears, this file stops compiling;
//! * **the engine's modules are not public paths** — `jobq::engine::domain::Job` and
//!   `jobq::engine::store::memory::MemoryStore` are the crate's own business now, and a second
//!   backend added later must not arrive as a third path either;
//! * **the store port cannot be implemented outside the crate** — `Store` carries a sealed
//!   supertrait that lives in a private module, so the promise the port's documentation makes (that
//!   every backend passes the conformance suite of `jobq/3.5-store-port-conformance`) is one only
//!   this crate can keep;
//! * **the types that will grow say so** — `#[non_exhaustive]` on `JobState`, `Error`, `Job` and
//!   `SubmitRequest`, so a caller writes its wildcard arms and its `..` now rather than after the
//!   next release.
//!
//! **Why three of the four read the sources.** An absent public path is not observable from a crate
//! that is not allowed to mention it, a private supertrait is invisible to a program that only calls
//! the port, and "this must not compile" has no expression in Rust outside a compile-fail test —
//! which this course cannot run (docs/authoring.md §6.6). So the spec pins the *source facts* that
//! make the compile-time promise true, and says at every assertion why the source is where it has to
//! look. The half the compiler can check is the course's witness
//! `course/tracks/jobq/api/3.2-public-api-baseline.rs`: `kiln check` compiles it as
//! `tests/semver_baseline.rs`, it pins every public signature, and it fails to build the moment
//! `#[non_exhaustive]` is dropped from either enum — the two files are the two halves of one claim,
//! and a learner who satisfies one and not the other has not finished the step.

use std::path::{Path, PathBuf};

use jobq::engine::{
    Engine, Error, Job, JobId, JobState, MemoryStore, Queue, Store, SubmitRequest, WorkerId,
};

/// The queue every case in this file uses.
const QUEUE: &str = "emails";

/// One source file of the crate under test, with the text the assertions read.
struct Source {
    /// Path, for a failure message a learner can follow.
    path: PathBuf,
    /// Whole file, read once.
    text: String,
}

/// The crate's sources, in the two groups the assertions need.
///
/// `engine` is the part that has to stop being public paths; `all` adds the crate root, because a
/// re-export there (`pub use crate::engine::sealed::Sealed;`) leaks a private module just as well.
struct Sources {
    /// Files under `src/engine`, in path order.
    engine: Vec<Source>,
    /// The same, plus `src/lib.rs`.
    all: Vec<Source>,
}

/// The surface, held the way a caller holds it: everything below arrives from `jobq::engine` alone.
#[tokio::test]
async fn the_engine_runs_a_job_with_every_name_taken_from_the_reexports() {
    let engine = Engine::new(MemoryStore::new());
    let queue = Queue::parse(QUEUE).expect("`emails` is a name this crate accepts");
    let worker = WorkerId::new("spec-worker");

    let id: JobId = engine
        .submit(
            SubmitRequest::new(
                queue.clone(),
                serde_json::json!({ "to": "alice@example.com" }),
            )
            .with_priority(3),
        )
        .await
        .expect("submitting to an empty memory store cannot fail");

    let claimed: Vec<Job> = engine
        .claim(&worker, &queue, 8)
        .await
        .expect("claiming from a memory store cannot fail");
    assert_eq!(
        claimed.len(),
        1,
        "the surface has to be enough to run the engine, not only to name it: submit, claim, fail \
         and inspect are the five methods `Engine` re-exports"
    );
    assert_eq!(
        claimed[0].priority(),
        3,
        "the builder's priority reached the row"
    );

    let state: JobState = engine
        .fail(&worker, &id, "the smtp server said 421")
        .await
        .expect("recording a failure cannot fail");
    assert_eq!(state, JobState::Queued, "the job has attempts left");

    let read_back: Job = engine.inspect(&id).await.expect("the job is still there");
    assert_eq!(read_back.id(), id);
    assert_eq!(read_back.last_error(), Some("the smtp server said 421"));

    // `Store` is still nameable as a bound — a caller may be generic over the port even though it
    // cannot implement it (the third claim below).
    fn over_a_port<S: Store>(_engine: &Engine<S>) {}
    over_a_port(&engine);

    // And a failure comes back as the engine's own error type, classified for a shell.
    let missing: Error = engine
        .inspect(&JobId::new())
        .await
        .expect_err("a job that was never submitted is not there");
    assert_eq!(
        missing.exit_code(),
        3,
        "a missing job is exit code 3, not a panic and not a zero: the CLI of \
         `jobq/3.15-cli-contract` is built on this mapping"
    );
}

/// The engine's modules are internal: what a caller can name is what `engine` re-exports.
#[test]
fn the_engines_modules_are_not_public_paths() {
    let sources = engine_sources();

    // A step that teaches "the surface is the re-exports" has to check that the old paths are gone,
    // and a `pub mod` line is the whole of that: `jobq::engine::domain::Job` and
    // `jobq::engine::store::memory::MemoryStore` stop resolving the moment those declarations are
    // not `pub` any more. The check is by declaration rather than by name so that the `store`
    // module's next submodule (the durable backend of `jobq/3.5-store-port-conformance`) is covered
    // by it too, without this file being edited.
    let mut offenders = Vec::new();
    for source in &sources.engine {
        for (number, line) in source.text.lines().enumerate() {
            if line.trim_start().starts_with("pub mod ") {
                offenders.push(format!(
                    "{}:{}: {}",
                    source.path.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "every module under `src/engine` must be `pub(crate)` (or private), so that the only public \
         paths are the re-exports in `src/engine/mod.rs` — a caller that names \
         `jobq::engine::<module>::…` cannot be kept compatible when the module is split, and that \
         is what a library promises. `pub mod` found at:\n{}",
        offenders.join("\n")
    );

    // The four modules are not required to stay `pub`, but they are required to stay: flattening
    // the engine into one file would satisfy the rule above while meaning something else.
    let engine_root = engine_root(&sources);
    for module in ["domain", "error", "scheduler", "store"] {
        assert!(
            declares_module(&engine_root.text, module),
            "`src/engine/mod.rs` must still declare the engine's `{module}` module (as \
             `pub(crate) mod {module};` or `mod {module};`): this step moves paths, it does not \
             dissolve the layout the rest of the track builds on"
        );
    }
}

/// The port is sealed: only this crate's certified backends can implement it.
#[test]
fn the_store_port_cannot_be_implemented_outside_the_crate() {
    let sources = engine_sources();

    // (a) `Store` names the seal among its supertraits. This is what makes the refusal real: without
    //     the bound, an outside type with the right five methods is a backend as far as the compiler
    //     is concerned, and the engine would run code nobody has held to the conformance suite.
    let port = file_declaring(&sources, "pub trait Store");
    let header = port
        .text
        .split_once("pub trait Store")
        .expect("the declaration was found by `file_declaring`")
        .1
        .split('{')
        .next()
        .unwrap_or_default();
    assert!(
        header.contains("Sealed"),
        "`pub trait Store` must name its seal among its supertraits (`{}`): the port's own \
         documentation promises that every implementation passes the conformance suite of \
         `jobq/3.5-store-port-conformance`, and this crate can only run that suite for the backends \
         it ships. A trait a stranger can implement is a promise a stranger can break. Found: \
         `pub trait Store{header}`",
        port.path.display()
    );

    // (b) The seal exists, and it is not reachable as a public path. `pub trait Sealed` inside a
    //     private module is the pattern: the port may name it, no other crate can.
    let mut declarations = Vec::new();
    for source in &sources.all {
        for (number, line) in source.text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("mod sealed") || trimmed.starts_with("pub mod sealed") {
                declarations.push(format!(
                    "{}:{}: {trimmed}",
                    source.path.display(),
                    number + 1
                ));
            }
        }
    }
    assert!(
        !declarations.is_empty(),
        "no `mod sealed` declaration under `src/engine`: the seal the port needs (`Sealed`, holding \
         no methods, implemented by the crate's own backends) has to live somewhere, and \
         `crate::engine::sealed::Sealed` is where the port has to be able to name it"
    );
    for declaration in &declarations {
        let (_, line) = declaration
            .split_once(": ")
            .expect("the entry was formatted with a `: ` separator");
        assert!(
            !line.starts_with("pub mod"),
            "`{declaration}` declares the sealed module as a public path, which undoes the seal: \
             a module a caller can name is a module a caller can implement `Sealed` in, and then \
             `impl Store for TheirStore` compiles again"
        );
    }

    // (c) Nothing re-exports the seal. A `pub use` is the other way to make a private module's trait
    //     nameable — and it is the one that looks harmless in review.
    let mut leaks = Vec::new();
    for source in &sources.all {
        for (number, line) in source.text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("pub use") && trimmed.contains("Sealed") {
                leaks.push(format!(
                    "{}:{}: {trimmed}",
                    source.path.display(),
                    number + 1
                ));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "`Sealed` must not be re-exported: the port names it as a supertrait, which is enough for \
         the engine and useless to everybody else, and that asymmetry is the whole mechanism. \
         Found:\n{}",
        leaks.join("\n")
    );
}

/// The types that will grow say so, in the one place the compiler reads.
#[test]
fn the_types_that_will_grow_are_non_exhaustive() {
    let sources = engine_sources();

    // `JobState` and `Error` are the two the compiler enforces: outside the crate a `match` over
    // them needs a wildcard, so a variant added later is a warning-free edit for the crate and a
    // compile error for nobody. `Job` and `SubmitRequest` have private fields today, so the
    // attribute changes nothing yet — it is the same promise written where the compiler will read
    // it the day one of those fields becomes `pub`.
    for declaration in [
        "pub enum JobState",
        "pub enum Error",
        "pub struct Job",
        "pub struct SubmitRequest",
    ] {
        assert!(
            marked(&sources, declaration, "#[non_exhaustive]"),
            "`{declaration}` must carry `#[non_exhaustive]`: this crate's whole shape is that it \
             grows — the next step gives the error enum a source chain, the store port gains a \
             backend, jobs gain a lease — and a caller that wrote its wildcard arm and its `..` on \
             day one must not be rebuilt the day one of those lands"
        );
    }
}

/// Every `.rs` file of the engine, plus the crate root, in path order.
///
/// The engine's *source* is what these assertions are about: whether a module is a public path is a
/// property of the declaration, not of anything that can be called.
fn engine_sources() -> Sources {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let engine_paths = rust_files(&root.join("src/engine"));
    assert!(
        engine_paths.len() >= 4,
        "expected the engine's modules under {}, found {}: the source checks below are only worth \
         anything if they looked at the crate",
        root.join("src/engine").display(),
        engine_paths.len()
    );
    let read = |paths: Vec<PathBuf>| -> Vec<Source> {
        paths
            .into_iter()
            .map(|path| Source {
                text: std::fs::read_to_string(&path).unwrap_or_else(|error| {
                    panic!("{} could not be read: {error}", path.display())
                }),
                path,
            })
            .collect()
    };
    let mut all_paths = engine_paths.clone();
    all_paths.push(root.join("src/lib.rs"));
    all_paths.sort();
    Sources {
        engine: read(engine_paths),
        all: read(all_paths),
    }
}

/// Every `.rs` file below `root`, in path order, so a failure names the same file twice running.
fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{} could not be listed: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("the directory is readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// `src/engine/mod.rs`: the file the engine's own layout is declared in.
fn engine_root(sources: &Sources) -> &Source {
    sources
        .engine
        .iter()
        .find(|source| source.path.ends_with("engine/mod.rs"))
        .expect("`src/engine/mod.rs` is the engine's root and its source was just read")
}

/// The one source file that declares `declaration`, or a failure naming what was looked at.
///
/// The match is [`declares`] — the same whole-name comparison the attribute check below uses, and
/// the reason is the one written on that function: `contains` would let the line declaring
/// `JobEvent` answer for the line declaring `Job`, and the day a later step adds a type whose name
/// starts with an earlier one's, this assertion would fail while the promise it checks still held
/// (step `jobq/3.6-transaction-isolation` is where that happened, with a trail row next to the job).
fn file_declaring<'a>(sources: &'a Sources, declaration: &str) -> &'a Source {
    let found: Vec<&Source> = sources
        .all
        .iter()
        .filter(|source| source.text.lines().any(|line| declares(line, declaration)))
        .collect();
    assert!(
        found.len() == 1,
        "expected exactly one file under `src/engine` to declare `{declaration}`, found {}: {}",
        found.len(),
        found
            .iter()
            .map(|source| source.path.display().to_string())
            .collect::<Vec<String>>()
            .join(", ")
    );
    found[0]
}

/// `true` when `src/engine/mod.rs` gives `module` a declaration of its own.
fn declares_module(root: &str, module: &str) -> bool {
    root.lines().any(|line| {
        let line = line.trim();
        line == format!("mod {module};") || line == format!("pub(crate) mod {module};")
    })
}

/// `true` when `attribute` sits on the item declared as `declaration`.
///
/// A doc comment and the other attributes are the lines directly above the declaration, in any order
/// `rustfmt` leaves them in; a blank line ends the run, because an attribute does not survive one.
fn marked(sources: &Sources, declaration: &str, attribute: &str) -> bool {
    let source = file_declaring(sources, declaration);
    let lines: Vec<&str> = source.text.lines().collect();
    let index = lines
        .iter()
        .position(|line| declares(line, declaration))
        .unwrap_or_else(|| {
            panic!(
                "{} declares `{declaration}` on no line of its own",
                source.path.display()
            )
        });
    lines
        .iter()
        .take(index)
        .rev()
        .take_while(|above| is_comment_or_attribute(above))
        .any(|above| above.trim() == attribute)
}

/// `true` when `line` declares `declaration`.
///
/// The text has to be the whole item name: `pub struct Job` must not match the line that declares
/// `JobId`, and a comparison on the trimmed line would.
fn declares(line: &str, declaration: &str) -> bool {
    match line.trim_start().strip_prefix(declaration) {
        None => false,
        Some(rest) => matches!(
            rest.chars().next(),
            None | Some('{' | '(' | ';' | ':' | '<' | ' ' | '\t')
        ),
    }
}

/// `true` for the lines that may sit between an item and its own attributes.
fn is_comment_or_attribute(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("//") || line.starts_with("#[")
}
