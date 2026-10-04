//! Content locales: the fallback chain, step-body resolution and translation drift (plan §3 D5).
//!
//! English is the only source text. Every other locale is a translation that may be missing or
//! stale, and neither is an error: resolution walks the fallback chain (`zh-Hans → zh → en`),
//! reports the fallback to the learner, and `kiln i18n status` reports coverage, structure
//! problems and hash drift separately — structure breaks CI, drift is a warning for the
//! translator (M0 acceptance: `kiln --lang ja` must work on a repository with no `ja` file).

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{KilnError, Result};
use crate::info::Origin;

pub mod status;
pub mod ui;

pub use status::{LocaleReport, Report};
pub use ui::Ui;

/// Prefix every step body path starts with (authoring.md §1).
const STEP_PREFIX: &str = "docs/steps/";

/// A locale tag as it appears in a directory name: `en`, `zh-Hans`, `pt-BR`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Locale(String);

impl Locale {
    /// Validate and wrap a locale tag.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let well_formed = !text.is_empty()
            && text.starts_with(|c: char| c.is_ascii_lowercase())
            && text.split('-').all(|subtag| {
                !subtag.is_empty() && subtag.bytes().all(|b| b.is_ascii_alphanumeric())
            });
        if !well_formed {
            return Err(KilnError::Invalid(format!(
                "`{text}` is not a locale tag: expected a language subtag like `en`, `zh-Hans` or `pt-BR`"
            )));
        }
        Ok(Self(text.to_owned()))
    }

    /// Tag as written in directories and front matter.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Fallback chain for this locale: most specific first, the source locale last.
    ///
    /// `zh-Hans` yields `[zh-Hans, zh, en]`, `ja` yields `[ja, en]`, and the source locale
    /// yields just itself — the chain is derived, so adding a language is still "drop a
    /// directory in", never a code change.
    #[must_use]
    pub fn chain(&self, source: &Locale) -> Vec<Locale> {
        let mut chain = vec![self.clone()];
        let mut current = self.0.clone();
        while let Some((head, _)) = current.rsplit_once('-') {
            current = head.to_owned();
            let candidate = Locale(current.clone());
            if !chain.contains(&candidate) {
                chain.push(candidate);
            }
        }
        if !chain.contains(source) {
            chain.push(source.clone());
        }
        chain
    }

    /// `true` if this locale is one of the fallbacks of `requested` rather than the request.
    #[must_use]
    pub fn falls_back_to(&self, requested: &Locale) -> bool {
        self != requested
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Locale {
    type Err = KilnError;

    fn from_str(text: &str) -> Result<Self> {
        Self::parse(text)
    }
}

impl TryFrom<String> for Locale {
    type Error = KilnError;

    fn try_from(text: String) -> Result<Self> {
        Self::parse(&text)
    }
}

impl From<Locale> for String {
    fn from(locale: Locale) -> Self {
        locale.0
    }
}

/// Rewrite the locale segment of a step body path: `docs/steps/en/mailer/1.14.md` for `ja`
/// becomes `docs/steps/ja/mailer/1.14.md`, relative to the course directory.
///
/// The English source path in `track.toml` stays canonical: no metadata changes when a
/// translation appears, only a new directory does.
pub fn entry_for_locale(entry: &str, locale: &Locale) -> Result<PathBuf> {
    let rest = entry
        .strip_prefix(STEP_PREFIX)
        .ok_or_else(|| not_a_step_body(entry))?;
    let (_, tail) = rest.split_once('/').ok_or_else(|| not_a_step_body(entry))?;
    if tail.is_empty() {
        return Err(not_a_step_body(entry));
    }
    Ok(PathBuf::from(STEP_PREFIX).join(locale.as_str()).join(tail))
}

fn not_a_step_body(entry: &str) -> KilnError {
    KilnError::Invalid(format!(
        "`{entry}` is not a step body path: expected `{STEP_PREFIX}<locale>/<track>/<file>.md`"
    ))
}

/// Where the body of a step was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepDoc {
    /// A body exists; `fell_back` says whether a translation had to be skipped.
    Found {
        /// Absolute path of the body.
        path: PathBuf,
        /// Locale of the body that was found.
        locale: Locale,
        /// `true` when it is not the locale the learner asked for.
        fell_back: bool,
    },
    /// No body exists in any locale of the chain — the content is not authored yet.
    Missing {
        /// Path the source body would live at, for the error message.
        path: PathBuf,
    },
}

impl StepDoc {
    /// Path of the body, whether or not it exists.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            StepDoc::Found { path, .. } | StepDoc::Missing { path } => path,
        }
    }

    /// Locale the body is written in, or `None` when no body exists at all.
    #[must_use]
    pub fn locale(&self) -> Option<&Locale> {
        match self {
            StepDoc::Found { locale, .. } => Some(locale),
            StepDoc::Missing { .. } => None,
        }
    }
}

