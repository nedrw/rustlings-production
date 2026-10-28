//! probe = "sec/password-hash@apply"
//! title = "A record that cannot be read back"
//! hint = "Salt every password, store a versioned record with its cost, compare without stopping early, and upgrade the record the next time the password is in hand."
//!
//! Step `mailer/1.11-signup` never stores a password: it stores a record from which the password
//! cannot be recovered, and the shape of that record is what this probe re-checks. Three habits
//! decide whether the record is honest:
//!
//! * **a salt per record** — the same password hashed twice must be two different strings, or one
//!   leaked table compares two users' passwords in one glance;
//! * **the cost is part of the record** — a hash made at 1 024 rounds has to say `1024`, so a later
//!   login can tell that it should be replaced by one made at today's cost;
//! * **a comparison that does not stop early** — the verifier walks the whole digest, so its answer
//!   does not leak *where* a guess was wrong.
//!
//! The primitive here is deliberately weak (`digest` below is a toy mixing function, not a KDF),
//! because a probe runs offline, in seconds and with no dependencies. A real system uses
//! Argon2id or bcrypt; what is re-checked here is the record around the primitive — salt, cost,
//! version, comparison and upgrade — which is the part that gets forgotten.
//!
//! ## What you write
//!
//! * [`hash`] — build a record at today's cost. `None` when the salt is unusable: a salt is empty
//!   or contains `$`, which is the record's own separator;
//! * [`verify`] — recompute with the record's *own* cost and salt, and walk the whole digest;
//!   a wrong password and a damaged record are both `false`, never a panic;
//! * [`PasswordHash::needs_rehash`] — `true` when the record's cost is not today's, so the caller
//!   can replace it while the plaintext is in hand.
//!
//! ## What the tests ask
//!
//! Two salts, two records; the record containing nothing of the password; the right password and
//! only that one; damaged and foreign formats refused without a panic; and an old-cost record that
//! verifies *and* asks to be rehashed.

/// The cost every new record is made at today.
pub const ITERATIONS: u32 = 4_096;

/// One stored password: `v1$<iterations>$<salt>$<digest>`. Never the password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordHash(String);

impl PasswordHash {
    /// Read a record back; `None` for anything that is not the format above.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        fields(text).map(|_| Self(text.to_owned()))
    }

    /// The record as it is stored.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `true` when this record was made at a cost other than [`ITERATIONS`].
    #[must_use]
    pub fn needs_rehash(&self) -> bool {
        fields(self.as_str()).is_none_or(|(iterations, _, _)| iterations != ITERATIONS)
    }
}

/// Hash `password` with `salt` at the current cost.
///
/// Returns `None` when the salt cannot be part of the record: it is empty or contains `$`.
#[must_use]
pub fn hash(password: &str, salt: &str) -> Option<PasswordHash> {
    // The salt is a field of the record, so it cannot contain the field separator.
    if salt.is_empty() || salt.contains('$') {
        return None;
    }
    let digest = digest(ITERATIONS, salt, password);
    Some(PasswordHash(format!("v1${ITERATIONS}${salt}${digest}")))
}

/// Whether `password` is the one this record was made from.
///
/// A wrong password, a damaged record and a scheme this code does not know are all `false`.
#[must_use]
pub fn verify(password: &str, record: &PasswordHash) -> bool {
    let Some((iterations, salt, stored)) = fields(record.as_str()) else {
        return false;
    };
    // The record's own cost, not today's: an old record has to keep verifying, otherwise the
    // upgrade in `needs_rehash` can never happen.
    let computed = digest(iterations, salt, password);
    // No early return: the comparison should not say how much of a guess was right.
    let mut difference = 0u8;
    for (left, right) in computed.bytes().zip(stored.bytes()) {
        difference |= left ^ right;
    }
    difference == 0 && computed.len() == stored.len()
}

/// `(iterations, salt, digest)` of a well-formed record.
fn fields(text: &str) -> Option<(u32, &str, &str)> {
    let mut parts = text.split('$');
    if parts.next()? != "v1" {
        return None;
    }
    let iterations: u32 = parts.next()?.parse().ok()?;
    let salt = parts.next()?;
    let digest = parts.next()?;
    let well_formed = iterations > 0
        && !salt.is_empty()
        && digest.len() == 16
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        && parts.next().is_none();
    well_formed.then_some((iterations, salt, digest))
}

/// The toy mixing function this probe uses. Documented as weak on purpose: the point is the
/// record around it, and a probe cannot pull in Argon2id.
fn digest(iterations: u32, salt: &str, password: &str) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for _ in 0..iterations {
        for byte in salt.as_bytes().iter().chain(password.as_bytes()) {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{state:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_password_under_two_salts_is_two_records() {
        let first = hash("correct horse", "salt-a").expect("a usable salt");
        let second = hash("correct horse", "salt-b").expect("a usable salt");
        assert_ne!(first, second, "the salt is not in the record");
        assert!(verify("correct horse", &first));
        assert!(verify("correct horse", &second));
    }

    #[test]
    fn the_record_says_nothing_about_the_password() {
        let record = hash("correct horse battery", "salt-a").expect("a usable salt");
        let text = record.as_str();
        assert!(!text.contains("correct"), "{text}");
        assert!(!text.contains("horse"), "{text}");
        assert!(text.starts_with(&format!("v1${ITERATIONS}$salt-a$")), "{text}");
    }

    #[test]
    fn verify_accepts_the_password_and_only_that_one() {
        let record = hash("correct horse", "salt-a").expect("a usable salt");
        assert!(verify("correct horse", &record));
        assert!(!verify("correct horsf", &record));
        assert!(!verify("Correct horse", &record));
        assert!(!verify("", &record));
    }

    #[test]
    fn an_unusable_salt_is_refused() {
        assert!(hash("pw", "").is_none());
        assert!(hash("pw", "salt$a").is_none(), "the salt would split the record");
    }

    #[test]
    fn a_damaged_or_foreign_record_is_refused_without_a_panic() {
        let record = hash("pw", "salt-a").expect("a usable salt");
        let mut text = record.as_str().to_owned();
        let last = text.pop().expect("a record is not empty");
        let other = if last == '0' { '1' } else { '0' };
        let tampered = PasswordHash::parse(&format!("{text}{other}")).expect("still the format");
        assert!(!verify("pw", &tampered), "a flipped digest was accepted");

        for broken in [
            "v2$4096$salt-a$0123456789abcdef",
            "v1$4096$salt-a",
            "v1$notanumber$salt-a$0123456789abcdef",
            "v1$4096$$0123456789abcdef",
            "v1$4096$salt-a$0123",
            "v1$4096$salt-a$0123456789abcdef$x",
            "not even close",
        ] {
            assert!(PasswordHash::parse(broken).is_none(), "{broken}");
        }
    }

    #[test]
    fn an_old_cost_verifies_and_asks_to_be_rehashed() {
        // A record made before the cost was raised: the verifier has to use *this* record's cost,
        // and the caller has to learn that it is time to replace it.
        let salt = "old-salt";
        let text = format!("v1$1024${salt}${}", digest(1024, salt, "hunter2"));
        let old = PasswordHash::parse(&text).expect("a well-formed record");
        assert!(old.needs_rehash());
        assert!(verify("hunter2", &old));
        assert!(!verify("hunter3", &old));

        let current = hash("hunter2", salt).expect("a usable salt");
        assert!(!current.needs_rehash());
    }
}
