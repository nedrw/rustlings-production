//! The failures this service has, and what each one means to a caller and to a shell.
//!
//! Two maps have to exist and agree — the lesson of `mailer/1.5-errors`: what a failure means for
//! the process ([`Error::exit_code`]) and what it means for an HTTP caller ([`Error::status`]).
//! A domain failure is a 4xx, a broken configuration or a database that is away is a 5xx, and
//! [`Error::kind`] is the stable token a caller may switch on (never a sentence, which would make
//! every wording fix an API break).

use crate::domain::EndpointUrl;

/// Everything that can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A command line argument the service does not accept.
    #[error("{0}")]
    Usage(String),
    /// A setting is missing, or two of them contradict each other.
    #[error("{0}")]
    Config(String),
    /// The database could not be reached, migrated, or queried.
    #[error("database: {0}")]
    Database(#[source] sqlx::Error),
    /// The request carries no usable identity.
    #[error("authentication required: {0}")]
    Unauthenticated(String),
    /// The caller may not do this to this resource — and must not learn whether it exists.
    #[error("not found")]
    NotFound,
    /// The request is well-formed HTTP but not a valid request for this route.
    #[error("{0}")]
    BadRequest(String),
    /// The endpoint target is not one this service is willing to deliver to.
    #[error("target rejected: {0}")]
    Target(String),
    /// Something failed while talking to a subscriber.
    #[error("delivery failed: {0}")]
    Delivery(String),
    /// The work cannot be accepted right now (queue full, tenant over quota).
    #[error("{0}")]
    Unavailable(String),
    /// The endpoint's URL is not a usable target, kept with its cause.
    #[error("bad endpoint url: {0}")]
    Endpoint(#[from] crate::domain::ValueError),
}

impl Error {
    /// The code the process exits with when this failure ends it.
    ///
    /// 1 is the deployment's problem (configuration, database), 2 the caller's (usage, bad request)
    /// — the table `mailer/1.5-errors` established.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        todo!("step relay/2.1-project-skeleton")
    }

    /// The status an HTTP caller sees for this failure.
    #[must_use]
    pub fn status(&self) -> u16 {
        todo!("step relay/2.1-project-skeleton")
    }

    /// Stable, lower-case token used in error bodies and in logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        todo!("step relay/2.1-project-skeleton")
    }

    /// `true` when the same request could succeed later, so an honest 503 is not a 400.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        todo!("step relay/2.1-project-skeleton")
    }
}
