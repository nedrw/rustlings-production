//! `kiln hint [step]` — one nudge, and where to read the rest.
//!
//! A hint is one line in `track.toml` (authoring.md §2: step metadata lives in one place). It
//! points at the shape of the solution without writing it, and it always comes with the commands
//! that judge the step, because a hint you cannot act on is a riddle.

use crate::cmd::Session;
use crate::error::{EXIT_NOT_AUTHORED, EXIT_OK, Result};

/// Print the step's hint.
pub fn run(session: &Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    super::run::print_step_header(session, &step)?;

    let Some(hint) = &step.hint else {
        println!();
        println!("  this step has no hint yet — its content is not authored.");
        return Ok(EXIT_NOT_AUTHORED);
    };

    println!();
    println!("hint: {hint}");
    println!();
    println!("next:");
    println!("  kiln run  {}", step.qualified_id());
    println!("  kiln check {}", step.qualified_id());
    Ok(EXIT_OK)
}
