//! probe = "test/integration-layering@apply"
//! title = "Every test owns what it touches"
//! hint = "Two tests on one resource is a collision scheduled by the test runner; a unit test on a real dependency is an integration test that forgot to be slow."
//!
//! Step `mailer/1.19-test-layering` runs the same service against a fake in unit tests and against
//! a real mail-server container in the integration ones. Two rules make that layering honest:
//!
//! * **isolation** — every test gets its own resources. Two tests that name the same port or the
//!   same database are not wrong today; they are wrong on the runner that schedules them
//!   together, which is the day nobody is looking;
//! * **the layer decides what may be real** — a unit test that reaches for `real:…` is an
//!   integration test in disguise: slow, environment-dependent, and it stops being a unit test.
//!
//! The suite here is data, because a probe runs offline and cannot start containers. What is
//! re-checked is the audit a reviewer would do by hand.
//!
//! ## What you write
//!
//! [`audit`] — every finding, sorted and deduplicated (the enum and its fields derive `Ord`):
//!
//! * [`Finding::Shared`] for each pair of tests that names the same resource, with the resource and
//!   both test names. A fake is a resource like any other: "it is only a fake" is how one test's
//!   inbox becomes the next test's starting state;
//! * [`Finding::UnitTouchesReal`] for a `Unit` test that names a `real:` resource.
//!
//! ## What the tests ask
//!
//! A properly layered suite (nothing); two tests on one port; a unit test with a real container;
//! two unit tests sharing a fake; one resource used once; and multiple findings arriving sorted.

use std::collections::BTreeSet;

/// Which layer a test belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Fast, no real dependency.
    Unit,
    /// Slower, allowed to talk to the real thing.
    Integration,
}

/// One test and what it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Test {
    /// The test's name, as the runner prints it.
    pub name: String,
    /// Its layer.
    pub layer: Layer,
    /// Resources it touches. A name starting with `real:` is a real dependency; anything else is a
    /// fixture.
    pub resources: Vec<String>,
}

/// Something the layering gets wrong.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Finding {
    /// Two tests name the same resource.
    Shared {
        /// The resource both name.
        resource: String,
        /// The first test, in name order.
        first: String,
        /// The second.
        second: String,
    },
    /// A unit test names a real dependency.
    UnitTouchesReal {
        /// The test.
        test: String,
        /// The resource it reached for.
        resource: String,
    },
}

/// Every finding about `tests`, sorted and deduplicated.
#[must_use]
pub fn audit(tests: &[Test]) -> Vec<Finding> {
    let mut findings = BTreeSet::new();
    // Name order first, so a pair is always reported the same way round.
    let mut ordered: Vec<&Test> = tests.iter().collect();
    ordered.sort_by(|left, right| left.name.cmp(&right.name));

    for (position, left) in ordered.iter().enumerate() {
        for right in ordered.iter().skip(position + 1) {
            for resource in &left.resources {
                if right.resources.contains(resource) {
                    findings.insert(Finding::Shared {
                        resource: resource.clone(),
                        first: left.name.clone(),
                        second: right.name.clone(),
                    });
                }
            }
        }
    }

    for test in &ordered {
        if test.layer != Layer::Unit {
            continue;
        }
        for resource in &test.resources {
            if resource.starts_with("real:") {
                findings.insert(Finding::UnitTouchesReal {
                    test: test.name.clone(),
                    resource: resource.clone(),
                });
            }
        }
    }

    findings.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test(name: &str, layer: Layer, resources: &[&str]) -> Test {
        Test {
            name: name.to_owned(),
            layer,
            resources: resources.iter().map(|r| (*r).to_owned()).collect(),
        }
    }

    fn shared(resource: &str, first: &str, second: &str) -> Finding {
        Finding::Shared {
            resource: resource.to_owned(),
            first: first.to_owned(),
            second: second.to_owned(),
        }
    }

    #[test]
    fn a_layered_suite_has_nothing_to_report() {
        let tests = [
            test("unit_signup", Layer::Unit, &["fake:mail", "fake:db"]),
            test(
                "integration_signup",
                Layer::Integration,
                &["real:mail", "db:signup"],
            ),
        ];
        assert_eq!(audit(&tests), []);
    }

    #[test]
    fn two_tests_on_one_port_collide() {
        let tests = [
            test("a_first", Layer::Integration, &["real:port:8080"]),
            test("b_second", Layer::Integration, &["real:port:8080"]),
        ];
        assert_eq!(
            audit(&tests),
            [shared("real:port:8080", "a_first", "b_second")]
        );
    }

    #[test]
    fn a_unit_test_that_reaches_for_the_real_thing_is_called_out() {
        let tests = [test("unit_send", Layer::Unit, &["real:mail"])];
        assert_eq!(
            audit(&tests),
            [Finding::UnitTouchesReal {
                test: "unit_send".to_owned(),
                resource: "real:mail".to_owned(),
            }]
        );
    }

    #[test]
    fn a_shared_fake_is_a_collision_too() {
        let tests = [
            test("a_one", Layer::Unit, &["fake:mail"]),
            test("b_two", Layer::Unit, &["fake:mail"]),
        ];
        assert_eq!(audit(&tests), [shared("fake:mail", "a_one", "b_two")]);
    }

    #[test]
    fn a_resource_used_once_is_fine() {
        let tests = [
            test("a_one", Layer::Unit, &["db:a"]),
            test("b_two", Layer::Unit, &["db:b"]),
        ];
        assert_eq!(audit(&tests), []);
    }

    #[test]
    fn findings_arrive_sorted_and_deduplicated() {
        let tests = [
            test("unit_send", Layer::Unit, &["real:mail", "fake:mail"]),
            test("unit_mail", Layer::Unit, &["fake:mail"]),
            test("integration_send", Layer::Integration, &["real:mail"]),
        ];
        assert_eq!(
            audit(&tests),
            [
                shared("fake:mail", "unit_mail", "unit_send"),
                shared("real:mail", "integration_send", "unit_send"),
                Finding::UnitTouchesReal {
                    test: "unit_send".to_owned(),
                    resource: "real:mail".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn three_tests_on_one_resource_are_three_pairs() {
        let tests = [
            test("a", Layer::Integration, &["real:port"]),
            test("b", Layer::Integration, &["real:port"]),
            test("c", Layer::Integration, &["real:port"]),
        ];
        let findings = audit(&tests);
        assert_eq!(findings.len(), 3, "{findings:?}");
        let mut expected = BTreeSet::new();
        expected.insert(shared("real:port", "a", "b"));
        expected.insert(shared("real:port", "a", "c"));
        expected.insert(shared("real:port", "b", "c"));
        let got: BTreeSet<Finding> = findings.into_iter().collect();
        assert_eq!(got, expected);
    }
}
