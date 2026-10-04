//! probe = "sec/hmac-signing@apply"
//! title = "The material, the window, and the tag that is not optional"
//! hint = "Sign `v1.<instant>.<delivery>.<body bytes>`, make the two instants one value, refuse a window on either side, and compare the tag without an early exit."
//!
//! Step `relay/2.11-hmac-signing` gives a webhook target a way to tell a real delivery from a
//! request anybody could have posted to a URL they read out of a log. The MAC is the primitive; the
//! policy around it is three sentences, and each one is a trap somebody falls into:
//!
//! 1. **the tag covers the bytes that were sent.** Signing a re-serialisation of the event produces
//!    a signature the receiver cannot reproduce, because JSON has many spellings for the same value
//!    and the one that arrived is the one that matters. [`signing_string`] takes `&[u8]` for that
//!    reason;
//! 2. **the tag is bound to the request it is for, and it expires.** The instant and the delivery id
//!    are inside the signed material, so a captured request is not a bearer token and a delivery id
//!    cannot be swapped. The instant appears twice — in `x-relay-timestamp` and inside the signature
//!    — and a receiver that checks one and hashes the other has a hole exactly the size of a replay,
//!    so this one refuses when they disagree;
//! 3. **the comparison does not stop early.** A tag comparison that returns at the first wrong byte
//!    tells the other end how much of a guess was right, one byte at a time, and a signature is
//!    exactly the value an attacker gets to guess at with as many requests as they like. This is the
//!    one place in the service where `a == b` is the wrong line of code.
//!
//! ## What you write
//!
//! * [`signing_string`] — the material: `v1.<timestamp>.<delivery>.<body bytes>`, as bytes;
//! * [`sign`] — the header the adapter puts in the request;
//! * [`verify`] — the receiver's half, in the order a receiver reads its own log line;
//! * [`tags_match`] — the comparison, folding rather than returning early.
//!
//! The [`Mac`] trait, [`Secret`] and its redacting `Debug`, the [`SignatureError`] variants and the
//! parsing of the signature's four parts are given: the first is the injection point (see below),
//! the second is a rule about log lines rather than about cryptography, and the rest is string
//! handling.
//!
//! ## What the tests ask
//!
//! The signed material is compared to the exact string `v1.1760000000.<delivery>.<body>` — and to a
//! different one for a body that is the same JSON spelled differently. The same input signs the same
//! header every time. Both edges of the window are inside it and one second past either edge is not.
//! A header the receiver cannot read is refused before any tag is compared, an instant that does not
//! match the one inside the signature is refused as that rather than as a bad tag, a signature for
//! one delivery does not verify as another, a tampered body is refused, and every one-character
//! mutation of a tag fails the same way — which is what a test can ask of a comparison whose
//! *timing* it cannot measure.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **the MAC is not in it**. No `hmac`, no `sha2`, no `subtle`:
//! re-checking HMAC-SHA256 from memory is not what a three-minute probe is for, and a hand-rolled
//! hash in a course file is worse than no hash at all. The MAC is injected instead, which is what
//! makes the exercise about everything *around* the tag — and that is where the step's traps live.
//! Two consequences, stated rather than hidden: the golden vector here is the signed *material* (the
//! bytes, which are this service's format) rather than the tag, which belongs to a real MAC; and
//! [`tags_match`] is checked for the failure it must have on every mutation, because whether it
//! returns early is a property of the code rather than something a test in this file can time. The
//! real step answers that with `subtle::ConstantTimeEq`, and the shape of the check there is the
//! same one as here.

use std::fmt;

/// The version this service signs, and the only one a receiver accepts. It is inside the signed
/// material so that it cannot be edited in transit.
pub const VERSION: &str = "v1";

/// How long a signature is good for, in the window the service ships with. A receiver on the other
/// side of a clock skew needs more than an instant.
pub const MAX_AGE_S: i64 = 300;

/// The MAC this service computes a tag with.
///
/// In the service it is `hmac::Hmac<Sha256>`; in `relay/2.11` the adapter builds one from the
/// endpoint's secret. It is a parameter here so that this file is about the policy around the tag
/// rather than about a hash — see the note at the top.
pub trait Mac {
    /// The tag of `message` under `key`, as lower-case hex.
    fn tag(&self, key: &[u8], message: &[u8]) -> String;
}

