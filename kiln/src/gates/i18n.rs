//! `i18n` — the body the learner is reading keeps the structure of its template (plan §3 D5).
//!
//! The gate judges the *requested* locale, not the source: on a course with no translations it
//! checks the English body, and it checks a translation the moment one appears. Structure problems
//! (a missing section, a changed code block, missing front matter) fail the step; drift — a
//! translation made from an older English source — is reported and only warns, because the English
//! text is the truth and the translation is work in progress (authoring.md §8.4).

use crate::error::Result;
use crate::gates::runner::GateContext;
use crate::gates::{GateOutcome, GateSpec};
use crate::i18n::{self, Locale};

/// Check this step's body in the learner's locale against its template and its source.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    let source = Locale::parse(&ctx.course.manifest().default_locale)?;
    let check = i18n::check_body(
        &ctx.course.dir(),
        &ctx.step.entry,
        ctx.step.origin,
        ctx.locale,
        &source,
    )?;

    if !check.problems.is_empty() {
        return Ok(GateOutcome::failed(format!(
            "the step body is not usable in `{}`:\n    {}",
            ctx.locale,
            check.problems.join("\n    ")
        )));
    }

    let where_it_is = match check.found.as_ref() {
        Some(locale) if check.fell_back => format!("`{locale}` (no `{}` body yet)", ctx.locale),
        Some(locale) => format!("`{locale}`"),
        None => "the source locale".to_owned(),
    };
    let structure = i18n::required_sections(ctx.step.origin).len();
    let mut detail = format!("{structure} section(s) present in {where_it_is}");
    if !check.drift.is_empty() {
        detail.push_str(&format!("; drift warning: {}", check.drift.join("; ")));
    }
    Ok(GateOutcome::passed(detail))
}
