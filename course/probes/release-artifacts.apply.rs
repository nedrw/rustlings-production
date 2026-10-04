//! probe = "release/artifacts@apply"
//! title = "An artifact is a promise about a file"
//! hint = "Name the archive from the version and the target, write `digest  name` with two spaces, refuse a digest that does not match — and refuse to release a changelog that does not say what changed."
//!
//! Step `jobq/3.25-release-artifacts` is the last step of the track, and it is about the two files a
//! release is: the artifact and the thing that lets somebody check they got it. Everything in this
//! file is a rule somebody paid for once:
//!
//! * **the name carries the version and the target.** `jobq-0.4.0-aarch64-apple-darwin.tar.gz` is
//!   two facts in a file name, and a release that reuses a name for different bytes is a release
//!   whose checksum cannot save it — a proxy, a mirror or a CI cache will hand out the old one;
//! * **a checksum file is a format, not a hash.** `<hex>  <name>`, **two spaces**, one line per
//!   file, sorted — that is what `sha256sum -c` reads, and a file a human can read but the tool
//!   cannot is a file that gets checked by hand at the moment nobody has time;
//! * **verification is both directions.** A digest that does not match is obvious; a manifest that
//!   names a file nobody shipped is the one that goes unnoticed, and it is the one that means the
//!   release is incomplete;
//! * **the changelog is part of the artifact.** A version whose section is missing, empty, or still
//!   sitting under an `Unreleased` heading has published a number without saying what is in it, and
//!   that is the state in which a rollback cannot be reasoned about. The section is checked, not
//!   trusted.
//!
//! ## What you write
//!
//! * [`plan`] — the names one release publishes, from its version and its target;
//! * [`checksums`] — the checksum file for a set of artifacts;
//! * [`verify`] — a downloaded set of files against the manifest it came with;
//! * [`changelog_ok`] — whether a changelog is ready to be the release's account of itself.
//!
//! [`Artifact`], [`Plan`], [`Refused`] and [`digest`] are given: the refusal vocabulary is the
//! interface a release script branches on, and the hash is given on purpose — see below.
//!
//! ## What the tests ask
//!
//! The plan names the archive, the two binaries and the changelog, and the archive's name carries
//! both the version and the target. A checksum file is the exact text `sha256sum -c` expects, sorted
//! by name. One flipped byte is a refusal that names the file and both digests, and so is a manifest
//! line for a file nobody shipped, and a file that is not in the manifest. The changelog: a missing
//! section, an empty one, and a version still sitting under `Unreleased` are three different
//! refusals, and a changelog that says what changed passes.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no `cargo-dist`, no tarball and no real hash**: the "artifacts" are
//! byte strings, the archive is never built, and [`digest`] is a 64-bit FNV rather than SHA-256. A
//! hand-rolled SHA-256 in a course file would be worse than no hash at all (the same reasoning as
//! `sec/hmac-signing@apply`, which injects its MAC), and what this exercise is about is everything
//! around the hash: the name, the format, the two directions of verification and the changelog. The
//! real step builds the two binaries, runs `cargo-dist`, and its spec checks the version the binary
//! reports against the tag it was built at — none of which a probe can do.
//!
//! Also missing on purpose: no signing (a signature is a key to manage, and the step's answer is
//! checksums plus a pinned CI job), no platform-specific packaging beyond the target triple in the
//! name, and no release *policy* — which version is allowed to be a major bump is a decision for a
//! human and a CHANGELOG entry, not for a probe.

/// One file a release publishes, with its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The file name, exactly as it will be downloaded.
    pub name: String,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// What one release publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The version being released.
    pub version: String,
    /// The target triple it was built for.
    pub target: String,
    /// The executables inside the archive, in the order the archive holds them.
    pub binaries: Vec<String>,
    /// The archive's file name.
    pub archive: String,
    /// The checksum file's name.
    pub checksums: String,
    /// The changelog's name.
    pub changelog: String,
}

/// Why a release was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The bytes do not hash to what the manifest says.
    ChecksumMismatch {
        /// The file.
        name: String,
        /// What the manifest says.
        expected: String,
        /// What the bytes hash to.
        found: String,
    },
    /// A file is not in the manifest at all.
    NotInManifest {
        /// The file.
        name: String,
    },
    /// The manifest names a file that was not shipped: the release is incomplete.
    MissingFile {
        /// The file the manifest promises.
        name: String,
    },
    /// The changelog has no section for this version.
    NoChangelogSection {
        /// The version being released.
        version: String,
    },
    /// The section for this version is there and empty.
    EmptySection {
        /// The version being released.
        version: String,
    },
    /// There is still an `Unreleased` section above this version's: the release forgot to promote
    /// it, so the version does not say what is in it.
    UnreleasedNotPromoted {
        /// The version being released.
        version: String,
    },
}

