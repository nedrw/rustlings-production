//! probe = "supply/deny-policy@apply"
//! title = "Default deny, and every refusal says why"
//! hint = "Unknown licence, unlisted source, banned crate, over-budget unsafe: four refusals with four reasons — and being on no list is a refusal, not a pass."
//!
//! Step `relay/2.21-supply-policy` writes the repository's dependency policy as a file a tool
//! reads rather than as a paragraph a reviewer remembers. Two things make it a policy instead of a
//! wish list:
//!
//! 1. **it is default-deny.** A licence nobody has approved, a source nobody has approved, and a
//!    licence nobody could read are all refusals. The failure mode this prevents is a policy file
//!    whose lists are empty in week one — and an empty allowlist that allows everything is exactly
//!    the same file as one that allows nothing, so the answer has to be "nothing";
//! 2. **every refusal carries the sentence a reviewer reads.** `denied` is not a building block:
//!    "the build failed" is what sends people to `rm deny.toml`, and "`openssl-sys` is replaced by
//!    `rustls` in this workspace, see ADR 0005" is what gets a dependency swapped. A licence that
//!    is off the list names the expression; a ban carries the ban's own reason;
//! 3. **the `unsafe` budget is a number this repository spends, not a vibe.** It covers this
//!    workspace's own crates — what a third-party crate does inside itself is a licence question —
//!    and going over it refuses the build with the total and the budget in the reason.
//!
//! ## What you write
//!
//! * `licence_allowed` — `None` is a refusal, and an `OR` expression is allowed when *one* of its
//!   alternatives is;
//! * `source_allowed` — the source list, and the boundary that makes a prefix a family of sources
//!   rather than a wildcard;
//! * `ban_verdict` — a ban is a name, an optional version, and its reason;
//! * [`enforce`] — one run over a manifest: a verdict per crate, the workspace's `unsafe` total,
//!   and [`Report::allowed`] / [`Report::denials`].
//!
//! [`alternatives`] (the expression reader), the [`Denial`] Display, [`Report::over_budget`] and the
//! plumbing that tells a workspace crate from a dependency are given: reading `A OR B` is not the
//! lesson, deciding what to do with an unreadable one is.
//!
//! ## What the tests ask
//!
//! A crate on every list is allowed. A crate with no licence, a licence off the list, a source
//! nobody allowed, a banned crate — each refused with its own reason, and `MIT OR GPL-3.0-only`
//! allowed because a crate whose licence offers a choice may be used under the permitted one, while
//! the `AND` spelling of the same two is refused. `git+https://github.com/acme-evil/x` is not under
//! a `git+https://github.com/acme/` entry and `crates.io.evil` is not `crates.io`. Five hundred
//! `unsafe` uses inside a dependency spend nothing, because the budget is ours; the workspace's own
//! ones do, and one over the budget refuses the run.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies and no lockfile: a policy is a function of a manifest, so a probe hands
//! it one. **The manifest is a slice of structs** where the real tool reads `Cargo.lock` and
//! `cargo metadata` — the counts and sources here are what that reader would produce, handed in so
//! that the decision is what is under test. The expression reader handles the flat grammar this
//! repository's policy actually uses (`A`, `A OR B`, `A AND B`); parentheses and `WITH` are refused
//! rather than guessed at, which is the same default-deny rule applied one level down, and a full
//! SPDX parser is a crate this probe does not have.

use std::fmt;

/// One dependency, as one run of the policy reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crate {
    /// The package name.
    pub name: String,
    /// The exact version in the graph. A ban may name one.
    pub version: String,
    /// The licence as the package declares it, or `None` when it declares none. An SPDX
    /// expression, so `MIT OR Apache-2.0` is one licence with two alternatives.
    pub licence: Option<String>,
    /// Where it comes from, in the text cargo writes in the lockfile: `crates.io`,
    /// `git+https://github.com/acme/relay?rev=abc`, `path+file:///repo/crates/relay`.
    pub source: String,
    /// How many `unsafe` uses it contains, as the counting tool reports them. Only this
    /// workspace's own crates spend the budget.
    pub unsafe_uses: u32,
}

/// A crate the policy refuses by name, whatever its licence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ban {
    /// The package name.
    pub name: String,
    /// The one version this ban is about, or `None` for every version of it.
    pub version: Option<String>,
    /// Why, in a sentence a reviewer can act on.
    pub reason: String,
}

