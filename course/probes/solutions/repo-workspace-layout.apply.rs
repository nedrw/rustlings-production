//! probe = "repo/workspace-layout@apply"
//! title = "The arrow points one way"
//! hint = "A binary may depend on the library; the library may not depend on the binary — and no two packages may depend on each other."
//!
//! Step `mailer/1.1-workspace` splits one crate into a library and a thin binary, and the split is
//! a direction: the binary is a shell that reads configuration and calls `run`, so it depends on
//! the library and nothing depends on it. A library that reaches back into the binary is the split
//! undone — the same file, under two names.
//!
//! The workspace here is a list of packages, because a probe runs offline and cannot ask cargo.
//! What is re-checked is the shape that `cargo` would enforce much later and much less clearly: a
//! build that works is not a layering that works.
//!
//! ## What you write
//!
//! [`check`] — every violation, sorted and without duplicates:
//!
//! * [`Violation::LibraryDependsOnBinary`] for each edge from a `Lib` to a `Bin`;
//! * [`Violation::Cycle`] for each group of packages that can reach itself. One group is one
//!   violation, however many edges it has; a package that depends on itself is a cycle of one.
//!
//! A dependency on a package that is not in the list is not a layout problem — it is a build error,
//! and reporting it here would hide the real one.
//!
//! ## What the tests ask
//!
//! A clean split; a library reaching into the binary; a two-package cycle and a three-package
//! cycle (one violation each, members in name order, no duplicate reports); a missing package
//! ignored; and two different violations arriving sorted.

use std::collections::BTreeSet;

/// What a package is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A library: something to depend on.
    Lib,
    /// A binary: a shell that depends.
    Bin,
}

/// One package and what it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// The package's name.
    pub name: String,
    /// Library or binary.
    pub kind: Kind,
    /// Packages this one depends on, by name.
    pub deps: Vec<String>,
}

/// Something the layout gets wrong.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Violation {
    /// Packages that can reach themselves.
    Cycle {
        /// The members of the cycle, in name order.
        members: Vec<String>,
    },
    /// A library that depends on a binary.
    LibraryDependsOnBinary {
        /// The library that reached back.
        library: String,
        /// The binary it reached into.
        binary: String,
    },
}

