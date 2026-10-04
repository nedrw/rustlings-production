//! The store the conformance suite runs against: one suite, every backend.
//!
//! Step `jobq/3.5-store-port-conformance` is about a *suite* that runs more than once, so the spec
//! never names a backend: it asks this module for one. `KILN_BACKEND` decides which — `memory` (the
//! default, so `kiln run` and a plain `cargo test` need no database) or `postgres` — and the
//! `conformance@2` gate re-runs the step's `run.cmd` once per backend with that variable set. That
//! is what "one suite, N implementations" means in practice: **one spec file, run N times**, not N
//! spec files that drift apart.
//!
//! Two rules, and both are about what an assertion can say rather than about this fixture:
//!
//! * **the container belongs to the value.** A postgres backend owns the container its database
//!   lives in, so dropping the [`Backend`] takes the database away and the next case starts empty:
//!   no container, no database, no port, no rows, shared with anybody;
//! * **the fixture answers with the port, not with a concrete type.** [`Backend::store`] hands out
//!   `&dyn Store`, so a spec written against the seam cannot reach for a method only one backend
//!   has — and every failure names the backend ([`Backend::name`]), because the same assertion is
//!   about to run against two of them.
//!
//! The schema is applied before the store is handed over ([`backend`]), so every case starts from
//! the same place; [`Backend::migrate`] runs that step again for the case that is about it.

use jobq::engine::{Error, MemoryStore, PostgresStore, Store};

use super::postgres::Postgres;

/// The variable that says which backend to build.
pub const BACKEND: &str = "KILN_BACKEND";

/// The backend this run is against: `memory` unless the environment says otherwise.
fn kind() -> String {
    match std::env::var(BACKEND) {
        Ok(name) if !name.trim().is_empty() => name.trim().to_owned(),
        _ => "memory".to_owned(),
    }
}

/// A store for one case, with whatever that store needs kept alive.
///
/// The variant *is* the answer to "which backend": a spec that has to ask is a spec that is about
/// to assert something only one of them can do.
pub enum Backend {
    /// The in-process store: no container, no connection, nothing to clean up.
    Memory(MemoryStore),
    /// The durable store, and the container the database lives in.
    Postgres {
        /// The port, backed by the container's database.
        store: PostgresStore,
        /// Held so the database outlives the store that uses it.
        container: Postgres,
    },
}

impl Backend {
    /// Which backend this is, for a failure message.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Memory(_) => "memory",
            Backend::Postgres { .. } => "postgres",
        }
    }

    /// The store, as the engine's port sees it.
    #[must_use]
    pub fn store(&self) -> &dyn Store {
        match self {
            Backend::Memory(store) => store,
            Backend::Postgres { store, .. } => store,
        }
    }

    /// Bring the schema up to date again.
    ///
    /// Every backend has this step, because a backend that has to be prepared before it can be used
    /// is a backend whose preparation has to be safe to repeat: it runs on every start of the
    /// daemon, and against a database that is already current it has to change nothing. The
    /// in-memory store has no schema, so its answer is `Ok(())` — a no-op rather than a special
    /// case the spec has to know about.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the backend refused to apply its schema.
    pub async fn migrate(&self) -> Result<(), Error> {
        // One call, because `jobq/3.7-leases-and-recovery` put the step on the port: a backend with
        // no schema answers `Ok(())`, which is the answer this match used to spell out by hand.
        self.store().migrate().await
    }
}

/// Build the backend `KILN_BACKEND` names, with its schema already applied.
///
/// Panics when the variable names a backend this course has not certified, and when the container
/// cannot be started: inside a case, a fixture that cannot start is a failure with a sentence.
pub async fn backend() -> Backend {
    match kind().as_str() {
        "memory" => Backend::Memory(MemoryStore::new()),
        "postgres" => {
            let container = Postgres::start();
            let store = PostgresStore::connect(&container.url())
                .await
                .expect("the container's database is reachable: it just answered a query");
            store
                .migrate()
                .await
                .expect("this project's migration applies to the schema it describes");
            Backend::Postgres { store, container }
        }
        other => panic!(
            "`{BACKEND}={other}` names no backend this suite knows: the certified ones are \
             `memory` (the default) and `postgres`, and `conformance.toml` is the file that \
             declares them"
        ),
    }
}
