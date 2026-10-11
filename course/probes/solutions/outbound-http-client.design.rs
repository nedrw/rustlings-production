//! probe = "outbound/http-client@design"
//! title = "The canonical form, the signed-header list, and who may retry"
//! hint = "Build the signed text from the request's own fields, lower-case and sort the header names before you sign them, and spend no second attempt on a 403."
//!
//! Step `lakelet/4.21-cold-tiering` is where an HTTP client stops being plumbing: `relay/2.7`'s client sent bytes and classified an answer, and this one is asked for a request that is *signed*. What `relay/2.11-hmac-signing` argued about a webhook's tag is what an S3-shaped endpoint argues about every request, and the step's half of it is the *canonical form*: the exact text a signature is computed over.
//!
//! 1. **the canonical form is built from the request that is sent.** The method, the path, the query as pairs, the headers and the body are one value, and the signed text is rendered from that value. The alternative — rendering the request on the wire and signing a re-serialisation of it — agrees until somebody adds a header or encodes a `/`, and then it fails as a `403` whose message says the signature did not match and nothing about which of the fifteen lines differed;
//! 2. **the signed-header list is derived from the request's own headers.** Names are lower-cased, sorted and de-duplicated *before* anything is signed, so two spellings of one header are one signature and two orders of the same headers are one signature. A list a caller maintained by hand beside the request stops matching it the day somebody adds a header — and a header that is sent and not signed, or signed and not sent, is a `403` from an endpoint that checked both directions;
//! 3. **the body's hash is inside the signed text.** A payload hash that travelled next to the signature instead of inside it would let a body move under a valid signature; here the hash of the body that is sent is one of the lines that is signed, so a body of a different length is a different signature. That is the property a signed request has over an unsigned one, and it is the whole reason a `put` is worth repeating: its key is the identity of what it wrote;
//! 4. **a status class is a value, and a retry is a count.** `2xx` answered, `404` absent (absence is a value: a `get` answers `None`), `5xx` retryable, every other `4xx` refused. A `403` is in that last group, and it is the one an operator is sent for: another attempt signs the same bytes and gets the same answer, so a retry spent on it is an attempt the next request needs. Nothing here reads a clock — no deadline, no backoff, no jitter — because a policy two machines disagree about is not a policy a tier can carry out.
//!
//! ## What you write
//!
//! * [`Config::scope`] — the credential scope: which day, which region, which service;
//! * [`signed_headers`] — every header the request carries except the signature, lower-cased and sorted;
//! * [`canonical`] — the exact text that gets signed: the method, the canonical path, the canonical query, the signed-header block, the signed-header list and the payload hash, in that order;
//! * [`sign`] — the string to sign and the tag over it, as lower-case hex;
//! * [`classify`] — the four words a caller acts on, and [`Class::attempts`] — how many times this client sends a request whose first answer was this one.
//!
//! [`Request`], [`Config`], [`Class`], [`encoded`] (the percent-encoding both the signature and the wire use), [`element`] (the endpoint's own words out of a refusal body), [`digest`], [`mac`], [`hex`] and [`signing_key`] are given. The MAC is given rather than written because the shape of a keyed MAC is `relay/2.11`'s lesson and a re-check of it here would be a re-teach; what is *not* given is the text it is applied to, which is this step's decision.
//!
//! ## What the tests ask
//!
//! The canonical request, byte for byte, for one request written out in full. The same request with its headers in another order, in another case and with one of them repeated signs the same bytes, and the signed-header list is the normalised one. A header added to a signed request changes the signature, and an `authorization` header already on it does not — the two directions of "the list is the request's own headers". One byte more of body is a different signature, and the same request signs the same bytes twice. The scope is the date stamp, the region, the service and `aws4_request`, pinned; the signature over the vendor's own scope is pinned too, so a case cannot pass by comparing the function with itself. And the four words: a `2xx`, a `404` about a key (absent) against a `404` about the bucket (not absent), a `5xx` retried `ATTEMPTS` times, and a `403` sent exactly once — the one retry this file exists to refuse.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies and **no socket**. The step's client writes HTTP/1.1 over a `TcpStream`, blocks on a runtime it owns, sends a request `ATTEMPTS` times, reads a body to the `content-length` it promised, and refuses `https` and a body with no length at all — every one of those is about bytes on a wire, and a probe that opened a socket would be checking this machine rather than the client. There is no sha256 and no HMAC here either: the crate-free stand-in below is a *cheap deterministic* digest, documented as one, and the MAC is the shape (`ipad`/`opad`, an inner pass and an outer one) over it. The consequence is stated rather than hidden: the pinned signature in this file is *this* file's, not the vendor's, while the step's own spec pins the request the vendor's SigV4 documentation signs. What is re-checked here is the same thing that case re-checks — that one value is both what is signed and what is sent, that the header list cannot drift from the request, and that the payload hash is inside the text.
//!
//! Missing on purpose, beyond the socket: the retry loop itself (a count is checked, not a loop), the `x-amz-content-sha256` header being set from the same function that the signature hashes (both are given here), the operator-facing refusal message that quotes the scope (that is wording, and it belongs to the step's `s3.rs`), and the connection, keep-alive and `content-length` machinery `outbound/http-client@apply` already re-checks.