/// A digest of `bytes`, as sixteen lower-case hex characters.
///
/// FNV-1a 64, and given rather than written: a real release publishes SHA-256, and a hand-rolled
/// SHA-256 in a course file teaches nothing about releases while being wrong in ways nobody would
/// notice. What is checked here is the *format* and the *verification*, which is where the mistakes
/// that matter live.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Plan {
    /// What releasing `version` for `target` publishes.
    ///
    /// The archive is `jobq-<version>-<target>.tar.gz`, its checksum file is the archive's name with
    /// `.sha256`, and the two executables it holds are `jobqd` (the daemon) and `jobqc` (the client),
    /// in that order.
    #[must_use]
    pub fn of(version: &str, target: &str) -> Self {
        todo!()
    }
}

/// The checksum file for `artifacts`: one `digest  name` line each, sorted by name.
///
/// Two spaces, because that is the separator `sha256sum -c` reads and a tab is a file that a human
/// can read and the tool cannot.
#[must_use]
pub fn checksums(artifacts: &[Artifact]) -> String {
    todo!()
}

/// Check a downloaded set of files against the manifest it came with.
///
/// Both directions: every artifact must be in the manifest with the digest it has, and every line of
/// the manifest must name an artifact that is actually here. The second half is the one an
/// incomplete release slips past — a manifest promising a file nobody built looks exactly like a
/// manifest, and the machine that consumes the release fails much later, somewhere else.
///
/// # Errors
///
/// [`Refused::NotInManifest`], [`Refused::ChecksumMismatch`] or [`Refused::MissingFile`], naming the
/// first file that is wrong.
pub fn verify(manifest: &str, artifacts: &[Artifact]) -> Result<(), Refused> {
    todo!()
}

