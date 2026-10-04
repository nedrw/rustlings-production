//! Spec for `relay/2.21-supply-policy`: the policy file, the licences it judges, and the `unsafe`
//! budget.
//!
//! `kiln run` copies this file into the project as `tests/spec_policy.rs`.
//!
//! The gate that runs `cargo deny check` is `?deny`, a *should* gate, and it stays that way until a
//! later milestone (plan §6): `cargo-deny` is not installed on every machine, and a gate that
//! cannot run must never turn a good step red. What this file is, then, is the half of the policy
//! that can be judged offline and everywhere: it reads `deny.toml` and it reads the dependency
//! tree, and it checks that the first covers the second.
//!
//! Four things are judged, and the reason each one is a *structure* rather than a behaviour is
//! written down where it is judged (authoring.md §6.6 — a policy file is not something a running
//! service can be asked about):
//!
//! * **the policy is a policy.** `deny.toml` parses, the licence allowlist is not empty, the source
//!   policy denies what nobody named, and there is at least one crate ban. A file whose `allow` is
//!   empty and whose `unknown-registry` is a `TODO` is a file that says nothing;
//! * **the allowlist covers the tree.** Every crate this project resolves — read from `Cargo.lock`
//!   together with the manifest cargo extracted into its registry cache — declares a licence, and
//!   that licence is satisfied by the allowlist. `MIT OR Apache-2.0` is satisfied by either,
//!   `A AND B` needs both, and `Apache-2.0 WITH LLVM-exception` is a licence of its own;
//! * **the bans hold today.** A crate the policy bans is a crate the lock file does not contain.
//!   A ban that is already violated is a policy nobody read;
//! * **the `unsafe` budget is empty, and stated where the compiler enforces it.** This is the
//!   evidence for `unsafe/budget@aware`: the crate roots carry `#![forbid(unsafe_code)]`, so an
//!   `unsafe` block is a *compile error* rather than a review comment, and the spec counts the
//!   `unsafe` keywords under `src/` to show that the budget the attribute declares is the budget
//!   the code spends. The attribute is what the compiler reads, which is why it is checked as text:
//!   a spec that compiled a deliberately unsound file to watch the compiler refuse it would be
//!   writing into the learner's tree, and `spec/` is read-only (plan §3 D2).

use std::path::{Path, PathBuf};

/// The policy file this step delivers, relative to the project.
const POLICY: &str = "deny.toml";
/// How many crates have to have been judged for the licence check to mean anything.
///
/// The floor exists because the failure mode of a check like this is silence: a registry cache that
/// moved would make the loop body run zero times and the spec pass while judging nothing.
const JUDGED: usize = 50;

/// The project's own root, as the running test sees it.
fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The policy, parsed.
fn policy() -> toml::Value {
    let path = project().join(POLICY);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is the policy this step delivers, and it cannot be read: {error}",
            path.display()
        )
    });
    toml::from_str(&text).unwrap_or_else(|error| panic!("{POLICY} is not TOML: {error}"))
}

/// One field of the policy, as text, with a message that says what it is for.
fn asked<'a>(policy: &'a toml::Value, table: &str, key: &str) -> &'a str {
    policy
        .get(table)
        .and_then(|section| section.get(key))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("`{POLICY}` has no `{table}.{key}`: {policy}"))
}

/// One array of strings of the policy, as strings.
fn listed(policy: &toml::Value, table: &str, key: &str) -> Vec<String> {
    policy
        .get(table)
        .and_then(|section| section.get(key))
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|| panic!("`{POLICY}` has no `{table}.{key}` array of strings: {policy}"))
}

