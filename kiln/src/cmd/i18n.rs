//! `kiln i18n status` — coverage, structure and drift across locales.
//!
//! The three questions are answered separately because they have different consequences (plan §3
//! D5): coverage is information, a structure problem fails the command (CI treats it as an error),
//! and drift is only a warning — the English source is always the truth, and quietly "fixing" a
//! translation's hash would hide the work it needs.

use crate::cmd::Session;
use crate::error::{EXIT_FAILED, EXIT_OK, Result};
use crate::i18n::{Locale, status};

/// Print the i18n report for the requested locale.
pub fn status(session: &Session) -> Result<u8> {
    let report = status::report(session.course(), session.locale())?;

    println!(
        "source locale: {} — {} authored steps, {} step bodies",
        report.source,
        report.steps_total,
        report
            .for_locale(&report.source)
            .map_or(0, |locale| locale.covered())
    );
    println!(
        "requested: {} — fallback chain {}",
        report.requested,
        report
            .chain
            .iter()
            .map(Locale::as_str)
            .collect::<Vec<_>>()
            .join(" -> ")
    );

    for locale in &report.locales {
        println!();
        let source = if locale.is_source { " [source]" } else { "" };
        println!(
            "{} ({}/{}){source}",
            locale.locale,
            locale.covered(),
            locale.total()
        );
        // Only a catalogue that exists is reported. The course translates its *content*; the
        // runner's own sentences (errors, hints, gate names, the failure text of a spec) stay in
        // English by policy — they quote code, paths and identifiers, and a learner reading Rust
        // compiler output all day is not served by a translated `error[E0308]`. A line that says
        // "cli catalogue: none" reads as a gap to be filled, which is the opposite of the policy.
        if let Some(path) = &locale.catalog {
            println!("  cli catalogue: {}", path.display());
        }
        if !locale.missing.is_empty() && locale.is_source {
            println!(
                "  not authored yet: {} step body/bodies",
                locale.missing.len()
            );
        }
        for problem in &locale.structure {
            println!("  ✗ {problem}");
        }
        for warning in &locale.drift {
            println!("  ! {warning}");
        }
    }

    let structure = report.structure_errors();
    let drift = report.drift_warnings();
    println!();
    println!(
        "structure: {} error(s) — CI fails on these; drift: {drift} warning(s) — only the translator acts on these",
        structure
    );

    if structure > 0 {
        Ok(EXIT_FAILED)
    } else {
        Ok(EXIT_OK)
    }
}
