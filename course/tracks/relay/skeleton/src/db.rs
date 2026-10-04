//! The database: one pool, the migrations, and the two questions the probes ask.
//!
//! Step `relay/2.1-project-skeleton` implements it. The order is the lesson from `mailer/1.9-postgres`:
//! connect, migrate, and only then start serving — a service that answers requests before its schema
//! is in place is a service that answers them with errors.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::error::Error;

/// The service's connection to Postgres.
#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect a pool to `url`.
    /// # Errors
    ///
    /// Returns [`Error::Database`] when the pool cannot be built or the server refuses the
    /// connection.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        let _ = url;
        todo!("step relay/2.1-project-skeleton")
    }

    /// Apply the migrations embedded in this binary.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] when a migration fails.
    pub async fn migrate(&self) -> Result<(), Error> {
        todo!("step relay/2.1-project-skeleton")
    }

    /// `true` when the database answers a trivial query: what `GET /readyz` reports.
    pub async fn ready(&self) -> bool {
        todo!("step relay/2.1-project-skeleton")
    }

    /// The pool, for the queries the rest of the service runs.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Create the tenants this configuration names, and refresh their keys.
    ///
    /// Idempotent on purpose: it runs on every start, so a rotated key in the configuration is
    /// live after a restart, and an unchanged configuration changes nothing.
    pub async fn seed_tenants(&self, tenants: &[(String, String)]) -> Result<(), Error> {
        let _ = tenants;
        todo!("step relay/2.1-project-skeleton")
    }
}

/// How many connections a pool keeps, and how long it waits for one.
///
/// Constants rather than settings for now: the numbers are a deployment decision this course makes
/// once, in `mailer/1.9-postgres`, and a second place to configure them would be a second place to
/// get them wrong.
pub const MAX_CONNECTIONS: u32 = 8;
/// How long a request waits for a connection before it fails.
pub const ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Build the pool options every connection in this service uses.
#[must_use]
pub fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(ACQUIRE_TIMEOUT)
}