/// The policy exists, and it decides rather than describes.
#[test]
fn the_policy_denies_what_nobody_named_and_allows_what_it_judged() {
    let policy = policy();

    let allow = listed(&policy, "licenses", "allow");
    assert!(
        !allow.is_empty(),
        "the licence allowlist is empty: every licence is denied, including the ones this project \
         actually depends on, so `cargo deny check licenses` would fail on every crate it looked at \
         — and the half of the policy that is written down has to be the half that is decided"
    );
    assert!(
        allow.iter().any(|licence| licence == "MIT"),
        "this project depends on MIT-licensed crates (`serde`, `tokio`, …), so an allowlist that \
         does not name MIT is an allowlist for a different project: {allow:?}"
    );
    let threshold = policy
        .get("licenses")
        .and_then(|section| section.get("confidence-threshold"))
        .and_then(toml::Value::as_float)
        .unwrap_or_else(|| panic!("`{POLICY}` has no `licenses.confidence-threshold`: {policy}"));
    assert!(
        threshold >= 0.8,
        "the confidence threshold decides how much of a licence text has to match before cargo-deny \
         believes it read that licence, and {threshold} is below the tool's own default: a policy \
         nobody can trust to recognise a licence is a policy that passes on a guess"
    );

    assert_eq!(
        asked(&policy, "sources", "unknown-registry"),
        "deny",
        "a crate from a registry nobody named is a crate from a source this project never reviewed: \
         `warn` is a policy that ships it anyway"
    );
    assert_eq!(
        asked(&policy, "sources", "unknown-git"),
        "deny",
        "and so is a crate from a git URL nobody named — the source of a dependency is part of the \
         dependency"
    );
    let spec = asked(&policy, "sources", "required-git-spec");
    assert!(
        ["rev", "tag", "branch", "any"].contains(&spec),
        "`required-git-spec` has to be one of the specifier names cargo-deny reads, and `{spec}` is \
         not: a typo here is a policy that does not load"
    );
    assert_ne!(
        spec, "any",
        "`any` accepts a git source with no specifier at all, which is a dependency pinned to \
         whatever the default branch holds this morning"
    );

    assert_eq!(
        asked(&policy, "bans", "wildcards"),
        "deny",
        "a `*` version requirement is a dependency nobody can review: what lands in the build is \
         whatever was published last"
    );
    let banned = listed(&policy, "bans", "deny");
    assert!(
        !banned.is_empty(),
        "the ban list is empty: a policy that bans nothing is a policy with nothing in it"
    );
}

/// Every crate this project resolves is licensed, and its licence is one the policy allows.
#[test]
fn every_dependency_is_under_a_licence_the_policy_allows() {
    let allow: Vec<String> = listed(&policy(), "licenses", "allow")
        .into_iter()
        .map(|licence| licence.to_ascii_uppercase())
        .collect();
    let (expressions, missing) = declared_licences();

    assert!(
        expressions.len() >= JUDGED,
        "this check judged {} crate(s) and skipped {missing} (locked for another target, or not \
         extracted by cargo on this machine). A licence check that judged almost nothing is a check \
         that passes for the wrong reason: run `cargo fetch` and look again",
        expressions.len()
    );

    let mut refused = Vec::new();
    for (crate_name, expression) in &expressions {
        if !satisfied(expression, &allow) {
            refused.push(format!("{crate_name} ({expression})"));
        }
    }
    assert!(
        refused.is_empty(),
        "{} crate(s) are under a licence this policy does not allow: {}. The answer is a decision — \
         add the licence deliberately, or drop the dependency — not a widened allowlist: \
         {allow:?}",
        refused.len(),
        refused.join(", ")
    );
}

/// A ban the tree already violates is a policy nobody read.
#[test]
fn the_banned_crates_are_absent_from_the_lock_file() {
    let banned = listed(&policy(), "bans", "deny");
    let locked: Vec<String> = locked_packages()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for name in &banned {
        assert!(
            !locked.contains(name),
            "`{name}` is banned by `{POLICY}` and is in `Cargo.lock`: a ban that is already violated \
             is a policy nobody read, and the two answers to it are to remove the dependency or to \
             remove the ban deliberately"
        );
    }
}

/// The `unsafe` budget: stated at the crate roots, and empty under `src/`.
#[test]
fn the_unsafe_budget_is_stated_and_empty() {
    for root in ["src/lib.rs", "src/main.rs"] {
        let source = read(&project().join(root));
        assert!(
            source.contains("forbid(unsafe_code)"),
            "`{root}` does not forbid `unsafe`: an `unsafe` block in this service would be a review \
             comment instead of a compile error, and a budget that only lives in a review is a \
             budget nobody spends deliberately. A crate root is the only place the compiler reads \
             it from, and this crate has two roots, so both need it"
        );
    }

    let sources = rust_sources(&project().join("src"));
    assert!(
        sources.len() >= 10,
        "only {} file(s) under `src/` were read, so this count says nothing: {sources:?}",
        sources.len()
    );
    let mut spent = Vec::new();
    for path in &sources {
        let source = read(path);
        // Two things are taken out before the count, and both are the difference between a
        // keyword and a sentence about one: comments (the crate root has a comment that names the
        // keyword, and `relay/2.11-hmac-signing`'s docs talk about constant-time comparisons) and
        // the attribute's own spelling (`forbid(unsafe_code)`).
        let code = without_comments(&source).replace("unsafe_code", "");
        let count = code.matches("unsafe").count();
        if count > 0 {
            spent.push(format!("{}: {count}", path.display()));
        }
    }
    assert!(
        spent.is_empty(),
        "`unsafe` is forbidden at the crate roots and found under `src/`: {}. {} file(s) were \
         counted, so this is not a small-sample answer — the budget is the attribute, and it is \
         empty",
        spent.join(", "),
        sources.len()
    );
}

