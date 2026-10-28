//! probe = "api/pagination@apply"
//! title = "A cursor is a position, not an offset"
//! hint = "Page by the last row you saw, not by how many rows you have counted: rows added at the front move every offset behind them."
//!
//! Step `relay/2.4-cursor-pagination` is the rule that a page tells the caller where to continue
//! *by row*, not by count. The failure it prevents is quiet: with `OFFSET`, a row inserted while
//! somebody is reading moves every later page down by one, and the reader sees an item twice (or
//! never sees one at all) without any error.
//!
//! The feed here is a `Vec` in memory, because a probe runs offline and in seconds. The shape is
//! the query's:
//!
//! * ids are handed out in order, so a larger id is a newer row;
//! * a page is the newest `limit` rows *below the cursor* — `WHERE id < $cursor ORDER BY id DESC
//!   LIMIT $limit`;
//! * `next` is a promise: it exists only when a row below the last one on this page exists.
//!   `items.len() < limit` is not that promise — a full page can be the last one.
//!
//! ## What you write
//!
//! [`Feed::page_after`] — the rows below `cursor` (all of them when it is `None`), newest first,
//! at most `limit`, and where to continue. A cursor that no longer names a row still works: it is
//! an id, not a pointer, so a deleted row's position remains a usable place to continue from.
//!
//! ## What the tests ask
//!
//! Walking a feed of ten rows in pages of three; posting rows *between* two pages and continuing —
//! the new rows must not appear (they are above the cursor) and the old ones must not be skipped
//! or repeated; a page that happens to end exactly at the feed's end; a cursor whose row was
//! deleted; and the empty feed.

/// One row of the feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The id the feed handed out when the item was posted; larger means newer.
    pub id: u64,
    /// What the item says.
    pub title: String,
}

/// One page of the feed, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The rows on this page.
    pub items: Vec<Item>,
    /// Where to continue: the id of the last row on this page, or `None` when nothing follows.
    pub next: Option<u64>,
}

/// The feed.
#[derive(Debug, Default)]
pub struct Feed {
    items: Vec<Item>,
    next_id: u64,
}

impl Feed {
    /// An empty feed. The first posted item gets id 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
        }
    }

    /// Post an item and return the id it was given.
    pub fn post(&mut self, title: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Item {
            id,
            title: title.to_owned(),
        });
        id
    }

    /// Delete an item by id. Deleting something that is gone is not an error.
    pub fn delete(&mut self, id: u64) {
        self.items.retain(|item| item.id != id);
    }

    /// The newest rows below `cursor`, at most `limit` of them, and where to continue.
    #[must_use]
    pub fn page_after(&self, cursor: Option<u64>, limit: usize) -> Page {
        // `WHERE id < $cursor ORDER BY id DESC LIMIT $limit`: the rows are stored oldest first,
        // so the newest end is the back of the vector.
        let items: Vec<Item> = self
            .items
            .iter()
            .filter(|item| cursor.is_none_or(|cursor| item.id < cursor))
            .rev()
            .take(limit)
            .cloned()
            .collect();

        // `next` is a promise about the row *after* this page, so it is answered by looking for
        // one. `items.len() == limit` only means the page is full — the feed may end right there.
        let next = items.last().and_then(|last| {
            self.items
                .iter()
                .any(|item| item.id < last.id)
                .then_some(last.id)
        });
        Page { items, next }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(count: u64) -> Feed {
        let mut feed = Feed::new();
        for index in 1..=count {
            feed.post(&format!("item {index}"));
        }
        feed
    }

    fn ids(page: &Page) -> Vec<u64> {
        page.items.iter().map(|item| item.id).collect()
    }

    #[test]
    fn the_first_page_is_the_newest_and_its_cursor_is_its_last_row() {
        let feed = feed(5);
        let page = feed.page_after(None, 2);
        assert_eq!(ids(&page), [5, 4]);
        assert_eq!(page.next, Some(4));
    }

    #[test]
    fn walking_the_feed_visits_every_item_exactly_once() {
        let feed = feed(10);
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = feed.page_after(cursor, 3);
            if page.items.is_empty() {
                break;
            }
            seen.extend(ids(&page));
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen, (1..=10).rev().collect::<Vec<u64>>());
    }

    #[test]
    fn rows_posted_between_two_pages_do_not_shift_the_walk() {
        let mut feed = feed(5);
        let first = feed.page_after(None, 2);
        assert_eq!(ids(&first), [5, 4]);

        // New rows go to the front. An offset of two would now start at the wrong row.
        feed.post("item 6");
        feed.post("item 7");

        let second = feed.page_after(first.next, 2);
        assert_eq!(ids(&second), [3, 2], "the walk skipped or repeated a row");
        let third = feed.page_after(second.next, 2);
        assert_eq!(ids(&third), [1]);
        assert_eq!(third.next, None);
    }

    #[test]
    fn a_page_that_ends_exactly_at_the_end_promises_nothing() {
        let feed = feed(4);
        let first = feed.page_after(None, 2);
        assert_eq!(ids(&first), [4, 3]);
        assert_eq!(first.next, Some(3));

        let second = feed.page_after(first.next, 2);
        assert_eq!(ids(&second), [2, 1], "a full page is not a promise of more");
        assert_eq!(second.next, None);
    }

    #[test]
    fn a_cursor_whose_row_was_deleted_still_continues() {
        let mut feed = feed(5);
        let first = feed.page_after(None, 2);
        assert_eq!(ids(&first), [5, 4]);
        feed.delete(4);

        let second = feed.page_after(first.next, 2);
        assert_eq!(ids(&second), [3, 2]);
        assert_eq!(second.next, Some(2));
    }

    #[test]
    fn an_empty_feed_and_a_cursor_past_the_oldest_row_have_no_pages() {
        let empty = Feed::new();
        let page = empty.page_after(None, 3);
        assert!(page.items.is_empty());
        assert_eq!(page.next, None);

        let feed = feed(3);
        let page = feed.page_after(Some(0), 3);
        assert!(page.items.is_empty());
        assert_eq!(page.next, None);
    }
}