/// Is this changelog ready to be the release's account of `version`?
///
/// Three rules, in the order a reader would notice them:
///
/// 1. the version has a section of its own (`## [0.4.0]`, or `## 0.4.0` with anything after it);
/// 2. the section says something — at least one line that is not blank before the next heading;
/// 3. there are no notes left above it. An empty `## [Unreleased]` heading is the placeholder every
///    changelog keeps, but one that still *holds* entries means this release did not promote them,
///    and the version it is publishing does not say what went out with it.
///
/// # Errors
///
/// One of the three changelog refusals, naming the version.
pub fn changelog_ok(changelog: &str, version: &str) -> Result<(), Refused> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "aarch64-apple-darwin";

    fn artifact(name: &str, bytes: &[u8]) -> Artifact {
        Artifact {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
        }
    }

    /// A release as it comes out of the plan: the archive, its checksum file and the changelog.
    fn release(version: &str) -> (Plan, Vec<Artifact>) {
        let plan = Plan::of(version, TARGET);
        let archive = artifact(&plan.archive, b"the archive the build produced");
        let changelog = artifact(
            &plan.changelog,
            b"# Changelog\n\n## [0.4.0]\n- the claim is one statement now\n",
        );
        (plan, vec![archive, changelog])
    }

    #[test]
    fn the_plan_names_the_archive_the_binaries_and_the_changelog() {
        let plan = Plan::of("0.4.0", TARGET);
        assert_eq!(
            plan.archive, "jobq-0.4.0-aarch64-apple-darwin.tar.gz",
            "the name carries the version and the target, because the file name is the only place \
             a person sees both at once"
        );
        assert_eq!(
            plan.binaries,
            vec!["jobqd".to_owned(), "jobqc".to_owned()],
            "an artifact nobody can run is not an artifact: the daemon and the client, in that order"
        );
        assert_eq!(plan.checksums, format!("{}.sha256", plan.archive));
        assert_eq!(plan.changelog, "CHANGELOG.md");
        assert_eq!(plan.version, "0.4.0");
        assert_eq!(plan.target, TARGET);

        // Two versions of one target do not collide, and neither do two targets of one version.
        assert_ne!(Plan::of("0.4.1", TARGET).archive, plan.archive);
        assert_ne!(
            Plan::of("0.4.0", "x86_64-unknown-linux-gnu").archive,
            plan.archive
        );
    }

    #[test]
    fn a_checksum_file_is_the_format_a_tool_reads() {
        let plan = Plan::of("0.4.0", TARGET);
        let artifacts = vec![
            artifact(&plan.archive, b"archive"),
            artifact(&plan.changelog, b"# Changelog\n"),
            artifact("jobqd", b"a binary"),
        ];
        let manifest = checksums(&artifacts);

        let lines: Vec<&str> = manifest.lines().collect();
        assert_eq!(lines.len(), 3, "one line per file: {manifest:?}");
        assert_eq!(
            lines,
            {
                let mut expected: Vec<String> = artifacts
                    .iter()
                    .map(|artifact| format!("{}  {}", digest(&artifact.bytes), artifact.name))
                    .collect();
                expected.sort();
                expected
            },
            "sorted by name, so two runs over the same set produce the same bytes"
        );
        assert!(
            lines.iter().all(|line| line.contains("  ")),
            "two spaces: a tab is a file a human reads and `sha256sum -c` refuses"
        );
        assert!(
            lines.iter().all(
                |line| line.split("  ").next().is_some_and(|hex| hex.len() == 16
                    && hex
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
            ),
            "and the digest is lower-case hex: {manifest:?}"
        );
        assert!(
            manifest.ends_with('\n'),
            "the last line ends, or `while read` drops it"
        );
        assert_eq!(
            checksums(&[]),
            "",
            "nothing to check is an empty file, not a broken one"
        );

        assert_eq!(
            verify(&manifest, &artifacts),
            Ok(()),
            "the files this manifest was written for verify"
        );
    }

    #[test]
    fn one_flipped_byte_is_a_refusal_that_names_the_file() {
        let plan = Plan::of("0.4.0", TARGET);
        let good = vec![artifact(&plan.archive, b"archive")];
        let manifest = checksums(&good);

        let mut tampered = good.clone();
        tampered[0].bytes = b"archivf".to_vec();
        let refused = verify(&manifest, &tampered).expect_err("the bytes changed");
        match refused {
            Refused::ChecksumMismatch {
                ref name,
                ref expected,
                ref found,
            } => {
                assert_eq!(
                    name, &plan.archive,
                    "the refusal names the file, not the release"
                );
                assert_eq!(expected, &digest(b"archive"));
                assert_eq!(found, &digest(b"archivf"));
                assert_ne!(expected, found);
            }
            other => panic!("a changed file is a checksum mismatch, not {other:?}"),
        }
    }

    #[test]
    fn a_manifest_naming_something_nobody_shipped_is_an_incomplete_release() {
        let plan = Plan::of("0.4.0", TARGET);
        let shipped = vec![artifact(&plan.archive, b"archive")];
        let manifest = format!(
            "{}{}  jobqd\n",
            checksums(&shipped),
            digest(b"a binary that was never built")
        );

        assert_eq!(
            verify(&manifest, &shipped),
            Err(Refused::MissingFile {
                name: "jobqd".to_owned()
            }),
            "the manifest promises a file that is not here: this is the half of verification nobody \
             notices until a user does"
        );

        // And the other direction: a file that is not in the manifest is a file nobody checked.
        let extra = vec![
            artifact(&plan.archive, b"archive"),
            artifact("an-afterthought", b"extra"),
        ];
        assert_eq!(
            verify(&checksums(&shipped), &extra),
            Err(Refused::NotInManifest {
                name: "an-afterthought".to_owned()
            })
        );
    }

    #[test]
    fn a_release_verifies_from_the_two_files_it_ships() {
        let (plan, artifacts) = release("0.4.0");
        let manifest = checksums(&[artifacts[1].clone()]);
        assert_eq!(
            verify(&manifest, &[artifacts[1].clone()]),
            Ok(()),
            "a release is the artifact and its checksum file: nothing else is needed to check it"
        );
        assert!(
            plan.archive.starts_with("jobq-0.4.0-"),
            "and the archive a user downloaded says which release it is"
        );

        // An empty manifest is not evidence about anything: the file that was not listed is exactly
        // the file that was not checked.
        assert_eq!(
            verify("", &artifacts),
            Err(Refused::NotInManifest {
                name: artifacts[0].name.clone()
            })
        );
    }

    #[test]
    fn the_changelog_has_to_say_what_the_version_contains() {
        let good =
            "# Changelog\n\n## [Unreleased]\n\n## [0.4.0] - 2026-01-05\n- one claim statement\n";
        assert_eq!(
            changelog_ok(good, "0.4.0"),
            Ok(()),
            "a promoted section with content is what `Unreleased` is supposed to become"
        );

        assert_eq!(
            changelog_ok("# Changelog\n\n## [0.3.0]\n- older\n", "0.4.0"),
            Err(Refused::NoChangelogSection {
                version: "0.4.0".to_owned()
            }),
            "a version with no section has not said what it contains"
        );

        assert_eq!(
            changelog_ok("# Changelog\n\n## [0.4.0]\n\n\n", "0.4.0"),
            Err(Refused::EmptySection {
                version: "0.4.0".to_owned()
            }),
            "an empty section is worse than none: it looks like the release was documented"
        );

        let forgot =
            "# Changelog\n\n## [Unreleased]\n- the fix that is going out\n\n## [0.4.0]\n- older\n";
        assert_eq!(
            changelog_ok(forgot, "0.4.0"),
            Err(Refused::UnreleasedNotPromoted {
                version: "0.4.0".to_owned()
            }),
            "the notes are still under `Unreleased`, so `0.4.0` does not say what went out with it"
        );
    }

    #[test]
    fn the_two_names_a_release_is_checked_by_are_the_plans_own() {
        let (plan, artifacts) = release("0.4.0");
        let names: Vec<&str> = artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect();
        assert!(
            names.contains(&plan.archive.as_str()),
            "the archive the plan names is an artifact of the release: {names:?}"
        );
        assert!(
            names.contains(&plan.changelog.as_str()),
            "and so is the changelog, because a version without one is a number without an account \
             of itself: {names:?}"
        );
        assert!(
            !names.contains(&plan.checksums.as_str()),
            "the checksum file is not hashed by itself: a manifest that covered itself could not be \
             used to check anything"
        );
    }
}
