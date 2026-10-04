//! probe = "sec/ssrf-defense@apply"
//! title = "Resolve once, check the address, connect to it"
//! hint = "An allowlist of schemes, a name check before any resolver, and the address you resolved — every address in the answer, not the hostname."
//!
//! Step `relay/2.12-ssrf-defense` is one sentence and two traps:
//!
//! 1. **check the address you are going to connect to, not the name you were given.** A subscriber
//!    hands the service a URL, and the service is the one with the network. `https://internal.example`
//!    that resolves to `10.0.0.5`, the metadata service at `169.254.169.254`, a loopback literal —
//!    all of them are the same request in different words. So [`check`] resolves *once*, validates
//!    every address in that one answer, and answers the address the connection must be made to.
//!    A client that checks the hostname and then lets the HTTP library resolve it again has closed
//!    nothing: that second lookup is the one an attacker controls;
//! 2. **a name that is local by definition does not need a resolver to be dangerous.**
//!    [`is_denied_name`] refuses `localhost` and the cloud metadata hostnames before anything is
//!    asked — cheap, and independent of whether the resolver is honest. It is the *second* line,
//!    not the first: every rule here that matters is about an address.
//!
//! ## What you write
//!
//! * [`allowed_address`] — is this an address on the public internet? Loopback, private,
//!   link-local (that is where the metadata service lives), `100.64/10`, multicast, the reserved
//!   top of the space, and the IPv6 equivalents. An IPv4-mapped IPv6 address is the IPv4 address it
//!   carries: `::ffff:127.0.0.1` is a way of writing the loopback, not a different thing;
//! * [`is_denied_name`] — the name list, applied before a resolver is asked;
//! * [`check`] — the order: scheme, name, literal, then one resolution whose every address has to
//!   pass. The answer is [`Approved`], and what it carries is the address to connect to.
//!
//! [`split_target`] is given: it splits `scheme://host:port` without pulling in a URL crate.
//!
//! ## What the tests ask
//!
//! A table of literals — `127.0.0.1`, `10.0.0.5`, `169.254.169.254`, `[::1]`, `[fe80::1]`,
//! `[::ffff:127.0.0.1]` — refused without a resolver being asked at all; `localhost` and the
//! metadata hostnames refused by name, still without a resolver; a resolver the test owns and can
//! read back, which is how "resolved once" is asserted rather than hoped for; and a rebinding
//! resolver that answers a public address and then a private one, where the second answer is the
//! one that must not reach a socket.
//!
//! ## Where this is honest, and where it is not
//!
//! No TLS, no HTTP client, no real DNS: the resolver is injected, which is what makes the rebinding
//! answer reproducible. The target parser knows `scheme://host:port` and nothing else — a real
//! service parses with a URL library, because the forms that try to be clever (`127.1`, `0x7f.1`,
//! `%31%32%37.0.0.1`, userinfo in front of the host) are that library's problem and its
//! normalisation, not something this probe pretends to close. A redirect is a new target and goes
//! through the same function; nothing here follows one.

use std::net::IpAddr;

/// Where a name resolves, injected: a probe has no DNS, and the resolver that answers twice is the
/// whole point of an SSRF test.
pub trait Resolver {
    /// The addresses this host has *right now*.
    ///
    /// # Errors
    ///
    /// Whatever the resolver said. A name that does not resolve is a refusal for the delivery, not
    /// a reason to be optimistic.
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String>;
}

/// A target, split into the three parts the check needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The scheme, lower-cased.
    pub scheme: String,
    /// The host: a name, or an IP literal without its brackets.
    pub host: String,
    /// The port, if the target names one.
    pub port: Option<u16>,
}

