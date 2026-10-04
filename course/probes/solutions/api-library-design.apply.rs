//! probe = "api/library-design@apply"
//! title = "The surface is a promise, and a break is a value"
//! hint = "Removal and a changed signature always break; a new variant or field breaks unless the type is `#[non_exhaustive]`; a new required method breaks unless the trait is sealed."
//!
//! Step `jobq/3.2-public-api-baseline` is where the engine stops being a program that happens
//! to be importable and becomes a library with a contract: the public surface is written down, a
//! witness file compiles against it, and a change that would break a caller is a *compile error*
//! in the course's own gate rather than a surprise in somebody else's build.
//!
//! Two halves of the same habit are re-checked here, and the second one is why the first exists.
//!
//! 1. **the surface is written down in the shape a caller writes it.** [`witness`] renders one
//!    line per public item. The two placeholders are the whole lesson: `..` where a caller would
//!    otherwise have to name every field, and `_` where a caller would otherwise have to name
//!    every variant. A witness that spells the fields out turns *adding* a field into a build
//!    break, which is the opposite of what the marker is for;
//! 2. **the difference between two surfaces is a value, not a diff a human reads.**
//!    [`compare`] classifies every difference as something a caller feels or something that is
//!    merely new, and it does it from the rules the release has to obey:
//!
//!    | difference | verdict |
//!    | --- | --- |
//!    | an item or a re-export that is gone | breaking, always |
//!    | a changed function signature or constant type | breaking, always |
//!    | a new enum variant | breaking — unless the enum is `#[non_exhaustive]` |
//!    | a new struct field | breaking — unless the struct is `#[non_exhaustive]` |
//!    | a new required method on a trait | breaking — unless the trait is sealed |
//!    | a trait that becomes sealed | breaking: an implementation outside the crate stops compiling |
//!    | a trait that opens up, a new item, a new re-export | compatible |
//!
//! ## What you write
//!
//! * [`witness`] — the surface as text, one line per item, in path order, with `..` and `_` for
//!   the two `#[non_exhaustive]` placeholders and `(sealed)`/`(open)` on a trait;
//! * [`compare`] — the differences between the surface a release shipped and the one it is about
//!   to ship, split into what was added and what breaks;
//! * [`Change::is_breaking`] — the one question the release script asks.
//!
//! [`Item`], [`Kind`] and the shape of the report are given: the surface is data here because a
//! single file cannot compile a witness against a crate that does not exist yet.
//!
//! ## What the tests ask
//!
//! The witness of a mixed surface is compared to the exact text, `..` and `_` included. Then
//! each rule gets its own pair of versions: the engine's own `Queue`, `JobState` and `Store`
//! before and after one edit, where the same edit is a break on one spelling of the type and
//! only an addition on the other.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, and **no crate to compile against**. The real gate is a witness
//! file that is copied into the learner's project and compiled (`semver@baseline`, handoff-m2
//! §1.3): deleting an item or changing a signature there is a build failure, which is stronger
//! than anything a test can assert. What is re-checked here is the *rule* behind that file —
//! which difference is a break, and which one only looks like one — because that rule is what
//! a learner has to be able to recite when they edit a public item at 6pm on a Friday.
//!
//! Known narrowings, stated rather than hidden: `pub(crate)` and re-exports are the same thing
//! as "the item is in the list or it is not", because a single file has no module tree to be
//! private in; a trait's *methods* are compared by name and not by signature, so a changed
//! method signature on a public trait is a break this file does not classify; and attributes
//! other than `#[non_exhaustive]` are not modelled.

/// One public item, as the release sees it: a path a caller writes, and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The path a caller names it by, re-export included (`jobq::engine::Queue`).
    pub path: String,
    /// What is at that path.
    pub kind: Kind,
}

/// What is at a public path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A public function: the signature is the contract.
    Fn {
        /// Parameter types, in order.
        params: Vec<String>,
        /// The return type.
        returns: String,
    },
    /// A public constant: its type is the contract.
    Const {
        /// The type a caller reads it as.
        ty: String,
    },
    /// A public struct with public fields.
    Struct {
        /// Field names, in declaration order.
        fields: Vec<String>,
        /// `#[non_exhaustive]`: a caller may not write the fields out and must use `..`.
        non_exhaustive: bool,
    },
    /// A public enum.
    Enum {
        /// Variant names, in declaration order.
        variants: Vec<String>,
        /// `#[non_exhaustive]`: a caller's `match` needs a `_` arm.
        non_exhaustive: bool,
    },
    /// A public trait.
    Trait {
        /// Required method names, in declaration order.
        methods: Vec<String>,
        /// Sealed: nobody outside the crate can implement it.
        sealed: bool,
    },
}