/// The policy, as one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// The licence ids this repository ships under. Default-deny: anything not here is refused.
    pub licences: Vec<String>,
    /// The sources dependencies may come from. An entry ending in `/` is a family of them
    /// (`git+https://github.com/acme/`); an entry that does not is one source, matched whole.
    pub sources: Vec<String>,
    /// The crates that may not enter the graph at all.
    pub bans: Vec<Ban>,
    /// How many `unsafe` uses this workspace's own crates may contain between them.
    pub unsafe_budget: u32,
}

/// Why a crate was refused, which is what a reviewer reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    /// The package declares no licence, and this repository assumes nothing about code that has
    /// not said what it is licensed under.
    NoLicence,
    /// Every alternative of the expression is off the allowlist.
    LicenceNotAllowed {
        /// The expression as it was declared, because which one it was is the whole reason.
        licence: String,
    },
    /// An expression this policy cannot read: parentheses, `WITH`, an empty term.
    LicenceUnreadable {
        /// The expression that could not be read.
        licence: String,
    },
    /// The source is not one the policy was told about.
    SourceNotAllowed {
        /// The source as the lockfile writes it.
        source: String,
    },
    /// A `bans` entry matched. The sentence is the ban's own.
    Banned {
        /// The reason the ban gives.
        reason: String,
    },
    /// This workspace's own crates use more `unsafe` than it allows itself.
    UnsafeOverBudget {
        /// The total the workspace's crates add up to.
        used: u32,
        /// The number the policy allows.
        budget: u32,
    },
}

impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLicence => f.write_str("no licence declared"),
            Self::LicenceNotAllowed { licence } => {
                write!(f, "licence `{licence}` is not one this repository ships")
            }
            Self::LicenceUnreadable { licence } => {
                write!(f, "unreadable licence expression `{licence}`")
            }
            Self::SourceNotAllowed { source } => {
                write!(f, "source `{source}` is not on the allowed list")
            }
            Self::Banned { reason } => f.write_str(reason),
            Self::UnsafeOverBudget { used, budget } => {
                write!(f, "uses {used} unsafe, the budget is {budget}")
            }
        }
    }
}

/// Is this crate part of this repository rather than a dependency of it? The `unsafe` budget is
/// about our own code: what a third-party crate does inside itself is a licence question.
#[must_use]
fn is_workspace(krate: &Crate) -> bool {
    krate.source.starts_with("path+")
}

/// Read a licence expression into its alternatives, each an `AND` of terms.
///
/// `None` is "this policy cannot read it", which is a refusal rather than a guess: parentheses and
/// `WITH` are a grammar this repository's policy does not use, and an expression nobody has decided
/// about must not be read as the most permissive thing it could mean.
#[must_use]
fn alternatives(licence: &str) -> Option<Vec<Vec<String>>> {
    let licence = licence.trim();
    if licence.is_empty() {
        return None;
    }
    let mut alternatives: Vec<Vec<String>> = Vec::new();
    for group in licence.split(" OR ") {
        let mut terms: Vec<String> = Vec::new();
        for term in group.split(" AND ") {
            let term = term.trim();
            let plain = !term.is_empty()
                && term
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'));
            if !plain {
                return None;
            }
            terms.push(term.to_owned());
        }
        alternatives.push(terms);
    }
    Some(alternatives)
}

/// May this repository ship a crate under this licence?
///
/// An `OR` expression passes when one alternative is fully on the list — a crate offered under two
/// licences may be used under the one that is allowed. No licence at all is a refusal, not a pass.
fn licence_allowed(policy: &Policy, licence: Option<&str>) -> Result<(), Denial> {
    // No licence is not "fine, probably": a package that has not said what it is licensed under
    // is the one case where nothing can be checked, and default-deny means refusing it.
    let Some(licence) = licence else {
        return Err(Denial::NoLicence);
    };
    let Some(alternatives) = alternatives(licence) else {
        return Err(Denial::LicenceUnreadable {
            licence: licence.to_owned(),
        });
    };
    // One alternative is enough, because a crate offered under two licences may be taken under
    // either: `MIT OR GPL-3.0-only` is a crate this repository can ship.
    let allowed = alternatives.iter().any(|terms| {
        terms
            .iter()
            .all(|term| policy.licences.iter().any(|allowed| allowed == term))
    });
    if allowed {
        Ok(())
    } else {
        Err(Denial::LicenceNotAllowed {
            licence: licence.to_owned(),
        })
    }
}

