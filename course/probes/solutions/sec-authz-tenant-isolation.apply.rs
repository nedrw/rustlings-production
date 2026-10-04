//! probe = "sec/authz-tenant-isolation@apply"
//! title = "Somebody else's id is not found"
//! hint = "Compare the session's id with the path's before anything is read, and scope the query — the boundary is in the statement, not in a filter after it."
//!
//! Step `mailer/1.18-authz-boundary` adds one route and the whole of it is a boundary: `GET
//! /subscribers/{id}/deliveries`. Authentication says *who is asking*; the id in the path says
//! *what they asked for*; and the gap between the two is where most real data leaks live.
//!
//! Three things make the answer right, and this probe re-checks all three:
//!
//! * **the session is the only id a handler may use.** The path is a request, and a request is
//!   something the caller wrote;
//! * **a resource somebody else owns does not exist for this caller.** The answer is `404`, the same
//!   answer an id nobody has gets: `403` would confirm that the id exists, which turns a protected
//!   route into an enumeration oracle;
//! * **the check goes first, or the query is scoped — ideally both.** A refused request must not
//!   have read anything, and a permitted one must not have read somebody else's rows.
//!
//! That last rule is why [`Table`] counts what it served. `rows_served` is the probe's substitute
//! for reading the SQL: a handler that fetches everything and filters in Rust leaves the other
//! subscriber's rows in the process's memory — in a log line, a panic message, a metrics dimension,
//! or the next refactor's response body — and the table can see it.
//!
//! ## What you write
//!
//! [`history`] and [`Answer::status`]. The table, the rows and the statements are given: the mistake
//! this exercise is about is which statement the handler asks, and in what order it asks.

use std::cell::Cell;

/// One delivery this service owed a subscriber: a copy of one issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// Which issue it was.
    pub issue: String,
    /// Its subject, joined from the `issues` table.
    pub subject: String,
    /// When it was handed to a mail server, as the database wrote it.
    pub sent_at: String,
}

/// Who is asking: what the session resolved. Never what the path says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The subscriber the cookie or token belongs to.
    pub subscriber: u64,
}

/// One row of the `deliveries` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The subscriber this delivery belongs to. The column the `WHERE` clause is about.
    pub subscriber: u64,
    /// The delivery itself.
    pub delivery: Delivery,
}

/// The table, and the two statements a handler can write against it.
#[derive(Debug, Default)]
pub struct Table {
    rows: Vec<Row>,
    served: Cell<usize>,
}

impl Table {
    /// A table with one row per `(subscriber, issue, subject)`.
    #[must_use]
    pub fn new(rows: &[(u64, &str, &str)]) -> Self {
        let rows = rows
            .iter()
            .map(|(subscriber, issue, subject)| Row {
                subscriber: *subscriber,
                delivery: Delivery {
                    issue: (*issue).to_owned(),
                    subject: (*subject).to_owned(),
                    sent_at: "2024-01-01 00:00:00+00".to_owned(),
                },
            })
            .collect();
        Self {
            rows,
            served: Cell::new(0),
        }
    }

    /// `SELECT … FROM deliveries WHERE subscriber_id = $1` — the statement the route is about.
    #[must_use]
    pub fn scoped(&self, subscriber: u64) -> Vec<Delivery> {
        let rows: Vec<Delivery> = self
            .rows
            .iter()
            .filter(|row| row.subscriber == subscriber)
            .map(|row| row.delivery.clone())
            .collect();
        self.served.set(self.served.get() + rows.len());
        rows
    }

    /// `SELECT * FROM deliveries` — the statement nobody should write on this route.
    #[must_use]
    pub fn everything(&self) -> Vec<(u64, Delivery)> {
        self.served.set(self.served.get() + self.rows.len());
        self.rows
            .iter()
            .map(|row| (row.subscriber, row.delivery.clone()))
            .collect()
    }

    /// How many rows this table handed to the process, ever.
    #[must_use]
    pub fn rows_served(&self) -> usize {
        self.served.get()
    }
}

/// What the route answers.
///
/// [`Answer::NotFound`] is one variant on purpose: "it is not yours" and "it was never there" are
/// the same answer, down to the body, because telling them apart is how a route enumerates ids for
/// whoever asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The caller's own history.
    History {
        /// The rows this caller was owed.
        deliveries: Vec<Delivery>,
    },
    /// No session: `401`. Being nobody is not being somebody else.
    Unauthenticated,
    /// `404`, for somebody else's id and for an id nobody has alike.
    NotFound,
}

impl Answer {
    /// The status this answer becomes.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::History { .. } => 200,
            Self::Unauthenticated => 401,
            Self::NotFound => 404,
        }
    }
}

