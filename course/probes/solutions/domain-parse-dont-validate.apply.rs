//! probe = "domain/parse-dont-validate@apply"
//! title = "Parse, don't validate"
//! hint = "A parser answers the canonical value or nothing — and the value it refuses is never quoted back."
//!
//! Step `mailer/1.10-domain-types` replaced "a `String` plus a `validate_email` call somewhere at
//! the edge" with two things: a type only the parser can build, and a parser that answers the
//! *canonical* value rather than a verdict. This probe re-does it for the two types the signup
//! route takes.
//!
//! The types and the limits are given. What you write is `parse` for each of them, and `Display` —
//! the tests check that `Display` and `as_str` agree, because two places printing two different
//! addresses is the same bug as storing two spellings of one.
//!
//! ## `Email`
//!
//! Canonical form: trimmed, and lower-cased as a whole — the database's `UNIQUE (email)` compares
//! bytes, so `Ada@Example.COM` and `ada@example.com` have to be one subscriber, and the parser is
//! where that decision is made once.
//!
//! | refused | `expected` |
//! | --- | --- |
//! | no `@`, or more than one | `EXPECT_ONE_AT` |
//! | a local part that is empty, over 64 bytes, holds anything outside letters, digits, `.`, `_`, `+` and `-`, starts or ends with `.`, or holds `..` | `EXPECT_LOCAL` |
//! | a domain that is empty, has no `.`, has an empty label, has a label starting or ending with `-`, or holds anything outside letters, digits and `-` | `EXPECT_DOMAIN` |
//! | an address longer than 254 bytes | `EXPECT_ADDRESS_LENGTH` |
//!
//! ## `SubscriberName`
//!
//! Canonical form: trimmed, with runs of spaces collapsed to one, and any letters — including
//! non-ASCII ones — kept as typed.
//!
//! | refused | `expected` |
//! | --- | --- |
//! | empty or only spaces, a control character anywhere, or more than 100 characters after the spaces are collapsed | `EXPECT_NAME` |
//!
//! ## The one rule that is not about the values
//!
//! [`Refused`] holds the field and what was expected, and **not** the value: the same parser is
//! handed passwords, and a refusal ends up in a log line. There is therefore no way to write a
//! failure that quotes what it refused — which is the point, and the reason the type is given
//! rather than left to you.

use std::fmt;

/// The largest address this service stores, in bytes.
pub const MAX_EMAIL_LENGTH: usize = 254;
/// The longest local part this service accepts, in bytes.
pub const MAX_EMAIL_LOCAL_LENGTH: usize = 64;
/// The longest name this service stores, in characters.
pub const MAX_NAME_LENGTH: usize = 100;

/// What an address with no `@`, or more than one, would have had to be.
pub const EXPECT_ONE_AT: &str = "exactly one `@`";
/// What a local part has to be.
pub const EXPECT_LOCAL: &str = "a local part of 1 to 64 bytes of letters, digits, `.`, `_`, `+` and `-`, with no dot at either end and no `..`";
/// What a domain has to be.
pub const EXPECT_DOMAIN: &str =
    "a domain of at least two labels of letters, digits and `-`, separated by dots";
/// What an address that is too long would have had to be.
pub const EXPECT_ADDRESS_LENGTH: &str = "an address of at most 254 bytes";
/// What a subscriber name has to be.
pub const EXPECT_NAME: &str = "1 to 100 characters with single spaces between words";

/// A value a parser refused.
///
/// The value itself is deliberately absent: it may be a password, and a message that quotes what it
/// refused is a password in a log line (step `mailer/1.10-domain-types`, TRAP 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// `"email"` or `"name"` — the field the caller has to correct.
    pub field: &'static str,
    /// What the value would have had to be, phrased for a message.
    pub expected: &'static str,
}

/// An address that went through the parser: the only way to make one is [`Email::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    /// Parse an address into its canonical form, or say what a usable one looks like.
    ///
    /// # Errors
    ///
    /// [`Refused`] with `field: "email"` and the constant from the table in this file's header.
    pub fn parse(input: &str) -> Result<Self, Refused> {
        let refused = |expected: &'static str| Refused {
            field: "email",
            expected,
        };

        // Trim and lower-case first: the database compares bytes, so `Ada@Example.COM` and
        // `ada@example.com` have to be one subscriber, and the parser is where that is decided once.
        let address = input.trim().to_lowercase();
        if address.len() > MAX_EMAIL_LENGTH {
            return Err(refused(EXPECT_ADDRESS_LENGTH));
        }
        let Some((local, domain)) = address.split_once('@') else {
            return Err(refused(EXPECT_ONE_AT));
        };
        if domain.contains('@') {
            return Err(refused(EXPECT_ONE_AT));
        }
        if !usable_local(local) {
            return Err(refused(EXPECT_LOCAL));
        }
        if !usable_domain(domain) {
            return Err(refused(EXPECT_DOMAIN));
        }
        Ok(Self(address))
    }

    /// The canonical form, as the database will store it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The local part: 1 to 64 bytes of letters, digits, `.`, `_`, `+` and `-`, with no dot at either