/// The key both ends know.
///
/// `Debug` is written by hand: a type that prints its secret puts that secret in a log aggregator,
/// a bug report and a screen share, and a log line is where a webhook secret most often escapes.
pub struct Secret(String);

impl Secret {
    /// The secret this endpoint was given.
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self(text.to_owned())
    }

    /// The bytes a MAC is keyed with.
    #[must_use]
    pub fn key(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Why a signature was refused.
///
/// The order the variants are checked in is the receiver's reading order, and it is what makes a
/// refusal name one thing: first the shape and the version, then the two instants made one value,
/// then the window, then the delivery, and only then the tag. A tag is compared once there is
/// something to compare it against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    /// No header, a version this receiver does not know, a field that is not a number, or a
    /// signature that is not four parts.
    Malformed,
    /// The instant in `x-relay-timestamp` and the one inside the signature are not the same value.
    TimestampMismatch {
        /// What the header said.
        header: i64,
        /// What the signature was made over.
        signed: i64,
    },
    /// Older than the window.
    TooOld {
        /// How far past it, in seconds.
        age_s: i64,
    },
    /// Further ahead than the window allows: a receiver's clock may be behind as well as ahead.
    InTheFuture {
        /// How far ahead, in seconds.
        ahead_s: i64,
    },
    /// A valid signature, for a different delivery. A tag that is not bound to the delivery is a
    /// bearer token.
    DeliveryMismatch {
        /// The delivery the receiver is checking.
        expected: String,
        /// The delivery the signature was made over.
        signed: String,
    },
    /// The tag does not match, for any reason: the body, the secret, or a guess.
    BadSignature,
}

/// What the receiver has in front of it: the two headers, the bytes that arrived, and the delivery
/// it is checking.
#[derive(Debug, Clone, Copy)]
pub struct Received<'a> {
    /// The `x-relay-timestamp` header, as it arrived.
    pub timestamp: Option<&'a str>,
    /// The `x-relay-signature` header, as it arrived.
    pub signature: Option<&'a str>,
    /// The body, byte for byte.
    pub body: &'a [u8],
    /// The delivery this request claims to be.
    pub delivery: &'a str,
}

/// What a verified request is: the two facts the receiver acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// The instant the signature was made over, now known to be the one in the header too.
    pub timestamp: i64,
    /// The delivery it was made for.
    pub delivery: String,
}

/// The bytes a tag is computed over: `v1.<timestamp>.<delivery>.<body bytes>`.
///
/// Bytes rather than a value, and the body appended rather than formatted: the receiver hashes the
/// bytes that arrived, and the only spelling of the body that both ends agree on is the one that
/// went on the wire.
#[must_use]
pub fn signing_string(timestamp: i64, delivery: &str, body: &[u8]) -> Vec<u8> {
    todo!()
}

/// Sign one request: `v1.<timestamp>.<delivery>.<tag>`, which is what `x-relay-signature` carries.
///
/// The instant is a parameter rather than `Utc::now()` inside the function, for the reason every
/// clock in this course is injected: a function that reads a clock cannot be judged without one.
#[must_use]
pub fn sign<M: Mac + ?Sized>(
    mac: &M,
    secret: &Secret,
    timestamp: i64,
    delivery: &str,
    body: &[u8],
) -> String {
    todo!()
}

/// Verify a signature: the receiver's half.
///
/// The checks are in the order a receiver reads its own log line, and the order is part of the
/// answer — a stale request whose delivery also does not match is stale:
///
/// 1. is this a version and a shape this receiver knows, and is the header's instant a number;
/// 2. is the instant inside the signature the same value as the one in the header — two instants
///    checked once is a hole exactly the size of a replay;
/// 3. is it inside the window, on both sides, with both edges inside;
/// 4. is the signature for *this* delivery;
/// 5. and only then: is the tag the one this material produces.
///
/// # Errors
///
/// One of the six [`SignatureError`]s, in that order.
pub fn verify<M: Mac + ?Sized>(
    mac: &M,
    secret: &Secret,
    received: &Received<'_>,
    now: i64,
    window_s: i64,
) -> Result<Verified, SignatureError> {
    todo!()
}

