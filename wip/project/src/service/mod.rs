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
pub mod handler;
pub mod logging;
pub mod worker;

use std::sync::Arc;
use std::time::Duration;

use crate::engine::{Error, MemoryStore, Queue, Store};
pub use config::{Config, Flags};
pub use logging::RunId;
use worker::{Shutdown, Worker, WorkerSettings};

/// Build the store the settings name, bring its schema up to date, announce that this process is
/// draining, and run the worker until a signal arrives.
///
/// The order is the point, and it is the same one `mailer/1.9-postgres` established: everything the
/// process needs is in place *before* it says it is running, because a process that announces first
/// and fails later is one an orchestrator routes work to. `jobq/3.7-leases-and-recovery` is where
/// the last piece of that order lands — the loop that actually claims, handles and settles — and
/// where the store stops being a seam: a durable URL is now opened, migrated and drained.
///
/// The signal does one thing: it stops the worker claiming. What is in flight gets
/// `worker.shutdown_seconds` to finish, what the deadline cuts is handed back, and the process exits
/// `0` — the contract `jobq/3.1-engine-boundary` pinned for the line below and for the exit code.
///
/// # Errors
///
/// [`Error::Config`] when the settings do not name a store this build knows how to open, or name a
/// handler that is not there; [`Error::Store`] when the store cannot be opened or migrated;
/// whatever the worker itself fails at.
pub async fn serve(config: Config, run: RunId) -> Result<(), Error> {
    let store: Arc<dyn Store> = open_store(&config).await?;
    // Every start, against a database that is usually already current: the second run has to change
    // nothing, which is the promise `mailer/1.9-postgres` made and this is where the daemon starts
    // relying on it.
    if config.store.migrate {
        store.migrate().await?;
    }
    let handler = handler::for_config(&config)?;
    let settings = worker_settings(&config);
    let queues: Vec<&str> = config.worker.queues.iter().map(Queue::as_str).collect();
    let worker = Worker::new(store, handler, settings);
    let shutdown = Shutdown::new();

    // The signal becomes the worker's own signal: one shape for "stop claiming", whoever sent it,
    // which is what lets the loop be tested without a process to send anything to.
    let signalling = tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown_signal().await;
            shutdown.trigger();
        }
    });

    let draining = tokio::spawn(async move { worker.run(shutdown).await });

    // Announced *after* the worker is running, so that the line a supervisor waits for means what it
    // says: a process that has said this claims work.
    tracing::info!(
        run = %run,
        queues = %queues.join(","),
        batch = config.worker.batch,
        worker = %config.worker.name,
        "draining"
    );

    let report = match draining.await {
        Ok(Ok(report)) => report,
        Ok(Err(error)) => return Err(error),
        Err(join) => {
            return Err(Error::Io {
                source: std::io::Error::other(format!("the worker did not finish: {join}")),
            });
        }
    };
    signalling.abort();
    tracing::info!(
        run = %run,
        claimed = report.claimed(),
        succeeded = report.succeeded(),
        failed = report.failed(),
        reclaimed = report.reclaimed(),
        handed_back = report.handed_back(),
        "stopped"
    );
    Ok(())
}

/// The worker settings the configuration describes.
///
/// One place where the configuration becomes the loop's own vocabulary, including the lease: the
/// store is built with the same number (see [`open_store`]) because the two have to agree — the
/// store stamps the instant and the worker renews before it, and a deployment that set them
/// separately would be a deployment whose renewals come too late.
fn worker_settings(config: &Config) -> WorkerSettings {
    WorkerSettings::new(
        crate::engine::WorkerId::new(config.worker.name.clone()),
        config.worker.queues.clone(),
    )
    .with_batch(config.worker.batch)
    .with_poll_interval(Duration::from_millis(config.worker.poll_interval_ms))
    .with_shutdown(Duration::from_secs(config.worker.shutdown_seconds))
    .with_lease(Duration::from_secs(config.worker.lease_seconds))
}

/// The backend the settings name, with the lease they name.
///
/// `memory` (and nothing at all) is the in-process store: honest for a demo, useful for a test, and
/// the one backend every *assembly* of this crate has (step `jobq/3.4-feature-assembly`). Anything
/// else is the durable backend, which this function never opens itself — it hands the URL to
/// [`open_durable`], the single place a durable store is reached from, and the place the `postgres`
/// feature gate belongs.
async fn open_store(config: &Config) -> Result<Arc<dyn Store>, Error> {
    let lease = Duration::from_secs(config.worker.lease_seconds);
    match config.store.url.as_deref() {
        None | Some("memory") => Ok(Arc::new(MemoryStore::new().with_lease(lease))),
        Some(other) => open_durable(other, lease).await,
    }
}

/// Open the durable backend the settings name.
///
/// The durable backend landed with `jobq/3.5-store-port-conformance`, behind the `postgres` feature
/// that step `jobq/3.4-feature-assembly` builds, and `jobq/3.7-leases-and-recovery` is the step that
/// finally opens one: the pool is handed the URL the settings named and the lease they named, and
/// the worker drains it like any other store.
///
/// The gate is deliberately in one function with one call site: a feature whose `cfg` is sprinkled
/// across the crate is a feature that is off in one code path and on in another, and the
/// combination nobody tried is the one that breaks.
#[cfg(feature = "postgres")]
async fn open_durable(url: &str, lease: Duration) -> Result<Arc<dyn Store>, Error> {
    let store = crate::engine::PostgresStore::connect(url)
        .await?
        .with_lease(lease);
    Ok(Arc::new(store))
}

/// What a build without the durable backend says when it is asked for one.
///
/// One sentence, and it names the *feature* rather than the URL: an operator with a database needs to
/// know which flag to rebuild with, and "use `memory`" tells them nothing they did not already know
/// (the probe of `jobq/3.4-feature-assembly` reads this sentence for exactly that reason).
#[cfg(not(feature = "postgres"))]
async fn open_durable(url: &str, _lease: Duration) -> Result<Arc<dyn Store>, Error> {
    Err(Error::Config {
        detail: format!(
            "`{}` names a durable store and this build has no `postgres` feature: rebuild with \
             `--features postgres`",
            redact_url(url)
        ),
    })
}

/// A connection string with the credentials taken out, for a message a person will read.
///
/// Only a build *without* the durable backend has anything to say about a durable URL, so this is
/// where it lives: the build that can open one never prints it.
#[cfg(not(feature = "postgres"))]
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
