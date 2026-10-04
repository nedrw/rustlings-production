//! The `jobqd` binary: a shell around the library.
//!
//! It parses the command line, reads the configuration, installs the log, and then drains queues
//! until a signal arrives — in the order the service depends on. Everything a test can reach lives
//! in the library; this file only decides when each piece happens and which exit code ends the
//! process.

use std::process::ExitCode;

use jobq::service::config::{Config, Flags};
use jobq::service::logging::RunId;
use jobq::service::{USAGE, logging};

fn main() -> ExitCode {
    let flags = match Flags::from_args(std::env::args().skip(1)) {
        Ok(flags) => flags,
        Err(error) => {
            eprintln!("jobqd: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if flags.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if flags.version {
        println!("jobqd {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let env: Vec<(String, String)> = std::env::vars().collect();
    let config = match Config::load(&flags, &env) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("jobqd: {error}");
            return ExitCode::from(error.exit_code());
        }
    };
    if flags.check_config {
        for line in config.report() {
            println!("{line}");
        }
        return ExitCode::SUCCESS;
    }

    let run = RunId::of_this_process();
    if let Err(error) = logging::init(config.log.format, &config.log.level, &run) {
        eprintln!("jobqd: {error}");
        return ExitCode::from(error.exit_code());
    }
    match serve(config, run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jobqd: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

/// Build the runtime, connect the store, migrate it and drain until a signal arrives.
///
/// The order is the point: nothing is claimed before the schema it needs is in place, and the
/// process says it is running only once it can actually run something.
fn serve(config: Config, run: RunId) -> Result<(), jobq::engine::Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| jobq::engine::Error::Config {
            detail: format!("tokio runtime: {error}"),
        })?;

    runtime.block_on(async move { jobq::service::serve(config, run).await })
}