/// Do these two tags match?
///
/// Folded rather than returned from early: the loop looks at every byte of both tags whatever it
/// found so far, and the answer is read once at the end. A comparison that stops at the first
/// difference tells the other end how much of a guess was right, one byte at a time.
///
/// The length is compared first, and that is not the leak that matters: a tag's length is the
/// algorithm's rather than the secret's, and a comparison that pads or truncates is a comparison
/// that accepts two different things.
#[must_use]
pub fn tags_match(expected: &str, provided: &str) -> bool {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MAC this file is given instead of `hmac::Hmac<Sha256>`: FNV-1a over `key || message`,
    /// as sixteen lower-case hex characters.
    ///
    /// It is not a MAC anybody should ship — it is not even a hash anybody should ship. It is a
    /// stand-in that makes every test below about the policy around the tag, and it is deterministic
    /// so that a golden header is a string this file can state.
    struct FakeMac;

    impl Mac for FakeMac {
        fn tag(&self, key: &[u8], message: &[u8]) -> String {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in key.iter().chain(message) {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            format!("{hash:016x}")
        }
    }

    const SECRET: &str = "whsec_2f8a1c";
    const DELIVERY: &str = "dlv_01J9Z4M6";
    const NOW: i64 = 1_760_000_100;

    /// The event as the API answers with it. A webhook's body is somebody's JSON, and the bytes that
    /// matter are the ones that went on the wire.
    fn body() -> Vec<u8> {
        BODY.to_vec()
    }

    /// The same bytes, as the fixture below borrows them: `body()` hands out a fresh copy, and a
    /// [`Received`] holds a slice.
    const BODY: &[u8] = br#"{"id":"dlv_01J9Z4M6","kind":"a"}"#;

    /// The same value, spelled differently. To everybody who hashes bytes that is a different body.
    const RESPELLED: &[u8] = br#"{ "id": "dlv_01J9Z4M6", "kind": "a" }"#;

    /// One byte of the event changed, which is the tamper a receiver exists to catch.
    const TAMPERED: &[u8] = br#"{"id":"dlv_01J9Z4M6","kind":"b"}"#;

    /// One signed request, as the adapter would send it.
    fn signed() -> (String, String) {
        let header = (NOW - 10).to_string();
        let signature = sign(&FakeMac, &Secret::new(SECRET), NOW - 10, DELIVERY, &body());
        (header, signature)
    }

    fn received<'a>(timestamp: &'a str, signature: &'a str) -> Received<'a> {
        Received {
            timestamp: Some(timestamp),
            signature: Some(signature),
            body: BODY,
            delivery: DELIVERY,
        }
    }

    #[test]
    fn the_signed_material_is_the_version_the_instant_the_delivery_and_the_bytes() {
        let material = signing_string(1_760_000_000, DELIVERY, &body());
        assert_eq!(
            String::from_utf8(material).expect("the material is text plus a body"),
            format!(
                "v1.1760000000.{DELIVERY}.{}",
                String::from_utf8_lossy(&body())
            ),
            "the golden material: a receiver in another language checks its implementation \
             against this, not against this one"
        );

        let reserialised = RESPELLED;
        assert_ne!(
            signing_string(1_760_000_000, DELIVERY, &body()),
            signing_string(1_760_000_000, DELIVERY, reserialised),
            "the same JSON spelled differently is different bytes, and the receiver hashes the \
             bytes that arrived"
        );
        assert!(
            signing_string(1_760_000_000, DELIVERY, &body())
                .starts_with(b"v1.1760000000.dlv_01J9Z4M6."),
            "the fields are joined in the order the format says, and the delivery id comes before \
             the body"
        );
        assert!(
            signing_string(1_760_000_000, DELIVERY, &body()).ends_with(BODY),
            "the body is appended, not encoded into the string"
        );
    }

    #[test]
    fn the_same_input_signs_the_same_header_every_time() {
        let (_, signature) = signed();
        assert_eq!(
            signature,
            signed().1,
            "a signature nobody can reproduce is not one"
        );
        assert!(
            signature.starts_with(&format!("v1.{}.{DELIVERY}.", NOW - 10)),
            "the header is the material with the body replaced by the tag: {signature}"
        );
        assert_eq!(
            signature.rsplit_once('.').expect("a tag").1.len(),
            16,
            "the tag is the sole output of the MAC, and nothing else is appended to it"
        );

        assert_ne!(
            sign(
                &FakeMac,
                &Secret::new(SECRET),
                NOW - 10,
                "dlv_other",
                &body()
            ),
            signature,
            "a signature for one delivery is not a signature for another"
        );
        assert_ne!(
            sign(&FakeMac, &Secret::new(SECRET), NOW, DELIVERY, &body()),
            signature,
            "the instant is inside the material, or a captured request never expires"
        );
        assert_ne!(
            sign(
                &FakeMac,
                &Secret::new("whsec_other"),
                NOW - 10,
                DELIVERY,
                &body()
            ),
            signature
        );
    }

    #[test]
    fn a_receiver_accepts_a_signature_inside_its_window_and_both_edges_are_inside() {
        let secret = Secret::new(SECRET);
        let body = body();
        let at = |timestamp: i64| {
            let signature = sign(&FakeMac, &secret, timestamp, DELIVERY, &body);
            let header = timestamp.to_string();
            verify(
                &FakeMac,
                &secret,
                &Received {
                    timestamp: Some(&header),
                    signature: Some(&signature),
                    body: &body,
                    delivery: DELIVERY,
                },
                NOW,
                MAX_AGE_S,
            )
        };

        assert_eq!(
            at(NOW - 10),
            Ok(Verified {
                timestamp: NOW - 10,
                delivery: DELIVERY.to_owned()
            })
        );
        assert_eq!(
            at(NOW - MAX_AGE_S),
            Ok(Verified {
                timestamp: NOW - MAX_AGE_S,
                delivery: DELIVERY.to_owned()
            }),
            "the edge of the window is inside it"
        );
        assert_eq!(
            at(NOW + MAX_AGE_S),
            Ok(Verified {
                timestamp: NOW + MAX_AGE_S,
                delivery: DELIVERY.to_owned()
            }),
            "clocks disagree in both directions, and the future edge is inside for the same reason"
        );
    }

    #[test]
    fn a_stale_or_future_instant_is_refused_with_how_far_off_it_was() {
        let secret = Secret::new(SECRET);
        let body = body();
        let at = |timestamp: i64| {
            let signature = sign(&FakeMac, &secret, timestamp, DELIVERY, &body);
            let header = timestamp.to_string();
            verify(
                &FakeMac,
                &secret,
                &Received {
                    timestamp: Some(&header),
                    signature: Some(&signature),
                    body: &body,
                    delivery: DELIVERY,
                },
                NOW,
                MAX_AGE_S,
            )
        };

        assert_eq!(
            at(NOW - MAX_AGE_S - 1),
            Err(SignatureError::TooOld {
                age_s: MAX_AGE_S + 1
            })
        );
        assert_eq!(
            at(NOW + MAX_AGE_S + 1),
            Err(SignatureError::InTheFuture {
                ahead_s: MAX_AGE_S + 1
            })
        );
    }

    #[test]
    fn a_header_the_receiver_cannot_read_is_refused_before_any_tag_is_compared() {
        let secret = Secret::new(SECRET);
        let (header, signature) = signed();
        let tag = signature.rsplit_once('.').expect("a tag").1.to_owned();

        assert_eq!(
            verify(
                &FakeMac,
                &secret,
                &Received {
                    timestamp: None,
                    ..received(&header, &signature)
                },
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::Malformed),
            "a request with no instant at all is not a request with a wrong one"
        );
        assert_eq!(
            verify(
                &FakeMac,
                &secret,
                &Received {
                    signature: None,
                    ..received(&header, &signature)
                },
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::Malformed),
            "nor is one with no signature"
        );

        let wrong = [
            format!("v2.{}.{DELIVERY}.{tag}", NOW - 10),
            format!("v1.{}.{DELIVERY}", NOW - 10),
            format!("v1.not-a-number.{DELIVERY}.{tag}"),
            format!("v1.{}.{DELIVERY}.{tag}.extra", NOW - 10),
            VERSION.to_owned(),
        ];
        for signature in wrong {
            assert_eq!(
                verify(
                    &FakeMac,
                    &secret,
                    &received(&header, &signature),
                    NOW,
                    MAX_AGE_S
                ),
                Err(SignatureError::Malformed),
                "`{signature}` is not a signature this receiver reads"
            );
        }

        assert_eq!(
            verify(
                &FakeMac,
                &secret,
                &received("not-a-number", &signature),
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::Malformed),
            "the instant header is a number or it is nothing"
        );
    }

    #[test]
    fn the_instant_in_the_header_and_the_one_inside_the_signature_have_to_be_one_value() {
        let (_, signature) = signed();
        let moved = (NOW - 9).to_string();

        assert_eq!(
            verify(
                &FakeMac,
                &Secret::new(SECRET),
                &received(&moved, &signature),
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::TimestampMismatch {
                header: NOW - 9,
                signed: NOW - 10
            }),
            "a receiver that checks the header and hashes a different instant has a hole exactly \
             the size of a replay"
        );
    }

    #[test]
    fn a_signature_is_for_one_delivery_and_not_for_another() {
        let (header, signature) = signed();
        assert_eq!(
            verify(
                &FakeMac,
                &Secret::new(SECRET),
                &Received {
                    timestamp: Some(&header),
                    signature: Some(&signature),
                    body: BODY,
                    delivery: "dlv_somebody_elses",
                },
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::DeliveryMismatch {
                expected: "dlv_somebody_elses".to_owned(),
                signed: DELIVERY.to_owned()
            }),
            "without the delivery inside the material a signature is a bearer token"
        );
    }

    #[test]
    fn a_tampered_body_is_refused_even_though_the_headers_are_untouched() {
        let (header, signature) = signed();

        assert_eq!(
            verify(
                &FakeMac,
                &Secret::new(SECRET),
                &Received {
                    timestamp: Some(&header),
                    signature: Some(&signature),
                    body: TAMPERED,
                    delivery: DELIVERY,
                },
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::BadSignature),
            "one byte of the body changed and the tag no longer covers what arrived"
        );
        assert_eq!(
            verify(
                &FakeMac,
                &Secret::new(SECRET),
                &Received {
                    timestamp: Some(&header),
                    signature: Some(&signature),
                    body: RESPELLED,
                    delivery: DELIVERY,
                },
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::BadSignature),
            "the same JSON spelled differently is a body nobody signed"
        );
        assert_eq!(
            verify(
                &FakeMac,
                &Secret::new("whsec_wrong"),
                &received(&header, &signature),
                NOW,
                MAX_AGE_S
            ),
            Err(SignatureError::BadSignature),
            "somebody who does not hold the key cannot produce the tag"
        );
    }

    #[test]
    fn every_one_character_mutation_of_a_tag_fails_the_same_way() {
        let secret = Secret::new(SECRET);
        let (header, signature) = signed();
        let (prefix, tag) = signature.rsplit_once('.').expect("a tag");
        let digits = b"0123456789abcdef";

        let mut tried = 0;
        for index in 0..tag.len() {
            for digit in digits {
                if tag.as_bytes()[index] == *digit {
                    continue;
                }
                let mut mutated = tag.as_bytes().to_vec();
                mutated[index] = *digit;
                let mutated = format!("{prefix}.{}", String::from_utf8_lossy(&mutated));
                assert_eq!(
                    verify(
                        &FakeMac,
                        &secret,
                        &received(&header, &mutated),
                        NOW,
                        MAX_AGE_S
                    ),
                    Err(SignatureError::BadSignature),
                    "`{mutated}` differs from the real tag at one character and has to fail \
                     exactly as the others do"
                );
                tried += 1;
            }
        }
        assert_eq!(tried, tag.len() * 15);

        for wrong in ["", "00", &tag[..tag.len() - 1], &format!("{tag}0")] {
            assert!(
                !tags_match(&tag, wrong),
                "a tag of another length is not a close guess"
            );
        }
        assert!(tags_match(&tag, &tag), "the real tag still matches itself");
    }
}
