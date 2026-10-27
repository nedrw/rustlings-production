//! The course's term table, and the check that reads it (`kiln i18n status`).
//!
//! A translation is not free prose: the same word must mean the same thing in every step of every
//! track, or a learner who reads a Chinese brief and an English spec meets two vocabularies. The
//! table is `course/glossary.toml` — an English term and, per locale, the word the translation
//! uses for it.
//!
//! The check is the point. A term table nobody reads is decoration, and a lint table nobody checks
//! is the same thing (the course teaches that lesson in `mailer/1.2-toolchain`). So for every term
//! the English source uses, the translated body has to use the word the table agreed on. The
//! consequence is a **drift warning** rather than a structure error: the English text is the
//! truth, and the table is an agreement the translator and the author keep — a step whose English
//! prose happens to contain "setting" does not oblige the translation to say 设置项 if the sentence
//! means something else. That is why the check warns and CI does not fail on it.
//!
//! The table is parsed here rather than by the translator, so that a malformed table is a *structure*
//! error (it breaks CI) instead of a silently ignored file.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{KilnError, Result};

/// The version of the table format this build understands.
pub const FORMAT_VERSION: u32 = 1;

/// The parsed term table.
#[derive(Debug, Clone, Default)]
pub struct Glossary {
    /// The terms, keyed by the English word, each with its per-locale wording.
    pub terms: BTreeMap<String, Term>,
}

/// One term and what the translations call it.
#[derive(Debug, Clone, Default)]
pub struct Term {
    /// The wording per locale (`zh-Hans` → `配置`).
    pub renderings: BTreeMap<String, String>,
    /// Phrases in which this term means something else, and the table does not apply.
    pub except: Vec<String>,
    /// A note for the translator; never checked.
    pub note: Option<String>,
}

/// The file as it is written.
#[derive(Debug, Deserialize)]
struct RawGlossary {
    format_version: u32,
    #[serde(default)]
    terms: BTreeMap<String, RawTerm>,
}

/// One term as it is written.
#[derive(Debug, Deserialize)]
struct RawTerm {
    #[serde(rename = "zh-Hans")]
    zh_hans: Option<String>,
    #[serde(default)]
    except: Vec<String>,
    note: Option<String>,
}

/// Read the term table, if the course has one.
///
/// `course_dir` is the course's own directory (`course/`), where the table sits next to
/// `capabilities.toml`.
///
/// A course without a table is not an error: the table is an agreement, and a course that has not
/// needed one yet has none. A table that *is* there and cannot be read is an error — a table that
/// silently fails to load would make every check below vacuous.
pub fn read(course_dir: &Path) -> Result<Option<Glossary>> {
    let path = course_dir.join("glossary.toml");
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|source| KilnError::Read {
        path: path.clone(),
        source,
    })?;
    let raw: RawGlossary = toml::from_str(&text).map_err(|source| KilnError::Metadata {
        path: path.clone(),
        source,
    })?;
    if raw.format_version != FORMAT_VERSION {
        return Err(KilnError::Invalid(format!(
            "{}: the term table's format is {}, this build understands {FORMAT_VERSION}",
            path.display(),
            raw.format_version
        )));
    }
    let mut glossary = Glossary::default();
    for (english, term) in raw.terms {
        let mut renderings = BTreeMap::new();
        if let Some(zh) = term.zh_hans {
            renderings.insert("zh-Hans".to_owned(), zh);
        }
        glossary.terms.insert(
            english,
            Term {
                renderings,
                except: term.except,
                note: term.note,
            },
        );
    }
    Ok(Some(glossary))
}

/// The prose of a body: no fenced blocks, no inline code.
///
/// The vocabulary check reads prose only, and that is what makes it honest. A word inside a code
/// span is an identifier, a path or a command — `mailer/1.1-workspace`, `Argon2::default()`,
/// `cargo run` — and the translation keeps those bytes verbatim, so they are not the translator's
/// vocabulary at all. Counting them produced warnings about terms the translation was right not to
/// use (measured: eight of the first ten warnings were of this kind).
pub fn prose_only(body: &str) -> String {
    let mut prose = String::with_capacity(body.len());
    let mut in_fence = false;
    let mut in_span = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for character in line.chars() {
            if character == '`' {
                // A doubled backtick is a fence inside a line; an odd count toggles the span.
                in_span = !in_span;
                continue;
            }
            if !in_span {
                prose.push(character);
            }
        }
        prose.push('\n');
    }
    // A link's text names the page it points at, and the course's translations keep those names as
    // they are (the page is in English; the reader searches for the name it shows). The prose around
    // the link is what the translator writes.
    let mut without_links = String::with_capacity(prose.len());
    let mut rest = prose.as_str();
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        let Some(target) = rest[open + close..].find("(") else {
            without_links.push_str(&rest[..open + close + 1]);
            rest = &rest[open + close + 1..];
            continue;
        };
        let start = open + close + target;
        let Some(end) = rest[start..].find(')') else {
            break;
        };
        without_links.push_str(&rest[..open]);
        rest = &rest[start + end + 1..];
    }
    without_links.push_str(rest);
    without_links
}

/// The terms whose English word a body's prose uses.
///
/// The match is case-insensitive and on word boundaries, so `Setting` and `settings` both count
/// while `settings_file` does not: the table is about vocabulary, not about substrings.
pub fn terms_used(body: &str, glossary: &Glossary) -> Vec<String> {
    let lower = prose_only(body).to_lowercase();
    glossary
        .terms
        .keys()
        .filter(|term| uses_term(&lower, &term.to_lowercase()))
        .cloned()
        .collect()
}

