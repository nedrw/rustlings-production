//! The binary: it asks the library what to do and exits with the code it decides.

use std::process::ExitCode;

const USAGE: &str = "\
usage: rpc [OPTIONS]

OPTIONS:
    --help       print this help and exit
    --version    print the version and exit
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(flag) = args.first() else {
        eprintln!("rpc: nothing to do yet (try --help)");
        return ExitCode::SUCCESS;
    };
    match flag.as_str() {
        "--help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "--version" => {
            println!("rpc {}", rpc::version());
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("rpc: unknown argument `{other}`\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
