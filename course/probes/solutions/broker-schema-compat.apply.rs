//! probe = "broker/schema-compat@apply"
//! title = "Additions are compatible, subtractions are not"
//! hint = "A removed field, a changed type and a required addition all take information away; an optional addition is the one that does not."
//!
//! Step `broker/7.8-schema-compat` judges a schema change from the consumer of the old shape. This
//! probe asks for the same judgement with the schema as a sorted list — the order of the answer is
//! part of it.
//!
//! ## What you write
//!
//! [`compat`] — every breaking difference between `old` and `new`, in this order:
//! [`Breaking::Removed`], [`Breaking::TypeChanged`], [`Breaking::RequiredAdded`], each sorted by
//! name inside its kind.
//!
//! ## What the tests ask
//!
//! An unchanged schema; an added optional field; an added required field; a removed field; a type
//! change; and a rename, which is both a removal and a required addition.

use std::collections::BTreeMap;

/// A field's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A string.
    Str,
    /// An integer.
    Int,
    /// A boolean.
    Bool,
}

/// One field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// Its type.
    pub kind: Kind,
    /// Whether a reader can ignore it.
    pub optional: bool,
}

/// A change that breaks a consumer of the old schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Breaking {
    /// A field is gone.
    Removed(String),
    /// A field's type moved.
    TypeChanged(String),
    /// A new field is required.
    RequiredAdded(String),
}

/// Every breaking difference, in kind order and sorted by name inside a kind.
#[must_use]
pub fn compat(old: &BTreeMap<String, Field>, new: &BTreeMap<String, Field>) -> Vec<Breaking> {
    let mut removed = Vec::new();
    let mut type_changed = Vec::new();
    let mut required_added = Vec::new();
    for (name, field) in old {
        match new.get(name) {
            None => removed.push(Breaking::Removed(name.clone())),
            Some(replacement) if replacement.kind != field.kind => {
                type_changed.push(Breaking::TypeChanged(name.clone()));
            }
            Some(_) => {}
        }
    }
    for (name, field) in new {
        if !old.contains_key(name) && !field.optional {
            required_added.push(Breaking::RequiredAdded(name.clone()));
        }
    }
    removed.extend(type_changed);
    removed.extend(required_added);
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(fields: &[(&str, Kind, bool)]) -> BTreeMap<String, Field> {
        fields
            .iter()
            .map(|(name, kind, optional)| {
                (
                    (*name).to_owned(),
                    Field {
                        kind: *kind,
                        optional: *optional,
                    },
                )
            })
            .collect()
    }

    fn base() -> BTreeMap<String, Field> {
        schema(&[("id", Kind::Str, false), ("count", Kind::Int, false)])
    }

    #[test]
    fn an_unchanged_schema_breaks_nothing() {
        assert_eq!(compat(&base(), &base()), []);
    }

    #[test]
    fn an_added_optional_field_is_compatible() {
        let mut new = base();
        new.insert(
            "trace".to_owned(),
            Field {
                kind: Kind::Str,
                optional: true,
            },
        );
        assert_eq!(compat(&base(), &new), []);
    }

    #[test]
    fn an_added_required_field_is_breaking() {
        let mut new = base();
        new.insert(
            "tenant".to_owned(),
            Field {
                kind: Kind::Str,
                optional: false,
            },
        );
        assert_eq!(
            compat(&base(), &new),
            [Breaking::RequiredAdded("tenant".to_owned())]
        );
    }

    #[test]
    fn a_removed_field_is_breaking() {
        let mut new = base();
        new.remove("count");
        assert_eq!(
            compat(&base(), &new),
            [Breaking::Removed("count".to_owned())]
        );
    }

    #[test]
    fn a_type_change_is_breaking() {
        let mut new = base();
        new.insert(
            "count".to_owned(),
            Field {
                kind: Kind::Bool,
                optional: false,
            },
        );
        assert_eq!(
            compat(&base(), &new),
            [Breaking::TypeChanged("count".to_owned())]
        );
    }

    #[test]
    fn a_rename_is_a_removal_and_a_required_addition() {
        let mut new = base();
        new.remove("count");
        new.insert(
            "total".to_owned(),
            Field {
                kind: Kind::Int,
                optional: false,
            },
        );
        assert_eq!(
            compat(&base(), &new),
            [
                Breaking::Removed("count".to_owned()),
                Breaking::RequiredAdded("total".to_owned()),
            ]
        );
    }
}