impl Kind {
    /// What this item is, as a word a report can use.
    #[must_use]
    pub fn noun(&self) -> &'static str {
        match self {
            Self::Fn { .. } => "function",
            Self::Const { .. } => "constant",
            Self::Struct { .. } => "struct",
            Self::Enum { .. } => "enum",
            Self::Trait { .. } => "trait",
        }
    }

    /// How a function's signature reads in a report.
    #[must_use]
    fn signature(params: &[String], returns: &str) -> String {
        format!("({}) -> {returns}", params.join(", "))
    }
}

/// What shipping the new surface does to a caller that compiled against the old one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Surface the new version added, in path order: a new item, or a new variant or field on a
    /// type that was allowed to grow (`jobq::JobState::Dead`).
    pub added: Vec<String>,
    /// Every difference a caller would feel, in path order. Empty means the release is
    /// compatible.
    pub breaking: Vec<String>,
}

impl Change {
    /// `true` when a caller that compiled before this release may stop compiling.
    #[must_use]
    pub fn is_breaking(&self) -> bool {
        !self.breaking.is_empty()
    }
}

/// The surface as a caller writes it, one item per line, in path order.
///
/// The two placeholders are what this rendering is for: a `#[non_exhaustive]` struct is witnessed
/// with `..` instead of its field list, and a `#[non_exhaustive]` enum with a trailing `_` instead
/// of its variant list, because that is exactly what a call site outside the crate has to write.
/// A witness that spells the fields out would make adding one a build failure, and a build
/// failure is how a caller learns about a change they were told not to care about.
#[must_use]
pub fn witness(surface: &[Item]) -> String {
    // Sorted by path, not by the order the surface was handed over in: the baseline file is
    // compared byte for byte, so the same surface has to render the same bytes however the
    // release happened to collect it.
    let mut items: Vec<&Item> = surface.iter().collect();
    items.sort_by(|left, right| left.path.cmp(&right.path));
    items
        .iter()
        .map(|item| witness_line(item))
        .collect::<Vec<String>>()
        .join("\n")
}

/// One line of the witness: what a caller writes to name this item and its whole contract.
fn witness_line(item: &Item) -> String {
    match &item.kind {
        Kind::Fn { params, returns } => {
            format!("fn {}{}", item.path, Kind::signature(params, returns))
        }
        Kind::Const { ty } => format!("const {}: {ty}", item.path),
        Kind::Struct {
            fields,
            non_exhaustive,
        } => format!(
            "struct {} {}",
            item.path,
            braces(fields, non_exhaustive, "..")
        ),
        Kind::Enum {
            variants,
            non_exhaustive,
        } => format!(
            "enum {} {}",
            item.path,
            braces(variants, non_exhaustive, "_")
        ),
        Kind::Trait { methods, sealed } => format!(
            "trait {} {} ({})",
            item.path,
            braces(methods, &false, ""),
            if *sealed { "sealed" } else { "open" }
        ),
    }
}

/// The `{ … }` of a type in a witness: its fields or variants, and the placeholder when the type
/// is `#[non_exhaustive]` — the one character that decides whether growing is a break.
fn braces(names: &[String], non_exhaustive: &bool, placeholder: &str) -> String {
    let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
    if *non_exhaustive {
        names.push(placeholder);
    }
    if names.is_empty() {
        "{}".to_owned()
    } else {
        format!("{{ {} }}", names.join(", "))
    }
}