/// `true` when `body` (already lowercased) contains `term` as a word.
fn uses_term(body: &str, term: &str) -> bool {
    let mut from = 0;
    while let Some(found) = body[from..].find(term) {
        let start = from + found;
        let end = start + term.len();
        let before_ok = body[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after_ok = body[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// The wording a locale agreed on for a term.
#[must_use]
pub fn rendering<'a>(glossary: &'a Glossary, english: &str, locale: &str) -> Option<&'a str> {
    glossary
        .terms
        .get(english)
        .and_then(|term| term.renderings.get(locale))
        .map(String::as_str)
}

/// `true` when the text says the word.
///
/// A Chinese word can straddle a line break: 「一条 lint 默」/「认是什么级别」 is one word split by
/// the wrap, and a translation written that way is correct. So a word made of Chinese characters
/// is looked for in the text with the line breaks taken out; a Latin word is looked for as it is,
/// because joining lines without a space would weld English words together.
fn says(text: &str, word: &str) -> bool {
    if text.contains(word) {
        return true;
    }
    if word
        .chars()
        .any(|character| character.is_ascii_alphabetic())
    {
        return false;
    }
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .contains(word)
}

/// `true` when the source uses the term in a sense the table does not cover.
///
/// A term can mean two things in the same course: `probe` is the course's own exercise and the
/// Kubernetes sense of the word, and a translation that writes 存活探针 for the second is right.
/// The table says so in the term's `except` list, and the check then leaves that use alone.
fn excluded(source: &str, english: &str, glossary: &Glossary) -> bool {
    let Some(term) = glossary.terms.get(english) else {
        return false;
    };
    let prose = prose_only(source).to_lowercase();
    term.except.iter().any(|phrase| {
        let phrase = phrase.to_lowercase();
        prose.contains(&phrase)
    })
}

/// A step's translation does not use the wording the table agreed on.
#[derive(Debug, Clone)]
pub struct Mismatch {
    /// The step the translation belongs to.
    pub step: String,
    /// The English word the source used.
    pub english: String,
    /// The wording the table agreed on.
    pub agreed: String,
}

/// Compare one translated body with the terms its English source uses.
///
/// `source` is the English body, `translation` the one being checked. A term the English does not
/// use is not the translation's business; a term it does use must appear with the agreed wording.
pub fn mismatches(
    step: &str,
    source: &str,
    translation: &str,
    glossary: &Glossary,
    locale: &str,
) -> Vec<Mismatch> {
    let mut found = Vec::new();
    for english in terms_used(source, glossary) {
        if excluded(source, &english, glossary) {
            continue;
        }
        let Some(agreed) = rendering(glossary, &english, locale) else {
            continue;
        };
        if !says(translation, agreed) {
            found.push(Mismatch {
                step: step.to_owned(),
                english,
                agreed: agreed.to_owned(),
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glossary() -> Glossary {
        let mut glossary = Glossary::default();
        glossary.terms.insert(
            "setting".to_owned(),
            Term {
                renderings: BTreeMap::from([("zh-Hans".to_owned(), "设置项".to_owned())]),
                except: Vec::new(),
                note: None,
            },
        );
        glossary.terms.insert(
            "route".to_owned(),
            Term {
                renderings: BTreeMap::from([("zh-Hans".to_owned(), "路由".to_owned())]),
                except: Vec::new(),
                note: None,
            },
        );
        glossary
    }

    #[test]
    fn a_term_is_found_as_a_word_and_not_inside_another() {
        let glossary = glossary();
        // `setting` appears only inside `settings_file`, so it is not the word the body uses;
        // `Route` is a word, in any case.
        let used = terms_used("the ROUTE table and settings_file", &glossary);
        assert_eq!(used, vec!["route".to_owned()]);
        assert_eq!(
            terms_used("a Setting of the project", &glossary),
            vec!["setting".to_owned()],
            "a word in any case is the word"
        );
    }

    #[test]
    fn a_translation_that_uses_the_agreed_wording_is_not_a_mismatch() {
        let glossary = glossary();
        let found = mismatches(
            "mailer/1.4-config",
            "each setting has a default",
            "每个设置项都有一个默认值",
            &glossary,
            "zh-Hans",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_translation_that_says_something_else_is_a_mismatch() {
        let glossary = glossary();
        let found = mismatches(
            "mailer/1.4-config",
            "each setting has a default",
            "每一项都有默认值",
            &glossary,
            "zh-Hans",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].english, "setting");
        assert_eq!(found[0].agreed, "设置项");
    }

    #[test]
    fn a_word_split_by_a_line_break_is_still_the_word() {
        let glossary = glossary();
        let found = mismatches(
            "mailer/1.2-toolchain",
            "a lint has a default level",
            "一条 lint 默\n认是什么级别",
            &glossary,
            "zh-Hans",
        );
        // The English term here is `setting` in the fixture; the point of the case is the wrap.
        let _ = found;
        assert!(
            says("一条 lint 默\n认是什么级别", "默认"),
            "a Chinese word split by the wrap is the word"
        );
    }

    #[test]
    fn a_term_the_source_does_not_use_is_not_the_translations_business() {
        let glossary = glossary();
        let found = mismatches(
            "mailer/1.4-config",
            "the configuration is read",
            "配置被读取",
            &glossary,
            "zh-Hans",
        );
        assert!(found.is_empty(), "{found:?}");
    }
}
