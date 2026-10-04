//! Who is calling: an API key belongs to a tenant, and nothing else in the request says who you are.
//!
//! Step `relay/2.1-project-skeleton` implements it. The key is never stored: the database keeps a
//! hash, the service hashes what the caller sent and looks *that* up, and a request that carries no
//! usable key gets 401 with the same body whether the key was absent, empty, or made up.

use sqlx::PgPool;

use crate::domain::{TenantId, TenantName};
use crate::error::Error;

/// The identity a request acts as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// Tenant the key belongs to.
    pub tenant_id: TenantId,
    /// Tenant's name, for logs and audit lines.
    pub name: TenantName,
}

impl Principal {
    /// Build a principal from the two things a lookup returns.
    #[must_use]
    pub fn new(tenant_id: TenantId, name: TenantName) -> Self {
        Self { tenant_id, name }
    }
}

/// The hash this service stores for an API key: SHA-256, hex, lower case.
#[must_use]
pub fn hash_api_key(key: &str) -> String {
    let _ = key;
    todo!("step relay/2.1-project-skeleton")
}

/// Look the key up and answer who it is, or `None` when it belongs to nobody.
///
/// `None` and `Err` are different answers: a key that matches no tenant is a 401, a database that
/// did not answer is a 500, and conflating them turns an outage into a wave of "wrong key" pages.
///
/// # Errors
///
/// Returns [`Error::Database`] when the lookup could not be answered at all.
pub async fn authenticate(pool: &PgPool, key: &str) -> Result<Option<Principal>, Error> {
    let _ = (pool, key);
    todo!("step relay/2.1-project-skeleton")
}
