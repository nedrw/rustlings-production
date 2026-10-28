//! probe = "broker/replay@apply"
//! title = "A replay is a slice, clamped and half-open"
//! hint = "`[from, to)`, both ends clamped to the log: `to` is not read, `from` past the end is empty, and an inverted range is empty rather than a panic."
//!
//! Step `broker/7.5-replay` reads an old range without touching anybody's position. This probe asks
//! for that range as a slice — the boundary arithmetic is the whole exercise, and a slice makes the
//! clamping visible.
//!
//! ## What you write
//!
//! [`range`] — `records[from..to]`, with both ends clamped to the log's length and an inverted or
//! empty range answering `&[]`.
//!
//! ## What the tests ask
//!
//! The half-open range; `to` past the end; `from` past the end; an inverted range; an empty range;
//! and the same range twice giving the same records.

/// `[from, to)` of `records`, clamped and never panicking.
#[must_use]
pub fn range(records: &[String], from: u64, to: u64) -> &[String] {
    // Both ends are caller input: clamp them to the log, and an inverted range is empty rather
    // than the panic a raw slice index would be.
    let len = records.len() as u64;
    let start = from.min(len) as usize;
    let end = to.min(len) as usize;
    if end <= start {
        return &[];
    }
    &records[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> Vec<String> {
        ["one", "two", "three", "four"]
            .iter()
            .map(|record| (*record).to_owned())
            .collect()
    }

    #[test]
    fn the_range_is_half_open() {
        let records = records();
        assert_eq!(range(&records, 1, 3), ["two".to_owned(), "three".to_owned()]);
    }

    #[test]
    fn to_past_the_end_is_clamped() {
        let records = records();
        assert_eq!(range(&records, 2, 99).len(), 2);
    }

    #[test]
    fn from_past_the_end_is_empty() {
        let records = records();
        assert!(range(&records, 9, 12).is_empty());
    }

    #[test]
    fn an_inverted_range_is_empty_not_a_panic() {
        let records = records();
        assert!(range(&records, 3, 1).is_empty());
    }

    #[test]
    fn an_empty_range_is_empty() {
        let records = records();
        assert!(range(&records, 2, 2).is_empty());
    }

    #[test]
    fn the_same_range_twice_is_the_same_records() {
        let records = records();
        assert_eq!(range(&records, 0, 2), range(&records, 0, 2));
    }
}