/// `(crate → licence expression)` for the crates this project resolves, and how many were skipped.
///
/// The tree comes from `Cargo.lock`, which cargo wrote when it built this project, and each
/// licence comes from the copy of the crate's manifest that cargo extracted into its registry
/// cache while building it. Both are offline and both are already on disk by the time a test runs,
/// which is the point: `cargo metadata` would start a second cargo inside a cargo test, and asking
/// the network for a licence is asking somebody else's uptime.
///
/// A crate that is locked for another target (the `windows-sys` of a Linux build) is never
/// extracted, so it cannot be judged here — that is the `missing` count, and the caller refuses to
/// accept a check that judged almost nothing on the strength of it. What judges the *whole* graph,
/// cross-target, is `cargo deny check licenses`, which is the `?deny` gate.
fn declared_licences() -> (Vec<(String, String)>, usize) {
    let Some(registry) = registry_sources() else {
        panic!(
            "no cargo registry cache was found (`$CARGO_HOME/registry/src` or `$HOME/.cargo/registry/\
             src`): the manifests of this project's dependencies are what this check reads, and \
             `cargo fetch` is what puts them there"
        )
    };
    let mut found = Vec::new();
    let mut missing = 0usize;
    for (name, version) in locked_packages() {
        // Cargo's registry layout: one directory per extracted crate, named `name-version`.
        let directory = registry.join(format!("{name}-{version}"));
        if !directory.is_dir() {
            missing += 1;
            continue;
        }
        let text = read(&directory.join("Cargo.toml"));
        let manifest: toml::Value = toml::from_str(&text).unwrap_or_else(|error| {
            panic!("{name}-{version}: the manifest cargo extracted is not TOML: {error}")
        });
        let package = manifest
            .get("package")
            .unwrap_or_else(|| panic!("{name}-{version}: the extracted manifest has no `package`"));
        let expression = package.get("license").and_then(toml::Value::as_str);
        let files = package.get("license-file").and_then(toml::Value::as_str);
        match (expression, files) {
            (Some(expression), _) => {
                found.push((format!("{name}-{version}"), expression.to_owned()))
            }
            (None, Some(file)) => found.push((
                format!("{name}-{version}"),
                format!("LicenseRef-file:{file}"),
            )),
            (None, None) => panic!(
                "{name}-{version} declares no `license` and no `license-file`: a dependency whose \
                 licence nobody can read is a dependency this policy cannot accept, whichever \
                 licence it is"
            ),
        }
    }
    (found, missing)
}

/// The crate names and versions `Cargo.lock` lists.
fn locked_packages() -> Vec<(String, String)> {
    let text = read(&project().join("Cargo.lock"));
    let lock: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("`Cargo.lock` is not TOML: {error} — run `cargo fetch`"));
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("`Cargo.lock` lists no packages, which cannot be true"));
    packages
        .iter()
        .filter_map(|package| {
            let name = package.get("name").and_then(toml::Value::as_str)?;
            let version = package.get("version").and_then(toml::Value::as_str)?;
            Some((name.to_owned(), version.to_owned()))
        })
        .collect()
}

/// The registry directory holding one directory per extracted crate, if there is one.
fn registry_sources() -> Option<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    let entries = std::fs::read_dir(cargo_home.join("registry").join("src")).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// Every `.rs` file under a directory, in path order.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// A source file with its comments taken out: what is left is code.
///
/// A line comment (`//`, which is also what `//!` is), a block comment, and the inside of a string
/// literal — a URL in a string is not a comment — are all skipped. What it does not do is track
/// character literals and lifetimes (`'a'` against `'a`), which would need a real lexer and cannot
/// change the answer: no `unsafe` keyword is ever written inside one.
fn without_comments(source: &str) -> String {
    let mut code = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    let (mut string, mut block, mut line) = (false, false, false);
    while let Some(character) = characters.next() {
        if line {
            if character == '\n' {
                line = false;
                code.push(character);
            }
            continue;
        }
        if block {
            if character == '*' && characters.peek() == Some(&'/') {
                characters.next();
                block = false;
            }
            continue;
        }
        if string {
            code.push(character);
            match character {
                '\\' => {
                    if let Some(escaped) = characters.next() {
                        code.push(escaped);
                    }
                }
                '"' => string = false,
                _ => {}
            }
            continue;
        }
        if character == '"' {
            string = true;
        } else if character == '/' {
            match characters.peek() {
                Some('/') => {
                    characters.next();
                    line = true;
                    continue;
                }
                Some('*') => {
                    characters.next();
                    block = true;
                    continue;
                }
                _ => {}
            }
        }
        code.push(character);
    }
    code
}

