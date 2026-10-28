//! CLI text: `course/locales/<locale>/ui.toml`.
//!
//! English lives in the code that prints it — one source of truth, and no second catalogue to
//! drift out of date. A translation adds `course/locales/<locale>/ui.toml` with the keys it
//! wants to override; anything missing keeps the English text, so a partial catalogue is
//! never an error and adding a language still means "add files, change no code".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::Locale;
use crate::error::{KilnError, Result};
use crate::info::Manifest;

/// Directory below `course/` that holds the CLI catalogues.
pub const LOCALES_DIR: &str = "locales";
/// File name of a CLI catalogue.
pub const CATALOG_FILE: &str = "ui.toml";

/// Shape of `ui.toml` on disk.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    string: BTreeMap<String, String>,
}

/// Where the CLI catalogue of a locale would live.
#[must_use]
pub fn catalog_path(course_dir: &Path, locale: &Locale) -> PathBuf {
    course_dir
        .join(LOCALES_DIR)
        .join(locale.as_str())
        .join(CATALOG_FILE)
}

/// The text a command prints, and which locale it came from.
#[derive(Debug, Clone)]
pub struct Ui {
    locale: Locale,
    fell_back: bool,
    strings: BTreeMap<String, String>,
    path: Option<PathBuf>,
}

impl Ui {
    /// Load the catalogue for `requested`, walking the fallback chain.
    ///
    /// A missing catalogue is not a problem: the CLI then prints its built-in English.
    pub fn load(course_dir: &Path, requested: &Locale, source: &Locale) -> Result<Self> {
        for locale in requested.chain(source) {
            let path = catalog_path(course_dir, &locale);
            if !path.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|source| KilnError::Read {
                path: path.clone(),
                source,
            })?;
            let file: CatalogFile =
                toml::from_str(&text).map_err(|source| KilnError::Metadata {
                    path: path.clone(),
                    source,
                })?;
            if let Some(declared) = &file.locale
                && declared != locale.as_str()
            {
                return Err(KilnError::Invalid(format!(
                    "{}: declares locale `{declared}` but lives in `{locale}`",
                    path.display()
                )));
            }
            return Ok(Self {
                fell_back: locale.falls_back_to(requested),
                locale,
                strings: file.string,
                path: Some(path),
            });
        }
        Ok(Self {
            locale: source.clone(),
            fell_back: requested != source,
            strings: BTreeMap::new(),
            path: None,
        })
    }

    /// Locale the text came from.
    #[must_use]
    pub fn locale(&self) -> &Locale {
        &self.locale
    }

    /// `true` when the requested locale had no catalogue at all.
    #[must_use]
    pub fn fell_back(&self) -> bool {
        self.fell_back
    }

    /// Path of the catalogue in use, when there is one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Number of overrides the catalogue provides.
    #[must_use]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// `true` when the catalogue provides no overrides.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// `true` when the catalogue overrides `key`.
    ///
    /// The difference between "this locale has no text for the key" and "this locale's text is
    /// the English one" only matters where a reader is told which of the two they are reading.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.strings.contains_key(key)
    }

    /// Override for `key`, or the English text the caller passes as the fallback.
    #[must_use]
    pub fn text<'a>(&'a self, key: &str, english: &'a str) -> &'a str {
        self.strings.get(key).map_or(english, String::as_str)
    }

    /// Welcome message: the manifest holds the English source, a catalogue may override it.
    #[must_use]
    pub fn welcome<'a>(&'a self, manifest: &'a Manifest) -> &'a str {
        self.text("welcome", &manifest.welcome)
    }

    /// Final message of a track, same override rule as [`Ui::welcome`].
    #[must_use]
    pub fn final_message<'a>(&'a self, manifest: &'a Manifest) -> &'a str {
        self.text("final_message", &manifest.final_message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn locale(text: &str) -> Locale {
        Locale::parse(text).unwrap()
    }

    #[test]
    fn without_a_catalogue_every_key_keeps_its_english_text() {
        let temp = TempDir::new("ui-fallback");
        let ui = Ui::load(temp.path(), &locale("ja"), &locale("en")).unwrap();
        assert!(ui.fell_back());
        assert!(ui.is_empty());
        assert_eq!(ui.locale(), &locale("en"));
        assert_eq!(ui.text("welcome", "Welcome"), "Welcome");
        assert!(ui.path().is_none());
    }

    #[test]
    fn a_catalogue_overrides_only_the_keys_it_lists() {
        let temp = TempDir::new("ui-override");
        temp.write(
            "locales/zh/ui.toml",
            "locale = \"zh\"\n\n[string]\nwelcome = \"欢迎\"\n",
        );
        let ui = Ui::load(temp.path(), &locale("zh"), &locale("en")).unwrap();
        assert!(!ui.fell_back());
        assert_eq!(ui.len(), 1);
        assert_eq!(ui.text("welcome", "Welcome"), "欢迎");
        assert_eq!(ui.text("final_message", "Done"), "Done");
    }

    #[test]
    fn a_missing_language_falls_through_its_parent_language() {
        let temp = TempDir::new("ui-parent");
        temp.write("locales/zh/ui.toml", "[string]\nwelcome = \"欢迎\"\n");
        let ui = Ui::load(temp.path(), &locale("zh-Hans"), &locale("en")).unwrap();
        assert_eq!(ui.locale(), &locale("zh"));
        assert!(ui.fell_back(), "the requested zh-Hans is a fallback");
        assert_eq!(ui.text("welcome", "Welcome"), "欢迎");
    }

    #[test]
    fn a_catalogue_that_declares_the_wrong_locale_is_rejected() {
        let temp = TempDir::new("ui-wrong-locale");
        temp.write("locales/zh/ui.toml", "locale = \"ja\"\n");
        let error = Ui::load(temp.path(), &locale("zh"), &locale("en")).unwrap_err();
        assert!(
            error.to_string().contains("declares locale `ja`"),
            "{error}"
        );
    }

    #[test]
    fn the_manifest_messages_can_be_overridden() {
        let temp = TempDir::new("ui-manifest");
        let manifest = Manifest {
            format_version: 1,
            default_locale: "en".into(),
            default_track: "mailer".into(),
            welcome: "Welcome".into(),
            final_message: "Done".into(),
        };
        let ui = Ui::load(temp.path(), &locale("en"), &locale("en")).unwrap();
        assert_eq!(ui.welcome(&manifest), "Welcome");
        assert_eq!(ui.final_message(&manifest), "Done");
    }
}
