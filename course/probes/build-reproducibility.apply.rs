//! probe = "build/reproducibility@apply"
//! title = "The same bytes, and the differences that are named rather than hoped away"
//! hint = "Sort the set before you hash it, name a difference that is only a build location instead of explaining it away, and call a set with any artifact missing differing."
//!
//! Step `lakelet/4.22-reproducible-build` is the last phase's last question: every number the phase measured came out of a build, and if two builds of one tree disagree then what those pages measured is not a thing at all. The claim the step can honestly make is *byte for byte, or the remaining differences are named and explained*, and `release/artifacts@apply` is the other half of it — what makes the bytes behind a release name reproducible. Four decisions decide it.
//!
//! 1. **the compiler, the dependencies and the location are inputs.** `rust-toolchain.toml` pins the channel, `Cargo.lock` plus `--locked` pins the versions, and the *location* is the one nobody thinks of: a compiler writes the names of the files it read into what it produces, and the file it read that nobody thinks of is the library it linked against, which lives under the target directory. The lever is `--remap-path-prefix`, passed to both builds over the whole build location — and a location reachable under two names is two locations, so the root is resolved before anything is built;
//! 2. **a timestamp is a difference that is left, and it is not in the bytes.** The two copies' modification times differ and always will, which is why the comparison is over **content** and why the block says `difference_timestamp=mtimes_differ` rather than pretending the two copies are one file. A comparison over metadata is a comparison that fails on a working build, and `SOURCE_DATE_EPOCH` — the standard lever for the tools that do stamp a time into their output — changes nothing in these artifacts, which is a fact worth naming rather than a flag worth setting;
//! 3. **ordering is not content.** A build's output arrives in whatever order the filesystem hands it over, so a set that is hashed in the order it was listed is a set that changes when nothing changed. Sorting by name before folding anything in is what makes "the same artifacts" a claim about bytes rather than about a directory walk;
//! 4. **a difference the report cannot name is a failure, and a difference the report can name is still a difference.** A missing artifact is not an artifact that agreed; an artifact that still names the directory it was built in is not one that was remapped; and a real difference that happens to sit beside a path difference must not be renamed as one. That distinction — named versus not — is the whole of the block a caller prints.
//!
//! ## What you write
//!
//! * [`digest`] — the set's digest: every artifact's name, its length and the digest of its bytes, folded in name order;
//! * [`compare`] — the two sets against each other: which names are identical, which differ, the reason for each one that does, whether any artifact still names its location, and whether the copies' modification times differ;
//! * [`verdict`] — the word a caller prints, over the differences this comparison actually found.
//!
//! [`Artifact`], [`hash_bytes`] (a cheap deterministic hash, documented as one), [`without`] (taking an artifact's own build location out of its bytes), [`Reason`], [`Differing`], [`Comparison`] and [`Verdict`] are given. The vocabulary is the interface here: a caller reads a `Comparison` and prints it, so the words it is reported in — `location`, `missing`, `bytes`, `reproducible`, `differs` — are the step's own block, and they are not this file's to rename.
//!
//! ## What the tests ask
//!
//! One set of bytes has one digest, written down rather than derived, and two runs of one tree — a different directory, a different moment — are that same set while one more byte is not. The same artifacts in another order are the same artifacts, digest and comparison both. Two remapped runs are identical, their modification times differ and the report says so, and a comparison whose copies were written in the same second says that instead. An artifact one build produced and the other did not is `missing` in both directions of the comparison and never dropped. A build whose artifacts carry their own directory differs, and the reason for each difference is `location` — while a genuine difference beside it is `bytes`, so a real bug is not explained away as a path. And the verdict is `reproducible` exactly when nothing differs.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies and **no build**: a probe cannot compile anything, run a toolchain, or produce two artifacts from one tree — the step's `scripts/repro.sh` does that, twice, under the pinned toolchain and with `--locked`, and its spec looks inside the two trees afterwards to check that the comparison happened at all. This file is the other end of the same claim: the comparison and the vocabulary it reports in. The consequence is stated rather than hidden — the digest here is this file's own cheap hash of bytes a case wrote, not a build's; the real script compares its artifacts with `cmp` and never prints a digest at all, because a digest on a page goes stale the next time a crate moves.
//!
//! Missing on purpose, beyond the build: the block itself and the two environment variables the spec sets (`LAKELET_REPRO_ROOT`, `LAKELET_REPRO_KEEP`), the pin being *checked* against the compiler that is answering rather than assumed, the source files a build reads (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/`), and the whole deployment half of the step — the two-stage image, the 25 MB ceiling, the configuration report the gate compares, and the supply-chain policy whose lists are read off this tree's own lockfile.

