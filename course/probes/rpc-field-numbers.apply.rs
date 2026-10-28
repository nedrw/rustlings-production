//! probe = "rpc/field-numbers@apply"
//! title = "Numbers on the wire, names in the file"
//! hint = "Compare by number: a removed number breaks a client that still sends it, and a number that changed type parses the old bytes as something else."
//!
//! Step `rpc/8.1-field-numbers` is the one fact that makes proto evolution understandable: the wire
//! carries `(number, value)` and never the name. This probe's input has no names at all — the
//! judgement is about numbers, so names are not part of the question.
//!
//! ## What you write
//!
//! [`wire`] — every breaking difference, sorted: [`Problem::Removed`] for a number that is gone,
//! [`Problem::Reused`] for one whose type changed.
//!
//! ## What the tests ask
//!
//! An unchanged message; an added number; a removed number; a type change on a stable number; a
//! number that moved; and several breaks sorted.

/// A field's type on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A length-delimited string.
    Text,
    /// A varint.
    Number,
    /// A single byte.
    Flag,
}

/// A change that breaks a client of the old message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Problem {
    /// A number is gone.
    Removed(u32),
    /// A number now carries another type.
    Reused(u32),
}

/// Every breaking difference between two messages, by number.
#[must_use]
pub fn wire(old: &[(u32, Kind)], new: &[(u32, Kind)]) -> Vec<Problem> {
    todo!("by number: removed, or the same number with another type")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Vec<(u32, Kind)> {
        vec![(1, Kind::Text), (2, Kind::Text), (3, Kind::Number)]
    }

    #[test]
    fn an_unchanged_message_breaks_nothing() {
        assert_eq!(wire(&base(), &base()), []);
    }

    #[test]
    fn a_new_number_is_compatible() {
        let mut new = base();
        new.push((4, Kind::Text));
        assert_eq!(wire(&base(), &new), []);
    }

    #[test]
    fn a_removed_number_is_breaking() {
        let new = vec![(1, Kind::Text)];
        assert_eq!(
            wire(&base(), &new),
            [Problem::Removed(2), Problem::Removed(3)]
        );
    }

    #[test]
    fn a_type_change_on_a_stable_number_is_breaking() {
        let mut new = base();
        new[2].1 = Kind::Text;
        assert_eq!(wire(&base(), &new), [Problem::Reused(3)]);
    }

    #[test]
    fn a_moved_number_is_a_removal() {
        let mut new = base();
        new[1].0 = 9;
        assert_eq!(wire(&base(), &new), [Problem::Removed(2)]);
    }

    #[test]
    fn several_breaks_arrive_sorted() {
        let mut new = base();
        new[0].1 = Kind::Flag;
        new.remove(2);
        let breaking = wire(&base(), &new);
        assert_eq!(breaking.len(), 2, "{breaking:?}");
        assert!(
            breaking.windows(2).all(|pair| pair[0] <= pair[1]),
            "{breaking:?}"
        );
    }
}
