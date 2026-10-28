//! probe = "repo/toolchain-pinning@apply"
//! title = "Pinned in exactly one place"
//! hint = "One file decides the compiler version; a second copy in the workflow is the copy that drifts."
//!
//! Step `mailer/1.2-toolchain` pins the toolchain in `rust-toolchain.toml` and then deletes every
//! other copy — including the one in CI, where repeating the version looks like diligence and is
//! the mechanism by which the two drift apart. The pinned file is the single source of truth, and
//! the workflow's job is to *use* it (`rustup show`), not to restate it.
//!
//! The files here are strings, because a probe runs offline and cannot ask rustup. What is
//! re-checked is the rule the course enforces by hand: one pin, every other file silent.
//!
//! ## What you write
//!
//! [`audit`] — what is wrong with the way these files pin the toolchain, in the order found:
//!
//! * [`Problem::NothingPinned`] when no file pins one at all;
//! * [`Problem::PinnedTwice`] for each extra pin that agrees with the first one (the same version
//!   in two places is still two places that can disagree tomorrow);
//! * [`Problem::Disagreement`] for each extra pin whose version does not match the first.
//!
//! [`scan`] is given: it is the mechanical part — a `channel = "…"` line (TOML) or a `toolchain: …`
//! line (CI), not a comment. Build the audit on top of it.
//!
//! ## What the tests ask
//!
//! One pin; the same pin twice; two different versions; nothing pinned; a commented line that only
//! looks like a pin; both syntaxes with their quotes stripped; and a third pin disagreeing with
//! the first.

/// One place a compiler version is pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// File the pin was found in.
    pub file: String,
    /// One-based line number.
    pub line: usize,
    /// The version, without quotes.
    pub version: String,
}

/// What is wrong with the way this repository pins its toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No file pins a version.
    NothingPinned,
    /// Two files pin versions that agree — still two copies to keep in step.
    PinnedTwice {
        /// The first pin found.
        first: Pin,
        /// The extra one.
        second: Pin,
    },
    /// Two files pin versions that do not agree.
    Disagreement {
        /// The first pin found.
        first: Pin,
        /// The one that disagrees.
        second: Pin,
    },
}

/// Every pin in `files`, in file order and then line order.
///
/// A pin is a line whose trimmed form starts with `channel =` or `toolchain:`; a `#` comment is
/// not a pin. The value has surrounding quotes and whitespace stripped.
#[must_use]
pub fn scan(files: &[(&str, &str)]) -> Vec<Pin> {
    let mut pins = Vec::new();
    for (file, content) in files {
        for (offset, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            let value = trimmed
                .strip_prefix("channel =")
                .or_else(|| trimmed.strip_prefix("channel="))
                .or_else(|| trimmed.strip_prefix("toolchain:"));
            let Some(value) = value else {
                continue;
            };
            let version = value.trim().trim_matches('"').trim();
            if version.is_empty() {
                continue;
            }
            pins.push(Pin {
                file: (*file).to_owned(),
                line: offset + 1,
                version: version.to_owned(),
            });
        }
    }
    pins
}

/// What is wrong with the way these files pin the toolchain, in the order found.
#[must_use]
pub fn audit(files: &[(&str, &str)]) -> Vec<Problem> {
    let pins = scan(files);
    let Some((first, rest)) = pins.split_first() else {
        return vec![Problem::NothingPinned];
    };
    // Every extra pin is judged against the first one: that is the pin the others are supposed to
    // be copies of, and the disagreement is what a reader of the first file would not expect.
    rest.iter()
        .map(|pin| {
            if pin.version == first.version {
                Problem::PinnedTwice {
                    first: first.clone(),
                    second: pin.clone(),
                }
            } else {
                Problem::Disagreement {
                    first: first.clone(),
                    second: pin.clone(),
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(file: &str, line: usize, version: &str) -> Pin {
        Pin {
            file: file.to_owned(),
            line,
            version: version.to_owned(),
        }
    }

    #[test]
    fn one_pin_in_one_file_is_the_whole_point() {
        let files = [
            ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.98.1\"\n"),
            ("ci.yml", "      - run: rustup show\n"),
        ];
        assert_eq!(audit(&files), []);
    }

    #[test]
    fn the_same_version_in_two_files_is_still_two_pins() {
        let files = [
            ("rust-toolchain.toml", "channel = \"1.98.1\"\n"),
            ("ci.yml", "  toolchain: 1.98.1\n"),
        ];
        assert_eq!(
            audit(&files),
            [Problem::PinnedTwice {
                first: pin("rust-toolchain.toml", 1, "1.98.1"),
                second: pin("ci.yml", 1, "1.98.1"),
            }]
        );
    }

    #[test]
    fn two_different_versions_are_a_disagreement() {
        let files = [
            ("rust-toolchain.toml", "channel = \"1.98.1\"\n"),
            (
                "ci.yml",
                "      - run: cargo +1.80 test\n  toolchain: 1.80.0\n",
            ),
        ];
        assert_eq!(
            audit(&files),
            [Problem::Disagreement {
                first: pin("rust-toolchain.toml", 1, "1.98.1"),
                second: pin("ci.yml", 2, "1.80.0"),
            }]
        );
    }

    #[test]
    fn nothing_pinned_is_its_own_problem() {
        let files = [("ci.yml", "      - run: cargo test\n")];
        assert_eq!(audit(&files), [Problem::NothingPinned]);
    }

    #[test]
    fn a_comment_is_not_a_pin() {
        assert_eq!(
            audit(&[("ci.yml", "# channel = \"1.80.0\"\n")]),
            [Problem::NothingPinned]
        );
    }

    #[test]
    fn both_syntaxes_are_recognised_and_quotes_are_stripped() {
        let files = [
            ("rust-toolchain.toml", "channel = \"1.98.1\"\n"),
            ("ci.yml", "toolchain:\n"),
            ("notes.md", "update the toolchain: later\n"),
        ];
        assert_eq!(scan(&files), [pin("rust-toolchain.toml", 1, "1.98.1")]);
        // One pin, so the audit is silent — and the scan is what made it so.
        assert_eq!(audit(&files), []);
    }

    #[test]
    fn every_extra_pin_is_reported_against_the_first() {
        let files = [
            ("rust-toolchain.toml", "channel = \"1.98.1\"\n"),
            ("ci.yml", "toolchain: 1.80.0\n"),
            ("docker.md", "channel = \"1.98.1\"\n"),
        ];
        assert_eq!(
            audit(&files),
            [
                Problem::Disagreement {
                    first: pin("rust-toolchain.toml", 1, "1.98.1"),
                    second: pin("ci.yml", 1, "1.80.0"),
                },
                Problem::PinnedTwice {
                    first: pin("rust-toolchain.toml", 1, "1.98.1"),
                    second: pin("docker.md", 1, "1.98.1"),
                },
            ]
        );
    }
}