/// Classify every difference between the surface `old` shipped and `new` is about to ship.
///
/// Additions are reported whether or not they break, because a release note needs both lists.
/// The rule the whole exercise turns on: **adding to a type is a break unless the type said it
/// would not be.** `#[non_exhaustive]` is that statement for a struct and an enum, and a sealed
/// trait is that statement for a trait — nobody outside the crate can have an implementation that
/// the new method would leave incomplete.
#[must_use]
pub fn compare(old: &[Item], new: &[Item]) -> Change {
    let mut before: Vec<&Item> = old.iter().collect();
    before.sort_by(|left, right| left.path.cmp(&right.path));
    let mut after: Vec<&Item> = new.iter().collect();
    after.sort_by(|left, right| left.path.cmp(&right.path));

    let mut added = Vec::new();
    let mut breaking = Vec::new();

    // A path that is new is an addition whichever way the release got there (a new item, or an
    // item that was `pub(crate)` and is now re-exported).
    for item in &after {
        if !before.iter().any(|other| other.path == item.path) {
            added.push(item.path.clone());
        }
    }
    for item in &before {
        let Some(other) = after.iter().find(|other| other.path == item.path) else {
            // Gone from the list, which is what a caller sees when it is deleted *or* when its
            // re-export is: to the caller those are one event.
            breaking.push(format!("`{}` is no longer public", item.path));
            continue;
        };
        difference(item, other, &mut added, &mut breaking);
    }

    added.sort();
    Change { added, breaking }
}

