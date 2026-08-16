//! Account binding: which Granola account this database is bound to.
//!
//! The `accounts` table is append-only history; the active binding is the
//! most recently inserted row. Rebinding appends rather than overwriting, so
//! "which account was this database bound to, and when" stays answerable.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

/// The binding currently in force: the most recently appended accounts row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveBinding {
    /// Stable WorkOS user id from the JWT `sub` claim.
    pub account_id: String,
    /// Granola user UUID captured from get-user-info at bind time.
    pub granola_user_id: Option<String>,
    /// Email captured from get-user-info at bind time.
    pub email: Option<String>,
    /// RFC 3339 timestamp of when the binding was created.
    pub first_seen_at: String,
}

/// Human-readable account label: the email when one was captured, with the
/// WorkOS id alongside; just the id otherwise.
pub fn account_label(email: Option<&str>, account_id: &str) -> String {
    match email {
        Some(email) => format!("{} ({})", email, account_id),
        None => account_id.to_string(),
    }
}

/// Fetch the active binding, or None if the database has never been bound.
pub fn get_active_binding(conn: &Connection) -> Result<Option<ActiveBinding>> {
    Ok(conn
        .query_row(
            "SELECT account_id, granola_user_id, email, first_seen_at
             FROM accounts ORDER BY id DESC LIMIT 1",
            [],
            |row| {
                Ok(ActiveBinding {
                    account_id: row.get(0)?,
                    granola_user_id: row.get(1)?,
                    email: row.get(2)?,
                    first_seen_at: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// Append a binding row, making it the active binding. Never touches
/// `documents.source_account_id`: a caller reaching for this (rebind) is
/// declaring an account switch, and stamping already-present rows with the
/// new account would fabricate provenance. NULL is the honest value for
/// rows whose account is unknowable.
pub fn bind_account(
    conn: &Connection,
    account_id: &str,
    granola_user_id: Option<&str>,
    email: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO accounts (account_id, granola_user_id, email, first_seen_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            account_id,
            granola_user_id,
            email,
            chrono::Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// Append a binding row and, when this is the first-ever binding (the
/// accounts table was still empty inside the transaction), backfill
/// `documents.source_account_id` on every unstamped row: those rows entered
/// the database while it implicitly belonged to this account. For sync's
/// auto-bind only; rebind uses [`bind_account`].
///
/// The check and both writes share one immediate transaction, so a binding
/// appearing between the caller's "no binding" check and this call cannot
/// cause a stray backfill. The backfill UPDATE fires the `documents_au`
/// trigger per row (a titles_fts delete+insert each), a one-time cost
/// proportional to the number of documents.
///
/// Returns the number of backfilled documents.
pub fn bind_account_with_backfill(
    conn: &Connection,
    account_id: &str,
    granola_user_id: Option<&str>,
    email: &str,
) -> Result<usize> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    let first_bind: bool =
        tx.query_row("SELECT COUNT(*) = 0 FROM accounts", [], |row| row.get(0))?;

    tx.execute(
        "INSERT INTO accounts (account_id, granola_user_id, email, first_seen_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            account_id,
            granola_user_id,
            email,
            chrono::Utc::now().to_rfc3339()
        ],
    )?;

    let backfilled = if first_bind {
        tx.execute(
            "UPDATE documents SET source_account_id = ?1 WHERE source_account_id IS NULL",
            [account_id],
        )?
    } else {
        0
    };

    tx.commit()?;
    Ok(backfilled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use serde_json::json;

    fn db_with_documents() -> Connection {
        build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "First"},
                "doc-2": {"id": "doc-2", "title": "Second"}
            }
        }))
    }

    fn source_account_of(conn: &Connection, doc_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT source_account_id FROM documents WHERE id = ?1",
            [doc_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn get_active_binding_none_when_never_bound() {
        let conn = db_with_documents();
        assert_eq!(get_active_binding(&conn).unwrap(), None);
    }

    #[test]
    fn backfilling_bind_stamps_existing_documents_on_first_bind() {
        let conn = db_with_documents();

        let backfilled =
            bind_account_with_backfill(&conn, "user_01AAA", Some("uuid-1"), "old@example.com")
                .unwrap();
        assert_eq!(backfilled, 2);

        assert_eq!(
            source_account_of(&conn, "doc-1"),
            Some("user_01AAA".to_string())
        );
        assert_eq!(
            source_account_of(&conn, "doc-2"),
            Some("user_01AAA".to_string())
        );

        let binding = get_active_binding(&conn).unwrap().unwrap();
        assert_eq!(binding.account_id, "user_01AAA");
        assert_eq!(binding.granola_user_id.as_deref(), Some("uuid-1"));
        assert_eq!(binding.email.as_deref(), Some("old@example.com"));
        assert!(chrono::DateTime::parse_from_rfc3339(&binding.first_seen_at).is_ok());
    }

    #[test]
    fn backfilling_bind_on_already_bound_db_does_not_backfill() {
        // The guard against a binding appearing between the caller's check
        // and the insert: the backfill fires only when the accounts table is
        // still empty inside the transaction.
        let conn = db_with_documents();
        bind_account_with_backfill(&conn, "user_01AAA", Some("uuid-1"), "old@example.com").unwrap();

        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc-3', 'Late')",
            [],
        )
        .unwrap();

        let backfilled =
            bind_account_with_backfill(&conn, "user_01BBB", Some("uuid-2"), "new@example.com")
                .unwrap();
        assert_eq!(backfilled, 0);
        assert_eq!(source_account_of(&conn, "doc-3"), None);
    }

    #[test]
    fn plain_bind_never_backfills_even_on_a_never_bound_db() {
        // Regression test for rebind semantics: a user running rebind is
        // declaring "I switched accounts", so stamping pre-existing rows
        // with the new account would fabricate provenance. NULL is the
        // honest value; only sync's auto-bind backfills.
        let conn = db_with_documents();

        bind_account(&conn, "user_01BBB", Some("uuid-2"), "new@example.com").unwrap();

        assert_eq!(source_account_of(&conn, "doc-1"), None);
        assert_eq!(source_account_of(&conn, "doc-2"), None);

        let binding = get_active_binding(&conn).unwrap().unwrap();
        assert_eq!(binding.account_id, "user_01BBB");
    }

    #[test]
    fn plain_bind_appends_history_and_leaves_stamps_alone() {
        let conn = db_with_documents();
        bind_account_with_backfill(&conn, "user_01AAA", Some("uuid-1"), "old@example.com").unwrap();

        bind_account(&conn, "user_01BBB", Some("uuid-2"), "new@example.com").unwrap();

        // Provenance of already-stamped rows is untouched.
        assert_eq!(
            source_account_of(&conn, "doc-1"),
            Some("user_01AAA".to_string())
        );

        // The new binding is active; history keeps both rows.
        let binding = get_active_binding(&conn).unwrap().unwrap();
        assert_eq!(binding.account_id, "user_01BBB");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
