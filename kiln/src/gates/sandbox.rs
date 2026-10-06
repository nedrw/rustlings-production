//! A copy of the project, made and stamped so that a build in it can only be its own.
//!
//! Two slow gates judge a step in a copy rather than in the learner's tree, and both need the same
//! two answers, so they are given once here:
//!
//! * `mutants` rewrites one operator per attempt and must not leave that in the learner's files;
//! * `coverage` builds with `-C instrument-coverage`, which is a *different build* from the one the
//!   learner's own commands need ([`crate::scratch::cache`] keeps it out of their way).
//!
//! # The copy
//!
//! [`copy_project`] copies the tree and leaves behind what a build regenerates: cargo's `target/`,
//! version control's `.git/`, and kiln's own `.kiln/` (progress, and the probe copies `kiln probe`
//! keeps there — each of which holds a build directory of its own). It also carries the toolchain
//! pin that applies to the project, because a copy that compiles with whatever toolchain happens to
//! be the machine's default would judge something other than this project — the same rule
//! `kiln selftest` states for its sandbox, and the one that keeps a profile written in the copy
//! readable by the tools of the toolchain the gate probed.
//!
//! Profile files (`.profraw`) are left behind with them, and for a reason of this file's own: a
//! gate that reads a profile out of the copy would otherwise merge data of a run nobody here knows
//! about — a learner's own coverage run, whose file would travel into the copy and be stamped as
//! fresh as everything else. Only files a run *writes* under the copy are that run's data.
//!
//! # The stamp
//!
//! [`make_fresh`] sets every copied file's modification time ahead of the clock, and it is cargo's
//! rule rather than this gate's: a build directory is keyed by package *name and version*, not by
//! the directory a package lives in, so a copy of the project and the project itself are one build
//! slot, and cargo decides whether a slot is up to date by comparing the modification times of its
//! sources with the build it already has. `fs::copy` brings the original's timestamps along, which
//! can make a fresh copy look *older* than an artifact of the tree it came from — and the run would
//! then be served that artifact. Both gates have been on the wrong side of this, measured: a mutant
//! run judged a binary built before its rewrite, and the coverage gate reported a percentage that
//! described another sandbox's tree (handoff M2 §5.2).
//!
//! A stamped copy rebuilds the *project*, and only the project: the dependencies it needs are not
//! in the copied tree, so their artifacts in a shared build directory stay as fresh as they were.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{KilnError, Result};

/// The file that pins a toolchain to a directory and everything below it.
const PIN: &str = "rust-toolchain.toml";

/// Copy a project into a sandbox, with the toolchain pin that applies to it.
///
/// `from` is the project itself (the learner's own tree), `to` is a directory that does not have to
/// exist yet. Every copied file is stamped by [`make_fresh`], the pin included: the point of the
/// copy is that a build in it is this tree's build, and half a copy is not.
pub fn copy_project(from: &Path, to: &Path) -> Result<()> {
    copy_tree(from, to)?;
    carry_pin(from, to)
}

/// Copy one directory of a project, recursively, without what a build regenerates.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|source| KilnError::Write {
        path: to.to_path_buf(),
        source,
    })?;
    let entries = std::fs::read_dir(from).map_err(|source| KilnError::Read {
        path: from.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| KilnError::Read {
            path: from.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        // The three directories that are large and regenerable: cargo's build output, the version
        // control history, and kiln's own state (a probe copy below `.kiln/` carries a build
        // directory of its own, which the run it belongs to is the only one that can use).
        if matches!(name.to_str(), Some("target" | ".git" | ".kiln")) {
            continue;
        }
        let source_path = entry.path();
        let target_path = to.join(&name);
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            copy_tree(&source_path, &target_path)?;
        } else if kind.is_file() {
            // A profile file is not source: it is the data of a run this project knows nothing
            // about, and a measurement that merged it would be about that run.
            if source_path
                .extension()
                .is_some_and(|part| part == "profraw")
            {
                continue;
            }
            // Byte for byte: a project may hold images, fixtures or a lock file, and none of them is
            // text this gate has an opinion about.
            std::fs::copy(&source_path, &target_path).map_err(|source| KilnError::Write {
                path: target_path.clone(),
                source,
            })?;
            make_fresh(&target_path)?;
        }
    }
    Ok(())
}

