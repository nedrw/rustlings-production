//! Spec for `jobq/3.25-release-artifacts`: the tag, the two binaries, the checksum file, and the
//! changelog that says what changed.
//!
//! `kiln run` copies this file into the project as `tests/spec_release.rs`; it is the course's file,
//! not the learner's (plan §3 D2). The cases are the facts a release is made of, and each one is a
//! sentence somebody would otherwise have to trust:
//!
//! * **the tag names the version.** A release is built, published and quoted under a tag, and the
//!   version inside the artifacts has to be the one the tag says — so the tag is *checked* against
//!   the crate's version rather than composed from it at a call site;
//! * **the name carries the version and the target.** Two builds of one version for two targets are
//!   two files, and a name that carries one of the two facts is a name a mirror is free to confuse;
//! * **a checksum file is a format, not a hash.** `<digest>  <name>`, two spaces, sorted, one line
//!   per file: that is what `sha256sum -c` reads. The two published SHA-256 vectors are in here so
//!   that "it hashes something" cannot pass for "it hashes with SHA-256";
//! * **verification is both directions.** A digest that does not match is obvious; a manifest that
//!   names a file nobody shipped is the one that reaches a user, and a file that is not in the
//!   manifest is a file nobody checked;
//! * **the changelog is part of the release.** A version whose section is missing, empty, or still
//!   sitting under an `Unreleased` heading has published a number without saying what is in it.
//!
//! Two cases are about this tree rather than about a fixture: its own `CHANGELOG.md` accounts for
//! the version it is, and its own `docs/release.md` states the release the tool produces. Both are
//! read from the **working directory** rather than through `CARGO_MANIFEST_DIR`, and that is a
//! deliberate detail. `CARGO_MANIFEST_DIR` is baked into the test binary when it is compiled, while
//! `kiln`'s slow path shares one build directory between sandboxes and copies of a project (a cargo
//! build slot is keyed by package name and version, not by directory), so a binary can end up
//! reading the files of a copy that has been deleted. Cargo starts a test with its working
//! directory set to the package root of the tree cargo was asked about, which is the file this
//! case means.
//!
//! The three cases that drive `src/bin/jobq-release.rs` are end-to-end on purpose: two of them use
//! the binaries this tree just built, because "the version in the binaries is the release's" is a
//! claim about *those* files rather than about a fixture.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use jobq::engine::{
    Artifact, BINARIES, CHANGELOG, CHECKSUMS, DIGEST_HEX, Refusal, Release, VERSION, changelog_ok,
    checksums, digest, verify,
};

/// A target triple that is not this machine's, so that a name built from it cannot pass by accident.
const TARGET: &str = "aarch64-apple-darwin";

/// The page this step delivers, read from the project it is part of.
const PAGE: &str = "docs/release.md";

/// A directory of one test's own, so two runs never share a file.
fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jobq-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory is writable");
    dir
}

/// The release this crate is, for a target that is not this machine's.
fn release() -> Release {
    Release::of(&format!("v{VERSION}"), TARGET)
        .unwrap_or_else(|refused| panic!("`v{VERSION}` is this crate's tag: {refused}"))
}

/// One artifact of a release, as a name and the bytes it is a promise about.
fn artifact(name: &str, bytes: &[u8]) -> Artifact {
    Artifact::of(name, bytes.to_vec())
}

/// A release's worth of artifacts: the two binaries and the changelog, in the order a release lists
/// them.
fn shipped() -> Vec<Artifact> {
    vec![
        artifact(BINARIES[0], b"the daemon, as the build produced it"),
        artifact(BINARIES[1], b"the client, as the build produced it"),
        artifact(CHANGELOG, b"# Changelog\n\n## [Unreleased]\n"),
    ]
}

/// One file, read from the tree this test was started in.
fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

/// One file's bytes, read from the tree this test was started in.
fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

/// The directory this tree's two binaries were built into.
fn binaries() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_jobqd"))
        .parent()
        .expect("the daemon is a file in a directory")
        .to_path_buf()
}