/// `GET /subscribers/{id}/deliveries`.
///
/// `identity` is what the session resolved, `path_id` is what the caller typed into the URL, and
/// the job is to keep them apart.
#[must_use]
pub fn history(table: &Table, identity: Option<&Identity>, path_id: u64) -> Answer {
    // Who is asking comes from the session. Without one there is nothing to compare the path with,
    // and being nobody is not being somebody else.
    let Some(identity) = identity else {
        return Answer::Unauthenticated;
    };

    // The comparison is one line, and it goes *before* anything is read: a request that is going to
    // be refused must not have had the other subscriber's rows in this process's memory — a log
    // line, a panic message and the next refactor's response body all read from there.
    if identity.subscriber != path_id {
        return Answer::NotFound;
    }

    // And the query is scoped as well. The handler's comparison is a fact about this handler; the
    // `WHERE` is a fact about the statement, which is what a later route cannot forget.
    Answer::History {
        deliveries: table.scoped(identity.subscriber),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two subscribers, each with deliveries of their own: what one of them must never see is the
    /// other's rows.
    fn table() -> Table {
        Table::new(&[
            (1, "issue-1", "Hello"),
            (1, "issue-2", "Again"),
            (2, "issue-1", "Hello"),
            (2, "issue-3", "Somebody else's news"),
        ])
    }

    fn ada() -> Identity {
        Identity { subscriber: 1 }
    }

    /// The issue names in an answer, so a failure names what leaked instead of a debug dump.
    fn issues(answer: &Answer) -> Vec<String> {
        match answer {
            Answer::History { deliveries } => deliveries
                .iter()
                .map(|delivery| delivery.issue.clone())
                .collect(),
            other => panic!("this request has to be answered with a history, not {other:?}"),
        }
    }

    #[test]
    fn a_subscriber_reads_their_own_history() {
        let answer = history(&table(), Some(&ada()), 1);
        assert_eq!(
            issues(&answer),
            vec!["issue-1".to_owned(), "issue-2".to_owned()],
            "the caller's own rows, and only those"
        );
        assert_eq!(answer.status(), 200, "their own data is theirs to read");
    }

    #[test]
    fn somebody_elses_id_and_an_id_nobody_has_get_the_same_answer() {
        let somebody_elses = history(&table(), Some(&ada()), 2);
        let nobodys = history(&table(), Some(&ada()), 999);

        assert_eq!(
            somebody_elses,
            Answer::NotFound,
            "an id that belongs to another subscriber is not this caller's to read"
        );
        assert_eq!(
            somebody_elses, nobodys,
            "the two answers have to be identical — status *and* body: a `403` here would confirm \
             that the id exists, which is an enumeration oracle"
        );
        assert_eq!(somebody_elses.status(), 404, "not yours is not found");
    }

    #[test]
    fn being_nobody_is_not_being_somebody_else() {
        let answer = history(&table(), None, 1);
        assert_eq!(
            answer,
            Answer::Unauthenticated,
            "without a session the route does not know who is asking, which is a different answer \
             from knowing and refusing"
        );
        assert_eq!(answer.status(), 401);

        // The three answers are three statuses: a route that answers one for all of them is a route
        // whose support queue is people asking what happened.
        let mut statuses = [
            history(&table(), Some(&ada()), 1).status(),
            Answer::Unauthenticated.status(),
            Answer::NotFound.status(),
        ];
        statuses.sort_unstable();
        assert_eq!(
            statuses,
            [200, 401, 404],
            "being nobody, asking for somebody else's, and asking for your own are three different \
             answers"
        );
    }

    #[test]
    fn a_refused_read_serves_no_rows_at_all() {
        // The check goes first: a request that is going to be refused must not have had the other
        // subscriber's rows in this process's memory.
        let table = table();
        history(&table, Some(&ada()), 2);
        history(&table, Some(&ada()), 999);
        history(&table, None, 1);
        assert_eq!(
            table.rows_served(),
            0,
            "a refused request reads nothing: fetching everything and filtering in Rust is the \
             same leak with extra steps"
        );

        // A permitted one reads exactly the caller's rows — not the table.
        let answer = history(&table, Some(&ada()), 1);
        assert_eq!(
            issues(&answer),
            vec!["issue-1".to_owned(), "issue-2".to_owned()]
        );
        assert_eq!(
            table.rows_served(),
            2,
            "two of the four rows belong to the caller; the boundary is in the statement"
        );

        // And the other subscriber's read is their own, from their own scope.
        let grace = Identity { subscriber: 2 };
        let answer = history(&table, Some(&grace), 2);
        assert_eq!(
            issues(&answer),
            vec!["issue-1".to_owned(), "issue-3".to_owned()]
        );
        assert_eq!(table.rows_served(), 4);
    }
}
