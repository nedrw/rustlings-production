//! probe = "outbound/http-client@apply"
//! title = "Three deadlines, three words, and a redirect policy"
//! hint = "2xx success, 408/429 and 5xx and a connection that never answered retryable, the rest permanent — then count hops, refuse loops, and clamp every layer to the attempt."
//!
//! Step `relay/2.7-outbound-client` is the one component between the worker and somebody else's
//! server, and every failure of a delivery is a failure of *time*: a target that accepts the
//! connection and answers nothing holds the worker forever. Three decisions come with it.
//!
//! 1. **three deadlines, because they answer three questions.** Connect is the handshake, read is
//!    *one* read, and the attempt is everything together, redirects included. Only the third bounds
//!    a chain: four hops that each answer inside their own read deadline have no total otherwise;
//! 2. **the answer is classified into three words, and the delivery layer decides.** `2xx` success,
//!    `408` and `429` retryable because the subscriber is asking for another attempt, every other
//!    `4xx` permanent, `5xx` retryable, and a connection that never answered also retryable —
//!    nothing about the subscriber was learned. A client that retries on its own is a second retry
//!    policy nobody can see, which is why nothing here retries;
//! 3. **`Location` is a stranger's instruction.** Hops are counted, a URL the attempt has already
//!    visited is a loop rather than a chain, and a `301`/`302` in front of a webhook means "the same
//!    thing, over there" — the method and the body are kept — while a `303` is a `GET` with no body,
//!    because turning a webhook POST into a `GET` silently delivers an empty payload and records it
//!    as delivered.
//!
//! ## What you write
//!
//! * [`classify`] — the three words, for a status or for a connection that never answered;
//! * [`Attempt::follow`] — one hop: is this a hop at all, does it say where, may this attempt take
//!   it, and where does it land. A refused hop leaves the attempt exactly where it was;
//! * [`Refusal::outcome`] — every refusal of the redirect policy is permanent;
//! * [`Policy::effective`] — the three deadlines filled in from the defaults, then clamped to the
//!   attempt they run inside.
//!
//! [`resolve`] (the join `relay/2.1`'s URL parser does), [`Answer`], [`Attempt::new`] and the
//! accessors are given: URL joining is not this step's lesson, and the two refusals that are a
//! count and a set of strings are.
//!
//! ## What the tests ask
//!
//! Every status class on the three words, including the `408`/`429` pair and a connection that
//! never answered. A `301`, `302`, `307` and `308` keep the method and the body; a `303` becomes a
//! `GET` with nothing in it; a `304` and a `302` with no `Location` are refusals. A chain that
//! comes back to a URL it has visited — through a relative `Location` and a `..`, so the loop is
//! found in the normalised string — is refused and names the URL; so is the hop past
//! `Policy::redirects`. `https` is a hop this client cannot take, and every refusal is permanent.
//! Finally the deadlines: the defaults fill in what the config left out, and no layer outlives the
//! attempt it runs inside.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies and **no sockets**: a probe that opened one would need a target, a
//! port and a clock, and what it would mostly check is the runtime rather than the client. What is
//! left, and what the step's traps are about, is the part a reviewer argues with: the
//! classification, the redirect policy and the effective deadlines. Deliberately out of reach here:
//! the connection and its reuse, the request line and the hop-by-hop header set, `content-length`
//! and a body that stops early (both of which are about bytes on a wire), the pool, and keep-alive —
//! each is a real part of `relay/2.7` and none of them is a decision this file can make for you.
//! [`resolve`] is given rather than written and is a plain string join: the skeleton's parser has
//! the real one, and the normalisation here exists so that a loop is a comparison.

use std::fmt;

/// The defaults the build ships when the configuration says nothing. Chosen so that the sum of a
/// connect and a read fits inside one attempt with room to spare.
pub const DEFAULT_CONNECT_MS: u64 = 5_000;
pub const DEFAULT_READ_MS: u64 = 10_000;
pub const DEFAULT_ATTEMPT_MS: u64 = 30_000;