/// end and no `..` anywhere.
fn usable_local(local: &str) -> bool {
    (1..=MAX_EMAIL_LOCAL_LENGTH).contains(&local.len())
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character))
}

/// The domain: labels of letters, digits and `-`, separated by dots, none of them empty and none
/// starting or ending with a `-`.
fn usable_domain(domain: &str) -> bool {
    domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

impl fmt::Display for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A subscriber's name that went through the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriberName(String);

impl SubscriberName {
    /// Parse a name into its canonical form, or say what a usable one looks like.
    ///
    /// # Errors
    ///
    /// [`Refused`] with `field: "name"` and [`EXPECT_NAME`].
    pub fn parse(input: &str) -> Result<Self, Refused> {
        let refused = Refused {
            field: "name",
            expected: EXPECT_NAME,
        };

        // A control character is not a name: it is how a newline or a bell ends up in a log line.
        if input.chars().any(char::is_control) {
            return Err(refused);
        }

        // Trim, and collapse runs of spaces: `Ada   Lovelace` and `Ada Lovelace` are one person.
        let mut name = String::new();
        for word in input.split(' ').filter(|word| !word.is_empty()) {
            if !name.is_empty() {
                name.push(' ');
            }
            name.push_str(word);
        }
        if name.is_empty() || name.chars().count() > MAX_NAME_LENGTH {
            return Err(refused);
        }
        Ok(Self(name))
    }

    /// The canonical form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SubscriberName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One value a parser has to accept, and what it has to hold afterwards.
    struct Accepted {
        input: &'static str,
        stored: &'static str,
    }

    /// One value a parser has to refuse, and which rule it broke.
    struct Denied {
        input: String,
        expected: &'static str,
    }

    fn email(input: &str) -> Email {
        Email::parse(input).unwrap_or_else(|refused| {
            panic!("`{input}` has to be accepted: it was refused as {refused:?}")
        })
    }

    fn name(input: &str) -> SubscriberName {
        SubscriberName::parse(input).unwrap_or_else(|refused| {
            panic!("`{input}` has to be accepted: it was refused as {refused:?}")
        })
    }

    fn refuses_email(input: &str) -> Refused {
        Email::parse(input)
            .err()
            .unwrap_or_else(|| panic!("`{input}` has to be refused"))
    }

    fn refuses_name(input: &str) -> Refused {
        SubscriberName::parse(input)
            .err()
            .unwrap_or_else(|| panic!("`{input}` has to be refused"))
    }

    #[test]
    fn an_accepted_value_is_stored_in_one_canonical_form() {
        let addresses = [
            Accepted {
                input: "Ada@Example.COM",
                stored: "ada@example.com",
            },
            Accepted {
                input: "  grace@example.org  ",
                stored: "grace@example.org",
            },
            Accepted {
                input: "ada+newsletter@sub.example.co.uk",
                stored: "ada+newsletter@sub.example.co.uk",
            },
            Accepted {
                input: "x-y_z.1@ex-ample.com",
                stored: "x-y_z.1@ex-ample.com",
            },
        ];
        for Accepted { input, stored } in addresses {
            let address = email(input);
            assert_eq!(
                address.as_str(),
                stored,
                "`{input}` has to be stored as `{stored}`: the address the table holds is the one \
                 the parser produced, because that is the one the unique constraint compares"
            );
            assert_eq!(
                address.to_string(),
                stored,
                "`Display` and `as_str` have to agree, or two places print two addresses"
            );
            assert_eq!(
                email(address.as_str()),
                address,
                "parsing what was stored changes nothing: normalisation is a function with fixed \
                 points, not a thing that happens once"
            );
        }

        let names = [
            Accepted {
                input: "Ada Lovelace",
                stored: "Ada Lovelace",
            },
            Accepted {
                input: "  Ada   Lovelace  ",
                stored: "Ada Lovelace",
            },
            Accepted {
                input: "Ana María",
                stored: "Ana María",
            },
        ];
        for Accepted { input, stored } in names {
            let parsed = name(input);
            assert_eq!(
                parsed.as_str(),
                stored,
                "`{input}` has to be stored as `{stored}`"
            );
            assert_eq!(
                parsed.to_string(),
                stored,
                "`Display` and `as_str` have to agree"
            );
            assert_eq!(
                name(parsed.as_str()),
                parsed,
                "parsing a stored name changes nothing"
            );
        }
    }

    #[test]
    fn a_refused_address_says_which_rule_it_broke() {
        let long_local = format!("{}@example.com", "a".repeat(65));
        let long_address = format!("{}@{}.com", "a".repeat(60), "b".repeat(190));
        let denials = [
            Denied {
                input: String::new(),
                expected: EXPECT_ONE_AT,
            },
            Denied {
                input: "   ".to_owned(),
                expected: EXPECT_ONE_AT,
            },
            Denied {
                input: "not-an-address".to_owned(),
                expected: EXPECT_ONE_AT,
            },
            Denied {
                input: "ada@@example.com".to_owned(),
                expected: EXPECT_ONE_AT,
            },
            Denied {
                input: "@example.com".to_owned(),
                expected: EXPECT_LOCAL,
            },
            Denied {
                input: ".ada@example.com".to_owned(),
                expected: EXPECT_LOCAL,
            },
            Denied {
                input: "ada.@example.com".to_owned(),
                expected: EXPECT_LOCAL,
            },
            Denied {
                input: "ada..lovelace@example.com".to_owned(),
                expected: EXPECT_LOCAL,
            },
            Denied {
                input: "ada example@test.com".to_owned(),
                expected: EXPECT_LOCAL,
            },
            Denied {
                input: long_local,
                expected: EXPECT_LOCAL,
            },
            Denied {
                input: "ada@".to_owned(),
                expected: EXPECT_DOMAIN,
            },
            Denied {
                input: "ada@example".to_owned(),
                expected: EXPECT_DOMAIN,
            },
            Denied {
                input: "ada@.com".to_owned(),
                expected: EXPECT_DOMAIN,
            },
            Denied {
                input: "ada@example..com".to_owned(),
                expected: EXPECT_DOMAIN,
            },
            Denied {
                input: "ada@-example.com".to_owned(),
                expected: EXPECT_DOMAIN,
            },
            Denied {
                input: "ada@exa_mple.com".to_owned(),
                expected: EXPECT_DOMAIN,
            },
            Denied {
                input: "ada@例え.com".to_owned(),
                expected: EXPECT_DOMAIN,
            },
            Denied {
                input: long_address,
                expected: EXPECT_ADDRESS_LENGTH,
            },
        ];
        for denied in &denials {
            let refused = refuses_email(&denied.input);
            assert_eq!(
                refused,
                Refused {
                    field: "email",
                    expected: denied.expected,
                },
                "`{}` has to be refused for the right reason — every row here is a bug a real \
                 service has had",
                denied.input
            );
        }
    }

    #[test]
    fn a_refused_name_says_what_a_name_has_to_be() {
        let denials = [
            String::new(),
            "   ".to_owned(),
            "a".repeat(MAX_NAME_LENGTH + 1),
            "Ada\nLovelace".to_owned(),
            "Ada\tLovelace".to_owned(),
        ];
        for input in &denials {
            let refused = refuses_name(input);
            assert_eq!(
                refused,
                Refused {
                    field: "name",
                    expected: EXPECT_NAME,
                },
                "`{input:?}` has to be refused: a control character is not a name, and neither is an \
                 empty one"
            );
        }
    }

    #[test]
    fn the_limits_are_the_ones_this_file_documents() {
        assert_eq!(MAX_EMAIL_LENGTH, 254, "the address limit changed");
        assert_eq!(MAX_EMAIL_LOCAL_LENGTH, 64, "the local-part limit changed");
        assert_eq!(MAX_NAME_LENGTH, 100, "the name limit changed");

        // The boundaries are part of the contract: one byte over is where a database or a mail
        // server would refuse the row later, at the worst possible moment.
        let longest_local = format!("{}@example.com", "a".repeat(MAX_EMAIL_LOCAL_LENGTH));
        email(&longest_local);
        refuses_email(&format!(
            "{}@example.com",
            "a".repeat(MAX_EMAIL_LOCAL_LENGTH + 1)
        ));

        let longest_address = format!(
            "{}@{}.com",
            "a".repeat(60),
            "b".repeat(MAX_EMAIL_LENGTH - 60 - 1 - 4)
        );
        assert_eq!(longest_address.len(), MAX_EMAIL_LENGTH);
        email(&longest_address);

        let longest_name = "a".repeat(MAX_NAME_LENGTH);
        assert_eq!(
            name(&longest_name).as_str().chars().count(),
            MAX_NAME_LENGTH
        );
    }
}