/// May a dependency come from this source?
///
/// A source matches an entry whole, or lives under it when the entry ends in `/`: `crates.io`
/// allows one source, and a prefix that is not a directory is not a wildcard.
fn source_allowed(policy: &Policy, source: &str) -> Result<(), Denial> {
    // Whole-string equality for an entry that is not a directory, and a prefix for one that is:
    // `crates.io` is one source, and `git+https://github.com/acme/` is every checkout under it.
    // A prefix that does not end in `/` matching by prefix is how `crates.io.evil` gets in.
    let allowed = policy.sources.iter().any(|entry| {
        entry == source || (entry.ends_with('/') && source.starts_with(entry.as_str()))
    });
    if allowed {
        Ok(())
    } else {
        Err(Denial::SourceNotAllowed {
            source: source.to_owned(),
        })
    }
}

/// Does a ban cover this crate?
fn ban_verdict(policy: &Policy, krate: &Crate) -> Result<(), Denial> {
    for ban in &policy.bans {
        let name_matches = ban.name == krate.name;
        let version_matches = ban
            .version
            .as_ref()
            .is_none_or(|version| *version == krate.version);
        if name_matches && version_matches {
            // The ban's own sentence: this is the line that tells a reviewer what to do about it.
            return Err(Denial::Banned {
                reason: ban.reason.clone(),
            });
        }
    }
    Ok(())
}

/// What the policy decided about one crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It may enter the graph.
    Allowed,
    /// It may not, and this is why.
    Denied(Denial),
}

/// One crate's verdict, as the report carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The package name.
    pub name: String,
    /// The version the verdict is about.
    pub version: String,
    /// What the policy decided.
    pub verdict: Verdict,
}

/// What one run of the policy says about a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// One verdict per crate, in the manifest's order.
    pub findings: Vec<Finding>,
    /// This workspace's own `unsafe` uses, added up.
    pub unsafe_used: u32,
    /// What they were measured against.
    pub unsafe_budget: u32,
}

impl Report {
    /// The budget's verdict, or `None` while the workspace is inside it.
    ///
    /// A refusal at exactly the budget is not a refusal: the number is what the workspace may use.
    #[must_use]
    pub fn over_budget(&self) -> Option<Denial> {
        (self.unsafe_used > self.unsafe_budget).then_some(Denial::UnsafeOverBudget {
            used: self.unsafe_used,
            budget: self.unsafe_budget,
        })
    }

    /// Is the graph allowed? Every crate, and the budget.
    #[must_use]
    pub fn allowed(&self) -> bool {
        self.over_budget().is_none()
            && self
                .findings
                .iter()
                .all(|finding| finding.verdict == Verdict::Allowed)
    }

    /// The lines CI prints, in the manifest's order: `name version: reason` for every refused
    /// crate, and — when the workspace is over budget — `workspace: reason` last.
    #[must_use]
    pub fn denials(&self) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for finding in &self.findings {
            if let Verdict::Denied(denial) = &finding.verdict {
                lines.push(format!("{} {}: {denial}", finding.name, finding.version));
            }
        }
        // The budget is a fact about the run rather than about one crate, so it is named for what
        // it is: the workspace.
        if let Some(denial) = self.over_budget() {
            lines.push(format!("workspace: {denial}"));
        }
        lines
    }
}