/// What the delivery layer may do with one attempt's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It arrived: the subscriber answered `2xx`.
    Success,
    /// It will never arrive: another attempt would produce this answer again.
    Permanent,
    /// It did not arrive, and another attempt is a reasonable thing to do.
    Retryable,
}

/// What one attempt came back with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The subscriber answered with this status.
    Status(u16),
    /// There was no answer at all: a refused connection, a deadline that fired, a body that stopped
    /// early. Nothing about the subscriber was learned.
    Broken,
}

/// Read one attempt's answer into the three words the delivery layer acts on.
///
/// This is about the answer an attempt *ended* with, by which point a redirect has either been
/// followed or refused: `2xx` is success, `408` and `429` ask for another attempt, `5xx` and a
/// broken connection may be retried, and everything else — every other `4xx`, and every status this
/// client cannot act on — is permanent. Spending the retry budget on a `410` is the budget the next
/// delivery needs.
#[must_use]
pub fn classify(answer: &Answer) -> Outcome {
    match answer {
        // `408` and `429` are the two 4xx that ask for another attempt, and they are checked
        // before the 4xx arm that refuses the rest: the classes are a first look, not the rule.
        Answer::Status(408 | 429) => Outcome::Retryable,
        Answer::Status(200..=299) => Outcome::Success,
        Answer::Status(400..=499) => Outcome::Permanent,
        Answer::Status(500..=599) => Outcome::Retryable,
        // A connection that never answered, and every status this client cannot act on: the first
        // learned nothing, the second will say the same thing next time.
        Answer::Broken => Outcome::Retryable,
        Answer::Status(_) => Outcome::Permanent,
    }
}

/// The method of a request. A webhook delivery is a `POST`; a `303` makes it a `GET`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// No body.
    Get,
    /// The event.
    Post,
}

/// One request on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Where it goes, in the canonical spelling [`resolve`] produces.
    pub url: String,
    /// What it is.
    pub method: Method,
    /// The bytes that go in it: the event as the API answers with it.
    pub body: Vec<u8>,
}

/// The client's policy: how many hops a chain may take, and what the three deadlines are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// How many redirects one attempt may follow. `0` is a client that does not follow any.
    pub redirects: u32,
    /// How long the handshake may take, or `None` for [`DEFAULT_CONNECT_MS`].
    pub connect_ms: Option<u64>,
    /// How long one read may take, or `None` for [`DEFAULT_READ_MS`].
    pub read_ms: Option<u64>,
    /// How long everything together may take, redirects included, or `None` for
    /// [`DEFAULT_ATTEMPT_MS`].
    pub attempt_ms: Option<u64>,
}

/// The three deadlines one attempt actually runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    /// The handshake.
    pub connect_ms: u64,
    /// One read.
    pub read_ms: u64,
    /// The whole attempt, and the only one that bounds a chain.
    pub attempt_ms: u64,
}

impl Policy {
    /// The three deadlines, filled in from the defaults and then clamped to the attempt.
    ///
    /// A read deadline longer than the attempt's cannot happen — the attempt is what cancels the
    /// whole thing, so a layer that outlives it is a number nobody will ever see fire. Filling in
    /// first and clamping second is the order that makes an unset layer follow a configured
    /// attempt, rather than the build's default outliving a short one.
    #[must_use]
    pub fn effective(&self) -> Effective {
        // Fill in first: an unset layer takes the build's default, and a configured attempt then
        // clamps it. Clamping before filling in would let `DEFAULT_ATTEMPT_MS` outlive a one-second
        // attempt.
        let attempt_ms = self.attempt_ms.unwrap_or(DEFAULT_ATTEMPT_MS);
        Effective {
            connect_ms: self
                .connect_ms
                .unwrap_or(DEFAULT_CONNECT_MS)
                .min(attempt_ms),
            read_ms: self.read_ms.unwrap_or(DEFAULT_READ_MS).min(attempt_ms),
            attempt_ms,
        }
    }
}

