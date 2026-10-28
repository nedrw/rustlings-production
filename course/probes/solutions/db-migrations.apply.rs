//! probe = "db/migrations@apply"
//! title = "A migration that has run is history"
//! hint = "The schema is a sequence of files that only grows: an applied file is never edited, never deleted, and a new one never lands before the last one that ran."
//!
//! Step `mailer/1.9-postgres` makes the schema part of the build: numbered files applied in name
//! order, each recorded with a checksum. Editing a file that already ran is the failure the
//! checksum exists to catch — every deployment that applied the old text now disagrees with the
//! repository, and the next start fails on a migration it cannot explain. The same rule says what
//! to do instead: add `0002_…`.
//!
//! ## What you write
//!
//! [`plan`] — what the next start would do, given what the database has already applied:
//!
//! * a database with nothing applied gets every migration, in name order;
//! * otherwise every applied migration must still be there and unchanged:
//!   [`Problem::Missing`] when the file is gone, [`Problem::Modified`] when its checksum differs.
//!   These are checked first — a schema that disagrees with its own history is not a schema to add
//!   to;
//! * a *new* migration whose name sorts before the last applied one is
//!   [`Problem::OutOfOrder`]: it would have to be applied in the middle of history;
//! * everything else is pending, in name order.
//!
//! [`checksum`] is given: the same one the record was written with.
//!
//! ## What the tests ask
//!
//! An empty database; a database up to date; an edited file; a deleted file; a new file numbered
//! before the newest applied one; and up-to-date meaning exactly an empty plan.

/// One migration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// The file name, which is the version (`0001_subscribers.sql`).
    pub name: String,
    /// The file's text.
    pub sql: String,
}

/// Why the next start cannot go ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A migration the database applied is no longer in the repository.
    Missing(String),
    /// A migration the database applied has different text now.
    Modified(String),
    /// A new migration sorts before one that already ran.
    OutOfOrder(String),
}

/// What the next start would apply, in order; or why it must not.
///
/// `applied` is what the database recorded, `(name, checksum)` in application order.
pub fn plan(applied: &[(String, u64)], migrations: &[Migration]) -> Result<Vec<String>, Problem> {
    // The history first: an applied migration that is gone or edited means the repository and the
    // database disagree, and adding to a disagreement is how a schema splits in two.
    for (name, recorded) in applied {
        match migrations
            .iter()
            .find(|migration| &migration.name == name)
        {
            None => return Err(Problem::Missing(name.clone())),
            Some(migration) => {
                if checksum(&migration.sql) != *recorded {
                    return Err(Problem::Modified(name.clone()));
                }
            }
        }
    }

    let last_applied = applied.iter().map(|(name, _)| name.as_str()).max();
    let mut pending: Vec<&Migration> = migrations
        .iter()
        .filter(|migration| {
            !applied
                .iter()
                .any(|(name, _)| name == &migration.name)
        })
        .collect();
    pending.sort_by(|left, right| left.name.cmp(&right.name));

    if let Some(last) = last_applied {
        for migration in &pending {
            if migration.name.as_str() < last {
                return Err(Problem::OutOfOrder(migration.name.clone()));
            }
        }
    }
    Ok(pending
        .into_iter()
        .map(|migration| migration.name.clone())
        .collect())
}

/// The checksum a record was written with.
#[must_use]
pub fn checksum(sql: &str) -> u64 {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in sql.as_bytes() {
        state ^= u64::from(*byte);
        state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migration(name: &str, sql: &str) -> Migration {
        Migration {
            name: name.to_owned(),
            sql: sql.to_owned(),
        }
    }

    fn applied(migrations: &[Migration]) -> Vec<(String, u64)> {
        migrations
            .iter()
            .map(|migration| (migration.name.clone(), checksum(&migration.sql)))
            .collect()
    }

    fn three() -> Vec<Migration> {
        vec![
            migration("0001_subscribers.sql", "CREATE TABLE subscribers"),
            migration("0002_tokens.sql", "ALTER TABLE subscribers ADD token"),
            migration("0003_index.sql", "CREATE INDEX ON subscribers"),
        ]
    }

    #[test]
    fn an_empty_database_gets_everything_in_name_order() {
        let migrations = vec![
            migration("0002_tokens.sql", "b"),
            migration("0001_subscribers.sql", "a"),
        ];
        assert_eq!(
            plan(&[], &migrations),
            Ok(vec![
                "0001_subscribers.sql".to_owned(),
                "0002_tokens.sql".to_owned(),
            ])
        );
    }

    #[test]
    fn a_database_up_to_date_has_an_empty_plan() {
        let migrations = three();
        assert_eq!(plan(&applied(&migrations), &migrations), Ok(Vec::new()));
    }

    #[test]
    fn a_database_behind_gets_only_what_is_new() {
        let migrations = three();
        let applied = applied(&migrations[..2]);
        assert_eq!(
            plan(&applied, &migrations),
            Ok(vec!["0003_index.sql".to_owned()])
        );
    }

    #[test]
    fn an_edited_migration_is_refused_before_anything_is_added() {
        let migrations = three();
        let mut applied = applied(&migrations[..2]);
        // The file changed after it ran: same name, different checksum.
        applied[0].1 = checksum("CREATE TABLE something_else");
        assert_eq!(
            plan(&applied, &migrations),
            Err(Problem::Modified("0001_subscribers.sql".to_owned()))
        );
    }

    #[test]
    fn a_deleted_migration_is_refused() {
        let migrations = three();
        let applied = applied(&migrations[..2]);
        let without_first = &migrations[1..];
        assert_eq!(
            plan(&applied, without_first),
            Err(Problem::Missing("0001_subscribers.sql".to_owned()))
        );
    }

    #[test]
    fn a_new_migration_numbered_into_history_is_refused() {
        let migrations = three();
        let applied = applied(&migrations[..2]);
        let mut with_old_number = migrations.clone();
        with_old_number.push(migration("0001b_extra.sql", "ALTER TABLE subscribers ADD note"));
        assert_eq!(
            plan(&applied, &with_old_number),
            Err(Problem::OutOfOrder("0001b_extra.sql".to_owned()))
        );
    }

    #[test]
    fn a_new_migration_after_the_last_one_is_just_pending() {
        let migrations = three();
        let applied = applied(&migrations[..2]);
        let mut with_new = migrations.clone();
        with_new.push(migration("0004_audit.sql", "CREATE TABLE audit"));
        assert_eq!(
            plan(&applied, &with_new),
            Ok(vec![
                "0003_index.sql".to_owned(),
                "0004_audit.sql".to_owned(),
            ])
        );
    }
}
