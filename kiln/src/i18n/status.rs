//! Coverage, structure and drift across locales (`kiln i18n status`).
//!
//! The three questions are deliberately kept apart, because they have different consequences
//! (plan §3 D5): coverage says what is translated, a structure problem breaks CI, and drift —
//! a translation whose `source_hash` no longer matches the English source — only warns, because
//! the English source is always the truth.

use std::path::PathBuf;

use super::{Locale, entry_for_locale, hash_file, read_step_body, ui};
use crate::error::{KilnError, Result};
use crate::info::{Course, Step};

/// Health of one locale.
#[derive(Debug, Clone)]
pub struct LocaleReport {
    /// The locale being reported on.
    pub locale: Locale,
    /// `true` for the locale that holds the source text.
    pub is_source: bool,
    /// Step ids whose body exists in this locale.
    pub present: Vec<String>,
    /// Step ids whose body is absent.
    pub missing: Vec<String>,
    /// Problems that must break CI (missing front-matter fields, changed headings or code).
    pub structure: Vec<String>,
    /// Translations whose `source_hash` no longer matches the English source (warnings).
    pub drift: Vec<String>,
    /// `course/locales/<locale>/ui.toml`, when a CLI catalogue exists.
    pub catalog: Option<PathBuf>,
}

impl LocaleReport {
    /// Number of step bodies present.
    #[must_use]
    pub fn covered(&self) -> usize {
        self.present.len()
    }

    /// Total number of steps the locale is measured against.
    #[must_use]
    pub fn total(&self) -> usize {
        self.present.len() + self.missing.len()
    }
}

/// The whole i18n picture for one requested locale.
#[derive(Debug, Clone)]
pub struct Report {
    /// Locale holding the source text.
    pub source: Locale,
    /// Locale the learner asked for.
    pub requested: Locale,
    /// Fallback chain that was walked, most specific first.
    pub chain: Vec<Locale>,
    /// Number of authored steps.
    pub steps_total: usize,
    /// One report per locale, source included.
    pub locales: Vec<LocaleReport>,
}

impl Report {
    /// Structure problems across every locale.
    #[must_use]
    pub fn structure_errors(&self) -> usize {
        self.locales
            .iter()
            .map(|locale| locale.structure.len())
            .sum()
    }

    /// Drift warnings across every locale.
    #[must_use]
    pub fn drift_warnings(&self) -> usize {
        self.locales.iter().map(|locale| locale.drift.len()).sum()
    }

    /// Report of one locale, if it was measured.
    #[must_use]
    pub fn for_locale(&self, locale: &Locale) -> Option<&LocaleReport> {
        self.locales.iter().find(|report| &report.locale == locale)
    }
}

/// Locales that have a directory under `course/docs/steps/`.
///
/// A repository with no translations at all yields an empty list — that is the current state of
/// the course, not an error.
pub fn available_locales(course: &Course) -> Result<Vec<Locale>> {
    let steps_dir = course.dir().join("docs").join("steps");
    let Ok(entries) = std::fs::read_dir(&steps_dir) else {
        return Ok(Vec::new());
    };

    let mut locales = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| KilnError::Read {
            path: steps_dir.clone(),
            source,
        })?;
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        locales.push(Locale::parse(&name).map_err(|_| {
            KilnError::Invalid(format!(
                "{} is not a locale directory name",
                entry.path().display()
            ))
        })?);
    }
    locales.sort();
    Ok(locales)
}