/// One file a build produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The name a release build gives it: `lakeletd`, `lakeletq`, `lakelet-load`, `liblakelet.rlib`.
    pub name: String,
    /// Its bytes, byte for byte — which is the only thing two builds of one tree are compared on.
    pub bytes: Vec<u8>,
    /// When the copy was written, in seconds. Two builds of one tree differ here and always will.
    pub mtime_s: u64,
    /// The directory the build happened in, which a compiler writes into what it produces.
    pub built_at: String,
}

/// A cheap deterministic hash of `bytes`, as lower-case hex: **not** sha256, and it says so.
///
/// Given for the same reason a probe has no crates at all: the step's script compares its artifacts
/// with `cmp` and the real hashes around this step (`sha2`, in the tier's client) are not something
/// three minutes should re-derive from memory, and a hand-rolled cryptographic hash in a course file
/// would be worse than none. What is here is a function of the bytes that changes when they change,
/// so a *set* can be named by one number. Nothing in this file is safe against an attacker.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for lane in 0..4u64 {
        let mut state: u64 = 0xcbf2_9ce4_8422_2325 ^ lane.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for byte in bytes {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for byte in state.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

/// `bytes` with every occurrence of `needle` taken out.
///
/// Given, because it is the mechanism rather than the decision: it is how an artifact's *own* build
/// location is taken out of the bytes it was written into, which is what makes "the difference is
/// only the path" a computation rather than an opinion.
#[must_use]
pub fn without(bytes: &[u8], needle: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return bytes.to_vec();
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(at) = rest
        .windows(needle.len())
        .position(|window| window == needle)
    {
        out.extend_from_slice(rest.get(..at).unwrap_or_default());
        rest = rest.get(at + needle.len()..).unwrap_or_default();
    }
    out.extend_from_slice(rest);
    out
}

/// Why two artifacts of the same name are not the same artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The bytes differ, and the only difference is the directory each one was built in. The
    /// difference `--remap-path-prefix` exists to remove, named rather than hoped away.
    Location,
    /// The name is in one build and not the other. A missing artifact is a difference, never an
    /// artifact that agreed.
    Missing,
    /// The bytes differ, and nothing here names the reason.
    Bytes,
}

impl Reason {
    /// The word this reason is spelled with in a report.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Location => "location",
            Self::Missing => "missing",
            Self::Bytes => "bytes",
        }
    }
}

/// One artifact that is not the same in two builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Differing {
    /// Which artifact.
    pub name: String,
    /// Why it is not the same.
    pub reason: Reason,
}

/// What one comparison found, in the vocabulary a caller prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// The names both builds produced with the same bytes, in name order. A name is here whatever
    /// the two copies' modification times were: this list is about content.
    pub identical: Vec<String>,
    /// The names that are not the same, in name order, each with the reason this file can name.
    pub differing: Vec<Differing>,
    /// `true` when at least one artifact still carries the directory it was built in, which is the
    /// difference the remap removes — a fact the step's script fails a run over, and one the report
    /// states separately from the verdict.
    pub location_named: bool,
    /// `true` when two artifacts of one name were written at different moments. Named rather than
    /// hidden, and not a difference in content: `difference_timestamp=mtimes_differ`.
    pub mtimes_differ: bool,
}

/// What a caller prints as the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing in the set differs.
    Reproducible,
    /// Something does. A difference the report names is still a difference.
    Differs,
}

impl Verdict {
    /// The word this verdict is spelled with in a report.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reproducible => "reproducible",
            Self::Differs => "differs",
        }
    }
}

