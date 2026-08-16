//! The account log: every Granola account this database has synced from.
//!
//! Append-only: an account is recorded the first time a sync sees its token,
//! with the email captured then. Nothing enforces single-account use; data
//! from multiple accounts coexisting in one database is supported by design,
//! and the per-table `source_account_id` stamps record which rows arrived
//! under which account.

use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior};

/// Tables carrying a `source_account_id` column. Everything else that is
/// account-tied hangs off `documents` and derives provenance through
/// `document_id`.
const STAMPED_TABLES: [&str; 6] = [
    "documents",
    "people",
    "calendars",
    "events",
    "templates",
    "recipes",
];

/// One account this database has synced from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    /// Stable WorkOS user id from the JWT `sub` claim.
    pub account_id: String,
    /// Granola user UUID captured from get-user-info when first seen.
    pub granola_user_id: Option<String>,
    /// Email captured from get-user-info when first seen. Recording refuses
    /// email-less identities, so every row has one.
    pub email: String,
    /// RFC 3339 timestamp of when this account was first seen.
    pub first_seen_at: String,
}

/// Human-readable account label: the email with the WorkOS id alongside.
pub fn account_label(email: &str, account_id: &str) -> String {
    format!("{} ({})", email, account_id)
}

/// Whether this account has been recorded in the log before.
pub fn account_seen(conn: &Connection, account_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE account_id = ?1)",
        [account_id],
        |row| row.get(0),
    )?)
}

/// Every account this database has synced from, in first-seen order.
pub fn list_accounts(conn: &Connection) -> Result<Vec<AccountRecord>> {
    let mut stmt = conn.prepare(
        "SELECT account_id, granola_user_id, email, first_seen_at
         FROM accounts ORDER BY id",
    )?;
    let records = stmt
        .query_map([], |row| {
            Ok(AccountRecord {
                account_id: row.get(0)?,
                granola_user_id: row.get(1)?,
                email: row.get(2)?,
                first_seen_at: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(records)
}

/// Record a newly seen account in the log. Idempotent: recording an account
/// that is already in the log changes nothing, so the check-then-act pair of
/// [`account_seen`] and this function cannot duplicate rows however they
/// interleave.
///
/// When this is the first account ever recorded (the accounts table was
/// still empty inside the transaction), also backfill `source_account_id` on
/// every unstamped row of every stamped table: those rows entered the
/// database while it implicitly synced from this one account. Later accounts
/// never backfill; NULL is the honest value for rows whose account is
/// unknowable. The check and all writes share one immediate transaction, so
/// a concurrent recording cannot cause a stray backfill.
///
/// The documents backfill UPDATE fires the `documents_au` trigger per row (a
/// titles_fts delete+insert each), a one-time cost proportional to the
/// number of documents.
///
/// Returns the number of backfilled rows across all tables (always 0 for
/// every account after the first).
pub fn record_account(
    conn: &Connection,
    account_id: &str,
    granola_user_id: Option<&str>,
    email: &str,
) -> Result<usize> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    let first_account: bool =
        tx.query_row("SELECT COUNT(*) = 0 FROM accounts", [], |row| row.get(0))?;

    tx.execute(
        "INSERT INTO accounts (account_id, granola_user_id, email, first_seen_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_id) DO NOTHING",
        rusqlite::params![
            account_id,
            granola_user_id,
            email,
            chrono::Utc::now().to_rfc3339()
        ],
    )?;

    let mut backfilled = 0;
    if first_account {
        for table in STAMPED_TABLES {
            backfilled += tx.execute(
                &format!(
                    "UPDATE {} SET source_account_id = ?1 WHERE source_account_id IS NULL",
                    table
                ),
                [account_id],
            )?;
        }
    }

    tx.commit()?;
    Ok(backfilled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use serde_json::json;

    /// A database with unstamped rows in several stamped tables.
    fn db_with_rows() -> Connection {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "First"},
                "doc-2": {"id": "doc-2", "title": "Second"}
            }
        }));
        conn.execute("INSERT INTO people (id, name) VALUES ('p-1', 'Alice')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO events (id, summary) VALUES ('e-1', 'Standup')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO calendars (id) VALUES ('c-1')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO templates (id, title) VALUES ('t-1', 'Notes')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO recipes (id, slug) VALUES ('r-1', 's')", [])
            .unwrap();
        conn
    }

    fn stamp_of(conn: &Connection, table: &str, id: &str) -> Option<String> {
        conn.query_row(
            &format!("SELECT source_account_id FROM {} WHERE id = ?1", table),
            [id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn recording_the_same_account_twice_keeps_one_row() {
        // account_seen (bare connection) and record_account (its own
        // transaction) form a check-then-act pair; the insert itself must be
        // idempotent so a race between them cannot duplicate log rows.
        let conn = db_with_rows();

        record_account(&conn, "user_01AAA", Some("uuid-1"), "a@example.com").unwrap();
        record_account(&conn, "user_01AAA", Some("uuid-1"), "a@example.com").unwrap();

        let records = list_accounts(&conn).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].account_id, "user_01AAA");
    }

    #[test]
    fn account_seen_false_until_recorded() {
        let conn = db_with_rows();
        assert!(!account_seen(&conn, "user_01AAA").unwrap());

        record_account(&conn, "user_01AAA", Some("uuid-1"), "a@example.com").unwrap();
        assert!(account_seen(&conn, "user_01AAA").unwrap());
        assert!(!account_seen(&conn, "user_01BBB").unwrap());
    }

    #[test]
    fn first_account_backfills_every_stamped_table() {
        let conn = db_with_rows();

        let backfilled =
            record_account(&conn, "user_01AAA", Some("uuid-1"), "a@example.com").unwrap();
        // 2 documents + 1 each in people, events, calendars, templates, recipes.
        assert_eq!(backfilled, 7);

        for (table, id) in [
            ("documents", "doc-1"),
            ("documents", "doc-2"),
            ("people", "p-1"),
            ("events", "e-1"),
            ("calendars", "c-1"),
            ("templates", "t-1"),
            ("recipes", "r-1"),
        ] {
            assert_eq!(
                stamp_of(&conn, table, id),
                Some("user_01AAA".to_string()),
                "{}/{} should be backfilled",
                table,
                id
            );
        }

        let records = list_accounts(&conn).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].account_id, "user_01AAA");
        assert_eq!(records[0].email, "a@example.com");
        assert!(chrono::DateTime::parse_from_rfc3339(&records[0].first_seen_at).is_ok());
    }

    #[test]
    fn later_accounts_never_backfill() {
        let conn = db_with_rows();
        record_account(&conn, "user_01AAA", Some("uuid-1"), "a@example.com").unwrap();

        // An unstamped row arriving before the second account is recorded.
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc-3', 'Late')",
            [],
        )
        .unwrap();

        let backfilled =
            record_account(&conn, "user_01BBB", Some("uuid-2"), "b@example.com").unwrap();
        assert_eq!(backfilled, 0);
        assert_eq!(stamp_of(&conn, "documents", "doc-3"), None);

        // Rows stamped under the first account are untouched.
        assert_eq!(
            stamp_of(&conn, "documents", "doc-1"),
            Some("user_01AAA".to_string())
        );

        // Both accounts are in the log, in first-seen order.
        let records = list_accounts(&conn).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].account_id, "user_01AAA");
        assert_eq!(records[1].account_id, "user_01BBB");
    }
}