use std::fmt;

/// How many times one request is sent before the client gives up.
///
/// Three, and it is a *count*: no deadline, no backoff, no jitter. What makes repeating safe is the
/// caller's business — a `put` repeats because its key is the identity of what it writes, a `get`
/// and a `list` repeat because they change nothing, and a `delete` repeats because it is idempotent.
pub const ATTEMPTS: u32 = 3;

/// The signing algorithm, as the `authorization` header spells it.
pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// One request: what is signed, and what goes on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    /// A request with no query, no headers and no body.
    #[must_use]
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_owned(),
            path: path.to_owned(),
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// The same request, with one query parameter.
    #[must_use]
    pub fn with_query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    /// The same request, with one more header.
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// The same request, carrying `bytes` as its body.
    #[must_use]
    pub fn with_body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = bytes.into();
        self
    }

    /// The method, as the request line spells it.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The path, as it was given and not yet encoded.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The query, as pairs: the canonical form sorts them, and a string that arrived pre-sorted
    /// cannot be checked by anything.
    #[must_use]
    pub fn query_pairs(&self) -> &[(String, String)] {
        &self.query
    }

    /// The headers, in the order they were added.
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The body, byte for byte.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// One header's value, matched without regard to case.
    #[must_use]
    pub fn header_of(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(found, _)| found.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The hex digest of this request's own body: what this request names its payload by.
    ///
    /// Given, because it is the one line the header and the signature have to agree about rather
    /// than a decision: the step's client sets `x-amz-content-sha256` from this function *and* the
    /// canonical request calls it.
    #[must_use]
    pub fn payload_hash(&self) -> String {
        hex(&digest(&self.body))
    }
}

/// The configuration a signature is built from: the endpoint's identity and a date that is a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `host:port` the request is sent to.
    pub endpoint: String,
    /// The bucket the operations address.
    pub bucket: String,
    /// The region the signature is scoped to.
    pub region: String,
    /// The service the signature is scoped to.
    pub service: String,
    /// The instant, as `20150830T123600Z`: `x-amz-date`, the date stamp and the signature all come
    /// from it, so nothing in a tier reads a clock and a pinned vector is a test rather than a flake.
    pub date: String,
    /// The access key the credential names.
    pub access_key: String,
    /// The secret the signing key is derived from.
    pub secret_key: String,
}

impl Config {
    /// The date stamp the credential scope starts with: the instant's own first eight characters.
    ///
    /// Given: it is a slice, not a decision.
    #[must_use]
    pub fn stamp(&self) -> String {
        self.date.get(..8).unwrap_or_default().to_owned()
    }

    /// The credential scope: `date/region/service/aws4_request`.
    ///
    /// The four parts are the four things a signature is *bound* to, which is why an endpoint
    /// quotes the scope back when it refuses one: a request signed for another day, another region
    /// or another service is a request nobody accepts.
    #[must_use]
    pub fn scope(&self) -> String {
        format!(
            "{}/{}/{}/aws4_request",
            self.stamp(),
            self.region,
            self.service
        )
    }
}

/// What a caller has to do about one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// The request was carried out; the body is the answer.
    Answered,
    /// There is no such object. Absence is a value: a `get` answers `None`, and a `delete` of
    /// something that is gone has reached the state it asked for.
    Absent,
    /// The endpoint refused, and repeating the request gets the same refusal.
    Refused,
    /// The endpoint could not answer, and the request may be sent again.
    Retryable,
}

