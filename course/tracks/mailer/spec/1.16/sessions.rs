//! Spec for `mailer/1.16-sessions`: logging in is how a caller stops being anonymous, and a session
//! is the credential they carry afterwards.
//!
//! `kiln run` copies this file into the project as `tests/spec_sessions.rs`; it is the course's
//! file, not the learner's (plan §3 D2). Everything is judged from outside the service, and for this
//! step that means **a real process**: a cookie and an `Authorization` header are two transports for
//! one credential, and the thing that speaks them is a client
//! (`tests/support/service.rs`), not a function call.
//!
//! What the tests are careful about:
//!
//! * **a session is checked against the database, not against the request.** A well-formed token
//!   nobody was issued is a `401`, and so is one that was revoked or that has expired;
//! * **the token is never stored.** `psql` is asked for the row, and the row holds a digest: a
//!   database is not a set of live sessions;
//! * **nothing a caller brings becomes an identity.** Every login opens a new session, and a cookie
//!   that arrives with the login is read as a value the server has to check, never as one it may
//!   trust.

#[path = "support/postgres.rs"]
mod postgres;
#[path = "support/service.rs"]
mod service;

use postgres::Postgres;
use service::Service;

/// The name the session travels under: a cookie for a browser, a bearer token for a program.
const COOKIE: &str = "session";

/// The password the fixture subscribes with. It is long enough to be a password this service
/// accepts (step `mailer/1.10-domain-types`) and short enough to type.
const PASSWORD: &str = "correct horse battery staple";

