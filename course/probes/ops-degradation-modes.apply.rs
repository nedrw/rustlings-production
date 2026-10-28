//! probe = "ops/degradation-modes@apply"
//! title = "What still works when the dependency is gone"
//! hint = "A feature names what it needs and what it does without it: serve, serve stale, or refuse — and a dependency it does not need changes nothing."
//!
//! Step `relay/2.24-degradation-drill` writes down, per feature, what the service does when a
//! dependency is away. The answer is a decision somebody made in advance: reading a cached feed is
//! `serve stale`, sending a new newsletter is `refuse`, and a health check that needs nothing keeps
//! working. Without the table, the decision is made by whoever is on call at 3 a.m.
//!
//! ## What you write
//!
//! [`mode`] — what `feature` does while `down` names the dependencies that are away:
//!
//! * none of the feature's needs are down → [`Mode::Full`];
//! * a needed dependency is down → the feature's declared [`Fallback`]: `Refuse` becomes
//!   [`Mode::Refused`], `Serve` and `Stale` become [`Mode::Degraded`] with that fallback;
//! * a dependency the feature does not need being down changes nothing.
//!
//! ## What the tests ask
//!
//! Everything up; a needed dependency down with each of the three fallbacks; an unrelated
//! dependency down; two needs with one down; and a feature that needs nothing at all.

/// What a feature can do without the dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// Say no, clearly.
    Refuse,
    /// Answer from the last good data.
    Stale,
    /// Answer without the dependency's contribution.
    Serve,
}

/// What a feature is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every dependency it needs is there.
    Full,
    /// Running with a declared fallback.
    Degraded(Fallback),
    /// Refusing requests.
    Refused,
}

/// One feature and what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    /// The feature's name.
    pub name: &'static str,
    /// Dependencies it cannot be full without.
    pub needs: Vec<&'static str>,
    /// What it does when one of them is away.
    pub fallback: Fallback,
}

/// What `feature` is doing while `down` is away.
#[must_use]
pub fn mode(feature: &Feature, down: &[&str]) -> Mode {
    todo!("only the feature's own needs matter, and the fallback decides what is left")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(name: &'static str, needs: &[&'static str], fallback: Fallback) -> Feature {
        Feature {
            name,
            needs: needs.to_vec(),
            fallback,
        }
    }

    #[test]
    fn everything_up_is_full() {
        let feed = feature("feed", &["db"], Fallback::Stale);
        assert_eq!(mode(&feed, &[]), Mode::Full);
    }

    #[test]
    fn a_needed_dependency_down_uses_the_declared_fallback() {
        let feed = feature("feed", &["db"], Fallback::Stale);
        assert_eq!(mode(&feed, &["db"]), Mode::Degraded(Fallback::Stale));

        let newsletter = feature("newsletter", &["db"], Fallback::Refuse);
        assert_eq!(mode(&newsletter, &["db"]), Mode::Refused);

        let search = feature("search", &["db"], Fallback::Serve);
        assert_eq!(mode(&search, &["db"]), Mode::Degraded(Fallback::Serve));
    }

    #[test]
    fn an_unrelated_dependency_down_changes_nothing() {
        let feed = feature("feed", &["db"], Fallback::Stale);
        assert_eq!(mode(&feed, &["mail"]), Mode::Full);
    }

    #[test]
    fn one_of_two_needs_down_is_enough_to_degrade() {
        let publish = feature("publish", &["db", "mail"], Fallback::Refuse);
        assert_eq!(mode(&publish, &["db"]), Mode::Refused);
        assert_eq!(mode(&publish, &["mail"]), Mode::Refused);
        assert_eq!(mode(&publish, &["db", "mail"]), Mode::Refused);
    }

    #[test]
    fn a_feature_that_needs_nothing_is_full_while_the_world_burns() {
        let health = feature("healthz", &[], Fallback::Refuse);
        assert_eq!(mode(&health, &["db", "mail", "cache"]), Mode::Full);
    }
}
