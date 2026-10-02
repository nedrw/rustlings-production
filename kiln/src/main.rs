//! `kiln` — the course runner's binary.
//!
//! A shell: argument parsing and everything else live in the library, so the same code paths can be
//! tested and reused (`cargo xtask ci` calls [`kiln::cmd::main`] with `selftest`).

use std::process::ExitCode;

fn main() -> ExitCode {
    kiln::cmd::main(std::env::args())
}
