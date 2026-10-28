//! Spec for `platform/9.5-oidc` — four checks, in the order that makes the audience matter.

use platform::oidc::{ClaimError, Claims, validate};

fn claims() -> Claims {
    Claims {
        issuer: "https://id.example.test".to_owned(),
        audience: "mailer".to_owned(),
        subject: "user-1".to_owned(),
        expires_at: 1_000,
        not_before: 0,
    }
}

#[test]
fn a_valid_token_answers_its_subject() {
    assert_eq!(
        validate(&claims(), "https://id.example.test", "mailer", 500),
        Ok("user-1")
    );
}

#[test]
fn another_issuer_is_refused() {
    assert_eq!(
        validate(&claims(), "https://evil.example.test", "mailer", 500),
        Err(ClaimError::WrongIssuer)
    );
}

#[test]
fn a_token_for_another_audience_is_refused() {
    assert_eq!(
        validate(&claims(), "https://id.example.test", "relay", 500),
        Err(ClaimError::WrongAudience)
    );
}

#[test]
fn the_audience_is_checked_before_the_expiry() {
    let mut claims = claims();
    claims.audience = "relay".to_owned();
    // Both wrong: the audience is the contract the order names first.
    assert_eq!(
        validate(&claims, "https://id.example.test", "mailer", 5_000),
        Err(ClaimError::WrongAudience)
    );
}

#[test]
fn the_expiry_second_is_already_expired() {
    assert_eq!(
        validate(&claims(), "https://id.example.test", "mailer", 1_000),
        Err(ClaimError::Expired)
    );
}

#[test]
fn a_token_that_is_not_valid_yet_is_refused() {
    let mut claims = claims();
    claims.not_before = 600;
    assert_eq!(
        validate(&claims, "https://id.example.test", "mailer", 500),
        Err(ClaimError::NotYetValid)
    );
}