/// Measure coverage, structure and drift for a requested locale.
pub fn report(course: &Course, requested: &Locale) -> Result<Report> {
    let source = Locale::parse(&course.manifest().default_locale)?;
    let course_dir = course.dir();
    let steps: Vec<&Step> = course.steps().collect();

    let mut locales = available_locales(course)?;
    for locale in [source.clone(), requested.clone()] {
        if !locales.contains(&locale) {
            locales.push(locale);
        }
    }
    locales.sort();

    let mut reports = Vec::with_capacity(locales.len());
    for locale in locales {
        let is_source = locale == source;
        let mut report = LocaleReport {
            is_source,
            catalog: ui::catalog_path(&course_dir, &locale)
                .is_file()
                .then(|| ui::catalog_path(&course_dir, &locale)),
            locale: locale.clone(),
            present: Vec::new(),
            missing: Vec::new(),
            structure: Vec::new(),
            drift: Vec::new(),
        };

        for step in &steps {
            let id = step.qualified_id();
            let source_path = course_dir.join(entry_for_locale(&step.entry, &source)?);
            let path = course_dir.join(entry_for_locale(&step.entry, &locale)?);
            if !path.is_file() {
                report.missing.push(id);
                continue;
            }
            report.present.push(id.clone());
            let body = match read_step_body(&path) {
                Ok(body) => body,
                Err(error) => {
                    report.structure.push(format!("{id}: {error}"));
                    continue;
                }
            };

            if let Some(declared) = &body.front_matter.id
                && declared != &id
            {
                report
                    .structure
                    .push(format!("{id}: front matter declares id `{declared}`"));
            }
            if is_source {
                continue;
            }

            match &body.front_matter.locale {
                Some(declared) if declared == locale.as_str() => {}
                Some(declared) => report.structure.push(format!(
                    "{id}: front matter declares locale `{declared}`, expected `{locale}`"
                )),
                None => report
                    .structure
                    .push(format!("{id}: front matter has no `locale`")),
            }
            if body.front_matter.source_hash.is_none() {
                report
                    .structure
                    .push(format!("{id}: front matter has no `source_hash`"));
            }
            if body.front_matter.translator.is_none() {
                report
                    .structure
                    .push(format!("{id}: front matter has no `translator`"));
            }

            match read_step_body(&source_path) {
                Ok(english) => {
                    if english.headings != body.headings {
                        report.structure.push(format!(
                            "{id}: headings differ from the {source} source ({} versus {})",
                            body.headings.len(),
                            english.headings.len()
                        ));
                    }
                    if english.fences != body.fences {
                        report.structure.push(format!(
                            "{id}: a code block differs from the {source} source"
                        ));
                    }
                }
                Err(_) => report
                    .structure
                    .push(format!("{id}: no {source} source to compare against")),
            }

            if let Some(declared) = &body.front_matter.source_hash {
                let actual = hash_file(&source_path)?;
                if declared != &actual {
                    report.drift.push(format!(
                        "{id}: translation was made from {declared}, the {source} source is now {actual}"
                    ));
                }
            }
        }

        reports.push(report);
    }

    Ok(Report {
        chain: requested.chain(&source),
        steps_total: steps.len(),
        source,
        requested: requested.clone(),
        locales: reports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, ledger_toml, load_course, one_track, step_toml};
    use std::path::Path;

    /// A course with exactly one step, `mailer/1.1-workspace`.
    fn one_step_course(root: &Path) -> Course {
        load_course(
            root,
            &one_track(
                &[step_toml(
                    "mailer",
                    "1.1-workspace",
                    &["repo/workspace-layout@apply"],
                )],
                &["repo/workspace-layout@apply"],
            ),
            &ledger_toml(&[("repo/workspace-layout", "apply", "mailer/1.1")]),
        )
    }

    #[test]
    fn locales_without_a_steps_directory_produce_no_locales() {
        let temp = TempDir::new("i18n-no-locales");
        let course = one_step_course(temp.path());
        assert_eq!(available_locales(&course).unwrap(), Vec::<Locale>::new());
    }

    #[test]
    fn a_locale_directory_is_discovered() {
        let temp = TempDir::new("i18n-locales");
        temp.write(
            "course/docs/steps/zh-Hans/mailer/1.1-workspace.md",
            "# \u{4e2d}\u{6587}\n",
        );
        let course = one_step_course(temp.path());
        assert_eq!(
            available_locales(&course).unwrap(),
            [Locale::parse("zh-Hans").unwrap()]
        );
    }

    #[test]
    fn a_directory_that_is_not_a_locale_tag_is_reported() {
        let temp = TempDir::new("i18n-bad-locale");
        temp.write(
            "course/docs/steps/NOT_A_LOCALE/mailer/1.1-workspace.md",
            "# x\n",
        );
        let course = one_step_course(temp.path());
        let error = available_locales(&course).unwrap_err();
        assert!(
            error.to_string().contains("not a locale directory name"),
            "{error}"
        );
    }

    #[test]
    fn missing_bodies_are_counted_not_blamed() {
        let temp = TempDir::new("i18n-coverage");
        let course = one_step_course(temp.path());
        let report = report(&course, &Locale::parse("en").unwrap()).unwrap();
        assert_eq!(report.steps_total, course.steps().count());
        let source = report.for_locale(&Locale::parse("en").unwrap()).unwrap();
        assert_eq!(source.covered(), 0, "no step body exists yet");
        assert_eq!(source.total(), report.steps_total);
        assert!(
            source.structure.is_empty(),
            "nothing to check without bodies"
        );
        assert!(report.chain.starts_with(&[Locale::parse("en").unwrap()]));
    }

    #[test]
    fn a_stale_translation_is_reported_as_drift() {
        let temp = TempDir::new("i18n-drift");
        temp.write(
            "course/docs/steps/en/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\n+++\n\n# Title\n",
        );
        temp.write(
            "course/docs/steps/zh/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\nlocale = \"zh\"\nsource_hash = \"sha256:0000\"\ntranslator = \"addict\"\n+++\n\n# \u{6807}\u{9898}\n",
        );
        let course = one_step_course(temp.path());
        let report = report(&course, &Locale::parse("zh").unwrap()).unwrap();
        let zh = report.for_locale(&Locale::parse("zh").unwrap()).unwrap();
        assert_eq!(zh.covered(), 1);
        assert_eq!(zh.drift.len(), 1, "{zh:?}");
        // Headings are compared as text, so a translated heading is a structure error.
        assert_eq!(zh.structure.len(), 1, "{zh:?}");
        assert_eq!(report.structure_errors(), 1);
        assert_eq!(report.drift_warnings(), 1);
    }

    #[test]
    fn a_translation_missing_its_front_matter_fields_is_a_structure_error() {
        let temp = TempDir::new("i18n-structure");
        temp.write(
            "course/docs/steps/en/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\n+++\n\n# Title\n",
        );
        temp.write(
            "course/docs/steps/zh/mailer/1.1-workspace.md",
            "# \u{6807}\u{9898}\n",
        );
        let course = one_step_course(temp.path());
        let report = report(&course, &Locale::parse("zh").unwrap()).unwrap();
        let zh = report.for_locale(&Locale::parse("zh").unwrap()).unwrap();
        assert!(
            zh.structure.iter().any(|p| p.contains("`locale`")),
            "{zh:?}"
        );
        assert!(
            zh.structure.iter().any(|p| p.contains("`source_hash`")),
            "{zh:?}"
        );
        assert!(
            zh.structure.iter().any(|p| p.contains("`translator`")),
            "{zh:?}"
        );
    }
}
