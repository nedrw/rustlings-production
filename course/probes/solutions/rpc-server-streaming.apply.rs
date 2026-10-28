//! probe = "rpc/server-streaming@apply"
//! title = "A window is the size of a batch between acknowledgements"
//! hint = "Cut the items into batches of the window: the sender stops there and waits, and a window of zero sends nothing at all."
//!
//! Step `rpc/8.4-server-streaming` is the flow-control window seen from the sender's side. This
//! probe asks for the batches it produces — the same arithmetic, without the state machine.
//!
//! ## What you write
//!
//! [`chunks`] — `items` cut into consecutive slices of at most `window`; an empty list for a window
//! of zero or no items.
//!
//! ## What the tests ask
//!
//! A window of two over five items; the last short batch; a window larger than the list; a window of
//! one; a window of zero; and no items.

/// `items` in batches of at most `window`.
#[must_use]
pub fn chunks<'a>(items: &'a [&'a str], window: usize) -> Vec<&'a [&'a str]> {
    if window == 0 {
        return Vec::new();
    }
    items.chunks(window).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<&'static str> {
        vec!["a", "b", "c", "d", "e"]
    }

    #[test]
    fn a_window_of_two_over_five_items_is_three_batches() {
        let items = items();
        let chunks = chunks(&items, 2);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], ["a", "b"]);
        assert_eq!(chunks[1], ["c", "d"]);
        assert_eq!(chunks[2], ["e"], "the last batch is short");
    }

    #[test]
    fn a_window_larger_than_the_list_is_one_batch() {
        let items = items();
        assert_eq!(chunks(&items, 99).len(), 1);
    }

    #[test]
    fn a_window_of_one_is_one_item_each() {
        let items = items();
        let chunks = chunks(&items, 1);
        assert_eq!(chunks.len(), 5);
        assert!(chunks.iter().all(|chunk| chunk.len() == 1));
    }

    #[test]
    fn a_zero_window_sends_nothing() {
        let items = items();
        assert!(chunks(&items, 0).is_empty());
    }

    #[test]
    fn no_items_is_no_batches() {
        let empty: Vec<&str> = Vec::new();
        assert!(chunks(&empty, 3).is_empty());
    }

    #[test]
    fn every_batch_is_within_the_window() {
        let items = items();
        for window in 1..=6 {
            assert!(
                chunks(&items, window)
                    .iter()
                    .all(|chunk| chunk.len() <= window),
                "window {window}"
            );
        }
    }
}