impl Class {
    /// The word this class is spelled with in a report.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::Absent => "absent",
            Self::Refused => "refused",
            Self::Retryable => "retryable",
        }
    }

    /// How many times this client sends a request whose first answer was this one.
    ///
    /// A `403` retried is the bug this number exists to refuse: another attempt signs the same
    /// bytes and gets the same answer, and every attempt spent on it is an attempt the next
    /// request needs.
    #[must_use]
    pub const fn attempts(self) -> u32 {
        match self {
            Self::Retryable => ATTEMPTS,
            Self::Answered | Self::Absent | Self::Refused => 1,
        }
    }
}

/// Every header the request carries except the signature, lower-cased and sorted.
///
/// The list a signature covers is derived from the request rather than maintained beside it, and
/// the normalisation is what makes two orders of the same headers one signature and two spellings
/// of one header one signature.
#[must_use]
pub fn signed_headers(request: &Request) -> Vec<String> {
    let mut names: Vec<String> = request
        .headers()
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .filter(|name| name != "authorization")
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The exact text a signature is computed over: one value, from the request that is sent.
///
/// The shape is SigV4's: the method, the canonical path, the canonical query, the signed headers
/// one per line as `name:value`, an empty line, the signed-header list joined with `;`, and the
/// hex digest of the body that is sent. The payload hash is *this request's own*, so a body that
/// changes after it was hashed changes the signature with it.
#[must_use]
pub fn canonical(request: &Request) -> String {
    let uri = request
        .path()
        .split('/')
        .map(encoded)
        .collect::<Vec<String>>()
        .join("/");
    let mut pairs: Vec<(String, String)> = request.query_pairs().to_vec();
    pairs.sort();
    let query = pairs
        .iter()
        .map(|(name, value)| format!("{}={}", encoded(name), encoded(value)))
        .collect::<Vec<String>>()
        .join("&");
    let names = signed_headers(request);
    let headers = names
        .iter()
        .map(|name| {
            let value = request.header_of(name).unwrap_or_default().trim();
            format!("{name}:{value}\n")
        })
        .collect::<Vec<String>>()
        .join("");
    format!(
        "{}\n{uri}\n{query}\n{headers}\n{}\n{}",
        request.method(),
        names.join(";"),
        request.payload_hash()
    )
}

/// The signature over `request`: the string to sign, and the tag over it, as lower-case hex.
///
/// The string to sign names the algorithm, the instant, the scope and the *digest of the canonical
/// request* — in that order — and the tag is the keyed MAC of that text under
/// [`signing_key`]. A signature computed over a re-serialisation of the request, or beside the
/// payload hash rather than over it, is a signature the endpoint cannot reproduce.
#[must_use]
pub fn sign(config: &Config, request: &Request) -> String {
    let to_sign = format!(
        "{ALGORITHM}\n{}\n{}\n{}",
        config.date,
        config.scope(),
        hex(&digest(canonical(request).as_bytes()))
    );
    hex(&mac(&signing_key(config), to_sign.as_bytes()))
}

/// What a caller has to do about an answer with this status and this body.
///
/// `2xx` is an answer, `500..=599` is the endpoint failing to answer, and every other status is the
/// endpoint answering *no* — with one reading that takes the body seriously: a `404` whose code is
/// anything but a missing key is not an absent object, it is a request for something that is not
/// there at all, and reporting it as absence is how a tier reports a whole store as gone.
#[must_use]
pub fn classify(status: u16, body: &str) -> Class {
    match status {
        200..=299 => Class::Answered,
        404 => match element(body, "Code").as_deref() {
            None | Some("NoSuchKey") => Class::Absent,
            Some(_) => Class::Refused,
        },
        500..=599 => Class::Retryable,
        _ => Class::Refused,
    }
}

/// `text` with everything that is not unreserved percent-encoded, upper case, byte for byte.
///
/// Given: the same encoding is used for the signature and for the wire, so a key with a space or a
/// `+` in it is one request rather than two.
#[must_use]
pub fn encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The text inside the first `<tag>…</tag>` of `text`, or `None`.
///
/// Given: an endpoint that refuses explains itself in its body as a small XML document, and the
/// `<Code>` element is the one line of it a caller can act on.
#[must_use]
pub fn element(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let rest = text.get(start..)?;
    let end = rest.find(&close)?;
    rest.get(..end).map(str::to_owned)
}

/// bytes as lower-case hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A cheap deterministic digest of `bytes`: **not** sha256, and it says so.
///
/// A probe has no `sha2` and no `hmac`, and `relay/2.11-hmac-signing`'s file made the same trade
/// for the same reason: re-checking a hash from memory is not what three minutes are for, and a
/// hand-rolled *cryptographic* hash in a course file would be worse than none. What is here is a
/// function of the bytes that changes when they change, in thirty-two bytes so that the code
/// around it — the hex, the MAC, the key schedule — has the shape the step's has. Nothing in this
/// file is safe against an attacker and nothing here claims to be.
#[must_use]
pub fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (lane, word) in out.chunks_mut(8).enumerate() {
        let mut state: u64 =
            0xcbf2_9ce4_8422_2325 ^ (lane as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for byte in bytes {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        word.copy_from_slice(&state.to_le_bytes());
    }
    out
}

/// The keyed MAC of `message` under `key`: a keyed hash with a MAC's shape.
///
/// Given, and deliberately the *shape* rather than the primitive: the key is folded through an
/// inner pad and an outer one with the message between them, which is the HMAC construction, over
/// [`digest`] rather than over sha256. What the shape buys is what a signature needs — two keys
/// over one message give two tags, and one key over two messages gives two tags — and what it does
/// not buy is anything an attacker cannot undo.
#[must_use]
pub fn mac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut inner: Vec<u8> = key.iter().map(|byte| byte ^ 0x36).collect();
    inner.extend_from_slice(message);
    let inner = digest(&inner);
    let mut outer: Vec<u8> = key.iter().map(|byte| byte ^ 0x5c).collect();
    outer.extend_from_slice(&inner);
    digest(&outer)
}

/// The signing key: the secret folded through the date, the region, the service and the request
/// type.
///
/// Given: four taps of the same MAC, which is the shape the track already argued in
/// `relay/2.11-hmac-signing`. Each step shrinks what a leaked key is worth — a key for one day is
/// not a key for another — and the key never changes while one config is in use, which is why
/// [`Config`]'s fields are values.
#[must_use]
pub fn signing_key(config: &Config) -> [u8; 32] {
    let secret = format!("AWS4{}", config.secret_key);
    let date = mac(secret.as_bytes(), config.stamp().as_bytes());
    let region = mac(&date, config.region.as_bytes());
    let service = mac(&region, config.service.as_bytes());
    mac(&service, b"aws4_request")
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key and secret the tier's fixture signs with.
    const ACCESS: &str = "AKIDLAKELETTIER4X21";
    const SECRET: &str = "lakelet-tier-fixture-secret";
    const DATE: &str = "20261008T000000Z";

    /// The stand-in digest's hex for the body `b"lakelet"`, written down rather than derived.
    const LAKELET_HASH: &str = "a7d444cca95cb6c47c4b9b28239698ec791fe3bb4c388358ae966c3f6643d394";

    /// The signature this file produces for [`put`] under [`config`]. Pinned, so that the case is a
    /// check of the canonical form and the key schedule rather than a comparison of [`sign`] with
    /// itself.
    const SIGNATURE: &str = "3609a0af86971467634d5fd44c09cac1449d1e9bab1fcb0391fbb6e8456fe86d";

    fn config() -> Config {
        Config {
            endpoint: "tier.internal:9000".to_owned(),
            bucket: "lakelet-tier".to_owned(),
            region: "eu-central-1".to_owned(),
            service: "s3".to_owned(),
            date: DATE.to_owned(),
            access_key: ACCESS.to_owned(),
            secret_key: SECRET.to_owned(),
        }
    }

    /// The `x-amz-content-sha256` a request with this body carries.
    fn hash_of(bytes: &[u8]) -> String {
        Request::new("PUT", "/x")
            .with_body(bytes.to_vec())
            .payload_hash()
    }

    /// The request a `put` of one segment makes, as the tier's own client builds it: the host, the
    /// instant, the body, and the header that names the body.
    fn body(bytes: &[u8]) -> Request {
        Request::new("PUT", "/lakelet-tier/segments/0.lks")
            .with_header("host", "tier.internal:9000")
            .with_header("x-amz-date", DATE)
            .with_body(bytes.to_vec())
            .with_header("x-amz-content-sha256", &hash_of(bytes))
    }

    fn put() -> Request {
        body(b"lakelet")
    }

    #[test]
    fn the_canonical_request_is_one_pinned_text_built_from_the_request() {
        let expected = format!(
            "PUT\n/lakelet-tier/segments/0.lks\n\n\
             host:tier.internal:9000\n\
             x-amz-content-sha256:{LAKELET_HASH}\n\
             x-amz-date:{DATE}\n\n\
             host;x-amz-content-sha256;x-amz-date\n\
             {LAKELET_HASH}"
        );
        assert_eq!(
            canonical(&put()),
            expected,
            "the canonical request is the method, the canonical path, the canonical query, the \
             signed headers one per line, an empty line, the signed-header list and the payload \
             hash — in that order, because a signature over a re-serialisation of the same request \
             agrees until somebody adds one header"
        );
        assert_eq!(
            put().payload_hash(),
            LAKELET_HASH,
            "and the hash inside it is the hash of the body that is sent, which is what the \
             `x-amz-content-sha256` header carries too"
        );

        let spaced = Request::new("GET", "/lakelet-tier/segments/a b.lks")
            .with_header("host", "tier.internal:9000");
        assert!(
            canonical(&spaced).starts_with("GET\n/lakelet-tier/segments/a%20b.lks\n"),
            "a key with a space is encoded once, the same way in the signature and on the wire: {}",
            canonical(&spaced)
        );
    }

    #[test]
    fn header_case_and_order_do_not_change_the_signed_bytes() {
        let first = Request::new("GET", "/lakelet-tier/segments/0.lks")
            .with_header("Host", "tier.internal:9000")
            .with_header("X-Amz-Date", DATE)
            .with_header("x-amz-date", DATE)
            .with_header("x-amz-content-sha256", &hash_of(b""));
        let second = Request::new("GET", "/lakelet-tier/segments/0.lks")
            .with_header("x-amz-content-sha256", &hash_of(b""))
            .with_header("x-amz-date", DATE)
            .with_header("host", "tier.internal:9000");

        assert_eq!(
            signed_headers(&first),
            vec![
                "host".to_owned(),
                "x-amz-content-sha256".to_owned(),
                "x-amz-date".to_owned()
            ],
            "the list is lower-cased, sorted, and holds each header once however many times the \
             request spells it"
        );
        assert_eq!(
            canonical(&first),
            canonical(&second),
            "two orders of one set of headers are one signed text: the block is the sorted list, \
             not the order somebody added them in"
        );
        assert_eq!(
            sign(&config(), &first),
            sign(&config(), &second),
            "and therefore one signature, which is what makes the normalisation a decision rather \
             than tidiness"
        );
    }

    #[test]
    fn the_signed_list_is_every_header_the_request_carries_and_no_other() {
        let plain = put();
        let extra = plain.clone().with_header("x-amz-meta-note", "cold");
        assert!(
            signed_headers(&extra).contains(&"x-amz-meta-note".to_owned()),
            "a header that is sent is a header that is signed: {:?}",
            signed_headers(&extra)
        );
        assert_ne!(
            canonical(&plain),
            canonical(&extra),
            "and it lands in the signed text: a header sent and not signed is a 403 from an \
             endpoint that checked both directions"
        );
        assert_ne!(
            sign(&config(), &plain),
            sign(&config(), &extra),
            "which is the signature moving with the request rather than beside it"
        );

        let again = plain.clone().with_header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=stale, Signature=stale",
        );
        assert!(
            !signed_headers(&again).contains(&"authorization".to_owned()),
            "the signature is what the request carries, not what it covers: {:?}",
            signed_headers(&again)
        );
        assert_eq!(
            sign(&config(), &again),
            sign(&config(), &plain),
            "so re-signing a request that already carries a signature does not sign the last \
             signature — which is what makes a retry the same request"
        );
    }

    #[test]
    fn a_body_of_a_different_length_is_a_different_signature() {
        let short = put();
        let long = body(b"lakelets");
        assert_ne!(
            short.payload_hash(),
            long.payload_hash(),
            "the payload hash names the body: {} against {}",
            short.payload_hash(),
            long.payload_hash()
        );
        assert_ne!(
            canonical(&short),
            canonical(&long),
            "and it is one of the lines that is signed"
        );
        assert_ne!(
            sign(&config(), &short),
            sign(&config(), &long),
            "so a body that moves after the signature was computed is a signature nobody can \
             verify, which is the failure this whole shape exists to prevent"
        );
        assert_eq!(
            sign(&config(), &put()),
            sign(&config(), &short),
            "and the same request signs the same bytes twice: the instant is a value in the config \
             and nothing here reads a clock"
        );
    }

    #[test]
    fn the_scope_is_the_date_stamp_the_region_the_service_and_the_algorithm() {
        assert_eq!(
            config().scope(),
            "20261008/eu-central-1/s3/aws4_request",
            "the scope starts with the instant's own first eight characters: the date without its \
             time, because the scope names a day rather than a moment"
        );
        let vendor = Config {
            date: "20150830T123600Z".to_owned(),
            region: "us-east-1".to_owned(),
            service: "service".to_owned(),
            ..config()
        };
        assert_eq!(
            vendor.scope(),
            "20150830/us-east-1/service/aws4_request",
            "and the time part stays out of it — it is in the string to sign, not in the scope"
        );
    }

    #[test]
    fn the_signature_is_pinned_at_one_known_vector_and_the_scope_is_inside_it() {
        assert_eq!(
            sign(&config(), &put()),
            SIGNATURE,
            "a pinned signature is what makes this a check of the canonical form rather than a \
             comparison of the function with itself"
        );

        let other_secret = Config {
            secret_key: "lakelet-tier-fixture-secreu".to_owned(),
            ..config()
        };
        assert_ne!(
            sign(&other_secret, &put()),
            SIGNATURE,
            "one byte of secret is a different signature, which is the whole point of a key"
        );

        let other_region = Config {
            region: "us-east-2".to_owned(),
            ..config()
        };
        assert_ne!(
            sign(&other_region, &put()),
            SIGNATURE,
            "and the scope is signed: a request signed for another region is a 403 an operator can \
             read and act on"
        );

        let other_date = Config {
            date: "20261009T000000Z".to_owned(),
            ..config()
        };
        assert_ne!(
            sign(&other_date, &put()),
            SIGNATURE,
            "the instant is signed twice — in `x-amz-date` and in the string to sign — and a \
             signature that moved with only one of them is a hole exactly the size of a replay"
        );
    }

    #[test]
    fn a_2xx_is_an_answer_and_a_404_is_absence_only_when_it_is_about_a_key() {
        for status in [200, 201, 204, 206] {
            assert_eq!(
                classify(status, ""),
                Class::Answered,
                "`{status}` is the request carried out"
            );
        }
        assert_eq!(
            classify(404, "<Error><Code>NoSuchKey</Code></Error>"),
            Class::Absent,
            "absence is a value: the object is not there, and that is not a refusal"
        );
        assert_eq!(
            classify(404, ""),
            Class::Absent,
            "an endpoint that says nothing about which 404 it is means the key it was asked for is \
             not there"
        );
        assert_eq!(
            classify(404, "<Error><Code>NoSuchBucket</Code></Error>"),
            Class::Refused,
            "a bucket that is not there is a misconfiguration rather than a cold segment: reading \
             it as absence is how a tier reports a whole store as gone and writes a hydrate that \
             fetches nothing"
        );
        assert_eq!(
            classify(400, "<Error><Code>InvalidRequest</Code></Error>"),
            Class::Refused,
            "the endpoint answered, and the answer is no"
        );
    }

    #[test]
    fn a_5xx_is_retryable_and_a_403_is_refused_and_a_refused_answer_is_sent_once() {
        for status in [500, 502, 503, 504] {
            assert_eq!(
                classify(status, ""),
                Class::Retryable,
                "`{status}` is the endpoint failing to answer, so the same request is worth sending \
                 again"
            );
            assert_eq!(
                Class::Retryable.attempts(),
                ATTEMPTS,
                "a retry is a count: `{status}` is sent at most {} times",
                ATTEMPTS
            );
        }
        for status in [400, 401, 403, 410, 422] {
            assert_eq!(
                classify(status, ""),
                Class::Refused,
                "`{status}` is an answer and the answer is no"
            );
            assert_eq!(
                classify(status, "").attempts(),
                1,
                "a `{status}` retried is the bug: another attempt signs the same bytes and gets \
                 the same refusal, so the attempt is spent on nothing"
            );
        }
        assert_eq!(
            Class::Answered.attempts(),
            1,
            "a `2xx` is an answer, not a reason to send the request again"
        );
        assert_eq!(
            Class::Absent.attempts(),
            1,
            "and neither is absence: the object is not there, and a second `get` will not find it"
        );
    }
}
