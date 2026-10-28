//! probe = "sec/oidc@apply"
//! title = "Verify with the algorithm you chose, never the one the token names"
//! hint = "Refuse an algorithm you do not accept before reading anything else, then check issuer, audience, not-before and expiry, in that order."
//!
//! Step `platform/9.5-oidc` is the four checks a token goes through, plus the hole that made JWTs
//! famous: a verifier that trusts the token's own `alg`. This probe puts the algorithm first.
//!
//! ## What you write
//!
//! [`validate`] — the subject when every check passes, else the first failure, in the order
//! algorithm, issuer, audience, not-before, expiry.
//!
//! ## What the tests ask
//!
//! A valid RS256 token; `alg: none` refused before anything else; a wrong issuer; a wrong audience;
//! the audience before the expiry; and the expiry boundary.

/// The claims this service reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Who minted it.
    pub issuer: String,
    /// Who it is for.
    pub audience: String,
    /// Who it identifies.
    pub subject: String,
    /// The first second at which it is invalid.
    pub expires_at: u64,
    /// The first second at which it is valid.
    pub not_before: u64,
    /// The algorithm the token claims.
    pub algorithm: String,
}

/// Why a token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The algorithm is not one this service verifies with.
    UnsupportedAlgorithm,
    /// Another provider minted it.
    WrongIssuer,
    /// It is for another audience.
    WrongAudience,
    /// It is not valid yet.
    NotYetValid,
    /// It has expired.
    Expired,
}

/// The subject of a valid token, or the first check it failed.
///
/// # Errors
///
/// See [`Error`]; the checks run algorithm, issuer, audience, not-before, expiry.
pub fn validate<'a>(
    token: &'a Token,
    issuer: &str,
    audience: &str,
    now: u64,
) -> Result<&'a str, Error> {
    todo!("the algorithm this service accepts, then issuer, audience, not-before, expiry")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> Token {
        Token {
            issuer: "https://id.example.test".to_owned(),
            audience: "mailer".to_owned(),
            subject: "user-1".to_owned(),
            expires_at: 1_000,
            not_before: 0,
            algorithm: "RS256".to_owned(),
        }
    }

    #[test]
    fn a_valid_token_answers_its_subject() {
        assert_eq!(
            validate(&token(), "https://id.example.test", "mailer", 500),
            Ok("user-1")
        );
    }

    #[test]
    fn an_unsupported_algorithm_is_refused_before_anything_else() {
        let mut token = token();
        token.algorithm = "none".to_owned();
        token.audience = "relay".to_owned();
        assert_eq!(
            validate(&token, "https://id.example.test", "mailer", 500),
            Err(Error::UnsupportedAlgorithm)
        );
    }

    #[test]
    fn another_issuer_is_refused() {
        assert_eq!(
            validate(&token(), "https://evil.example.test", "mailer", 500),
            Err(Error::WrongIssuer)
        );
    }

    #[test]
    fn a_token_for_another_audience_is_refused() {
        assert_eq!(
            validate(&token(), "https://id.example.test", "relay", 500),
            Err(Error::WrongAudience)
        );
    }

    #[test]
    fn the_audience_is_checked_before_the_expiry() {
        let mut token = token();
        token.audience = "relay".to_owned();
        assert_eq!(
            validate(&token, "https://id.example.test", "mailer", 5_000),
            Err(Error::WrongAudience)
        );
    }

    #[test]
    fn the_expiry_second_is_already_expired() {
        assert_eq!(
            validate(&token(), "https://id.example.test", "mailer", 1_000),
            Err(Error::Expired)
        );
    }
}
