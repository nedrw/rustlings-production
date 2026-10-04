//! The `relay` binary: a shell around the library.
//!
//! It parses the command line, reads the configuration, installs the log, connects the database and
//! then serves — in the order the service depends on. Everything a test can reach lives in the
//! library; this file only decides when each piece happens and which exit code ends the process.

use std::process::ExitCode;

use relay::config::{Config, Flags};
use relay::http;
use relay::logging::{self, RunId};
use relay::{USAGE, error::Error};

fn main() -> ExitCode {
    let flags = match Flags::from_args(std::env::args()) {
        Ok(flags) => flags,
        Err(error) => {
            eprintln!("relay: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if flags.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if flags.version {
        println!("relay {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let env: Vec<(String, String)> = std::env::vars().collect();
    let config = match Config::load(&flags, &env) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("relay: {error}");
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
    logging::init(config.log.format, &config.log.level, &run);
    match serve(config, run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("relay: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

/// Build the runtime, the state and the listener, then serve until a signal arrives.
fn serve(config: Config, run: RunId) -> Result<(), Error> {
    let address = format!("{}:{}", config.listen.host, config.listen.port);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::Config(format!("tokio runtime: {error}")))?;

    runtime.block_on(async move {
        if config.database.url.is_none() {
            return Err(Error::Config(
                "no database configured: set RELAY__DATABASE__URL or database.url in the config file"
                    .to_owned(),
            ));
        };
        let listener = tokio::net::TcpListener::bind(&address)
            .await
            .map_err(|error| Error::Config(format!("cannot bind {address}: {error}")))?;
        let state = http::AppState::start(config, &run).await?;
        http::serve_with_shutdown(listener, state, &run, shutdown_signal()).await
    })
}

/// Resolve when the process is asked to stop: `SIGTERM` (an orchestrator) or `Ctrl-C` (a person).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