/// Why a redirect was not followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The chain is at the hop ceiling. Nothing is sent.
    TooManyHops {
        /// How many hops this attempt has taken.
        hops: u32,
        /// How many it was allowed.
        limit: u32,
    },
    /// This attempt has already visited this URL.
    Loop {
        /// The URL it came back to.
        url: String,
    },
    /// The answer asks for a hop and does not say where.
    NoLocation {
        /// The status that asked.
        status: u16,
    },
    /// This status is not a hop this client takes (`300` and `304` are answers, not redirections).
    NotARedirect {
        /// The status.
        status: u16,
    },
    /// A hop this client cannot speak: `https` needs a TLS stack, and a subscriber that answers
    /// one is a subscriber this client cannot deliver to.
    UnsupportedScheme {
        /// The URL it was pointed at.
        url: String,
    },
}

impl Refusal {
    /// Every refusal of the redirect policy is permanent: the chain did not change, and another
    /// attempt would walk into the same wall.
    #[must_use]
    pub fn outcome(&self) -> Outcome {
        Outcome::Permanent
    }
}

/// One attempt at one delivery, and the hops it has taken.
///
/// The state a redirect policy needs is exactly this: where it is now, where it has already been,
/// and how many hops that took. It belongs to the *attempt* rather than to the client, because the
/// second attempt at a delivery starts its chain again.
#[derive(Debug, Clone)]
pub struct Attempt {
    policy: Policy,
    request: Request,
    visited: Vec<String>,
    hops: u32,
}

impl Attempt {
    /// A fresh attempt at this request: the request itself is the first URL visited, and no hops
    /// have been taken.
    #[must_use]
    pub fn new(policy: Policy, request: Request) -> Self {
        Self {
            visited: vec![request.url.clone()],
            policy,
            request,
            hops: 0,
        }
    }

    /// The request this attempt sends *now*.
    #[must_use]
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// How many hops have been taken.
    #[must_use]
    pub fn hops(&self) -> u32 {
        self.hops
    }

    /// Take the hop this redirect asks for, or refuse it and leave the attempt where it was.
    ///
    /// The order is the reading order, and it is what makes a refusal name one thing rather than
    /// another: is this a hop at all, does it say where, may this attempt take it, and where does
    /// it land. A refusal changes nothing — no hop is counted and the request is untouched — so a
    /// caller that gives up on the chain reports the answer it actually got.
    ///
    /// # Errors
    ///
    /// One of the five [`Refusal`]s, each of which is permanent.
    pub fn follow(&mut self, status: u16, location: Option<&str>) -> Result<(), Refusal> {
        // Is it a hop this client takes? `303` means "fetch that instead" — a `GET` with nothing in
        // it — and `301`/`302`/`307`/`308` in front of a webhook mean "the same thing, over there".
        let keep = match status {
            301 | 302 | 307 | 308 => true,
            303 => false,
            other => return Err(Refusal::NotARedirect { status: other }),
        };

        // Does it say where? An empty `Location` says nothing, so it is the same refusal as a
        // missing one — and either way the attempt has not moved.
        let Some(url) = location.and_then(|location| resolve(&self.request.url, location)) else {
            return Err(Refusal::NoLocation { status });
        };

        // May this attempt take it? The ceiling is counted here, before anything changes, so a
        // chain that is over budget is refused where it stands.
        if self.hops >= self.policy.redirects {
            return Err(Refusal::TooManyHops {
                hops: self.hops,
                limit: self.policy.redirects,
            });
        }

        // And where does it land? A scheme this client cannot speak is permanent; a URL it has
        // already visited in this attempt is a loop rather than a chain.
        if !url.starts_with("http://") {
            return Err(Refusal::UnsupportedScheme { url });
        }
        if self.visited.contains(&url) {
            return Err(Refusal::Loop { url });
        }

        self.hops += 1;
        self.visited.push(url.clone());
        self.request = Request {
            url,
            method: if keep {
                self.request.method
            } else {
                Method::Get
            },
            body: if keep {
                self.request.body.clone()
            } else {
                Vec::new()
            },
        };
        Ok(())
    }
}