/// The set's digest: every artifact's name, its length and the digest of its bytes.
///
/// Built in **name order**, because a set has no order: a digest folded in the order a directory
/// walk happened to hand the files over would call two runs of one tree two builds. Nothing that
/// moves goes into it — not a modification time, not the directory a build happened in — which is
/// what makes it a name for what a build produced rather than for when it did.
#[must_use]
pub fn digest(artifacts: &[Artifact]) -> String {
    todo!()
}

/// Two builds of what should be one tree, compared name by name.
///
/// Both lists are answered in name order, because a report that changed its order between two runs
/// over the same files would make a page that quotes it wrong for no reason. A name one build
/// produced and the other did not is [`Reason::Missing`] — never dropped, because a comparison that
/// silently compared the intersection is a green line over a build that shipped something else. A
/// name both produced with different bytes is [`Reason::Location`] when the only difference is the
/// directory each was built in and [`Reason::Bytes`] otherwise — a real difference must not be
/// renamed as a path. A modification time is not content, so equal bytes are `identical` whatever
/// the mtimes were, and the difference between the mtimes is reported beside them.
#[must_use]
pub fn compare(first: &[Artifact], second: &[Artifact]) -> Comparison {
    todo!()
}

/// The answer a caller prints, over the differences this comparison found.
///
/// `Reproducible` exactly when nothing differs. A difference in modification times alone is not a
/// difference in content, so it does not make the verdict `Differs` — it is named in
/// [`Comparison::mtimes_differ`] and a caller prints it beside the verdict.
#[must_use]
pub fn verdict(comparison: &Comparison) -> Verdict {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a release build of this tree produces.
    const NAMES: [&str; 3] = ["lakeletd", "lakeletq", "liblakelet.rlib"];

    /// The digest of [`build`] at [`LEFT`] with the location remapped, written down rather than
    /// derived: a digest computed on both sides of an assertion only says the function still does
    /// what it did a moment ago.
    const SET_DIGEST: &str = "49ffc730ed9222799457fb2f357c6c0523b50403d33e5d0ffe022529b2169df9";

    /// The first of the two directories a run builds in — deliberately the shorter one, because two
    /// paths of the same width would hide a difference that is only the path.
    const LEFT: &str = "/build/a/tree";

    /// The second, three levels deeper.
    const RIGHT: &str = "/build/b/deeper/tree";

    /// One artifact of a build: its content is its own name plus `content`, and unless the build was
    /// remapped, the directory it happened in is written in front of that — the way a compiler
    /// writes the path of a file it read into what it produces.
    fn artifact(name: &str, root: &str, mtime_s: u64, remapped: bool) -> Artifact {
        let mut bytes: Vec<u8> = Vec::new();
        if !remapped {
            bytes.extend_from_slice(root.as_bytes());
            bytes.push(b'/');
        }
        bytes.extend_from_slice(format!("{name} content").as_bytes());
        Artifact {
            name: name.to_owned(),
            bytes,
            mtime_s,
            built_at: root.to_owned(),
        }
    }

    /// The artifacts one release build produced, at one location and one moment.
    fn build(root: &str, remapped: bool, mtime_s: u64) -> Vec<Artifact> {
        NAMES
            .iter()
            .map(|name| artifact(name, root, mtime_s, remapped))
            .collect()
    }

    /// The reasons a comparison found, in the order it reported them.
    fn reasons(comparison: &Comparison) -> Vec<Reason> {
        comparison
            .differing
            .iter()
            .map(|differing| differing.reason)
            .collect()
    }

    #[test]
    fn the_set_digest_is_pinned_and_it_names_content_rather_than_metadata() {
        let first = build(LEFT, true, 1_700_000_000);
        assert_eq!(
            digest(&first),
            SET_DIGEST,
            "one set of bytes has one digest, and the number is written down here rather than \
             derived"
        );
        assert_eq!(
            digest(&build(RIGHT, true, 1_700_000_060)),
            SET_DIGEST,
            "two runs of one tree are one set of bytes: neither the moment they were written nor \
             the directory they happened in is in the digest, because a digest that named those \
             would call every build a different build"
        );

        let mut changed = build(LEFT, true, 1_700_000_000);
        changed[0].bytes.push(b'!');
        assert_ne!(
            digest(&changed),
            SET_DIGEST,
            "and one more byte in one artifact is a different set"
        );
    }

    #[test]
    fn the_digest_moves_with_a_name_and_with_a_byte_and_not_with_an_order() {
        let base = build(LEFT, true, 1_700_000_000);

        let mut renamed = base.clone();
        renamed[0].name = "lakeletd.debug".to_owned();
        assert_ne!(
            digest(&renamed),
            digest(&base),
            "the name is part of the set: which artifact a set of bytes came from is what a report \
             is for"
        );

        let mut one_byte = base.clone();
        one_byte[2].bytes.push(b'!');
        assert_ne!(digest(&one_byte), digest(&base), "and so is content");

        let mut reordered = base.clone();
        reordered.reverse();
        assert_eq!(
            digest(&reordered),
            digest(&base),
            "while the order the artifacts arrived in is not: a set has no order, so the digest \
             sorts by name before it folds anything in"
        );
    }

    #[test]
    fn the_same_bytes_in_another_order_are_the_same_set() {
        let first = build(LEFT, true, 1_700_000_000);
        let mut reversed = first.clone();
        reversed.reverse();

        let comparison = compare(&first, &reversed);
        assert!(
            comparison.differing.is_empty(),
            "two orders of one build are one build: {:?}",
            comparison.differing
        );
        assert_eq!(
            comparison.identical,
            vec![
                "lakeletd".to_owned(),
                "lakeletq".to_owned(),
                "liblakelet.rlib".to_owned()
            ],
            "and the names are answered in name order, because a report that changed its order \
             between two runs would make the page that quotes it wrong for no reason"
        );
    }

    #[test]
    fn two_runs_of_one_tree_are_identical_and_a_timestamp_difference_is_named() {
        let left = build(LEFT, true, 1_700_000_000);
        let right = build(RIGHT, true, 1_700_000_060);
        let comparison = compare(&left, &right);

        assert_eq!(
            comparison.identical,
            vec![
                "lakeletd".to_owned(),
                "lakeletq".to_owned(),
                "liblakelet.rlib".to_owned()
            ],
            "the same bytes are the same artifact, whatever moment the copy was written at"
        );
        assert!(
            comparison.differing.is_empty(),
            "and nothing differs: {:?}",
            comparison.differing
        );
        assert!(
            comparison.mtimes_differ,
            "the two copies were written at different moments and the report says so — \
             `difference_timestamp=mtimes_differ` — rather than pretending they are one file"
        );
        assert!(
            !comparison.location_named,
            "and neither build names the directory it happened in: that is what the remap buys, \
             and a block whose artifacts still carried it would be proving nothing about the other \
             side"
        );
        assert_eq!(
            verdict(&comparison),
            Verdict::Reproducible,
            "a difference in when a copy was written is not a difference in what it is"
        );
        assert_eq!(
            Verdict::Reproducible.as_str(),
            "reproducible",
            "which is the word a caller prints"
        );

        let same_moment = build(LEFT, true, 1_700_000_000);
        let comparison = compare(&left, &same_moment);
        assert!(
            !comparison.mtimes_differ,
            "and two copies written in the same second report that instead, which is what makes \
             the other line a measurement rather than a constant"
        );
        assert_eq!(verdict(&comparison), Verdict::Reproducible);
    }

    #[test]
    fn a_missing_artifact_is_differing_and_is_never_silently_dropped() {
        let full = build(LEFT, true, 1_700_000_000);
        let mut short = full.clone();
        short.retain(|artifact| artifact.name != "liblakelet.rlib");

        assert_eq!(
            compare(&full, &short).differing,
            vec![Differing {
                name: "liblakelet.rlib".to_owned(),
                reason: Reason::Missing,
            }],
            "an artifact one build produced and the other did not is the sharpest difference a run \
             can have: a comparison that quietly compared the two intersections would print a green \
             line over a build that shipped something else"
        );
        assert_eq!(
            compare(&short, &full).differing,
            compare(&full, &short).differing,
            "and it is a difference whichever way round the two builds are named"
        );
        assert_eq!(
            verdict(&compare(&full, &short)),
            Verdict::Differs,
            "a missing artifact is a difference rather than an absence of one"
        );
        assert!(
            compare(&full, &short)
                .identical
                .contains(&"lakeletd".to_owned()),
            "while the artifacts both builds produced are still reported as identical: {:?}",
            compare(&full, &short).identical
        );
    }

    #[test]
    fn a_build_that_embeds_its_own_directory_differs_and_the_reason_is_the_location() {
        let left = build(LEFT, false, 1_700_000_000);
        let right = build(RIGHT, false, 1_700_000_000);
        let comparison = compare(&left, &right);

        assert!(
            comparison.location_named,
            "both builds still carry the directory they happened in: that is the difference \
             `--remap-path-prefix` exists to remove"
        );
        assert_eq!(
            reasons(&comparison),
            vec![Reason::Location, Reason::Location, Reason::Location],
            "the bytes differ and the only difference is the directory each was built in: naming \
             that as `bytes` would leave an operator with a difference nobody can act on"
        );
        assert!(
            comparison.identical.is_empty(),
            "and none of them is identical: {:?}",
            comparison.identical
        );
        assert_eq!(
            verdict(&comparison),
            Verdict::Differs,
            "it is still a difference — the remap is what removes it, and a run whose artifacts \
             still name their location has not been remapped"
        );

        // The same build against itself: nothing differs, and the location is still named, which is
        // the fact the step's script fails a whole run over.
        let same = compare(&left, &left);
        assert!(
            same.location_named,
            "an unremapped build names its location"
        );
        assert!(
            same.differing.is_empty(),
            "even when there is nothing to compare it against"
        );
        assert_eq!(verdict(&same), Verdict::Reproducible);
    }

    #[test]
    fn a_difference_that_is_not_the_location_is_named_as_bytes() {
        let left = build(LEFT, true, 1_700_000_000);
        let mut right = build(RIGHT, true, 1_700_000_000);
        right[0].bytes.extend_from_slice(b"!");

        let comparison = compare(&left, &right);
        assert!(
            !comparison.location_named,
            "neither artifact names a directory, so this difference is not a path"
        );
        assert_eq!(
            comparison.differing,
            vec![Differing {
                name: "lakeletd".to_owned(),
                reason: Reason::Bytes,
            }],
            "one byte of difference is a difference, and the reason is the one that sends somebody \
             looking"
        );
        assert_eq!(
            comparison.identical.len(),
            2,
            "and the other two artifacts are the same two files"
        );
    }

    #[test]
    fn a_location_and_a_real_difference_together_are_not_explained_away_as_a_path() {
        let left = build(LEFT, false, 1_700_000_000);
        let mut right = build(RIGHT, false, 1_700_000_000);
        right[1].bytes.extend_from_slice(b" and one byte more");

        assert_eq!(
            reasons(&compare(&left, &right)),
            vec![Reason::Location, Reason::Bytes, Reason::Location],
            "the location explains two of the three artifacts and not the third: naming a real \
             difference as a path is how a report talks somebody out of a bug they have"
        );
    }

    #[test]
    fn the_verdict_is_reproducible_exactly_when_nothing_differs() {
        let left = build(LEFT, true, 1_700_000_000);
        assert_eq!(
            verdict(&compare(&left, &left)),
            Verdict::Reproducible,
            "a set against itself differs nowhere"
        );

        let mut right = build(RIGHT, true, 1_700_000_060);
        assert_eq!(
            verdict(&compare(&left, &right)),
            Verdict::Reproducible,
            "and two runs of one tree are reproducible while their modification times are not \
             equal: the verdict is about content"
        );

        right[0].bytes.push(b'!');
        assert_eq!(
            verdict(&compare(&left, &right)),
            Verdict::Differs,
            "one byte is enough for the other word"
        );
        assert_eq!(
            Verdict::Differs.as_str(),
            "differs",
            "which is what a caller prints"
        );
        assert_eq!(Reason::Location.as_str(), "location");
        assert_eq!(Reason::Missing.as_str(), "missing");
        assert_eq!(Reason::Bytes.as_str(), "bytes");
    }
}
