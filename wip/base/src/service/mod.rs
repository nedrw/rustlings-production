//! The daemon: the shell around the engine that makes it a service.
//!
//! Two modules, one direction: [`config`] decides what the process was asked to do, and [`logging`]
//! decides where the answers go. Neither of them implements queueing — that is the engine's job, and
//! the arrow only points this way (step `jobq/3.1-engine-boundary`).

/// Usage text, printed by `--help` and after a bad argument.
pub const USAGE: &str = "\
usage: jobqd [OPTIONS]

OPTIONS:
    --config <FILE>          read settings from a TOML file
    --worker-name <NAME>     name this worker in the log and in the rows it holds
    --check-config           print the effective settings and exit
    --help                   print this help and exit
    --version                print the version and exit

Settings are read from the file given by --config and overridden by the environment, where every
setting is spelled `JOBQ__SECTION__KEY` (for example `JOBQ__STORE__URL`).
";

pub mod config;
pub mod logging;

use crate::engine::{Error, MemoryStore, Queue};
pub use config::{Config, Flags};
pub use logging::RunId;

/// Build the store the settings name, announce that this process is draining, and wait for a
/// signal.
///
/// The order is the point, and it is the same one `mailer/1.9-postgres` established: everything
/// the process needs is in place *before* it says it is running, because a process that announces
/// first and fails later is one an orchestrator routes work to.
///
/// # Errors
///
/// [`Error::Config`] when the settings do not name a store this build knows
/// how to open.
pub async fn serve(config: Config, run: RunId) -> Result<(), Error> {
    // The store is opened here rather than inside the drain loop: from step
    // `jobq/3.5-store-port-conformance` on this is a connection pool, and a pool that cannot be
    // opened must stop the process before it claims anything.
    let store = open_store(&config)?;
    let queues: Vec<&str> = config.worker.queues.iter().map(Queue::as_str).collect();

    tracing::info!(
        run = %run,
        queues = %queues.join(","),
        batch = config.worker.batch,
        worker = %config.worker.name,
        "draining"
    );

    shutdown_signal().await;
    tracing::info!(run = %run, "stopped");

    // Nothing drains yet: the loop that claims, handles and settles arrives with the step that
    // gives it a handler and a lease (`jobq/3.7-leases-and-recovery`). What this step owns is the
    // shape of the process — what it was told to do, what it says, and how it stops.
    drop(store);
    Ok(())
}

/// The backend the settings name.
///
/// `memory` (and nothing at all) is the in-process store: honest for a demo, useful for a test, and
/// the one backend every *assembly* of this crate has (step `jobq/3.4-feature-assembly`). Anything
/// else is the durable backend, which this function never opens itself — it hands the URL to
/// [`open_durable`], the single place a durable store is reached from, and the place the `postgres`
/// feature gate belongs.
fn open_store(config: &Config) -> Result<MemoryStore, Error> {
    match config.store.url.as_deref() {
        None | Some("memory") => Ok(MemoryStore::new()),
        Some(other) => open_durable(other),
    }
}

/// Open the durable backend the settings name.
///
/// The durable backend landed with `jobq/3.5-store-port-conformance`, behind the `postgres` feature
/// that step `jobq/3.4-feature-assembly` builds — and this function is still a seam rather than the
/// place a pool is opened, because what this process does with a store is drain it, and the loop
/// that would arrives with `jobq/3.7-leases-and-recovery`. So what it can honestly do today is
/// refuse in the terms of *this* build: a message that says whether the operator is looking at a
/// build that has the feature and no drain loop, or at a build without the feature at all.
///
/// The gate is deliberately in one function with one call site: a feature whose `cfg` is sprinkled
/// across the crate is a feature that is off in one code path and on in another, and the
/// combination nobody tried is the one that breaks.
fn open_durable(url: &str) -> Result<MemoryStore, Error> {
    Err(Error::Config {
        detail: format!(
            "`{}` names a durable store: {}",
            redact_url(url),
            durable_backend()
        ),
    })
}

/// What this build knows about a durable store, which is a compile-time fact.
///
/// Two answers, because there are two different problems and only one of them is permanent: a build
/// that has the feature has a store and no loop to drain with it, and a build without the feature is
/// waiting for a flag. Both name what to do next, because "use `memory`" tells an operator with a
/// database nothing.
#[cfg(feature = "postgres")]
fn durable_backend() -> &'static str {
    "this build has the `postgres` feature and the store it names — \
     `jobq/3.5-store-port-conformance` ships that — and what this process is still missing is the \
     worker loop that would drain one, which arrives with `jobq/3.7-leases-and-recovery`"
}

/// What this build knows about a durable store when the feature is off.
#[cfg(not(feature = "postgres"))]
fn durable_backend() -> &'static str {
    "this build was compiled without the `postgres` feature, so the in-memory engine is all it \
     has: rebuild with `--features postgres`"
}

/// A connection string with the credentials taken out, for a message a person will read.
fn redact_url(url: &str) -> String {
    match url.split_once('@') {
        Some((credentials, host)) => {
            let user = credentials
                .split_once(':')
                .map_or(credentials, |(user, _)| user);
            let scheme = credentials
                .split_once("://")
                .map_or("", |(scheme, _)| scheme);
            if scheme.is_empty() {
                format!("{user}@{host}")
            } else {
                format!("{scheme}://{user}@{host}")
            }
        }
        None => url.to_owned(),
    }
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
