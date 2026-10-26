//! `kiln prepare [step]` — put a step's starting state in place, without judging anything.
//!
//! `kiln run` does this as the first half of judging; doing it on its own is the learner's "set me
//! up" (start a step without pretending to have worked on it) and the maintenance path for tooling
//! that needs the step's tree — `kiln solution` also prepares first, but it goes all the way to the
//! reference implementation, which is a recorded event. Preparing is not: it is the state every
//! learner starts from, and it leaves no trace in `.kiln/state.toml`.

use crate::cmd::Session;
use crate::error::{EXIT_OK, Result};
use crate::project;

/// Put the step's scaffold (or skeleton) and spec copies in place.
pub fn run(session: &mut Session, reference: Option<&str>) -> Result<u8> {
    let step = super::resolve_step(session, reference)?;
    super::run::print_step_header(session, &step)?;

    let track = session.course().track(&step.track)?;
    let prepared = project::prepare(session.course(), track, &step)?;
    println!();
    super::run::print_preparation(&prepared);
    if prepared.is_empty() {
        println!("  already in place — nothing to lay");
    }
    println!();
    println!(
        "{} is set up — `kiln run {}` judges it, `kiln hint {}` reads the brief again",
        step.qualified_id(),
        step.qualified_id(),
        step.qualified_id()
    );
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::super::{Cli, Command};

    /// `prepare` exists and reaches the command: a learner can set a step up without judging it,
    /// and the maintenance tooling needs exactly that (the patch rebuild reads the stub state).
    #[test]
    fn prepare_takes_a_step() {
        let cli = Cli::try_parse_from(["kiln", "prepare", "mailer/1.1-workspace"])
            .expect("the command line parses");
        match cli.command {
            Some(Command::Prepare { step }) => {
                assert_eq!(step.as_deref(), Some("mailer/1.1-workspace"));
            }
            other => panic!("expected `prepare`, got {other:?}"),
        }
    }
}
