//! Integrity checking for database files arriving from outside this machine.

use std::path::Path;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use log::debug;
use rusqlite::{Connection, OpenFlags};

/// A grans database always has this table, so its absence means the file is a
/// database but not one of ours.
const SENTINEL_TABLE: &str = "documents";

/// Verify that a file is a structurally sound grans database.
///
/// Intended for a freshly downloaded file, before it replaces a working
/// database. Runs SQLite's `quick_check`, which reads every page, then confirms
/// the file actually carries grans data: an empty file is a perfectly valid
/// SQLite database and passes `quick_check` on its own.
pub fn check_pulled_database(path: &Path) -> Result<()> {
    let start = Instant::now();

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {} as a SQLite database", path.display()))?;

    run_quick_check(&conn)?;
    require_grans_schema(&conn)?;

    debug!("integrity check passed in {:?}", start.elapsed());

    Ok(())
}

/// Ask SQLite to read every page and report structural damage.
fn run_quick_check(conn: &Connection) -> Result<()> {
    let result: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .context("running PRAGMA quick_check; the file is not a readable SQLite database")?;

    if result != "ok" {
        bail!("SQLite reported corruption: {}", result);
    }

    Ok(())
}

/// Confirm the database carries grans tables rather than merely being valid.
fn require_grans_schema(conn: &Connection) -> Result<()> {
    let found: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [SENTINEL_TABLE],
            |row| row.get(0),
        )
        .context("reading the database schema")?;

    if found == 0 {
        return Err(anyhow!(
            "no '{}' table, so this is not a grans database",
            SENTINEL_TABLE
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn valid_database(dir: &TempDir) -> std::path::PathBuf {
        let path = dir.path().join("good.db");
        let conn = Connection::open(&path).unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('d1', 'T', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        path
    }

    #[test]
    fn accepts_a_valid_database() {
        let dir = TempDir::new().unwrap();
        let path = valid_database(&dir);

        assert!(check_pulled_database(&path).is_ok());
    }

    #[test]
    fn rejects_a_file_that_is_not_sqlite() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonsense.db");
        std::fs::write(&path, b"<html>504 Gateway Timeout</html>").unwrap();

        let err = check_pulled_database(&path).unwrap_err();

        assert!(
            err.to_string().to_lowercase().contains("database"),
            "unhelpful message: {}",
            err
        );
    }

    /// A zero-length file is a valid empty SQLite database, so `quick_check`
    /// alone would wave it through.
    #[test]
    fn rejects_an_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.db");
        std::fs::write(&path, b"").unwrap();

        assert!(check_pulled_database(&path).is_err());
    }

    /// A real SQLite database that simply is not a grans database.
    #[test]
    fn rejects_a_database_without_grans_tables() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("other.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE unrelated (id INTEGER)", []).unwrap();
        drop(conn);

        assert!(check_pulled_database(&path).is_err());
    }

    /// Pins a known limit rather than a capability.
    ///
    /// grans builds its FTS5 tables with `content=`, and SQLite compares an
    /// external-content index against its source table only under FTS5's own
    /// `integrity-check` with `rank=1`. Neither `quick_check` nor
    /// `integrity_check` runs that, so an index that has drifted from its
    /// source passes both. Search then silently returns too few rows while the
    /// database looks healthy.
    ///
    /// Closing this needs a writable connection, which this deliberately
    /// read-only check does not take. If that changes, this test should start
    /// failing and be inverted.
    #[test]
    fn does_not_notice_an_fts_index_that_drifted_from_its_source() {
        let dir = TempDir::new().unwrap();
        let path = valid_database(&dir);

        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO transcript_utterances (id, document_id, text)
                 VALUES ('u1', 'd1', 'deployment rollback discussion'),
                        ('u2', 'd1', 'quarterly planning notes');
             INSERT INTO transcript_fts(transcript_fts) VALUES('rebuild');",
        )
        .unwrap();

        // Drop source rows without the index maintenance that db::transcripts
        // normally performs, leaving the index describing rows that are gone.
        conn.execute("DELETE FROM transcript_utterances", []).unwrap();
        drop(conn);

        assert!(
            check_pulled_database(&path).is_ok(),
            "if this now fails, the FTS drift gap has been closed; invert the test"
        );
    }

    #[test]
    fn rejects_a_truncated_database() {
        let dir = TempDir::new().unwrap();
        let path = valid_database(&dir);

        // Lop off the tail, leaving a valid header over a damaged body.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&path, &bytes).unwrap();

        assert!(check_pulled_database(&path).is_err());
    }
}