/// One file, as text, with a message that says which file it was.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()))
}

/// Whether an SPDX licence *expression* is satisfied by an allowlist.
///
/// The grammar is small, and this is all of it: `OR` between alternatives, `AND` inside one,
/// parentheses around either, `WITH` coupling a licence to an exception, and `/` — the spelling
/// this ecosystem used before SPDX expressions — read as `OR`. A crate whose expression is
/// `MIT OR Apache-2.0` may be taken under either licence; one that says `A AND B` needs both.
///
/// The alternatives are why this is not a `contains` on a string: an allowlist holding `MIT` does
/// permit a crate that offers `MIT OR GPL-3.0`, and a check that refused it would force the
/// allowlist to name every licence the project did *not* choose.
fn satisfied(expression: &str, allow: &[String]) -> bool {
    let text = expression
        .replace('(', " ( ")
        .replace(')', " ) ")
        .replace('/', " OR ");
    let tokens: Vec<String> = text
        .split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect();
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        allow,
    };
    let value = parser.alternatives();
    assert!(
        parser.at == tokens.len(),
        "`{expression}` did not parse as a licence expression (stopped at token {}): the policy \
         cannot judge a licence nobody can read",
        parser.at
    );
    value
}

/// A licence expression, from left to right.
struct Parser<'a> {
    /// The expression, split into tokens.
    tokens: &'a [String],
    /// How far it was read.
    at: usize,
    /// The licence names the policy allows, upper case.
    allow: &'a [String],
}

impl Parser<'_> {
    /// The `OR` level: true when any alternative is allowed.
    fn alternatives(&mut self) -> bool {
        let mut value = self.conjunction();
        while self.take("OR") {
            // Both sides are read, always: short-circuiting here would leave tokens unread and turn
            // a licence this policy refuses into a parse error.
            let other = self.conjunction();
            value = value || other;
        }
        value
    }

    /// The `AND` level: true only when every term is allowed.
    fn conjunction(&mut self) -> bool {
        let mut value = self.atom();
        while self.take("AND") {
            let other = self.atom();
            value = value && other;
        }
        value
    }

    /// One licence, one parenthesised expression, or a licence with its exception.
    fn atom(&mut self) -> bool {
        let Some(token) = self.next().cloned() else {
            return false;
        };
        if token == "(" {
            let value = self.alternatives();
            self.take(")");
            return value;
        }
        let mut name = token;
        if self.take("WITH") {
            let exception = self.next().cloned().unwrap_or_default();
            name = format!("{name} WITH {exception}");
        }
        self.allow.contains(&name)
    }

    /// The next token, without consuming it.
    fn peek(&self) -> Option<&String> {
        self.tokens.get(self.at)
    }

    /// The next token.
    fn next(&mut self) -> Option<&String> {
        let token = self.tokens.get(self.at);
        self.at += usize::from(token.is_some());
        token
    }

    /// Consume `token` when it is next.
    fn take(&mut self, token: &str) -> bool {
        if self.peek().is_some_and(|next| next == token) {
            self.at += 1;
            return true;
        }
        false
    }
}

/// The expression reader itself, on the expressions a policy has to get right.
#[test]
fn the_expression_reader_reads_the_licences_a_policy_has_to_get_right() {
    let allow: Vec<String> = [
        "MIT",
        "APACHE-2.0",
        "APACHE-2.0 WITH LLVM-EXCEPTION",
        "ZLIB",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (expression, expected) in [
        ("MIT", true),
        ("MIT OR Apache-2.0", true),
        ("MIT/Apache-2.0", true),
        ("MIT AND Apache-2.0", true),
        ("MIT AND GPL-3.0", false),
        ("GPL-3.0", false),
        ("(MIT OR Apache-2.0) AND Unicode-3.0", false),
        ("(MIT OR GPL-3.0) AND Zlib", true),
        ("Apache-2.0 WITH LLVM-exception", true),
        ("Apache-2.0 WITH LLVM-exception OR MIT", true),
        ("Unlicense OR MIT", true),
        ("GPL-3.0 AND MIT", false),
    ] {
        assert_eq!(
            satisfied(expression, &allow),
            expected,
            "`{expression}` against {allow:?}"
        );
    }
    assert!(
        !satisfied("LicenseRef-file:LICENSE", &allow),
        "a crate whose licence is a file nobody machine-read is not a crate whose licence is on the \
         allowlist: the policy has to name it (as `LicenseRef-…`) rather than pass it by default"
    );
}
