//! `kiln` — the course runner's binary.
//!
//! Deliberately thin: argument parsing and the exit code live here, everything else is in the
//! library so it can be unit tested and reused by `cargo xtask ci`.

use clap::Parser;
use std::process::ExitCode;

/// Command line interface of the course runner.
#[derive(Debug, Parser)]
#[command(
    name = "kiln",
    version,
    about = "Run and judge the steps of the rustlings-production course",
    arg_required_else_help = true
)]
struct Cli {}

fn main() -> ExitCode {
    let _ = Cli::parse();
    ExitCode::SUCCESS
}
