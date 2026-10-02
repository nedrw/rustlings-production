//! `kiln` — the course runner's binary.
//!
//! Deliberately thin: argument parsing lives in [`kiln::cmd::Cli`], everything else in the
//! library, so the same code paths can be unit tested and later reused by `cargo xtask ci`.

use clap::Parser;
use kiln::cmd::{Cli, run};
use std::process::ExitCode;

fn main() -> ExitCode {
    run(&Cli::parse())
}
