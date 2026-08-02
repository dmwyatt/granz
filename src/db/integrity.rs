//! Integrity checking: structural soundness of a database file arriving from
//! outside this machine, and agreement between an FTS5 index and its source.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use log::debug;
use rusqlite::{Connection, ErrorCode, OpenFlags};

/// A grans database always has this table, so its absence means the file is a
/// database but not one of ours.
const SENTINEL_TABLE: &str = "documents";

/// The FTS5 indexes that have a maintained write path, and so are expected to
/// agree with their source tables.
///
/// `notes_fts` is absent on purpose. Nothing has ever populated it in
/// production, so it fails this check on every database in existence and would
/// report a known gap as if it were damage. It belongs here once #85 gives it a
/// write path.
const MAINTAINED_FTS_TABLES: [&str; 3] = ["transcript_fts", "panels_fts", "titles_fts"];

/// Whether an FTS5 index still agrees with the table it indexes.
#[derive(Debug, PartialEq, Eq)]
pub enum FtsIndexState {
    /// Every source row is indexed and every indexed row exists.
    Consistent,
    /// The index and its source disagree, so search silently under-reports.
    /// Carries SQLite's own description.
    Drifted(String),
}

/// One index's result from [`check_fts_indexes`].
#[derive(Debug)]
pub struct FtsIndexReport {
    pub table: &'static str,
    pub state: FtsIndexState,
}

/// Compare each maintained FTS5 index against its source table.
///
/// Needs a writable connection: FTS5 spells its check as an `INSERT`, and
/// SQLite refuses that on a read-only connection even though the command writes
/// nothing.
pub fn check_fts_indexes(conn: &Connection) -> Result<Vec<FtsIndexReport>> {
    MAINTAINED_FTS_TABLES
        .iter()
        .map(|&table| check_fts_index(conn, table).map(|state| FtsIndexReport { table, state }))
        .collect()
}

/// Run FTS5's `integrity-check` at rank 1 against one index.
///
/// Rank 1 is the point of this. These are external-content tables (`content=`),
/// and only rank 1 compares the index against its source; rank 0 checks the
/// index's own internal consistency, as do `PRAGMA quick_check` and `PRAGMA
/// integrity_check`, all three of which report `ok` on a drifted index.
///
/// `table` is interpolated because SQLite does not bind identifiers. It is
/// always one of [`MAINTAINED_FTS_TABLES`]; this function is private so no
/// caller-supplied name can reach it.
fn check_fts_index(conn: &Connection, table: &str) -> Result<FtsIndexState> {
    let sql = format!("INSERT INTO {table}({table}, rank) VALUES('integrity-check', 1)");

    match conn.execute(&sql, []) {
        Ok(_) => Ok(FtsIndexState::Consistent),
        // Drift is how FTS5 reports a mismatch, and it is a finding rather than
        // a failure to look.
        Err(rusqlite::Error::SqliteFailure(err, msg)) if err.code == ErrorCode::DatabaseCorrupt => {
            Ok(FtsIndexState::Drifted(
                msg.unwrap_or_else(|| err.to_string()),
            ))
        }
        Err(err) => Err(err).with_context(|| format!("running integrity-check on {table}")),
    }
}

/// Discard each maintained FTS5 index and re-derive it from its source table.
///
/// The repair for a [`FtsIndexState::Drifted`] index, and a no-op in effect on a
/// consistent one.
pub fn rebuild_fts_indexes(conn: &Connection) -> Result<Vec<&'static str>> {
    for table in MAINTAINED_FTS_TABLES {
        conn.execute(
            &format!("INSERT INTO {table}({table}) VALUES('rebuild')"),
            [],
        )
        .with_context(|| format!("rebuilding {table}"))?;
    }

    Ok(MAINTAINED_FTS_TABLES.to_vec())
}

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
        conn.execute("CREATE TABLE unrelated (id INTEGER)", [])
            .unwrap();
        drop(conn);

        assert!(check_pulled_database(&path).is_err());
    }

    /// Force an index out of step with its source, the way an interrupted sync
    /// used to: write rows straight into the FTS table for a document the source
    /// table does not have.
    fn drift_the_transcript_index(conn: &Connection) {
        conn.execute(
            "INSERT INTO transcript_fts(rowid, text) VALUES (9001, 'orphaned row')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn reports_a_consistent_fts_index_as_consistent() {
        let dir = TempDir::new().unwrap();
        let path = valid_database(&dir);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text)
                 VALUES ('u1', 'd1', 'deployment rollback discussion')",
            [],
        )
        .unwrap();

        let reports = check_fts_indexes(&conn).unwrap();

        assert_eq!(reports.len(), 3);
        for report in &reports {
            assert_eq!(
                report.state,
                FtsIndexState::Consistent,
                "{} should be consistent",
                report.table
            );
        }
    }

    #[test]
    fn reports_a_drifted_fts_index_as_drifted() {
        let dir = TempDir::new().unwrap();
        let path = valid_database(&dir);
        let conn = Connection::open(&path).unwrap();
        drift_the_transcript_index(&conn);

        let reports = check_fts_indexes(&conn).unwrap();

        let transcripts = reports
            .iter()
            .find(|r| r.table == "transcript_fts")
            .unwrap();
        assert!(
            matches!(transcripts.state, FtsIndexState::Drifted(_)),
            "expected drift, got {:?}",
            transcripts.state
        );
        // The damage is confined to the index that was tampered with.
        let panels = reports.iter().find(|r| r.table == "panels_fts").unwrap();
        assert_eq!(panels.state, FtsIndexState::Consistent);
    }

    #[test]
    fn rebuild_repairs_a_drifted_fts_index() {
        let dir = TempDir::new().unwrap();
        let path = valid_database(&dir);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text)
                 VALUES ('u1', 'd1', 'deployment rollback discussion')",
            [],
        )
        .unwrap();
        drift_the_transcript_index(&conn);

        rebuild_fts_indexes(&conn).unwrap();

        for report in check_fts_indexes(&conn).unwrap() {
            assert_eq!(report.state, FtsIndexState::Consistent, "{}", report.table);
        }
        // Repair restores the index rather than merely emptying it.
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcript_fts WHERE transcript_fts MATCH 'rollback'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
    }

    /// Records why the FTS check is not part of [`check_pulled_database`],
    /// rather than pinning a gap: SQLite refuses the `INSERT` that FTS5 spells
    /// its check as, on the read-only connection this deliberately opens.
    #[test]
    fn the_fts_check_needs_a_writable_connection() {
        let dir = TempDir::new().unwrap();
        let path = valid_database(&dir);
        {
            let conn = Connection::open(&path).unwrap();
            drift_the_transcript_index(&conn);
        }

        let read_only =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let err = check_fts_indexes(&read_only).unwrap_err();

        assert!(
            err.to_string().contains("integrity-check"),
            "unhelpful message: {err}"
        );
        // And so the pull-time check, which is read-only by design, still passes
        // a drifted file. `grans db info` is where drift surfaces.
        assert!(check_pulled_database(&path).is_ok());
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
