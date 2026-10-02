//! `mailer` — a subscription service for a newsletter.
//!
//! Everything the service can do today is in this one file. The course's first step is
//! about where the code should live, not about what it does.

use std::process::ExitCode;

const USAGE: &str = "\
usage: mailer [OPTIONS]

OPTIONS:
    --help       print this help and exit
    --version    print the version and exit
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(flag) = args.first() else {
        eprintln!("mailer: no listener yet, nothing to do (try --help)");
        return ExitCode::SUCCESS;
    };
    match flag.as_str() {
        "--help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "--version" => {
            println!("mailer {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("mailer: unknown argument `{other}`\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
