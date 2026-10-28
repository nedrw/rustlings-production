//! probe = "sec/authn-session@apply"
//! title = "A session is a decision with a deadline"
//! hint = "Unknown, revoked, expired — and the order they are answered in is part of the contract; a token is live until the second before its deadline."
//!
//! Step `mailer/1.16-sessions` puts a session behind a cookie and gives it two ways to stop being
//! valid: the operator revokes it, or time runs out. Both have to be checked on **every** request
//! — remembered validity is the bug that lets a logged-out browser keep reading somebody's data.
//!
//! ## What you write
//!
//! [`Store::authenticate`] — the user behind a token, or why there is none:
//!
//! * a token nobody issued is [`AuthError::Unknown`];
//! * a revoked token is [`AuthError::Revoked`], even if it has also expired: revocation is a
//!   decision somebody made, and saying "expired" would hide it;
//! * a token whose `expires_at` is at or before `now` is [`AuthError::Expired`]. The deadline is
//!   exclusive: at the second it names, the session is already over.
//!
//! ## What the tests ask
//!
//! A live session; an unknown token; the deadline second itself; the second before it; a revoked
//! token; a revoked *and* expired token (the order of the answers); and revoking one session
//! leaving the others alone.

/// Why a request has no user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// No session was issued with this token.
    Unknown,
    /// The session was revoked.
    Revoked,
    /// The session's deadline has passed.
    Expired,
}

/// One issued session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    token: String,
    user: String,
    expires_at: u64,
    revoked: bool,
}

/// Every session the service knows about.
#[derive(Debug, Default)]
pub struct Store {
    sessions: Vec<Session>,
}

impl Store {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue a session for `user` that expires at `expires_at` (a unix second).
    pub fn issue(&mut self, user: &str, token: &str, expires_at: u64) {
        self.sessions.push(Session {
            token: token.to_owned(),
            user: user.to_owned(),
            expires_at,
            revoked: false,
        });
    }

    /// Revoke a session. Revoking a token nobody issued is not an error.
    pub fn revoke(&mut self, token: &str) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.token == token)
        {
            session.revoked = true;
        }
    }

    /// The user behind `token` at `now`.
    ///
    /// # Errors
    ///
    /// See [`AuthError`]; the answers are checked in the order unknown, revoked, expired.
    pub fn authenticate(&self, token: &str, now: u64) -> Result<&str, AuthError> {
        todo!("find the token, then answer revoked before expired")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let mut store = Store::new();
        store.issue("alice", "tok-a", 1_000);
        store.issue("bob", "tok-b", 2_000);
        store
    }

    #[test]
    fn a_live_session_returns_its_user() {
        assert_eq!(store().authenticate("tok-a", 500), Ok("alice"));
    }

    #[test]
    fn an_unknown_token_has_no_user() {
        assert_eq!(store().authenticate("tok-z", 500), Err(AuthError::Unknown));
    }

    #[test]
    fn the_deadline_second_is_already_expired() {
        assert_eq!(
            store().authenticate("tok-a", 1_000),
            Err(AuthError::Expired)
        );
    }

    #[test]
    fn the_second_before_the_deadline_is_still_live() {
        assert_eq!(store().authenticate("tok-a", 999), Ok("alice"));
    }

    #[test]
    fn a_revoked_token_is_refused() {
        let mut store = store();
        store.revoke("tok-a");
        assert_eq!(store.authenticate("tok-a", 500), Err(AuthError::Revoked));
    }

    #[test]
    fn revocation_is_reported_before_expiry() {
        let mut store = store();
        store.revoke("tok-a");
        // Both true: the operator's decision is the one the caller should see.
        assert_eq!(store.authenticate("tok-a", 5_000), Err(AuthError::Revoked));
    }

    #[test]
    fn revoking_one_session_leaves_the_others_alone() {
        let mut store = store();
        store.revoke("tok-a");
        assert_eq!(store.authenticate("tok-b", 500), Ok("bob"));
    }
}
