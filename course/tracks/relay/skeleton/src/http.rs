//! The HTTP surface: which routes exist, what each one may assume, and in which order.
//!
//! Step `relay/2.1-project-skeleton` implements it, and the order is the lesson of this step:
//! **correlation id → request log → authentication → handler**. A layer that runs outside the log
//! cannot be seen failing, and a request rejected by authentication must still appear in the log
//! with the id it was given — otherwise the only requests an operator cannot follow are the ones
//! that were refused.
//!
//! Three routes are public (`/healthz`, `/readyz`) and everything under `/endpoints` needs a
//! principal. The stub below declares the shape; the step's spec judges the behaviour from outside.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::db::Db;
use crate::error::Error;
use crate::logging::RunId;

/// Everything a handler may need, built once per process.
#[derive(Debug, Clone)]
pub struct AppState {
    config: Arc<Config>,
    db: Db,
    run: RunId,
}

impl AppState {
    /// Connect the database, apply the migrations, seed the configured tenants — in that order.
    ///
    /// A service that starts serving before its schema is in place answers with errors, and a
    /// service that starts before its tenants exist rejects every key.
    /// # Errors
    ///
    /// Returns [`Error::Config`] when no database is configured, and [`Error::Database`] when the
    /// database cannot be reached, migrated, or seeded.
    pub async fn start(config: Config, run: &RunId) -> Result<Self, Error> {
        let _ = (config, run);
        todo!("step relay/2.1-project-skeleton")
    }

    /// The configuration this process is running with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The database.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The id of this run.
    #[must_use]
    pub fn run(&self) -> &RunId {
        &self.run
    }
}

/// The routing table, with its layers.
pub fn router(state: AppState) -> Router {
    let _ = state;
    todo!("step relay/2.1-project-skeleton")
}

/// Serve until the listener fails, or forever.
/// # Errors
///
/// Returns the error the server stopped with.
pub async fn serve_on(listener: TcpListener, state: AppState, run: &RunId) -> Result<(), Error> {
    let _ = (listener, state, run);
    todo!("step relay/2.1-project-skeleton")
}

/// Serve until `shutdown` resolves, then stop accepting and let in-flight requests finish.
///
/// The deadline belongs to the caller (`main.rs` waits for a signal and gives the drain a few
/// seconds): this function's promise is only "no request is cut in half", and `serve_on` is the
/// same function with a future that never resolves.
/// # Errors
///
/// Returns the error the server stopped with.
pub async fn serve_with_shutdown(
    listener: TcpListener,
    state: AppState,
    run: &RunId,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), Error> {
    let _ = (listener, state, run, shutdown);
    todo!("step relay/2.1-project-skeleton")
}

/// The address a bound listener is reachable on, for the line that announces it.
#[must_use]
pub fn bound_address(listener: &TcpListener) -> SocketAddr {
    let _ = listener;
    todo!("step relay/2.1-project-skeleton")
}