/// Split `scheme://host:port` into its parts, without pulling in a URL crate.
///
/// Given, because this is string handling rather than the lesson. It refuses nothing: an unknown
/// scheme comes back as an unknown scheme, which [`check`] has a sentence of its own for.
#[must_use]
pub fn split_target(target: &str) -> Option<Target> {
    let (scheme, rest) = target.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }

    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        // An IPv6 literal: `[::1]` or `[::1]:8443`.
        let (host, rest) = bracketed.split_once(']')?;
        match rest.strip_prefix(':') {
            Some(port) => (host.to_owned(), Some(port.parse().ok()?)),
            None if rest.is_empty() => (host.to_owned(), None),
            None => return None,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port.parse().ok()?)),
            None => (authority.to_owned(), None),
        }
    };

    if host.is_empty() {
        return None;
    }
    Some(Target {
        scheme: scheme.to_ascii_lowercase(),
        host,
        port,
    })
}

/// A target the service is willing to deliver to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approved {
    /// The host that was asked for — for the log, and for the signature. The connection does not
    /// use it.
    pub host: String,
    /// The address that was checked *and* that the connection must be made to.
    pub addr: IpAddr,
    /// The port.
    pub port: u16,
}

/// Why a target was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a target this service sends to. A webhook over plain HTTP is a webhook somebody else can
    /// rewrite, and `file://` is not a delivery at all.
    Scheme {
        /// The scheme that was refused.
        scheme: String,
    },
    /// The target is the service's own network, the cloud's metadata service, or somewhere else
    /// that is not the public internet.
    Address {
        /// The host as the subscriber wrote it.
        host: String,
        /// The address that was refused — the one that would have been connected to.
        addr: IpAddr,
    },
    /// A name that is local by definition, refused before any resolver was asked.
    Name {
        /// The host.
        host: String,
    },
    /// The name did not resolve, or resolved to nothing.
    Resolution {
        /// The host.
        host: String,
        /// What the resolver said.
        error: String,
    },
    /// Not a target anybody can read.
    Malformed {
        /// What the subscriber sent.
        target: String,
    },
}

/// The names refused without asking a resolver.
///
/// `localhost` is loopback by definition (RFC 6761) and the metadata hostname is how a cloud
/// instance is asked for its own credentials. Both would be caught by the address rules if the
/// resolver answered honestly; refusing them by name is one less thing that depends on that.
pub const DENIED_NAMES: [&str; 2] = ["localhost", "metadata.google.internal"];

/// Whether a host is refused by name, before any resolver is asked.
///
/// Three rules: the name is in [`DENIED_NAMES`]; anything under `.localhost` is loopback too; and
/// anything under `.internal` is where a cloud's internal names live.
#[must_use]
pub fn is_denied_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    DENIED_NAMES.contains(&host.as_str())
        || host.ends_with(".localhost")
        || host.ends_with(".internal")
}