/// Every violation of the layering rule, sorted and deduplicated.
#[must_use]
pub fn check(packages: &[Package]) -> Vec<Violation> {
    let mut violations = BTreeSet::new();

    // The direction rule: a library may not depend on a binary.
    for package in packages {
        for dependency in &package.deps {
            let Some(target) = packages
                .iter()
                .find(|candidate| &candidate.name == dependency)
            else {
                continue;
            };
            if package.kind == Kind::Lib && target.kind == Kind::Bin {
                violations.insert(Violation::LibraryDependsOnBinary {
                    library: package.name.clone(),
                    binary: target.name.clone(),
                });
            }
        }
    }

    // The cycle rule: one violation per strongly connected group, not one per edge. Tarjan's
    // algorithm, kept inside the body because a probe is one file and the exercise is the body.
    struct State {
        index: Vec<usize>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        cycles: Vec<Vec<usize>>,
    }

    fn visit(node: usize, adjacency: &[Vec<usize>], state: &mut State) {
        state.index[node] = state.next;
        state.low[node] = state.next;
        state.next += 1;
        state.stack.push(node);
        state.on_stack[node] = true;

        for &next in &adjacency[node] {
            if state.index[next] == usize::MAX {
                visit(next, adjacency, state);
                state.low[node] = state.low[node].min(state.low[next]);
            } else if state.on_stack[next] {
                state.low[node] = state.low[node].min(state.index[next]);
            }
        }

        if state.low[node] == state.index[node] {
            let mut members = Vec::new();
            while let Some(top) = state.stack.pop() {
                state.on_stack[top] = false;
                members.push(top);
                if top == node {
                    break;
                }
            }
            let self_edge = members.len() == 1 && adjacency[members[0]].contains(&members[0]);
            if members.len() > 1 || self_edge {
                state.cycles.push(members);
            }
        }
    }

    let adjacency: Vec<Vec<usize>> = packages
        .iter()
        .map(|package| {
            package
                .deps
                .iter()
                .filter_map(|dependency| {
                    packages
                        .iter()
                        .position(|candidate| &candidate.name == dependency)
                })
                .collect()
        })
        .collect();

    let mut state = State {
        index: vec![usize::MAX; packages.len()],
        low: vec![0; packages.len()],
        on_stack: vec![false; packages.len()],
        stack: Vec::new(),
        next: 0,
        cycles: Vec::new(),
    };
    for node in 0..packages.len() {
        if state.index[node] == usize::MAX {
            visit(node, &adjacency, &mut state);
        }
    }
    for members in state.cycles {
        let mut names: Vec<String> = members
            .iter()
            .map(|&member| packages[member].name.clone())
            .collect();
        names.sort();
        violations.insert(Violation::Cycle { members: names });
    }

    violations.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, kind: Kind, deps: &[&str]) -> Package {
        Package {
            name: name.to_owned(),
            kind,
            deps: deps.iter().map(|dep| (*dep).to_owned()).collect(),
        }
    }

    fn lib(name: &str, deps: &[&str]) -> Package {
        package(name, Kind::Lib, deps)
    }

    fn bin(name: &str, deps: &[&str]) -> Package {
        package(name, Kind::Bin, deps)
    }

    #[test]
    fn a_clean_split_has_nothing_to_report() {
        let workspace = [lib("mailer", &[]), bin("mailer-cli", &["mailer"])];
        assert_eq!(check(&workspace), []);
    }

    #[test]
    fn a_library_that_depends_on_the_binary_is_named() {
        let workspace = [lib("mailer", &["mailer-cli"]), bin("mailer-cli", &[])];
        assert_eq!(
            check(&workspace),
            [Violation::LibraryDependsOnBinary {
                library: "mailer".to_owned(),
                binary: "mailer-cli".to_owned(),
            }]
        );
    }

    #[test]
    fn a_two_package_cycle_is_one_violation_with_both_members() {
        let workspace = [lib("a", &["b"]), lib("b", &["a"])];
        assert_eq!(
            check(&workspace),
            [Violation::Cycle {
                members: vec!["a".to_owned(), "b".to_owned()],
            }]
        );
    }

    #[test]
    fn a_three_package_cycle_is_one_violation_not_three() {
        let workspace = [lib("a", &["b"]), lib("b", &["c"]), lib("c", &["a"])];
        assert_eq!(
            check(&workspace),
            [Violation::Cycle {
                members: vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            }]
        );
    }

    #[test]
    fn a_package_that_depends_on_itself_is_a_cycle_of_one() {
        let workspace = [lib("a", &["a"])];
        assert_eq!(
            check(&workspace),
            [Violation::Cycle {
                members: vec!["a".to_owned()],
            }]
        );
    }

    #[test]
    fn a_missing_package_is_a_build_error_not_a_layout_violation() {
        let workspace = [lib("mailer", &["does-not-exist"])];
        assert_eq!(check(&workspace), []);
    }

    #[test]
    fn both_kinds_of_violation_arrive_sorted() {
        let workspace = [
            lib("mailer", &["mailer-cli"]),
            bin("mailer-cli", &[]),
            lib("a", &["b"]),
            lib("b", &["a"]),
        ];
        let violations = check(&workspace);
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(
            matches!(violations[0], Violation::Cycle { .. }),
            "{violations:?}"
        );
        assert!(
            matches!(violations[1], Violation::LibraryDependsOnBinary { .. }),
            "{violations:?}"
        );
    }

    #[test]
    fn a_dependency_cycle_across_a_binary_is_still_a_cycle() {
        // The direction rule and the cycle rule are separate: a binary may depend on a library,
        // but if the library also depends on a *third* lib that depends back on the binary, the
        // graph has a loop regardless of the kinds.
        let workspace = [
            bin("app", &["core"]),
            lib("core", &["util"]),
            lib("util", &["app"]),
        ];
        let violations: BTreeSet<Violation> = check(&workspace).into_iter().collect();
        assert!(
            violations.contains(&Violation::Cycle {
                members: vec!["app".to_owned(), "core".to_owned(), "util".to_owned()],
            }),
            "{violations:?}"
        );
        assert!(
            violations.contains(&Violation::LibraryDependsOnBinary {
                library: "util".to_owned(),
                binary: "app".to_owned(),
            }),
            "{violations:?}"
        );
    }

    #[test]
    fn a_name_used_twice_is_not_a_new_package() {
        // The graph is by name; the first package with a name wins, and a duplicate is not a
        // second node. (A workspace cannot have two packages with one name; this pins that the
        // check does not invent a cycle out of the duplicate.)
        let workspace = [lib("a", &["b"]), lib("b", &[]), lib("a", &[])];
        assert_eq!(check(&workspace), []);
    }
}