/// Give the copy the toolchain pin the project is built under.
///
/// rustup resolves a toolchain from the nearest pin at or above the working directory, and the
/// project's own pin is normally *above* it (this repository pins the toolchain at the root, one
/// directory over `app/<track>`). A copy made anywhere else therefore loses it and compiles with
/// the machine's default toolchain — which is a different compiler whenever the two are not the same
/// version, and on this course's own machine a different *sysroot*: the LLVM tools come from the
/// pinned one. Copying the file into the copy's root reproduces the environment the project is built
/// in, which is what makes a coverage profile from the copy readable by the tools of the toolchain
/// the gate probed.
fn carry_pin(from: &Path, to: &Path) -> Result<()> {
    let Some(pin) = pin_above(from) else {
        return Ok(());
    };
    let destination = to.join(PIN);
    std::fs::copy(&pin, &destination).map_err(|source| KilnError::Write {
        path: destination.clone(),
        source,
    })?;
    make_fresh(&destination)
}

/// The pin rustup would apply to a build in `dir`: the nearest one at or above it.
fn pin_above(dir: &Path) -> Option<PathBuf> {
    let mut here = dir;
    loop {
        let candidate = here.join(PIN);
        if candidate.is_file() {
            return Some(candidate);
        }
        let parent = here.parent()?;
        if parent.as_os_str().is_empty() || parent == here {
            return None;
        }
        here = parent;
    }
}

/// Make a file in the sandbox newer than any build that came before this gate.
///
/// Cargo keeps a build when no source is *newer* than it, and it identifies a package by name and
/// version rather than by the directory the package lives in — so a copy of `jobq` in a sandbox and
/// the learner's own `jobq` are one build slot in a shared `CARGO_TARGET_DIR`. `fs::copy` preserves
/// timestamps, which lets a fresh copy look *older* than an artifact of the tree it came from: cargo
/// calls the artifact current and the gate judges the wrong tree. A second ahead of now is
/// unambiguously newer, and it also clears the case a file system with second granularity would
/// otherwise hide — two writes inside one second look equal there.
pub fn make_fresh(path: &Path) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|source| KilnError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.set_modified(SystemTime::now() + Duration::from_secs(1))
        .map_err(|source| KilnError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// A project with one file of every kind the copy has to tell apart.
    fn project(temp: &TempDir) -> PathBuf {
        let project = temp.path().join("app/jobq");
        crate::test_support::write(&project, "src/lib.rs", "pub fn f() {}\n");
        crate::test_support::write(&project, "target/debug/jobq", "an artifact\n");
        crate::test_support::write(&project, ".git/HEAD", "ref: refs/heads/main\n");
        crate::test_support::write(&project, ".kiln/probes/x/src/lib.rs", "a probe\n");
        crate::test_support::write(&project, "default_1_0_2.profraw", "another run's data\n");
        project
    }

    #[test]
    fn a_copy_keeps_the_project_and_leaves_build_output_and_state_behind() {
        let temp = TempDir::new("sandbox-copy");
        let project = project(&temp);
        let copy = temp.path().join("sandbox/project");

        copy_project(&project, &copy).expect("the copy is writable");

        assert!(copy.join("src/lib.rs").is_file());
        for left_behind in ["target", ".git", ".kiln"] {
            assert!(
                !copy.join(left_behind).exists(),
                "{left_behind}/ was copied"
            );
        }
        assert!(
            !copy.join("default_1_0_2.profraw").exists(),
            "another run's profile data was copied into the measurement"
        );
    }

    #[test]
    fn every_copied_file_is_newer_than_the_clock_it_was_copied_at() {
        let temp = TempDir::new("sandbox-fresh");
        let project = project(&temp);
        let copy = temp.path().join("sandbox/project");
        let source = project.join("src/lib.rs");
        // A source that is old for real: `fs::copy` would carry this timestamp into the copy, and a
        // build from before this gate would then look newer than the copy's own sources.
        let long_ago = SystemTime::now() - Duration::from_secs(86_400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .expect("the fixture file is writable")
            .set_modified(long_ago)
            .expect("the fixture timestamp is settable");

        let before = SystemTime::now();
        copy_project(&project, &copy).expect("the copy is writable");

        let copied = std::fs::metadata(copy.join("src/lib.rs"))
            .expect("the copied file exists")
            .modified()
            .expect("the copy has a timestamp");
        assert!(copied > before, "the copy is stamped ahead of the clock");
    }

    #[test]
    fn the_toolchain_pin_the_project_is_built_under_comes_with_the_copy() {
        let temp = TempDir::new("sandbox-pin");
        let project = project(&temp);
        // The pin sits above the project, as this repository's does: one directory over `app/`.
        temp.write("rust-toolchain.toml", "[toolchain]\nchannel = \"1.98.1\"\n");
        let copy = temp.path().join("sandbox/project");

        copy_project(&project, &copy).expect("the copy is writable");

        assert_eq!(
            std::fs::read_to_string(copy.join("rust-toolchain.toml")).expect("the pin is copied"),
            "[toolchain]\nchannel = \"1.98.1\"\n"
        );
    }
}