/// The release tool, run as a pipeline runs it.
fn tool(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jobq-release"))
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("`jobq-release` could not be started: {error}"))
}

/// Everything a tool run said, for a failure message that can be acted on.
fn said(output: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

/// The files of a directory, by name, sorted — what a release carries.
fn files_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{} could not be listed: {error}", directory.display()))
        .map(|entry| {
            entry
                .expect("the directory is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn the_tag_is_the_version_and_nothing_else() {
    let tag = format!("v{VERSION}");
    let release = release();
    assert_eq!(release.tag(), tag, "the tag is what it was built from");
    assert_eq!(
        release.version(),
        VERSION,
        "and the version is this crate's, read from the manifest \
         rather than written a second time in a call site"
    );
    assert_eq!(release.target(), TARGET);

    assert_eq!(
        Release::of(VERSION, TARGET),
        Err(Refusal::NotATag {
            tag: VERSION.to_owned()
        }),
        "a bare version is not a tag: the prefix is what makes a release ref unambiguous next to a \
         branch called `0.1.0`"
    );
    assert_eq!(
        Release::of("v", TARGET),
        Err(Refusal::NotATag {
            tag: "v".to_owned()
        }),
        "`v` with nothing after it names no version"
    );
    assert_eq!(
        Release::of("V0.1.0", TARGET),
        Err(Refusal::NotATag {
            tag: "V0.1.0".to_owned()
        }),
        "the prefix is `v`: a tag that differs only in case is a second spelling of the same \
         release, which is exactly the ambiguity a tag exists to remove"
    );
    assert_eq!(
        Release::of(&format!("refs/tags/v{VERSION}"), TARGET),
        Err(Refusal::NotATag {
            tag: format!("refs/tags/v{VERSION}")
        }),
        "a CI job hands the tool a ref (`refs/tags/v0.1.0`) and the tag is the last segment of it: \
         taking the ref is how a release ends up named after a branch"
    );

    let other = "v9.9.9";
    assert_eq!(
        Release::of(other, TARGET),
        Err(Refusal::VersionMismatch {
            tag: other.to_owned(),
            version: VERSION.to_owned()
        }),
        "a tag of another version is not \"not a tag\" — it is a tag of something else, and the \
         refusal has to say which version this tree is"
    );
    assert_eq!(
        Release::of(&format!("v{VERSION}-rc.1"), TARGET),
        Err(Refusal::VersionMismatch {
            tag: format!("v{VERSION}-rc.1"),
            version: VERSION.to_owned()
        }),
        "a pre-release tag is a different version, not a prefix of this one"
    );

    for refused in [
        Refusal::NotATag {
            tag: "x".to_owned(),
        },
        Refusal::VersionMismatch {
            tag: "v0.0.1".to_owned(),
            version: VERSION.to_owned(),
        },
    ] {
        assert_eq!(
            refused.exit_code(),
            2,
            "a tag the tool cannot release is the call being wrong, which is exit code 2 in the \
             table `jobq/3.15-cli-contract` reads: {refused}"
        );
    }
}

#[test]
fn the_name_carries_the_version_and_the_target() {
    let release = release();
    assert_eq!(
        release.directory(),
        format!("jobq-{VERSION}-{TARGET}"),
        "a release directory is two facts in one name: which version it is and which machine it \
         was built for"
    );
    assert_ne!(
        release.directory(),
        Release::of(&format!("v{VERSION}"), "x86_64-unknown-linux-gnu")
            .expect("the same version for another target is still a release")
            .directory(),
        "two targets of one version are two releases, so one name cannot serve both"
    );

    assert_eq!(
        release.binaries(),
        ["jobqd", "jobqc"],
        "a release carries the daemon and the client, in that order — the order is part of the \
         contract because a checksum file is written from it"
    );
    assert_eq!(release.checksums(), CHECKSUMS);
    assert_eq!(CHECKSUMS, "SHA256SUMS");
    assert!(
        !release.binaries().contains(&release.checksums()),
        "the checksum file is not one of the files it checksums: a manifest that covered itself \
         could not be used to check anything"
    );
}

#[test]
fn a_checksum_file_is_the_format_a_tool_reads() {
    let artifacts = shipped();
    let manifest = checksums(&artifacts);
    let lines: Vec<&str> = manifest.lines().collect();
    assert_eq!(
        lines.len(),
        artifacts.len(),
        "one line per artifact: {manifest:?}"
    );

    let mut expected: Vec<String> = artifacts
        .iter()
        .map(|artifact| format!("{}  {}", artifact.digest(), artifact.name()))
        .collect();
    expected.sort();
    assert_eq!(
        lines, expected,
        "sorted by name, so that two runs over the same set produce the same bytes"
    );
    assert!(
        lines.iter().all(|line| line.contains("  ")),
        "two spaces: a tab is a file a human reads and `sha256sum -c` refuses: {manifest:?}"
    );
    assert!(
        manifest.ends_with('\n'),
        "the last line ends, or `while read` drops the last artifact"
    );
    assert_eq!(
        checksums(&[]),
        "",
        "nothing to check is an empty file, not a broken one"
    );

    assert_eq!(
        DIGEST_HEX, 64,
        "SHA-256 is 32 bytes, and a checksum file writes them as 64 lower-case hex characters"
    );
    for (bytes, expected) in [
        (
            &b""[..],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            &b"abc"[..],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
    ] {
        assert_eq!(
            digest(bytes),
            expected,
            "the two vectors every SHA-256 implementation is published with: a hash that is \
             almost SHA-256 produces a checksum file that almost works"
        );
    }
    assert_ne!(
        digest(b"0.1.0"),
        digest(b"0.1.1"),
        "and a version that changed changes the bytes it hashes to"
    );
    assert!(
        digest(b"archive")
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "lower-case hex, which is what every tool that reads a checksum file expects"
    );
}

#[test]
fn verification_is_both_directions() {
    let shipped = shipped();
    let manifest = checksums(&shipped);
    assert_eq!(
        verify(&manifest, &shipped),
        Ok(()),
        "the files this manifest was written for verify"
    );

    // The reader is liberal about the shape of a line: a manifest a person reordered or a tool
    // re-wrote with a tab is still a manifest about the same files.
    let mut reordered: Vec<&str> = manifest.lines().collect();
    reordered.reverse();
    let reordered = format!("{}\n", reordered.join("\n"));
    assert_eq!(
        verify(&reordered, &shipped),
        Ok(()),
        "the order of the lines is not a claim about the files"
    );

    let tampered = vec![
        shipped[0].clone(),
        artifact(BINARIES[1], b"the client, with one byte changed"),
        shipped[2].clone(),
    ];
    assert_eq!(
        verify(&manifest, &tampered),
        Err(Refusal::ChecksumMismatch {
            name: BINARIES[1].to_owned(),
            expected: shipped[1].digest(),
            found: tampered[1].digest(),
        }),
        "a changed byte is a refusal that names the file and both digests — \"the release is \
         corrupt\" sends a reader to unpack it by hand"
    );

    let promised = format!(
        "{manifest}{}  jobq-does-not-exist\n",
        digest(b"never built")
    );
    assert_eq!(
        verify(&promised, &shipped),
        Err(Refusal::MissingFile {
            name: "jobq-does-not-exist".to_owned()
        }),
        "the manifest promises a file nobody shipped: this is the half of verification nobody \
         notices until a user does"
    );

    assert_eq!(
        verify(&checksums(&shipped[..1]), &shipped),
        Err(Refusal::NotInManifest {
            name: shipped[1].name().to_owned()
        }),
        "and in the other direction: a file the manifest does not name is a file nothing checked"
    );
    assert_eq!(
        verify("", &shipped),
        Err(Refusal::NotInManifest {
            name: shipped[0].name().to_owned()
        }),
        "an empty manifest is not evidence about anything: the file that is not listed is exactly \
         the file that was not checked"
    );
    assert_eq!(
        verify("", &[]),
        Ok(()),
        "no files and no lines is nothing to be wrong"
    );

    for refused in [
        Refusal::ChecksumMismatch {
            name: "jobqd".to_owned(),
            expected: "a".repeat(DIGEST_HEX),
            found: "b".repeat(DIGEST_HEX),
        },
        Refusal::MissingFile {
            name: "jobqd".to_owned(),
        },
        Refusal::NotInManifest {
            name: "jobqd".to_owned(),
        },
    ] {
        assert_eq!(
            refused.exit_code(),
            1,
            "a release that does not check out is this deployment being broken, which is exit \
             code 1: {refused}"
        );
    }
}

#[test]
fn a_binary_that_does_not_report_the_release_is_refused() {
    let release = release();
    assert_eq!(
        release.accepts(BINARIES[0], &format!("jobqd {VERSION}\n")),
        Ok(()),
        "the version is the last word of the first line"
    );
    assert_eq!(
        release.accepts(BINARIES[0], &format!("jobqd {VERSION}")),
        Ok(()),
        "with or without the newline a shell would eat"
    );
    assert_eq!(
        release.accepts(
            BINARIES[1],
            &format!("jobqc {VERSION}\nwarn: a build stamp\n")
        ),
        Ok(()),
        "a second line is not part of the version: a program that logs after answering has still \
         answered"
    );
    assert_eq!(
        release.accepts(BINARIES[0], &format!("{VERSION}\n")),
        Ok(()),
        "and the check is about the version, not the name: the file it was read from is what the \
         release names"
    );

    assert_eq!(
        release.accepts(BINARIES[0], "jobqd 9.9.9\n"),
        Err(Refusal::WrongBinary {
            name: BINARIES[0].to_owned(),
            said: "9.9.9".to_owned(),
            version: VERSION.to_owned(),
        }),
        "the stale build: a binary that reports another version is not this release, whatever \
         directory it was found in"
    );
    assert_eq!(
        release.accepts(BINARIES[0], "jobqd\n"),
        Err(Refusal::WrongBinary {
            name: BINARIES[0].to_owned(),
            said: "jobqd".to_owned(),
            version: VERSION.to_owned(),
        }),
        "a line with no version in it is a refusal rather than a match"
    );
    assert_eq!(
        release.accepts(BINARIES[0], ""),
        Err(Refusal::WrongBinary {
            name: BINARIES[0].to_owned(),
            said: String::new(),
            version: VERSION.to_owned(),
        }),
        "a binary that said nothing has not said it is this release"
    );
    assert_eq!(
        release.accepts(BINARIES[1], &format!("jobqd {VERSION}\n")),
        Ok(()),
        "the file name and the name a binary prints are two different things, and only one of them \
         is in the contract"
    );
}

#[test]
fn the_changelog_has_to_account_for_the_version() {
    let promoted = format!(
        "# Changelog\n\n## [Unreleased]\n\n## [{VERSION}] - 2026-10-05\n\n- one claim \
         statement\n"
    );
    assert_eq!(
        changelog_ok(&promoted, VERSION),
        Ok(()),
        "a promoted section with content is what `Unreleased` is supposed to become"
    );
    assert_eq!(
        changelog_ok(
            &format!(
                "# Changelog\n\n## {VERSION}\n- the brackets are a convention, not the rule\n"
            ),
            VERSION
        ),
        Ok(()),
        "`## 0.1.0` and `## [0.1.0] - 2026-10-05` are the same section"
    );

    assert_eq!(
        changelog_ok("# Changelog\n\n## [0.0.1]\n- older\n", VERSION),
        Err(Refusal::NoChangelogSection {
            version: VERSION.to_owned()
        }),
        "a version with no section has not said what it contains"
    );
    assert_eq!(
        changelog_ok(
            &format!("# Changelog\n\n## [{VERSION}]\n\n<!-- TODO: what changed? -->\n"),
            VERSION
        ),
        Err(Refusal::EmptySection {
            version: VERSION.to_owned()
        }),
        "an empty section is worse than none: it looks like the release was documented, and a \
         section whose only line is a comment says nothing a reader can use"
    );
    assert_eq!(
        changelog_ok(
            &format!(
                "# Changelog\n\n## [Unreleased]\n- the fix that is going out\n\n## [{VERSION}]\n- \
                 older\n"
            ),
            VERSION
        ),
        Err(Refusal::UnreleasedNotPromoted {
            version: VERSION.to_owned()
        }),
        "the notes are still under `Unreleased`, so this version does not say what went out with it"
    );

    // The two ways a grep would be fooled, and the state a released tree actually keeps.
    assert_eq!(
        changelog_ok(
            &format!("# Changelog\n\n## [Unreleased]\n- {VERSION} is what we are calling it\n"),
            VERSION
        ),
        Err(Refusal::NoChangelogSection {
            version: VERSION.to_owned()
        }),
        "a version *mentioned* in a bullet is not a section: the rule is about the heading, and a \
         checker that grepped for the number would pass a changelog that says nothing"
    );
    assert_eq!(
        changelog_ok(
            &format!("# Changelog\n\n## [Unreleased]\n\n### [{VERSION}]\n- not a section\n"),
            VERSION
        ),
        Err(Refusal::NoChangelogSection {
            version: VERSION.to_owned()
        }),
        "a `###` heading is a section of a release rather than a release: nesting one is how a \
         version ends up under the wrong heading"
    );
    assert_eq!(
        changelog_ok(
            &format!("# Changelog\n\n## [Unreleased]\n\n## [{VERSION}] - 2026-10-05\n- shipped\n"),
            VERSION
        ),
        Ok(()),
        "an empty `Unreleased` at the top is exactly what a released tree keeps"
    );

    for refused in [
        Refusal::NoChangelogSection {
            version: VERSION.to_owned(),
        },
        Refusal::EmptySection {
            version: VERSION.to_owned(),
        },
        Refusal::UnreleasedNotPromoted {
            version: VERSION.to_owned(),
        },
    ] {
        assert_eq!(
            refused.exit_code(),
            1,
            "a tree that cannot be released is a broken deployment rather than a wrong call: \
             {refused}"
        );
    }
}

#[test]
fn the_tool_writes_the_release_the_tag_names() {
    let out = scratch("tool-release");
    let from = binaries();
    let tag = format!("v{VERSION}");
    let output = tool(&[
        "--tag",
        &tag,
        "--target",
        TARGET,
        "--from",
        &from.to_string_lossy(),
        "--out",
        &out.to_string_lossy(),
        "--changelog",
        CHANGELOG,
    ]);
    assert!(
        output.status.success(),
        "`jobq-release --tag {tag}` has to publish the binaries this tree built, and it exited \
         with {:?}: {}",
        output.status.code(),
        said(&output)
    );

    let directory = out.join(release().directory());
    let mut expected = vec![
        CHECKSUMS.to_owned(),
        BINARIES[0].to_owned(),
        BINARIES[1].to_owned(),
    ];
    expected.sort();
    assert_eq!(
        files_in(&directory),
        expected,
        "a release is the two binaries and the checksum file, and nothing else: a build stamp, a \
         README or a debug symbol that arrived unannounced is a file nobody agreed to ship"
    );

    let manifest = read_text(&directory.join(CHECKSUMS));
    let artifacts: Vec<Artifact> = BINARIES
        .iter()
        .map(|name| artifact(name, &read(&directory.join(name))))
        .collect();
    assert_eq!(
        verify(&manifest, &artifacts),
        Ok(()),
        "the checksum file the tool wrote has to check the files the tool wrote: {manifest:?}"
    );
    for name in BINARIES {
        assert_eq!(
            digest(&read(&from.join(name))),
            digest(&read(&directory.join(name))),
            "{name} is the file that was built, byte for byte — a release that re-encoded its \
             artifacts would be a different artifact"
        );
    }

    let printed: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    let written: Vec<String> = manifest.lines().map(str::to_owned).collect();
    assert_eq!(
        printed, written,
        "the lines the tool prints are the checksum file's lines: a pipeline log is where somebody \
         looks when a download fails"
    );
}

#[test]
fn a_from_directory_without_the_binaries_is_refused_and_named() {
    let out = scratch("tool-missing");
    let empty = scratch("tool-empty-from");
    let output = tool(&[
        "--tag",
        &format!("v{VERSION}"),
        "--target",
        TARGET,
        "--from",
        &empty.to_string_lossy(),
        "--out",
        &out.to_string_lossy(),
        "--changelog",
        CHANGELOG,
    ]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "a binary that is not where it was said to be is exit code 3 — the thing asked about is \
         not there: {}",
        said(&output)
    );
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(
        complaint.contains(BINARIES[0]),
        "the refusal names the file that was missing, not the directory: {complaint}"
    );
    assert!(
        !out.join(release().directory()).exists(),
        "and nothing was written: half a release in the output directory is the state somebody \
         publishes by accident"
    );
}

#[test]
fn the_tool_wants_files_it_can_run() {
    // A directory where a binary should be: the file is there and reading it is not something the
    // operating system does.
    let out = scratch("tool-not-readable");
    let from = scratch("tool-directory-in-from");
    std::fs::create_dir_all(from.join(BINARIES[0])).expect("a directory named after a binary");
    let refused = tool(&[
        "--tag",
        &format!("v{VERSION}"),
        "--target",
        TARGET,
        "--from",
        &from.to_string_lossy(),
        "--out",
        &out.to_string_lossy(),
        "--changelog",
        CHANGELOG,
    ]);
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a file the release is made of that cannot be read is this deployment being broken, not a \
         wrong call: {}",
        said(&refused)
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains(BINARIES[0]),
        "and the refusal names it: {}",
        said(&refused)
    );
    assert!(
        !out.join(release().directory()).exists(),
        "nothing is written for a release the tool could not make"
    );

    // And the other half of the same check: bytes that read fine, and a file that is not a program.
    let out = scratch("tool-not-runnable");
    let from = scratch("tool-text-in-from");
    std::fs::write(from.join(BINARIES[0]), b"#!/not/a/binary\n")
        .expect("a file in the build directory");
    let refused = tool(&[
        "--tag",
        &format!("v{VERSION}"),
        "--target",
        TARGET,
        "--from",
        &from.to_string_lossy(),
        "--out",
        &out.to_string_lossy(),
        "--changelog",
        CHANGELOG,
    ]);
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a file that exists and cannot be run is not this release's binary either — the tool \
         checks what it is about to ship by running it, not by looking at it: {}",
        said(&refused)
    );
    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(
        complaint.contains(BINARIES[0]),
        "and it names the file rather than the directory: {complaint}"
    );
    assert!(
        !out.join(release().directory()).exists(),
        "nothing is written when a binary cannot be run"
    );
}

#[test]
fn a_release_that_cannot_be_written_is_this_machine_and_not_the_release() {
    // The output directory is a file: the release is assembled correctly and there is nowhere to
    // put it. A tool that answered `0` here would have a release job reporting success while the
    // pipeline uploaded nothing.
    let out = scratch("tool-unwritable");
    let blocker = out.join("not-a-directory");
    std::fs::write(&blocker, b"a file where a directory has to be")
        .expect("a file in the scratch directory");
    let refused = tool(&[
        "--tag",
        &format!("v{VERSION}"),
        "--target",
        TARGET,
        "--from",
        &binaries().to_string_lossy(),
        "--out",
        &blocker.to_string_lossy(),
        "--changelog",
        CHANGELOG,
    ]);
    assert_eq!(
        refused.status.code(),
        Some(1),
        "nowhere to put the release is this deployment being broken, which is exit code 1: {}",
        said(&refused)
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("could not be written"),
        "and the refusal says what it could not do, rather than leaving a release job to guess: {}",
        said(&refused)
    );
    assert_eq!(
        std::fs::read(&blocker).expect("the file that blocked the write is still there"),
        b"a file where a directory has to be",
        "and nothing was truncated or replaced on the way in"
    );
}

#[test]
fn the_tool_refuses_what_it_cannot_release() {
    // The argument the `exit-code` gate hands every binary of this project: a program that accepts
    // what it does not understand starts with the wrong settings and never says so.
    let unknown = tool(&["--kiln-exit-code-probe"]);
    assert!(
        !unknown.status.success(),
        "an argument the tool does not understand has to end in a usage error"
    );
    assert!(
        said(&unknown).contains("--kiln-exit-code-probe"),
        "and the error has to name the argument it refused: {}",
        said(&unknown)
    );

    let missing = tool(&["--tag", &format!("v{VERSION}")]);
    assert_eq!(
        missing.status.code(),
        Some(2),
        "a call that leaves out a required flag is the call being wrong: {}",
        said(&missing)
    );
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("--target"),
        "and it names the flag that is missing rather than printing the whole usage: {}",
        said(&missing)
    );

    let out = scratch("tool-wrong-tag");
    let wrong = tool(&[
        "--tag",
        "v9.9.9",
        "--target",
        TARGET,
        "--from",
        &binaries().to_string_lossy(),
        "--out",
        &out.to_string_lossy(),
        "--changelog",
        CHANGELOG,
    ]);
    assert_eq!(
        wrong.status.code(),
        Some(2),
        "the tag and the manifest disagree, which the release job has to learn before it uploads \
         anything: {}",
        said(&wrong)
    );
    assert!(
        String::from_utf8_lossy(&wrong.stderr).contains(VERSION),
        "and the refusal says which version this tree is: {}",
        said(&wrong)
    );
    assert!(
        !out.join(release().directory()).exists(),
        "nothing is written for a tag this tree is not"
    );
}

#[test]
fn the_project_ships_a_changelog_that_accounts_for_this_version() {
    let text = read_text(Path::new(CHANGELOG));
    assert_eq!(
        changelog_ok(&text, VERSION),
        Ok(()),
        "this tree's own `{CHANGELOG}` does not account for `{VERSION}`: the release tool refuses \
         to publish a version whose notes are missing, empty or still under `Unreleased`, so the \
         file has to be promoted before the tag is cut"
    );
    assert!(
        !text.to_ascii_lowercase().contains("todo"),
        "`{CHANGELOG}` still has a placeholder in it: a version whose entry says what somebody \
         meant to write is a version nobody can roll back"
    );

    let headings: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("## "))
        .collect();
    let unreleased = headings.first().copied().unwrap_or_default();
    let released = headings.get(1).copied().unwrap_or_default();
    assert!(
        unreleased.contains("Unreleased"),
        "the first section of `{CHANGELOG}` is `Unreleased`, which is where the next change goes: \
         found {headings:?}"
    );
    assert!(
        released.contains(VERSION),
        "and the newest *released* section is `{VERSION}`, the version this tree is: found \
         {headings:?}"
    );
}

