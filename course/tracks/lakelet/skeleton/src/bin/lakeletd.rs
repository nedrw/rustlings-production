//! `lakeletd` — the telemetry store's daemon.
//!
//! What it does at startup, in order, and each step is a contract a spec leans on:
//!
//! 1. read the settings through the four layers (`--check-config` prints them and exits);
//! 2. install the logger;
//! 3. open the store — a store that cannot be opened is a *refusal*, before anything listens;
//! 4. bind the ingest port and log `listening on <addr>`, which is how a test learns the port the
//!    kernel handed out (port `0` means only the child knows);
//! 5. serve until `SIGTERM` or `SIGINT`, then stop accepting, flush, log `draining` and exit `0`.

use std::process::ExitCode;
use std::sync::Arc;

use lakelet::engine::Spool;
use lakelet::service::config::{self, Flags};
use lakelet::service::{ingest, logging};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Everything this program can tell a user about itself without starting.
const USAGE: &str = "\
usage: lakeletd [--config <path>] [--listen-addr <addr>] [--store-dir <path>] [--log-level <filter>] \
[--log-format text|json] [--check-config] [--help] [--version]";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lakeletd: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), lakelet::Error> {
    let args = std::env::args().skip(1);
    let flags = Flags::from_args(args)?;
    if flags.help {
        println!("{USAGE}");
        return Ok(());
    }
    if flags.version {
        println!("lakeletd {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let config = config::resolve(&flags, |name| std::env::var(name).ok())?;
    if flags.check_config {
        println!("{}", config.report());
        return Ok(());
    }
    logging::init(&config)?;

    let store = Arc::new(Mutex::new(Spool::open(&config.store_dir)?));
    let listener = TcpListener::bind(&config.listen_addr)
        .await
        .map_err(|error| lakelet::Error::Store {
            source: Box::new(error),
        })?;
    let address = listener
        .local_addr()
        .map_err(|error| lakelet::Error::Store {
            source: Box::new(error),
        })?;
    tracing::info!(store = %config.store_dir.display(), "store opened");
    println!("listening on {address}");

    // Both handlers are installed *before* the first connection is accepted, and failing to install
    // one is a startup failure: a daemon that cannot be asked to stop is a daemon nobody can operate.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| lakelet::Error::Store {
            source: Box::new(error),
        })?;
    let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| lakelet::Error::Store {
            source: Box::new(error),
        })?;
    let shutdown = async move {
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM"),
            _ = int.recv() => tracing::info!("SIGINT"),
        }
    };

    let served = ingest::serve(listener, store, shutdown).await;
    println!("draining");
    served
}