/// Resolve a step body for the requested locale, walking the fallback chain.
///
/// `course_dir` is the `course/` directory; `entry` is the source path from `track.toml`.
pub fn resolve_step_doc(
    course_dir: &Path,
    entry: &str,
    requested: &Locale,
    source: &Locale,
) -> Result<StepDoc> {
    for locale in requested.chain(source) {
        let path = course_dir.join(entry_for_locale(entry, &locale)?);
        if path.is_file() {
            return Ok(StepDoc::Found {
                fell_back: locale.falls_back_to(requested),
                path,
                locale,
            });
        }
    }
    Ok(StepDoc::Missing {
        path: course_dir.join(entry_for_locale(entry, source)?),
    })
}

/// The learner-facing notice for a fallback, or `None` when the requested locale was found.
#[must_use]
pub fn fallback_notice(
    step: &str,
    requested: &Locale,
    doc: &StepDoc,
    source: &Locale,
) -> Option<String> {
    match doc {
        StepDoc::Found { locale, .. } if locale.falls_back_to(requested) => Some(format!(
            "no `{requested}` body for {step} yet — falling back to `{locale}`"
        )),
        StepDoc::Missing { .. } if requested != source => Some(format!(
            "no step body for {step} in any of `{}` — content not authored yet",
            requested
                .chain(source)
                .iter()
                .map(Locale::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => None,
    }
}

/// Front matter of a step body (`+++ … +++`, authoring.md §2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontMatter {
    /// Step id the body belongs to (`mailer/1.14-worker`).
    #[serde(default)]
    pub id: Option<String>,
    /// Localized title.
    #[serde(default)]
    pub title: Option<String>,
    /// Locale of this body.
    #[serde(default)]
    pub locale: Option<String>,
    /// Hash of the English source this translation was made from (`sha256:<hex>`).
    #[serde(default)]
    pub source_hash: Option<String>,
    /// Who translated it.
    #[serde(default)]
    pub translator: Option<String>,
}

/// A parsed step body: front matter plus the structure the i18n checks compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepBody {
    /// Parsed front matter (empty when the file has none).
    pub front_matter: FrontMatter,
    /// Headings, in order, trimmed — the structure a translation must keep.
    pub headings: Vec<String>,
    /// Code fence contents, in order — a translation must not touch them.
    pub fences: Vec<String>,
    /// The prose after the front matter, headings and all.
    ///
    /// `lint-caps` reads one section of it: a `deepen` or `transfer` step has to say *where* it
    /// builds on, and "should be true" is not a rule a linter can enforce (authoring.md §4.3).
    pub text: String,
}

/// Read and parse a step body.
pub fn read_step_body(path: &Path) -> Result<StepBody> {
    let text = std::fs::read_to_string(path).map_err(|source| KilnError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let (front_matter, body) = split_front_matter(&text, path)?;
    Ok(StepBody {
        front_matter,
        headings: body
            .lines()
            .filter(|line| line.starts_with('#'))
            .map(|line| line.trim().to_owned())
            .collect(),
        fences: code_fences(body),
        text: body.to_owned(),
    })
}

/// The prose under one section heading (`## RECALL`), up to the next heading of any level.
///
/// Case-insensitive in the heading name, and forgiving about the level (`#`, `##`, `###`): the
/// section names are the contract, not the exact number of hashes.
#[must_use]
pub fn section_text(text: &str, section: &str) -> String {
    let wanted = section
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_ascii_uppercase();
    let mut inside = false;
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        let heading = line.trim_start().starts_with('#');
        if heading {
            let name = line
                .trim_start()
                .trim_start_matches('#')
                .trim()
                .to_ascii_uppercase();
            if inside {
                break;
            }
            inside = name == wanted;
            continue;
        }
        if inside {
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Split a body into front matter and prose.
fn split_front_matter<'a>(text: &'a str, path: &Path) -> Result<(FrontMatter, &'a str)> {
    let Some(rest) = text.strip_prefix("+++") else {
        return Ok((FrontMatter::default(), text));
    };
    let rest = rest.strip_prefix('\n').ok_or_else(|| {
        KilnError::Invalid(format!(
            "{}: front matter must start on its own line",
            path.display()
        ))
    })?;
    let (raw, body) = rest.split_once("\n+++").ok_or_else(|| {
        KilnError::Invalid(format!(
            "{}: front matter is not closed with `+++`",
            path.display()
        ))
    })?;
    let front_matter = toml::from_str(raw).map_err(|source| KilnError::Metadata {
        path: path.to_path_buf(),
        source,
    })?;
    Ok((front_matter, body))
}

/// Contents of every fenced code block, in order.
fn code_fences(body: &str) -> Vec<String> {
    let mut fences = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            match current.take() {
                Some(lines) => fences.push(lines.join("\n")),
                None => current = Some(Vec::new()),
            }
            continue;
        }
        if let Some(lines) = current.as_mut() {
            lines.push(line);
        }
    }
    fences
}

/// The section headings a body has to carry, by template (authoring.md §4).
///
/// One source of truth for two checks: `lint-caps` R5 reads the source body, and the `i18n` gate
/// reads the body the learner's locale resolves to.
#[must_use]
pub fn required_sections(origin: Origin) -> &'static [&'static str] {
    match origin {
        Origin::Teach => &["WHY", "TASK", "DONE WHEN", "TRAP", "FURTHER"],
        Origin::Deepen | Origin::Transfer => &["RECALL", "DELTA", "TASK", "DONE WHEN", "TRAP"],
    }
}