/// Join a `Location` to the URL the answer came from, in one canonical spelling.
///
/// `relay/2.1-project-skeleton`'s URL parser is what does this in the service; the join is given
/// here because it is not the lesson. Absolute (`http://host/p`), origin-relative (`/p`) and
/// relative (`p`, `../p`) locations all arrive in the wild. The result has a lower-case scheme and
/// authority, and `.`, `..` and empty segments resolved away — which is what makes a loop a
/// comparison of two strings rather than a browser's opinion of them.
///
/// `None` when the location is empty: a `Location` header nobody filled in is not a location.
#[must_use]
fn resolve(base: &str, location: &str) -> Option<String> {
    let location = location.trim();
    if location.is_empty() {
        return None;
    }
    let (base_scheme, base_authority, base_path) = split_url(base)?;
    let (scheme, authority, path) = if let Some((scheme, authority, path)) = split_url(location) {
        (
            scheme.to_ascii_lowercase(),
            authority.to_ascii_lowercase(),
            path.to_owned(),
        )
    } else if let Some(origin_relative) = location.strip_prefix('/') {
        (
            base_scheme.to_ascii_lowercase(),
            base_authority.to_ascii_lowercase(),
            format!("/{origin_relative}"),
        )
    } else {
        let directory = match base_path.rfind('/') {
            Some(index) => &base_path[..=index],
            None => "/",
        };
        (
            base_scheme.to_ascii_lowercase(),
            base_authority.to_ascii_lowercase(),
            format!("{directory}{location}"),
        )
    };
    Some(format!("{scheme}://{authority}{}", normalize(&path)))
}

/// `scheme://authority` and the path, split apart.
#[must_use]
fn split_url(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    Some(match rest.find('/') {
        Some(index) => (scheme, &rest[..index], &rest[index..]),
        None => (scheme, rest, "/"),
    })
}