/// Every difference one item's kind has between two versions, split into what a caller gains and
/// what a caller loses.
fn difference(old: &Item, new: &Item, added: &mut Vec<String>, breaking: &mut Vec<String>) {
    let path = &old.path;
    match (&old.kind, &new.kind) {
        (
            Kind::Fn {
                params: before,
                returns: before_returns,
            },
            Kind::Fn {
                params: after,
                returns: after_returns,
            },
        ) => {
            if before != after || before_returns != after_returns {
                breaking.push(format!(
                    "`{path}` changed signature: `{}` to `{}`",
                    Kind::signature(before, before_returns),
                    Kind::signature(after, after_returns)
                ));
            }
        }
        (Kind::Const { ty: before }, Kind::Const { ty: after }) => {
            if before != after {
                breaking.push(format!("`{path}` changed type: `{before}` to `{after}`"));
            }
        }
        (
            Kind::Struct {
                fields: before,
                non_exhaustive: before_marked,
            },
            Kind::Struct {
                fields: after,
                non_exhaustive: after_marked,
            },
        ) => {
            for field in before {
                if !after.contains(field) {
                    breaking.push(format!("`{path}` removed the field `{field}`"));
                }
            }
            for field in after {
                if before.contains(field) {
                    continue;
                }
                if *after_marked {
                    added.push(format!("{path}::{field}"));
                } else {
                    breaking.push(format!(
                        "`{path}` added the field `{field}`, and it is not `#[non_exhaustive]`: \
                         a caller that wrote the fields out stops compiling"
                    ));
                }
            }
            if !before_marked && *after_marked {
                // The marker is a promise about the *next* release, and a caller written before
                // it wrote the fields out: gaining the marker is itself the break.
                breaking.push(format!(
                    "`{path}` is `#[non_exhaustive]` now: a caller that wrote the fields out \
                     stops compiling"
                ));
            }
        }
        (
            Kind::Enum {
                variants: before,
                non_exhaustive: before_marked,
            },
            Kind::Enum {
                variants: after,
                non_exhaustive: after_marked,
            },
        ) => {
            for variant in before {
                if !after.contains(variant) {
                    breaking.push(format!("`{path}` removed the variant `{variant}`"));
                }
            }
            for variant in after {
                if before.contains(variant) {
                    continue;
                }
                if *after_marked {
                    added.push(format!("{path}::{variant}"));
                } else {
                    breaking.push(format!(
                        "`{path}` added the variant `{variant}`, and it is not \
                         `#[non_exhaustive]`: a `match` outside the crate stops compiling"
                    ));
                }
            }
            if !before_marked && *after_marked {
                breaking.push(format!(
                    "`{path}` is `#[non_exhaustive]` now: a `match` outside the crate stops \
                     compiling"
                ));
            }
        }
        (
            Kind::Trait {
                methods: before,
                sealed: before_sealed,
            },
            Kind::Trait {
                methods: after,
                sealed: after_sealed,
            },
        ) => {
            for method in before {
                if !after.contains(method) {
                    breaking.push(format!("`{path}` no longer requires `{method}`"));
                }
            }
            for method in after {
                if before.contains(method) {
                    continue;
                }
                if *after_sealed {
                    // The whole payoff of sealing: the crate is the only implementor, so a new
                    // required method cannot leave somebody else's `impl` incomplete.
                    added.push(format!("{path}::{method}"));
                } else {
                    breaking.push(format!(
                        "`{path}` added the required method `{method}`, and it is not sealed: \
                         an implementation outside the crate stops compiling"
                    ));
                }
            }
            if !before_sealed && *after_sealed {
                breaking.push(format!(
                    "`{path}` is sealed now: an implementation outside the crate stops compiling"
                ));
            }
        }
        _ => breaking.push(format!(
            "`{path}` changed from a {} to a {}",
            old.kind.noun(),
            new.kind.noun()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(path: &str, params: &[&str], returns: &str) -> Item {
        Item {
            path: path.to_owned(),
            kind: Kind::Fn {
                params: params.iter().map(|param| (*param).to_owned()).collect(),
                returns: returns.to_owned(),
            },
        }
    }

    fn constant(path: &str, ty: &str) -> Item {
        Item {
            path: path.to_owned(),
            kind: Kind::Const { ty: ty.to_owned() },
        }
    }

    fn structure(path: &str, fields: &[&str], non_exhaustive: bool) -> Item {
        Item {
            path: path.to_owned(),
            kind: Kind::Struct {
                fields: fields.iter().map(|field| (*field).to_owned()).collect(),
                non_exhaustive,
            },
        }
    }

    fn enumeration(path: &str, variants: &[&str], non_exhaustive: bool) -> Item {
        Item {
            path: path.to_owned(),
            kind: Kind::Enum {
                variants: variants
                    .iter()
                    .map(|variant| (*variant).to_owned())
                    .collect(),
                non_exhaustive,
            },
        }
    }

    fn trait_item(path: &str, methods: &[&str], sealed: bool) -> Item {
        Item {
            path: path.to_owned(),
            kind: Kind::Trait {
                methods: methods.iter().map(|method| (*method).to_owned()).collect(),
                sealed,
            },
        }
    }

    #[test]
    fn the_witness_writes_the_surface_down_the_way_a_caller_writes_it() {
        assert_eq!(
            Kind::signature(&["usize".to_owned()], "u64"),
            "(usize) -> u64",
            "a report spells a signature the way a caller writes it"
        );

        let surface = vec![
            func(
                "jobq::Engine::submit",
                &["SubmitRequest"],
                "Result<JobId, Error>",
            ),
            constant("jobq::DEFAULT_MAX_ATTEMPTS", "u32"),
            structure("jobq::EngineConfig", &["workers", "queue"], true),
            enumeration(
                "jobq::JobState",
                &["Queued", "Running", "Succeeded", "Failed"],
                false,
            ),
            trait_item("jobq::Store", &["enqueue", "claim"], true),
        ];

        assert_eq!(
            witness(&surface),
            "const jobq::DEFAULT_MAX_ATTEMPTS: u32\n\
             fn jobq::Engine::submit(SubmitRequest) -> Result<JobId, Error>\n\
             struct jobq::EngineConfig { workers, queue, .. }\n\
             enum jobq::JobState { Queued, Running, Succeeded, Failed }\n\
             trait jobq::Store { enqueue, claim } (sealed)"
                .to_owned(),
            "one line per item, in path order, and the two `#[non_exhaustive]` placeholders where \
             the item carries one"
        );

        // The order is the paths', not the order the surface happened to be listed in: the same
        // surface written down twice has to render the same bytes, or the baseline file churns.
        let mut shuffled = surface.clone();
        shuffled.reverse();
        assert_eq!(witness(&shuffled), witness(&surface));

        // The same enum without the marker is witnessed with its variants and no `_`: that
        // difference is the whole reason the marker is in the source.
        let exhaustive = vec![enumeration("jobq::JobState", &["Queued", "Running"], true)];
        assert_eq!(
            witness(&exhaustive),
            "enum jobq::JobState { Queued, Running, _ }"
        );
        // And a struct with no fields at all, marked: `{ .. }` rather than `{  }`.
        let empty = vec![structure("jobq::EngineConfig", &[], true)];
        assert_eq!(witness(&empty), "struct jobq::EngineConfig { .. }");
        let open = vec![trait_item("jobq::Store", &["claim"], false)];
        assert_eq!(witness(&open), "trait jobq::Store { claim } (open)");
    }

    #[test]
    fn an_item_or_a_re_export_that_is_gone_is_a_break() {
        let old = vec![
            func(
                "jobq::Engine::submit",
                &["SubmitRequest"],
                "Result<JobId, Error>",
            ),
            constant("jobq::DEFAULT_MAX_ATTEMPTS", "u32"),
        ];
        // `DEFAULT_MAX_ATTEMPTS` is still a constant, but it is no longer re-exported from the
        // crate root: to a caller, that is the same event as deleting it.
        let new = vec![func(
            "jobq::Engine::submit",
            &["SubmitRequest"],
            "Result<JobId, Error>",
        )];

        let change = compare(&old, &new);
        assert!(
            change.is_breaking(),
            "a caller that wrote `jobq::DEFAULT_MAX_ATTEMPTS` stops compiling: {change:?}"
        );
        assert_eq!(
            change.breaking,
            vec!["`jobq::DEFAULT_MAX_ATTEMPTS` is no longer public"],
            "and the report names the path, not the file it used to live in"
        );
        assert!(change.added.is_empty(), "nothing was added: {change:?}");
    }

    #[test]
    fn a_changed_signature_or_type_is_a_break_however_small() {
        let old = vec![
            func(
                "jobq::Engine::claim",
                &["WorkerId", "Queue", "usize"],
                "Vec<Job>",
            ),
            constant("jobq::QUEUE_NAME_MAX", "usize"),
        ];
        let new = vec![
            // `usize` became `u32`: the same function name, a different contract.
            func(
                "jobq::Engine::claim",
                &["WorkerId", "Queue", "u32"],
                "Vec<Job>",
            ),
            constant("jobq::QUEUE_NAME_MAX", "u64"),
        ];

        let change = compare(&old, &new);
        assert_eq!(
            change.breaking,
            vec![
                "`jobq::Engine::claim` changed signature: `(WorkerId, Queue, usize) -> Vec<Job>` \
                 to `(WorkerId, Queue, u32) -> Vec<Job>`"
                    .to_owned(),
                "`jobq::QUEUE_NAME_MAX` changed type: `usize` to `u64`".to_owned(),
            ],
            "a parameter type is as much of a promise as a name is: {change:?}"
        );

        // Changing what an item *is* is the same kind of event, and it is not silently "one
        // removal and one addition".
        let replaced = vec![structure("jobq::Engine::claim", &["worker"], false)];
        let change = compare(&old, &replaced);
        assert!(
            change.breaking.iter().any(|reason| reason
                == "`jobq::Engine::claim` changed from a function to a struct"),
            "{change:?}"
        );
        assert_eq!(
            change.breaking.len(),
            2,
            "and the constant is still gone: {change:?}"
        );
    }

    #[test]
    fn a_new_enum_variant_breaks_unless_the_enum_says_it_will_grow() {
        let old = [enumeration(
            "jobq::JobState",
            &["Queued", "Running", "Succeeded", "Failed"],
            false,
        )];
        let new = [enumeration(
            "jobq::JobState",
            &["Queued", "Running", "Succeeded", "Failed", "Dead"],
            false,
        )];

        let change = compare(&old, &new);
        assert_eq!(
            change.breaking,
            vec![
                "`jobq::JobState` added the variant `Dead`, and it is not `#[non_exhaustive]`: a \
                 `match` outside the crate stops compiling"
                    .to_owned()
            ],
            "{change:?}"
        );

        // The same edit, one attribute later: a caller's `_` arm already covers it, and the
        // addition is still worth naming.
        let marked_old = [enumeration("jobq::JobState", &["Queued", "Running"], true)];
        let marked_new = [enumeration(
            "jobq::JobState",
            &["Queued", "Running", "Succeeded"],
            true,
        )];
        let change = compare(&marked_old, &marked_new);
        assert!(
            !change.is_breaking(),
            "`#[non_exhaustive]` is the promise that this is not a break: {change:?}"
        );
        assert_eq!(change.added, vec!["jobq::JobState::Succeeded"]);

        // Removing a variant is a break in both spellings: the marker says the enum may grow,
        // never that it may shrink.
        let change = compare(&marked_new, &marked_old);
        assert_eq!(
            change.breaking,
            vec!["`jobq::JobState` removed the variant `Succeeded`".to_owned()],
            "{change:?}"
        );
    }

    #[test]
    fn a_new_struct_field_breaks_unless_the_struct_says_it_will_grow() {
        let old = [structure("jobq::Job", &["id", "queue"], false)];
        let new = [structure("jobq::Job", &["id", "queue", "priority"], false)];

        let change = compare(&old, &new);
        assert_eq!(
            change.breaking,
            vec![
                "`jobq::Job` added the field `priority`, and it is not `#[non_exhaustive]`: a \
                 caller that wrote the fields out stops compiling"
                    .to_owned()
            ],
            "{change:?}"
        );

        let marked_old = [structure("jobq::SubmitRequest", &["queue"], true)];
        let marked_new = [structure(
            "jobq::SubmitRequest",
            &["queue", "priority"],
            true,
        )];
        assert_eq!(
            compare(&marked_old, &marked_new).added,
            vec!["jobq::SubmitRequest::priority"],
            "a marked struct growing is what the marker is for"
        );
        assert!(!compare(&marked_old, &marked_new).is_breaking());

        // A field that is *gone* is a break either way: the marker promises additions, not
        // subtractions.
        let change = compare(&marked_new, &marked_old);
        assert_eq!(
            change.breaking,
            vec!["`jobq::SubmitRequest` removed the field `priority`"]
        );
    }

    #[test]
    fn a_new_required_method_breaks_unless_the_trait_is_sealed() {
        let old = [trait_item("jobq::Store", &["enqueue", "claim"], false)];
        let new = [trait_item(
            "jobq::Store",
            &["enqueue", "claim", "reap"],
            false,
        )];

        let change = compare(&old, &new);
        assert_eq!(
            change.breaking,
            vec![
                "`jobq::Store` added the required method `reap`, and it is not sealed: an \
                 implementation outside the crate stops compiling"
                    .to_owned()
            ],
            "an open trait is implemented by callers, so its methods are a contract with them: \
             {change:?}"
        );

        // Sealed: the crate is the only implementor, so a new required method is the crate's own
        // business — and adding it is still reported, because it may be an addition a caller
        // cares about even though it cannot break one.
        let sealed_old = [trait_item("jobq::Clock", &["now"], true)];
        let sealed_new = [trait_item("jobq::Clock", &["now", "monotonic"], true)];
        let change = compare(&sealed_old, &sealed_new);
        assert!(
            !change.is_breaking(),
            "nobody outside the crate implements a sealed trait: {change:?}"
        );
        assert_eq!(change.added, vec!["jobq::Clock::monotonic"]);

        // Dropping a method is a break in both spellings: an implementor may have written one
        // and a caller may have called one.
        let change = compare(&sealed_new, &sealed_old);
        assert_eq!(
            change.breaking,
            vec!["`jobq::Clock` no longer requires `monotonic`"]
        );
    }

    #[test]
    fn sealing_a_trait_is_a_break_and_opening_one_is_not() {
        let open = [trait_item("jobq::Store", &["claim"], false)];
        let sealed = [trait_item("jobq::Store", &["claim"], true)];

        let change = compare(&open, &sealed);
        assert_eq!(
            change.breaking,
            vec![
                "`jobq::Store` is sealed now: an implementation outside the crate stops \
                 compiling"
                    .to_owned()
            ],
            "{change:?}"
        );
        assert!(
            !compare(&sealed, &open).is_breaking(),
            "opening a trait only makes something newly possible"
        );
    }

    #[test]
    fn a_release_that_only_adds_says_what_it_added() {
        let old = vec![
            func(
                "jobq::Engine::submit",
                &["SubmitRequest"],
                "Result<JobId, Error>",
            ),
            enumeration("jobq::JobState", &["Queued", "Running"], true),
            structure("jobq::EngineConfig", &["workers"], true),
        ];
        let new = vec![
            func(
                "jobq::Engine::submit",
                &["SubmitRequest"],
                "Result<JobId, Error>",
            ),
            func("jobq::Engine::cancel", &["JobId"], "Result<(), Error>"),
            enumeration("jobq::JobState", &["Queued", "Running", "Failed"], true),
            structure("jobq::EngineConfig", &["workers", "queue"], true),
            constant("jobq::DEFAULT_MAX_ATTEMPTS", "u32"),
        ];

        let change = compare(&old, &new);
        assert!(!change.is_breaking(), "{change:?}");
        assert_eq!(
            change.added,
            vec![
                "jobq::DEFAULT_MAX_ATTEMPTS".to_owned(),
                "jobq::Engine::cancel".to_owned(),
                "jobq::EngineConfig::queue".to_owned(),
                "jobq::JobState::Failed".to_owned(),
            ],
            "between two minor versions the release note is the `added` list: {change:?}"
        );

        // A surface compared to itself is a release with nothing to say, in both lists.
        assert_eq!(
            compare(&new, &new),
            Change {
                added: Vec::new(),
                breaking: Vec::new()
            }
        );
    }
}
