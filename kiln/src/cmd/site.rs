//! `kiln site` — the course as a static site (plan §11, M5).
//!
//! The command is a thin shell around [`crate::site::build`], the way the other commands are thin
//! shells around their module: the same build runs here and in `cargo xtask ci` (`--check`), so a
//! page that renders for a maintainer renders in CI and the two cannot disagree.

use std::path::PathBuf;

use crate::cmd::Session;
use crate::error::{EXIT_OK, KilnError, Result};
use crate::i18n::Locale;
use crate::site::{self, Options};

/// Render every step body into a static site.
pub fn run(session: &Session, out: Option<&str>, langs: &[String], check: bool) -> Result<u8> {
    let mut locales = Vec::with_capacity(langs.len());
    for lang in langs {
        locales.push(Locale::parse(lang)?);
    }
    let options = if check {
        Options::check()
    } else {
        let out = out.map_or_else(|| PathBuf::from(site::DEFAULT_OUT), PathBuf::from);
        let mut options = Options::new(out);
        options.locales = locales;
        options
    };
    if check && !langs.is_empty() {
        return Err(KilnError::Invalid(
            "`--check` renders every locale — pass `--locale` only when writing a site".to_owned(),
        ));
    }
    let report = site::build(session.course(), &options)?;
    let names: Vec<&str> = report.locales.iter().map(Locale::as_str).collect();
    if check {
        println!(
            "kiln site · {} page(s) in {} locale(s) render — nothing written",
            report.pages,
            names.join(", ")
        );
    } else {
        println!(
            "kiln site · {} page(s) in {} locale(s) into {}",
            report.pages,
            names.join(", "),
            report.out.display()
        );
        println!(
            "  serve it with `python3 -m http.server -d {}`",
            report.out.display()
        );
    }
    Ok(EXIT_OK)
}
