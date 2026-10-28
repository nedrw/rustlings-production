//! Spec for `broker/7.8-schema-compat` — additive is compatible, taking information away is not.

use std::collections::BTreeMap;

use broker::schema::{Breaking, Field, Kind, compat};

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
    schema(&[("id", Kind::Text, false), ("status", Kind::Text, false)])
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
            kind: Kind::Text,
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
            kind: Kind::Text,
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
    new.remove("status");
    assert_eq!(
        compat(&base(), &new),
        [Breaking::Removed("status".to_owned())]
    );
}

#[test]
fn a_changed_type_is_breaking() {
    let mut new = base();
    new.insert(
        "status".to_owned(),
        Field {
            kind: Kind::Number,
            optional: false,
        },
    );
    assert_eq!(
        compat(&base(), &new),
        [Breaking::TypeChanged("status".to_owned())]
    );
}

#[test]
fn several_breaks_arrive_sorted() {
    let mut new = schema(&[("id", Kind::Text, false), ("status", Kind::Number, false)]);
    new.insert(
        "tenant".to_owned(),
        Field {
            kind: Kind::Text,
            optional: false,
        },
    );
    let breaking = compat(&base(), &new);
    assert_eq!(breaking.len(), 2, "{breaking:?}");
    assert!(
        breaking.windows(2).all(|pair| pair[0] <= pair[1]),
        "{breaking:?}"
    );
}