/// Whether the delivery worker may connect to this address.
///
/// Everything here is "this is not an address on the public internet": the loopback, the private
/// ranges, link-local — which is where `169.254.169.254` lives — the carrier-grade NAT range, the
/// unspecified address, multicast, broadcast and the reserved top of the space, plus the IPv6
/// equivalents. An IPv4-mapped IPv6 address is checked as the IPv4 address it carries.
#[must_use]
pub fn allowed_address(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                // 169.254.0.0/16: link-local, which is where the metadata service lives.
                || (octets[0] == 169 && octets[1] == 254)
                // 100.64.0.0/10: carrier-grade NAT, and the carrier is not the public internet.
                || (octets[0] == 100 && (64..128).contains(&octets[1]))
                // 240.0.0.0/4: reserved, and 255.255.255.255 is already the broadcast.
                || octets[0] >= 240)
        }
        IpAddr::V6(v6) => {
            // An IPv4-mapped address is the IPv4 address it carries, and the rules above are the
            // ones that know whether that address is public. `::ffff:127.0.0.1` is a spelling of
            // the loopback, not a different thing.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return allowed_address(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7: unique local, the IPv6 equivalent of the private ranges.
                || (segments[0] & 0xfe00) == 0xfc00
                // fe80::/10: link-local.
                || (segments[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// Check one target, and answer the address the connection has to use.
///
/// The order is the lesson: the scheme, then a name that is local by definition, then the literal
/// address if there is one, and only then one resolution — whose *every* address has to pass, not
/// its first one, because a resolver that answers with a good address and a private one has told the
/// truth about both.
///
/// # Errors
///
/// See [`Refusal`].
pub fn check(target: &str, resolver: &dyn Resolver) -> Result<Approved, Refusal> {
    let parsed = split_target(target).ok_or_else(|| Refusal::Malformed {
        target: target.to_owned(),
    })?;

    // 1. The scheme decides whether this is even a delivery, and it comes first because the answer
    //    is about the target as written: a refused scheme is not looked up.
    let port = match parsed.scheme.as_str() {
        "https" => parsed.port.unwrap_or(443),
        scheme => {
            return Err(Refusal::Scheme {
                scheme: scheme.to_owned(),
            });
        }
    };

    // 2. A name that is local by definition, refused before anybody is asked.
    if is_denied_name(&parsed.host) {
        return Err(Refusal::Name { host: parsed.host });
    }

    // 3. A literal is its own answer: there is nothing to resolve, and the address is checked like
    //    any other.
    if let Ok(addr) = parsed.host.parse::<IpAddr>() {
        if !allowed_address(addr) {
            return Err(Refusal::Address {
                host: parsed.host,
                addr,
            });
        }
        return Ok(Approved {
            host: parsed.host,
            addr,
            port,
        });
    }

    // 4. One resolution, and *every* address in it has to pass. A resolver that answers with a good
    //    address and a private one has told the truth about both, and picking the good one to check
    //    is how a delivery ends up at the private one after all.
    let addresses = resolver
        .resolve(&parsed.host)
        .map_err(|error| Refusal::Resolution {
            host: parsed.host.clone(),
            error,
        })?;
    let Some(addr) = addresses.first().copied() else {
        return Err(Refusal::Resolution {
            host: parsed.host,
            error: "the name resolved to nothing".to_owned(),
        });
    };
    for candidate in &addresses {
        if !allowed_address(*candidate) {
            return Err(Refusal::Address {
                host: parsed.host,
                addr: *candidate,
            });
        }
    }

    // The answer carries the address, not the name: the connection goes where the check looked.
    Ok(Approved {
        host: parsed.host,
        addr,
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A resolver the test decides the answers for, and can read back: "resolved once" is asserted
    /// against the questions that were actually asked.
    #[derive(Debug, Default)]
    struct FakeResolver {
        answers: BTreeMap<String, Result<Vec<IpAddr>, String>>,
        asked: Mutex<Vec<String>>,
    }

    impl FakeResolver {
        /// A resolver with one honest answer per host.
        fn with(answers: &[(&str, &[&str])]) -> Self {
            let answers = answers
                .iter()
                .map(|(host, addresses)| {
                    (
                        (*host).to_owned(),
                        Ok(addresses.iter().map(|addr| parse(addr)).collect()),
                    )
                })
                .collect();
            Self {
                answers,
                asked: Mutex::new(Vec::new()),
            }
        }

        /// The hosts this resolver was asked about, in order.
        fn asked(&self) -> Vec<String> {
            self.asked.lock().expect("the log is not poisoned").clone()
        }
    }

    impl Resolver for FakeResolver {
        fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
            self.asked
                .lock()
                .expect("the log is not poisoned")
                .push(host.to_owned());
            self.answers
                .get(host)
                .cloned()
                .unwrap_or_else(|| Err(format!("NXDOMAIN for {host}")))
        }
    }

    /// A resolver that answers one address and then another: the rebinding case, where the second
    /// answer is the one that must not reach a socket.
    #[derive(Debug)]
    struct Rebinding {
        answers: Vec<Vec<IpAddr>>,
        call: Mutex<usize>,
    }

    impl Rebinding {
        fn new(answers: &[&[&str]]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|addresses| addresses.iter().map(|addr| parse(addr)).collect())
                    .collect(),
                call: Mutex::new(0),
            }
        }

        fn calls(&self) -> usize {
            *self.call.lock().expect("the counter is not poisoned")
        }
    }

    impl Resolver for Rebinding {
        fn resolve(&self, _host: &str) -> Result<Vec<IpAddr>, String> {
            let mut call = self.call.lock().expect("the counter is not poisoned");
            let answer = self
                .answers
                .get(*call)
                .or_else(|| self.answers.last())
                .cloned()
                .unwrap_or_default();
            *call += 1;
            Ok(answer)
        }
    }

    /// An address literal from the text a target carries.
    fn parse(text: &str) -> IpAddr {
        text.parse()
            .unwrap_or_else(|_| panic!("`{text}` is not an address"))
    }

    /// The address of a refusal, or a panic naming what came back instead.
    fn refused_addr(result: Result<Approved, Refusal>) -> IpAddr {
        match result {
            Err(Refusal::Address { addr, .. }) => addr,
            other => panic!("expected the address to be refused, got {other:?}"),
        }
    }

    #[test]
    fn only_https_is_something_we_deliver_to() {
        let resolver = FakeResolver::with(&[("subscriber.test", &["93.184.216.34"])]);

        for target in [
            "http://subscriber.test/hook",
            "http://subscriber.test:8080/hook",
            "ftp://subscriber.test/hook",
        ] {
            assert!(
                matches!(check(target, &resolver), Err(Refusal::Scheme { .. })),
                "`{target}` is not a delivery target: {:?}",
                check(target, &resolver)
            );
        }
        assert!(
            resolver.asked().is_empty(),
            "a target we are not going to send to is not looked up either: {:?}",
            resolver.asked()
        );

        assert_eq!(
            check("https://subscriber.test/hook", &resolver),
            Ok(Approved {
                host: "subscriber.test".to_owned(),
                addr: parse("93.184.216.34"),
                port: 443,
            }),
            "https is the one scheme, and its default port is the one a connection would use"
        );
        assert_eq!(
            check("https://subscriber.test:8443/hook", &resolver),
            Ok(Approved {
                host: "subscriber.test".to_owned(),
                addr: parse("93.184.216.34"),
                port: 8443,
            }),
            "a target that names a port keeps it"
        );
        assert_eq!(resolver.asked(), vec!["subscriber.test"; 2]);

        assert_eq!(
            check("subscriber.test/hook", &resolver),
            Err(Refusal::Malformed {
                target: "subscriber.test/hook".to_owned(),
            }),
            "a target with no scheme is not a target"
        );
        assert_eq!(
            check("file:///etc/passwd", &resolver),
            Err(Refusal::Malformed {
                target: "file:///etc/passwd".to_owned(),
            }),
            "and neither is one with no host: `file://` is a path, not a place to deliver to"
        );
    }

    #[test]
    fn an_address_that_is_not_on_the_public_internet_is_refused() {
        let resolver = FakeResolver::default();

        let forbidden = [
            ("https://127.0.0.1/hook", "127.0.0.1"),
            ("https://127.1.2.3/hook", "127.1.2.3"),
            ("https://10.0.0.5/hook", "10.0.0.5"),
            ("https://172.16.4.4/hook", "172.16.4.4"),
            ("https://192.168.1.1/hook", "192.168.1.1"),
            ("https://100.64.0.1/hook", "100.64.0.1"),
            (
                "https://169.254.169.254/latest/meta-data/",
                "169.254.169.254",
            ),
            ("https://0.0.0.0/hook", "0.0.0.0"),
            ("https://224.0.0.1/hook", "224.0.0.1"),
            ("https://255.255.255.255/hook", "255.255.255.255"),
            ("https://[::1]/hook", "::1"),
            ("https://[::]/hook", "::"),
            ("https://[fc00::1]/hook", "fc00::1"),
            ("https://[fe80::1]/hook", "fe80::1"),
            ("https://[ff02::1]/hook", "ff02::1"),
            ("https://[::ffff:127.0.0.1]/hook", "::ffff:127.0.0.1"),
            ("https://[::ffff:10.0.0.5]/hook", "::ffff:10.0.0.5"),
        ];
        for (target, addr) in forbidden {
            assert_eq!(
                refused_addr(check(target, &resolver)),
                parse(addr),
                "`{target}` is the service's own network or the cloud's metadata service"
            );
        }
        assert!(
            resolver.asked().is_empty(),
            "a literal is its own answer: nothing was looked up to find out"
        );

        // And the addresses that are the point of the service.
        for (target, addr) in [
            ("https://93.184.216.34/hook", "93.184.216.34"),
            (
                "https://[2606:4700:4700::1111]/hook",
                "2606:4700:4700::1111",
            ),
        ] {
            assert_eq!(
                check(target, &resolver),
                Ok(Approved {
                    host: addr.to_owned(),
                    addr: parse(addr),
                    port: 443,
                }),
                "`{target}` is a public address"
            );
        }
        assert!(resolver.asked().is_empty());
    }

    #[test]
    fn a_name_that_is_local_by_definition_is_refused_without_resolving_it() {
        let resolver = FakeResolver::with(&[("subscriber.test", &["93.184.216.34"])]);

        for target in [
            "https://localhost/hook",
            "https://localhost:3000/hook",
            "https://api.localhost/hook",
            "https://metadata.google.internal/computeMetadata/v1/",
            "https://db.internal/hook",
        ] {
            assert!(
                matches!(check(target, &resolver), Err(Refusal::Name { .. })),
                "`{target}` is local by definition: {:?}",
                check(target, &resolver)
            );
        }
        assert!(
            resolver.asked().is_empty(),
            "the name list is the cheap half: it answers without a lookup, so a resolver that \
             lies about `localhost` does not get the chance: {:?}",
            resolver.asked()
        );

        // A name that is nobody's business but the resolver's still goes through it.
        assert!(check("https://subscriber.test/hook", &resolver).is_ok());
        assert_eq!(resolver.asked(), vec!["subscriber.test"]);
    }

    #[test]
    fn the_connection_goes_to_the_address_that_was_checked() {
        // The first answer is a real subscriber, the second is the service's own loopback: the
        // rebinding case, and the client that resolves again to connect is the hole.
        let resolver = Rebinding::new(&[&["93.184.216.34"], &["127.0.0.1"]]);

        assert_eq!(
            check("https://subscriber.test/hook", &resolver),
            Ok(Approved {
                host: "subscriber.test".to_owned(),
                addr: parse("93.184.216.34"),
                port: 443,
            }),
            "the address in the answer is the one that was validated"
        );
        assert_eq!(
            resolver.calls(),
            1,
            "the check resolved once: a second lookup is the one an attacker controls"
        );

        // What a client that checks the hostname and then resolves again at connect time sees.
        assert_eq!(
            refused_addr(check("https://subscriber.test/hook", &resolver)),
            parse("127.0.0.1"),
            "the second answer is refused, so the rebinding never reaches a socket"
        );
        assert_eq!(resolver.calls(), 2);
    }

    #[test]
    fn one_bad_address_in_the_answer_refuses_the_whole_answer() {
        for answers in [
            &["93.184.216.34", "10.0.0.5"][..],
            &["10.0.0.5", "93.184.216.34"][..],
            &["93.184.216.34", "169.254.169.254"][..],
        ] {
            let resolver = FakeResolver::with(&[("subscriber.test", answers)]);
            assert!(
                matches!(
                    check("https://subscriber.test/hook", &resolver),
                    Err(Refusal::Address { .. })
                ),
                "an answer that contains an address we may not connect to is refused, however \
                 good the rest of it looks: {answers:?}"
            );
            assert_eq!(
                resolver.asked(),
                vec!["subscriber.test"],
                "and it is refused from the one answer: there is no second lookup that might \
                 answer better"
            );
        }
    }

    #[test]
    fn a_name_that_does_not_resolve_is_a_refusal_and_not_a_panic() {
        let resolver = FakeResolver::with(&[("empty.test", &[])]);

        assert!(
            matches!(
                check("https://nothing.test/hook", &resolver),
                Err(Refusal::Resolution { .. })
            ),
            "NXDOMAIN is a refusal"
        );
        assert!(
            matches!(
                check("https://empty.test/hook", &resolver),
                Err(Refusal::Resolution { .. })
            ),
            "and so is a name that answers with nothing: the delivery is refused rather than sent \
             to whatever is at the end of a default route"
        );
    }
}