#[test]
fn the_page_states_the_release_the_tool_produces() {
    let page = read_text(Path::new(PAGE));
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a release procedure with a blank in it is a \
             procedure nobody has run"
        );
    }
    for name in BINARIES {
        assert!(
            page.contains(name),
            "{PAGE} does not name `{name}`, which is one of the two files a release is made of: a \
             page that does not say what is in the release is a page a reviewer has to guess at"
        );
    }
    assert!(
        page.contains(CHECKSUMS),
        "{PAGE} does not name `{CHECKSUMS}`, the file a user checks what they downloaded with"
    );
    assert!(
        page.contains(CHANGELOG),
        "{PAGE} does not name `{CHANGELOG}`, which is where the release's own account of itself \
         comes from"
    );
    assert!(
        page.contains("jobq-release"),
        "{PAGE} does not name the tool that produces the release, so the procedure it describes \
         cannot be run"
    );
    assert!(
        page.contains(&format!("v{VERSION}")),
        "{PAGE} does not show the tag this version is published under (`v{VERSION}`): the page and \
         the manifest are two statements of the same fact, and the spec is where they meet"
    );
    assert!(
        low.contains("sha256") || low.contains("checksum"),
        "{PAGE} does not say what a user checks the download with: a checksum file nobody is told \
         about is a checksum file nobody runs"
    );
}