/// One path, with `.`, `..` and empty segments resolved away and no trailing slash.
#[must_use]
fn normalize(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    format!("/{}", segments.join("/"))
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyHops { hops, limit } => {
                write!(f, "{hops} redirects taken, the limit is {limit}")
            }
            Self::Loop { url } => write!(f, "{url} was already visited by this attempt"),
            Self::NoLocation { status } => write!(f, "{status} asks for a hop and says nowhere"),
            Self::NotARedirect { status } => write!(f, "{status} is not a redirect"),
            Self::UnsupportedScheme { url } => write!(f, "this client cannot deliver to {url}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request to a subscriber's endpoint, which is what a delivery attempt is.
    fn delivery() -> Request {
        Request {
            url: "http://hooks.test/incoming".to_owned(),
            method: Method::Post,
            body: br#"{"kind":"a"}"#.to_vec(),
        }
    }

    fn policy(redirects: u32) -> Policy {
        Policy {
            redirects,
            connect_ms: None,
            read_ms: None,
            attempt_ms: None,
        }
    }

    fn attempt(redirects: u32) -> Attempt {
        Attempt::new(policy(redirects), delivery())
    }

    #[test]
    fn every_status_class_lands_on_one_of_three_words() {
        for status in [200, 201, 204, 299] {
            assert_eq!(
                classify(&Answer::Status(status)),
                Outcome::Success,
                "`{status}` is an answer"
            );
        }
        for status in [408, 429, 500, 502, 503] {
            assert_eq!(
                classify(&Answer::Status(status)),
                Outcome::Retryable,
                "`{status}` asks for another attempt"
            );
        }
        for status in [400, 401, 403, 404, 410, 422] {
            assert_eq!(
                classify(&Answer::Status(status)),
                Outcome::Permanent,
                "`{status}` will say the same thing next time"
            );
        }
        for status in [100, 101, 300, 304] {
            assert_eq!(
                classify(&Answer::Status(status)),
                Outcome::Permanent,
                "`{status}` is not an answer this client can act on"
            );
        }
        assert_eq!(
            classify(&Answer::Broken),
            Outcome::Retryable,
            "a connection that never answered learned nothing about the subscriber"
        );
    }

    #[test]
    fn a_redirect_the_subscriber_meant_as_the_same_thing_keeps_the_method_and_the_body() {
        let original = delivery();
        for status in [301, 302, 307, 308] {
            let mut hop = attempt(5);
            assert_eq!(hop.follow(status, Some("http://hooks.test/moved")), Ok(()));

            assert_eq!(hop.request().url, "http://hooks.test/moved");
            assert_eq!(
                hop.request().method,
                Method::Post,
                "a webhook POST turned into a GET delivers an empty payload and records it as \
                 delivered"
            );
            assert_eq!(hop.request().body, original.body);
            assert_eq!(hop.hops(), 1);
        }
    }

    #[test]
    fn a_three_oh_three_turns_the_request_into_a_get_with_nothing_in_it() {
        let mut hop = attempt(5);
        assert_eq!(hop.follow(303, Some("/see-other")), Ok(()));

        assert_eq!(
            hop.request().url,
            "http://hooks.test/see-other",
            "an origin-relative `Location` stays on the same host"
        );
        assert_eq!(hop.request().method, Method::Get);
        assert!(hop.request().body.is_empty());
    }

    #[test]
    fn a_hop_with_no_location_and_a_status_that_is_not_a_hop_are_refusals() {
        let mut hop = attempt(5);
        assert_eq!(
            hop.follow(302, None),
            Err(Refusal::NoLocation { status: 302 })
        );
        assert_eq!(
            hop.follow(302, Some("")),
            Err(Refusal::NoLocation { status: 302 }),
            "an empty header is a hop that says nowhere"
        );
        assert_eq!(
            hop.follow(304, Some("http://hooks.test/elsewhere")),
            Err(Refusal::NotARedirect { status: 304 })
        );
        assert_eq!(
            hop.follow(300, Some("http://hooks.test/elsewhere")),
            Err(Refusal::NotARedirect { status: 300 })
        );

        assert_eq!(hop.request(), &delivery(), "a refused hop changes nothing");
        assert_eq!(hop.hops(), 0);
        assert_eq!(
            Refusal::NotARedirect { status: 304 }.outcome(),
            Outcome::Permanent
        );
    }

    #[test]
    fn a_chain_that_comes_back_to_a_url_it_has_visited_is_a_loop() {
        let mut hop = attempt(5);
        assert_eq!(hop.follow(302, Some("http://hooks.test/moved")), Ok(()));
        assert_eq!(hop.follow(302, Some("./moved/final")), Ok(()));
        assert_eq!(hop.request().url, "http://hooks.test/moved/final");

        // Relative, with a `..`, back to the very first URL: the normalisation is what makes this
        // a loop rather than a chain that happens to look familiar.
        assert_eq!(
            hop.follow(302, Some("../../incoming")),
            Err(Refusal::Loop {
                url: "http://hooks.test/incoming".to_owned()
            })
        );
        assert_eq!(
            hop.follow(301, Some("HTTP://HOOKS.TEST/moved")),
            Err(Refusal::Loop {
                url: "http://hooks.test/moved".to_owned()
            }),
            "a loop is found in the spelling this client compares, not the one the subscriber sent"
        );

        assert_eq!(hop.request().url, "http://hooks.test/moved/final");
        assert_eq!(hop.hops(), 2, "a refused hop is not a hop taken");
        assert_eq!(
            Refusal::Loop {
                url: "http://hooks.test/x".to_owned()
            }
            .outcome(),
            Outcome::Permanent
        );
    }

    #[test]
    fn the_chain_stops_at_the_hop_ceiling() {
        let mut hop = attempt(2);
        assert_eq!(hop.follow(302, Some("http://hooks.test/a")), Ok(()));
        assert_eq!(hop.follow(302, Some("http://hooks.test/b")), Ok(()));
        assert_eq!(
            hop.follow(302, Some("http://hooks.test/c")),
            Err(Refusal::TooManyHops { hops: 2, limit: 2 }),
            "a chain of four hops that each answer inside their own read deadline has no bound \
             without this"
        );
        assert_eq!(hop.request().url, "http://hooks.test/b");

        let mut none = attempt(0);
        assert_eq!(
            none.follow(302, Some("http://hooks.test/a")),
            Err(Refusal::TooManyHops { hops: 0, limit: 0 })
        );
        assert_eq!(
            Refusal::TooManyHops { hops: 2, limit: 2 }.outcome(),
            Outcome::Permanent
        );
    }

    #[test]
    fn https_is_a_hop_this_client_cannot_take() {
        let mut hop = attempt(5);
        assert_eq!(
            hop.follow(302, Some("https://hooks.test/secure")),
            Err(Refusal::UnsupportedScheme {
                url: "https://hooks.test/secure".to_owned()
            }),
            "retrying will not grow a TLS implementation, so this is permanent rather than \
             retryable"
        );
        assert_eq!(
            Refusal::UnsupportedScheme {
                url: "https://hooks.test/secure".to_owned()
            }
            .outcome(),
            Outcome::Permanent
        );
        assert_eq!(hop.hops(), 0);
    }

    #[test]
    fn the_three_deadlines_are_filled_in_and_then_clamped_to_the_attempt() {
        let defaults = policy(5).effective();
        assert_eq!(defaults.connect_ms, DEFAULT_CONNECT_MS);
        assert_eq!(defaults.read_ms, DEFAULT_READ_MS);
        assert_eq!(defaults.attempt_ms, DEFAULT_ATTEMPT_MS);
        assert!(
            defaults.connect_ms + defaults.read_ms <= defaults.attempt_ms,
            "the two inner layers have to fit inside the one that contains them"
        );

        let configured = Policy {
            redirects: 5,
            connect_ms: Some(1_000),
            read_ms: Some(60_000),
            attempt_ms: Some(30_000),
        }
        .effective();
        assert_eq!(
            configured.connect_ms, 1_000,
            "a short layer stays where it was set"
        );
        assert_eq!(
            configured.read_ms, 30_000,
            "a read deadline past the attempt's is a number nobody will ever see fire"
        );
        assert_eq!(configured.attempt_ms, 30_000);

        let short = Policy {
            redirects: 5,
            connect_ms: None,
            read_ms: Some(10_000),
            attempt_ms: Some(250),
        }
        .effective();
        assert_eq!(short.attempt_ms, 250);
        assert_eq!(
            short.connect_ms, 250,
            "clamped after being filled in, not before"
        );
        assert_eq!(short.read_ms, 250);
    }

    #[test]
    fn a_location_is_joined_the_way_a_browser_would_and_normalised_for_comparison() {
        let base = "http://hooks.test/a/b";
        assert_eq!(resolve(base, "c").as_deref(), Some("http://hooks.test/a/c"));
        assert_eq!(
            resolve(base, "../c").as_deref(),
            Some("http://hooks.test/c")
        );
        assert_eq!(resolve(base, "/c").as_deref(), Some("http://hooks.test/c"));
        assert_eq!(
            resolve(base, "https://Other.Test/c").as_deref(),
            Some("https://other.test/c"),
            "the scheme and the authority are lower-cased, so two spellings of one host are one \
             host"
        );
        assert_eq!(resolve(base, "  "), None);
    }
}