/// Section names a body actually has, normalised for comparison (`## Task` counts as `TASK`).
#[must_use]
pub fn sections(headings: &[String]) -> Vec<String> {
    headings
        .iter()
        .map(|heading| heading.trim_start_matches('#').trim().to_ascii_uppercase())
        .collect()
}

/// What one step body looks like in the locale a learner asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyCheck {
    /// Locale of the body that was found, when one was.
    pub found: Option<Locale>,
    /// Path the body was found at (or would live at).
    pub path: PathBuf,
    /// `true` when the requested locale had to be given up on.
    pub fell_back: bool,
    /// Problems that make the body unusable (authoring.md §8.4).
    pub problems: Vec<String>,
    /// Warnings that do not block: a translation made from an older source.
    pub drift: Vec<String>,
}

/// Check one step body against its template and, when it is a translation, against its source.
///
/// Deliberately per step and per locale: the `i18n` gate judges the body the learner is reading
/// right now, while `kiln i18n status` reports the whole course.
pub fn check_body(
    course_dir: &Path,
    entry: &str,
    origin: Origin,
    requested: &Locale,
    source: &Locale,
) -> Result<BodyCheck> {
    let doc = resolve_step_doc(course_dir, entry, requested, source)?;
    let mut check = BodyCheck {
        found: doc.locale().cloned(),
        path: doc.path().to_path_buf(),
        fell_back: false,
        problems: Vec::new(),
        drift: Vec::new(),
    };
    let (path, locale) = match &doc {
        StepDoc::Found { path, locale, .. } => (path.clone(), locale.clone()),
        StepDoc::Missing { .. } => {
            check.problems.push(format!(
                "no step body in any of `{}` — content not authored yet",
                requested
                    .chain(source)
                    .iter()
                    .map(Locale::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            return Ok(check);
        }
    };
    check.fell_back = locale != *requested;

    let body = read_step_body(&path)?;
    let present = sections(&body.headings);
    let missing: Vec<&str> = required_sections(origin)
        .iter()
        .copied()
        .filter(|section| !present.iter().any(|found| found == section))
        .collect();
    if !missing.is_empty() {
        check.problems.push(format!(
            "origin = {origin}: the body has no `## {}` section",
            missing.join("`, `## ")
        ));
    }
    if origin != Origin::Teach && present.iter().any(|section| section == "WHY") {
        check.problems.push(format!(
            "origin = {origin} must not re-explain with `## WHY` — deepen/transfer steps use \
             RECALL/DELTA"
        ));
    }

    if &locale == source {
        return Ok(check);
    }

    // A translation: front matter, then the two things a translator must not change.
    match &body.front_matter.locale {
        Some(declared) if declared == locale.as_str() => {}
        Some(declared) => check
            .problems
            .push(format!("front matter declares locale `{declared}`")),
        None => check
            .problems
            .push("front matter has no `locale`".to_owned()),
    }
    if body.front_matter.source_hash.is_none() {
        check
            .problems
            .push("front matter has no `source_hash`".to_owned());
    }
    if body.front_matter.translator.is_none() {
        check
            .problems
            .push("front matter has no `translator`".to_owned());
    }

    let source_path = course_dir.join(entry_for_locale(entry, source)?);
    let english = read_step_body(&source_path)?;
    if english.headings != body.headings {
        check.problems.push(format!(
            "headings differ from the {source} source ({} versus {})",
            body.headings.len(),
            english.headings.len()
        ));
    }
    if english.fences != body.fences {
        check
            .problems
            .push(format!("a code block differs from the {source} source"));
    }
    if let Some(declared) = &body.front_matter.source_hash {
        let actual = crate::hash::file(&source_path)?;
        if declared != &actual {
            check.drift.push(format!(
                "the translation was made from {declared}; the {source} source is now {actual}"
            ));
        }
    }
    Ok(check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn locale(text: &str) -> Locale {
        Locale::parse(text).unwrap()
    }

    #[test]
    fn the_fallback_chain_ends_at_the_source_locale() {
        let source = locale("en");
        let chain: Vec<String> = locale("zh-Hans")
            .chain(&source)
            .iter()
            .map(Locale::as_str)
            .map(str::to_owned)
            .collect();
        assert_eq!(chain, ["zh-Hans", "zh", "en"]);

        let chain: Vec<String> = locale("zh-Hant-HK")
            .chain(&source)
            .iter()
            .map(Locale::as_str)
            .map(str::to_owned)
            .collect();
        assert_eq!(chain, ["zh-Hant-HK", "zh-Hant", "zh", "en"]);

        assert_eq!(locale("en").chain(&source), [locale("en")]);
    }

    #[test]
    fn a_requested_locale_without_a_directory_chain_is_still_valid() {
        assert_eq!(
            locale("ja").chain(&locale("en")),
            [locale("ja"), locale("en")]
        );
    }

    #[test]
    fn malformed_locale_tags_are_rejected() {
        for bad in ["", "EN", "zh Hans", "zh-", "-zh", "1en"] {
            assert!(Locale::parse(bad).is_err(), "{bad} should be rejected");
        }
        assert!(Locale::parse("pt-BR").is_ok());
    }

    #[test]
    fn the_locale_segment_is_the_only_thing_rewritten() {
        let path =
            entry_for_locale("docs/steps/en/mailer/1.14-worker.md", &locale("zh-Hans")).unwrap();
        assert_eq!(
            path.to_string_lossy(),
            "docs/steps/zh-Hans/mailer/1.14-worker.md"
        );
        assert!(entry_for_locale("docs/other/en/x.md", &locale("ja")).is_err());
    }

    #[test]
    fn resolution_falls_back_gracefully_when_nothing_exists() {
        let temp = TempDir::new("i18n-fallback");
        let course_dir = temp.path();
        let entry = "docs/steps/en/mailer/1.1-workspace.md";

        // No `ja` files at all: the chain is walked, nothing is found, and that is not an error.
        let doc = resolve_step_doc(course_dir, entry, &locale("ja"), &locale("en")).unwrap();
        assert!(matches!(doc, StepDoc::Missing { .. }));
        assert!(
            doc.path()
                .ends_with("docs/steps/en/mailer/1.1-workspace.md")
        );

        // English present: `zh` falls back to it and the notice says so.
        temp.write(
            entry,
            "+++\nid = \"mailer/1.1-workspace\"\n+++\n\n# Title\n",
        );
        let doc = resolve_step_doc(course_dir, entry, &locale("zh"), &locale("en")).unwrap();
        assert_eq!(doc.locale(), Some(&locale("en")));
        assert!(
            fallback_notice("mailer/1.1-workspace", &locale("zh"), &doc, &locale("en")).is_some()
        );

        // The request itself is found: no notice.
        let doc = resolve_step_doc(course_dir, entry, &locale("en"), &locale("en")).unwrap();
        assert!(
            fallback_notice("mailer/1.1-workspace", &locale("en"), &doc, &locale("en")).is_none()
        );
    }

    #[test]
    fn a_translation_wins_over_the_source() {
        let temp = TempDir::new("i18n-translation");
        let entry = "docs/steps/en/mailer/1.1-workspace.md";
        temp.write(entry, "# English\n");
        temp.write("docs/steps/zh-Hans/mailer/1.1-workspace.md", "# 中文\n");

        let doc = resolve_step_doc(temp.path(), entry, &locale("zh-Hans"), &locale("en")).unwrap();
        assert_eq!(doc.locale(), Some(&locale("zh-Hans")));
        assert_eq!(
            doc.path().file_name().unwrap().to_string_lossy(),
            "1.1-workspace.md"
        );
        assert!(
            fallback_notice(
                "mailer/1.1-workspace",
                &locale("zh-Hans"),
                &doc,
                &locale("en")
            )
            .is_none()
        );
    }

    #[test]
    fn front_matter_headings_and_fences_are_parsed() {
        let temp = TempDir::new("i18n-body");
        let path = temp.write(
            "body.md",
            "+++\nid = \"mailer/1.1-workspace\"\nlocale = \"en\"\n+++\n\n# One\n\ntext\n\n```sh\nkiln run\n```\n\n## Two\n",
        );
        let body = read_step_body(&path).unwrap();
        assert_eq!(
            body.front_matter.id.as_deref(),
            Some("mailer/1.1-workspace")
        );
        assert_eq!(body.headings, ["# One", "## Two"]);
        assert_eq!(body.fences, ["kiln run"]);
    }

    #[test]
    fn an_unclosed_front_matter_block_is_reported() {
        let temp = TempDir::new("i18n-broken-front-matter");
        let path = temp.write("body.md", "+++\nid = \"x\"\n\n# One\n");
        let error = read_step_body(&path).unwrap_err();
        assert!(error.to_string().contains("not closed"), "{error}");
    }
}
