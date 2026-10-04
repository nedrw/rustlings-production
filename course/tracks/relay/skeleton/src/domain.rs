//! The types the service is about: who is asking, which endpoint they own, and what a delivery
//! target may look like.
//!
//! Step `relay/2.1-project-skeleton` implements them. The rule is the one `mailer/1.10-domain-types`
//! taught and this track keeps: parse, do not validate afterwards — a value that exists is a value
//! that is legal, and the rest of the service never asks "is this string really an id".

use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

/// A tenant: the unit that owns endpoints, events, and quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantId(Uuid);

impl TenantId {
    /// A new, random id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// The underlying UUID, for the database and for JSON.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::new()
    }
}

impl FromStr for TenantId {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let _ = text;
        todo!("step relay/2.1-project-skeleton")
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A tenant's name, as it appears in configuration and in logs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantName(String);

impl TenantName {
    /// Parse a name: lower-case, hyphenated, short.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] when the name is empty, longer than 32 characters, or outside the
    /// lower-case alphabet this service uses everywhere.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        let _ = text;
        todo!("step relay/2.1-project-skeleton")
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An endpoint: where one tenant wants their events delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointId(Uuid);

impl EndpointId {
    /// A new, random id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// The underlying UUID.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for EndpointId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a string was not accepted as a domain value.
#[derive(Debug, thiserror::Error)]
pub enum ValueError {
    /// The text is not the shape the type accepts.
    #[error("{0}")]
    Invalid(String),
}

/// A delivery target: absolute, `http` or `https`, with a host and no fragment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointUrl(String);

impl EndpointUrl {
    /// The URL as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for EndpointUrl {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let _ = text;
        todo!("step relay/2.1-project-skeleton")
    }
}

impl fmt::Display for EndpointUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
