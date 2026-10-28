//! Spec for `rpc/8.1-field-numbers` — the wire is numbers, not names.

use rpc::{Breaking, Field, Kind, wire};

fn field(number: u32, name: &str, kind: Kind) -> Field {
    Field {
        number,
        name: name.to_owned(),
        kind,
    }
}

fn base() -> Vec<Field> {
    vec![
        field(1, "id", Kind::Text),
        field(2, "status", Kind::Text),
        field(3, "count", Kind::Number),
    ]
}

#[test]
fn renaming_a_field_is_compatible() {
    let mut new = base();
    new[1].name = "state".to_owned();
    assert_eq!(wire(&base(), &new), []);
}

#[test]
fn a_new_number_is_compatible() {
    let mut new = base();
    new.push(field(4, "trace", Kind::Text));
    assert_eq!(wire(&base(), &new), []);
}

#[test]
fn a_removed_number_is_breaking() {
    let new = vec![field(1, "id", Kind::Text)];
    assert_eq!(
        wire(&base(), &new),
        [Breaking::NumberRemoved(2), Breaking::NumberRemoved(3)]
    );
}

#[test]
fn the_same_number_with_another_type_is_breaking() {
    let mut new = base();
    new[2].kind = Kind::Text;
    assert_eq!(wire(&base(), &new), [Breaking::NumberReused(3)]);
}

#[test]
fn a_number_that_moved_is_a_removal_not_a_reuse() {
    let mut new = base();
    new[1].number = 9;
    assert_eq!(wire(&base(), &new), [Breaking::NumberRemoved(2)]);
}

#[test]
fn several_breaks_arrive_sorted() {
    let mut new = base();
    new[0].kind = Kind::Flag;
    new.remove(2);
    let breaking = wire(&base(), &new);
    assert_eq!(breaking.len(), 2, "{breaking:?}");
    assert!(
        breaking.windows(2).all(|pair| pair[0] <= pair[1]),
        "{breaking:?}"
    );
}