/// Run the policy over one manifest.
///
/// Every crate is read against all of it — licence, source and bans — because a crate may break
/// more than one rule and the first one it breaks is not the reason the other one does not matter.
/// The budget is measured over this workspace's own crates only, and it is a property of the run
/// rather than of any single crate.
#[must_use]
pub fn enforce(policy: &Policy, manifest: &[Crate]) -> Report {
    let mut findings = Vec::with_capacity(manifest.len());
    let mut unsafe_used = 0u32;
    for krate in manifest {
        if is_workspace(krate) {
            unsafe_used += krate.unsafe_uses;
        }
        // All three rules, in the order a reviewer reads them: what it is licensed under, where it
        // came from, and whether somebody already said no to it. The first refusal is kept rather
        // than the strictest one, because it is the one a reader would have found first.
        let verdict = match licence_allowed(policy, krate.licence.as_deref()) {
            Err(denial) => Verdict::Denied(denial),
            Ok(()) => match source_allowed(policy, &krate.source) {
                Err(denial) => Verdict::Denied(denial),
                Ok(()) => match ban_verdict(policy, krate) {
                    Err(denial) => Verdict::Denied(denial),
                    Ok(()) => Verdict::Allowed,
                },
            },
        };
        findings.push(Finding {
            name: krate.name.clone(),
            version: krate.version.clone(),
            verdict,
        });
    }
    Report {
        findings,
        unsafe_used,
        unsafe_budget: policy.unsafe_budget,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy as `relay/2.21` writes it, small enough to read in one screen.
    fn policy() -> Policy {
        Policy {
            licences: ["MIT", "Apache-2.0", "BSD-3-Clause", "Unicode-3.0"]
                .map(str::to_owned)
                .to_vec(),
            sources: [
                "crates.io".to_owned(),
                "git+https://github.com/acme/".to_owned(),
                "path+file:///repo/".to_owned(),
            ]
            .to_vec(),
            bans: vec![Ban {
                name: "openssl-sys".to_owned(),
                version: None,
                reason: "replaced by rustls in this workspace (ADR 0005)".to_owned(),
            }],
            unsafe_budget: 3,
        }
    }

    /// A dependency that passes every rule, so a test can change one thing about it.
    fn good(name: &str) -> Crate {
        Crate {
            name: name.to_owned(),
            version: "1.0.0".to_owned(),
            licence: Some("MIT".to_owned()),
            source: "crates.io".to_owned(),
            unsafe_uses: 0,
        }
    }

    /// A crate of this workspace, which is where the `unsafe` budget is spent.
    fn ours(name: &str, unsafe_uses: u32) -> Crate {
        Crate {
            source: "path+file:///repo/crates/relay".to_owned(),
            unsafe_uses,
            ..good(name)
        }
    }

    fn refusal(policy: &Policy, krate: &Crate) -> Denial {
        match enforce(policy, std::slice::from_ref(krate)).findings[0]
            .verdict
            .clone()
        {
            Verdict::Denied(denial) => denial,
            Verdict::Allowed => panic!("`{}` was expected to be refused", krate.name),
        }
    }

    #[test]
    fn a_crate_on_every_list_is_allowed() {
        let policy = policy();
        let report = enforce(&policy, &[good("serde"), ours("relay", 1)]);

        assert!(
            report.allowed(),
            "nothing here breaks a rule: {}",
            report.denials().join("; ")
        );
        assert_eq!(report.findings.len(), 2);
        assert_eq!(report.findings[0].verdict, Verdict::Allowed);
        assert!(report.denials().is_empty());
    }

    #[test]
    fn a_crate_that_declares_no_licence_is_refused_rather_than_assumed() {
        let policy = policy();
        let mut undeclared = good("mystery");
        undeclared.licence = None;

        assert_eq!(refusal(&policy, &undeclared), Denial::NoLicence);
        assert_eq!(Denial::NoLicence.to_string(), "no licence declared");
    }

    #[test]
    fn one_alternative_that_is_allowed_is_enough_and_both_terms_of_an_and_are_not() {
        let policy = policy();
        let mut copyleft = good("copyleft");
        copyleft.licence = Some("GPL-3.0-only".to_owned());
        assert_eq!(
            refusal(&policy, &copyleft),
            Denial::LicenceNotAllowed {
                licence: "GPL-3.0-only".to_owned()
            }
        );

        let mut either = good("either");
        either.licence = Some("MIT OR GPL-3.0-only".to_owned());
        assert_eq!(
            licence_allowed(&policy, either.licence.as_deref()),
            Ok(()),
            "a crate offered under two licences may be used under the one this repository ships"
        );

        let mut both = good("both");
        both.licence = Some("MIT AND GPL-3.0-only".to_owned());
        assert_eq!(
            refusal(&policy, &both),
            Denial::LicenceNotAllowed {
                licence: "MIT AND GPL-3.0-only".to_owned()
            },
            "`AND` means every term, and one of these is not on the list"
        );
    }

    #[test]
    fn an_expression_the_policy_cannot_read_is_refused_rather_than_guessed_at() {
        let policy = policy();
        for licence in [
            "(MIT OR Apache-2.0)",
            "MIT WITH LLVM-exception",
            "",
            "  ",
            "MIT OR",
        ] {
            let mut crate_ = good("odd");
            crate_.licence = Some(licence.to_owned());
            assert_eq!(
                refusal(&policy, &crate_),
                Denial::LicenceUnreadable {
                    licence: licence.to_owned()
                },
                "`{licence}` is not a licence this policy read"
            );
        }
    }

    #[test]
    fn a_source_nobody_allowed_is_refused_and_a_prefix_that_is_not_a_directory_is_one_source() {
        let policy = policy();

        let mut sibling = good("acme-utils");
        sibling.source = "git+https://github.com/acme/relay?rev=abc".to_owned();
        assert_eq!(source_allowed(&policy, &sibling.source), Ok(()));

        let mut lookalike = good("typosquat");
        lookalike.source = "crates.io.evil".to_owned();
        assert_eq!(
            refusal(&policy, &lookalike),
            Denial::SourceNotAllowed {
                source: "crates.io.evil".to_owned()
            },
            "an entry that is not a directory allows one source, matched whole"
        );

        let mut elsewhere = good("somebody-elses");
        elsewhere.source = "git+https://github.com/acme-evil/relay".to_owned();
        assert_eq!(
            refusal(&policy, &elsewhere),
            Denial::SourceNotAllowed {
                source: "git+https://github.com/acme-evil/relay".to_owned()
            },
            "`acme-evil` is not under `acme/`"
        );
    }

    #[test]
    fn a_banned_crate_is_refused_with_the_bans_own_sentence() {
        let policy = policy();
        let mut licensed = good("openssl-sys");
        licensed.licence = Some("Apache-2.0".to_owned());

        assert_eq!(
            refusal(&policy, &licensed),
            Denial::Banned {
                reason: "replaced by rustls in this workspace (ADR 0005)".to_owned()
            },
            "a crate may break more than one rule, and the ban is the sentence a reviewer acts on"
        );

        let mut versioned = policy.clone();
        versioned.bans = vec![Ban {
            name: "libgit2-sys".to_owned(),
            version: Some("0.16.1".to_owned()),
            reason: "0.16.1 has a known CVE; 0.16.2 is fine".to_owned(),
        }];
        let mut old = good("libgit2-sys");
        old.version = "0.16.1".to_owned();
        let mut new = good("libgit2-sys");
        new.version = "0.16.2".to_owned();

        assert!(matches!(refusal(&versioned, &old), Denial::Banned { .. }));
        assert_eq!(
            enforce(&versioned, std::slice::from_ref(&new)).findings[0].verdict,
            Verdict::Allowed,
            "a versioned ban is about one version, and the next one is a different crate"
        );
    }

    #[test]
    fn the_unsafe_budget_is_this_workspaces_own_code_and_the_report_says_when_it_is_spent() {
        let policy = policy();
        let mut heavy = good("heavy-dependency");
        heavy.unsafe_uses = 500;

        let inside = enforce(
            &policy,
            &[heavy.clone(), ours("relay", 3), ours("mailer", 0)],
        );
        assert_eq!(
            inside.unsafe_used, 3,
            "a dependency's unsafe is not this repository's"
        );
        assert!(inside.allowed(), "exactly the budget is inside it");
        assert_eq!(inside.over_budget(), None);

        let over = enforce(&policy, &[heavy, ours("relay", 3), ours("mailer", 1)]);
        assert_eq!(over.unsafe_used, 4);
        assert_eq!(
            over.over_budget(),
            Some(Denial::UnsafeOverBudget { used: 4, budget: 3 })
        );
        assert!(
            !over.allowed(),
            "every crate may be fine and the run still refused"
        );
        assert!(
            over.denials()
                .iter()
                .any(|line| line == "workspace: uses 4 unsafe, the budget is 3"),
            "the budget's refusal is a line CI prints, not a footnote: {:?}",
            over.denials()
        );
    }

    #[test]
    fn the_lines_ci_prints_name_the_crate_and_the_reason_in_manifest_order() {
        let policy = policy();
        let mut copyleft = good("copyleft");
        copyleft.licence = Some("GPL-3.0-only".to_owned());
        let mut undeclared = good("mystery");
        undeclared.licence = None;

        let report = enforce(
            &policy,
            &[copyleft, good("serde"), undeclared, ours("relay", 9)],
        );
        assert_eq!(
            report.denials(),
            vec![
                "copyleft 1.0.0: licence `GPL-3.0-only` is not one this repository ships"
                    .to_owned(),
                "mystery 1.0.0: no licence declared".to_owned(),
                "workspace: uses 9 unsafe, the budget is 3".to_owned(),
            ]
        );
        assert!(!report.allowed());
    }
}