/// The body of `POST /signup`, as a caller writes it.
fn signup_body(email: &str) -> String {
    format!(r#"{{"email":"{email}","name":"Ada Lovelace","password":"{PASSWORD}"}}"#)
}

/// The body of `POST /login`.
fn login_body(email: &str, password: &str) -> String {
    format!(r#"{{"email":"{email}","password":"{password}"}}"#)
}

/// The database setting the service reads (step `mailer/1.4-config`).
fn database_setting(url: &str) -> (&'static str, &str) {
    ("MAILER__DATABASE__URL", url)
}

/// Subscribe an address and confirm it, so there is a subscriber who may log in.
///
/// The token is read out of the row with `psql` rather than out of a log line: the confirmation
/// email's body is the only place it travels, and step `mailer/1.13-email-port` decided that the
/// log carries the envelope and not the body.
fn confirmed_subscriber(service: &Service, postgres: &Postgres, email: &str) {
    let answer = service.post("/signup", &[], &signup_body(email));
    assert_eq!(
        answer.status, 201,
        "the fixture signup failed: {}",
        answer.body
    );
    let token = postgres.query(&format!(
        "SELECT confirmation_token FROM subscribers WHERE email = '{email}'"
    ));
    let answer = service.get(&format!("/confirm?token={token}"));
    assert_eq!(
        answer.status, 200,
        "the fixture confirmation failed: {}",
        answer.body
    );
}

/// Log in and answer the response.
fn log_in(service: &Service, email: &str, password: &str) -> service::Response {
    service.post("/login", &[], &login_body(email, password))
}

/// The session token in a response's `Set-Cookie`, as a client would keep it.
fn session_of(answer: &service::Response) -> String {
    let cookie = answer
        .header("set-cookie")
        .unwrap_or_else(|| panic!("a login has to set a cookie: {:?}", answer.headers));
    let (name, value) = cookie
        .split_once(';')
        .and_then(|(pair, _)| pair.split_once('='))
        .unwrap_or_else(|| {
            panic!("a cookie is `name=value` with attributes after `;`: `{cookie}`")
        });
    assert_eq!(name.trim(), COOKIE, "the cookie has to be named `{COOKIE}`");
    value.trim().to_owned()
}

/// The header a browser would send back with that token.
fn cookie_header(token: &str) -> String {
    format!("{COOKIE}={token}")
}

/// The header a program would send.
fn bearer_header(token: &str) -> String {
    format!("Bearer {token}")
}

/// A well-formed session token nobody was issued.
fn unknown_token() -> String {
    "0".repeat(64)
}

#[test]
fn a_login_answers_with_a_session_cookie() {
    let postgres = Postgres::start();
    let service = Service::start(&[database_setting(&postgres.url())]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");

    let answer = log_in(&service, "ada@example.com", PASSWORD);
    assert_eq!(
        answer.status, 200,
        "a confirmed subscriber with the right password has to be let in: {}",
        answer.body
    );
    assert!(
        answer.body.contains("ada@example.com"),
        "the answer has to name the subscriber it is about: {}",
        answer.body
    );

    let token = session_of(&answer);
    assert_eq!(
        token.len(),
        64,
        "a session token is 32 bytes of randomness in hex (step `mailer/1.12-token` uses the same \
         spelling for a confirmation token): `{token}`"
    );
    assert!(
        token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "a session token is hex, so that it survives a cookie header and a shell: `{token}`"
    );

    // The attributes are the difference between a cookie and a token lying around in a browser.
    let cookie = answer
        .header("set-cookie")
        .expect("a login sets a cookie")
        .to_ascii_lowercase();
    for attribute in ["httponly", "samesite", "path=/"] {
        assert!(
            cookie.contains(attribute),
            "the session cookie is missing `{attribute}`: `{cookie}`"
        );
    }
}

#[test]
fn a_session_works_as_a_cookie_and_as_a_bearer_token() {
    let postgres = Postgres::start();
    let service = Service::start(&[database_setting(&postgres.url())]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");
    let token = session_of(&log_in(&service, "ada@example.com", PASSWORD));

    let answer = service.get_with("/me", &[("Cookie", &cookie_header(&token))]);
    assert_eq!(
        answer.status, 200,
        "the browser's way of carrying a session is a cookie: {}",
        answer.body
    );
    assert!(
        answer.body.contains("ada@example.com"),
        "and it names the subscriber the session belongs to: {}",
        answer.body
    );

    let answer = service.get_with("/me", &[("Authorization", &bearer_header(&token))]);
    assert_eq!(
        answer.status, 200,
        "the same token in `Authorization: Bearer` is the program's way: {}",
        answer.body
    );

    let answer = service.get("/me");
    assert_eq!(
        answer.status, 401,
        "no credential is not an identity: {}",
        answer.body
    );

    let answer = service.get_with("/me", &[("Cookie", &cookie_header(&unknown_token()))]);
    assert_eq!(
        answer.status, 401,
        "a well-formed token nobody was issued is not a session either: {}",
        answer.body
    );

    let answer = service.get_with(
        "/me",
        &[("Authorization", &bearer_header(&unknown_token()))],
    );
    assert_eq!(answer.status, 401, "{}", answer.body);
}

#[test]
fn a_wrong_password_is_not_a_session() {
    let postgres = Postgres::start();
    let service = Service::start(&[database_setting(&postgres.url())]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");

    let answer = log_in(&service, "ada@example.com", "the wrong one entirely");
    assert_eq!(
        answer.status, 401,
        "a password that does not match is a `401`, and it is not a session: {}",
        answer.body
    );
    assert!(
        answer.header("set-cookie").is_none(),
        "a failed login must not hand out a credential: {:?}",
        answer.headers
    );

    let answer = log_in(&service, "nobody@example.com", PASSWORD);
    assert_eq!(
        answer.status, 401,
        "an address nobody subscribed is the same answer as a wrong password — which of the two \
         it was is a fact about this service's subscribers: {}",
        answer.body
    );
}

#[test]
fn an_address_that_was_never_confirmed_cannot_log_in() {
    let postgres = Postgres::start();
    let service = Service::start(&[database_setting(&postgres.url())]);

    let answer = service.post("/signup", &[], &signup_body("ada@example.com"));
    assert_eq!(answer.status, 201, "{}", answer.body);

    let answer = log_in(&service, "ada@example.com", PASSWORD);
    assert_eq!(
        answer.status, 403,
        "the password is right and the address was never proven — confirmation would be a \
         decoration if this were a `200`: {}",
        answer.body
    );
    assert!(
        answer.header("set-cookie").is_none(),
        "and it is not a session: {:?}",
        answer.headers
    );
}

#[test]
fn a_logout_ends_the_session() {
    let postgres = Postgres::start();
    let service = Service::start(&[database_setting(&postgres.url())]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");
    let token = session_of(&log_in(&service, "ada@example.com", PASSWORD));

    let answer = service.call(
        "POST",
        "/logout",
        &[("Cookie", &cookie_header(&token))],
        None,
    );
    assert!(
        answer.status == 204,
        "a logout ends a session and has nothing to say about it: {} {}",
        answer.status,
        answer.body
    );
    let clearing = answer
        .header("set-cookie")
        .unwrap_or_else(|| panic!("a logout has to clear the cookie: {:?}", answer.headers))
        .to_ascii_lowercase();
    assert!(
        clearing.contains("max-age=0") || clearing.contains("expires="),
        "a signed-out browser has to stop sending the token: `{clearing}`"
    );

    let answer = service.get_with("/me", &[("Cookie", &cookie_header(&token))]);
    assert_eq!(
        answer.status, 401,
        "the token that was revoked is not a session any more: {}",
        answer.body
    );
    let revoked = postgres.query(&format!(
        "SELECT (revoked_at IS NOT NULL)::text FROM sessions \
          WHERE token_hash = encode(sha256('{token}'::bytea), 'hex')"
    ));
    assert_eq!(
        revoked, "true",
        "the row is what remembers that this device was signed out"
    );
}

#[test]
fn a_session_that_expired_is_not_a_session() {
    let postgres = Postgres::start();
    let service = Service::start(&[database_setting(&postgres.url())]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");
    let token = session_of(&log_in(&service, "ada@example.com", PASSWORD));

    // The clock a session is measured against is the database's (the same decision step
    // `mailer/1.12-token` made for a confirmation window), which is also what makes this test
    // possible: moving the row's deadline is moving time.
    postgres.execute(&format!(
        "UPDATE sessions SET expires_at = now() - interval '1 second' \
          WHERE token_hash = encode(sha256('{token}'::bytea), 'hex')"
    ));

    let answer = service.get_with("/me", &[("Cookie", &cookie_header(&token))]);
    assert_eq!(
        answer.status, 401,
        "a session past its deadline is not a session: {}",
        answer.body
    );
}

#[test]
fn every_login_is_a_new_session_and_the_token_is_never_stored() {
    let postgres = Postgres::start();
    let service = Service::start(&[database_setting(&postgres.url())]);
    confirmed_subscriber(&service, &postgres, "ada@example.com");

    let first = session_of(&log_in(&service, "ada@example.com", PASSWORD));
    let second = session_of(&log_in(&service, "ada@example.com", PASSWORD));
    assert_ne!(
        first, second,
        "every login is a *new* session: a token that repeats is a token an attacker can wait for \
         (session fixation)"
    );

    // A cookie that arrives with the login is a value the server checks, never one it adopts.
    let answer = service.post(
        "/login",
        &[("Cookie", &cookie_header(&unknown_token()))],
        &login_body("ada@example.com", PASSWORD),
    );
    assert_eq!(answer.status, 200, "{}", answer.body);
    let third = session_of(&answer);
    assert_ne!(
        third,
        unknown_token(),
        "a token the client brought was adopted as the session"
    );

    // What the table holds is a digest, and the digest is of this token: the token itself is not in
    // the database at all, which is what makes a dump of it worthless to whoever steals it.
    let stored = postgres.query(&format!(
        "SELECT count(*) FROM sessions WHERE token_hash = '{third}'"
    ));
    assert_eq!(
        stored, "0",
        "the token is in the database verbatim: a leaked table would be a set of live sessions"
    );
    let digest = postgres.query(&format!(
        "SELECT token_hash FROM sessions WHERE token_hash = encode(sha256('{third}'::bytea), 'hex')"
    ));
    assert_eq!(
        digest.len(),
        64,
        "the row has to hold the digest of the token, 64 hex characters of it: `{digest}`"
    );
}
