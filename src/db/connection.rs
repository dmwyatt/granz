use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use log::debug;
use rusqlite::{Connection, ErrorCode, OpenFlags};

use crate::db::migrations;

/// How long a statement waits for a lock another connection holds before it
/// gives up.
///
/// Several grans processes write the same database: a `sync` pulling meetings
/// from the API while a `search` stores the embedding chunks it just computed.
/// With no timeout SQLite reports "database is locked" the moment it finds the
/// write lock taken, and the caller discards work it has already paid for. The
/// write transactions here are short, so this covers them many times over while
/// still surfacing a genuinely stuck process instead of hanging on it.
const BUSY_TIMEOUT: Duration = Duration::from_secs(15);

/// Open a database, creating an empty one if the file is not there.
///
/// Only the migration path should create a database: everywhere else, a missing
/// file is a fact worth reporting rather than something to paper over with an
/// empty schema.
pub(crate) fn open_or_create(path: &Path) -> Result<Connection> {
    open(path, OpenFlags::default())
}

/// Open a database that already exists, for reading and writing.
pub fn open_existing(path: &Path) -> Result<Connection> {
    open(path, OpenFlags::default() & !OpenFlags::SQLITE_OPEN_CREATE)
}

/// Open a database that already exists, for reading only.
///
/// What the read-only paths want: reporting on a database must not write to it,
/// convert its journal mode, or run a migration as a side effect of being asked
/// for a row count.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    open(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
}

/// The one place in grans that builds a `Connection`.
///
/// Every connection needs the busy timeout, and a helper that can be bypassed
/// is a helper that will be: the next `Connection::open` written anywhere else
/// would silently get none of this, which is the bug class this module exists
/// to close.
fn open(path: &Path, flags: OpenFlags) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("opening the database at {}", path.display()))?;

    set_busy_timeout(&conn)?;

    Ok(conn)
}

/// Put the standard busy timeout back on a connection.
///
/// For the one caller that lowers it: the journal mode conversion, which has to
/// give up quickly rather than making every command wait out a lock it does not
/// need.
pub fn set_busy_timeout(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)
        .context("setting the database busy timeout")
}

/// Whether an open failed because there is no database at that path.
///
/// Opening without `CREATE` is how a caller asks "is one there?" without the
/// check-then-act gap of testing for the file first.
pub fn is_missing_database(err: &anyhow::Error) -> bool {
    err.downcast_ref::<rusqlite::Error>()
        .and_then(|e| e.sqlite_error_code())
        .is_some_and(|code| code == ErrorCode::CannotOpen)
}

/// Get the default database path.
/// All data is stored in a single database file.
pub fn default_db_path() -> Result<std::path::PathBuf> {
    let data_dir = crate::platform::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir.join("grans.db"))
}

/// Open or create the grans database.
/// Uses the migration system to ensure the schema is up-to-date.
/// Backs up the database before applying any pending migrations.
pub fn open_or_create_db() -> Result<Connection> {
    let db_path = default_db_path()?;
    debug!("Opening database at {}", db_path.display());
    let conn = migrations::open_and_migrate(&db_path)?;
    let version = migrations::get_schema_version(&conn).unwrap_or(0);
    debug!("Database opened (schema version {})", version);
    Ok(conn)
}

/// Open a database at a specific path.
/// Uses the migration system to ensure the schema is up-to-date.
pub fn open_db_at_path(path: &std::path::Path) -> Result<Connection> {
    debug!("Opening database at {} (custom path)", path.display());
    let conn = migrations::open_and_migrate(path)?;
    let version = migrations::get_schema_version(&conn).unwrap_or(0);
    debug!("Database opened (schema version {})", version);
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;
    use tempfile::TempDir;

    /// A write blocked by another connection has to wait for the lock rather
    /// than fail on the spot: `grans search` stores embedding chunks while a
    /// `grans sync` is writing, and a refused write throws away work that has
    /// already been computed.
    #[test]
    fn a_write_waits_for_a_lock_another_connection_holds() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let writer = open_db_at_path(&db_path).unwrap();
        let blocker = open_db_at_path(&db_path).unwrap();

        blocker
            .execute_batch(
                "BEGIN IMMEDIATE; INSERT INTO metadata (key, value) VALUES ('blocker', '1');",
            )
            .unwrap();

        let holder = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            blocker.execute_batch("COMMIT").unwrap();
        });

        writer
            .execute(
                "INSERT INTO metadata (key, value) VALUES ('writer', '1')",
                [],
            )
            .expect("write should wait out the other connection's lock");

        holder.join().unwrap();
    }

    /// The failure a busy timeout cannot rescue, and the one behind the
    /// "database is locked" warnings from the embedding store: a rollback
    /// journal makes a commit wait for every open reader, and SQLite refuses
    /// rather than wait when waiting could deadlock the two.
    #[test]
    fn a_writer_commits_while_a_reader_holds_a_read_transaction() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let writer = open_db_at_path(&db_path).unwrap();
        let reader = open_db_at_path(&db_path).unwrap();

        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM metadata", [], |row| row.get(0))
            .unwrap();

        writer
            .execute_batch(
                "BEGIN IMMEDIATE; \
                 INSERT INTO metadata (key, value) VALUES ('writer', '1'); \
                 COMMIT;",
            )
            .expect("an open reader should not block a commit");
    }

    #[test]
    fn the_database_runs_in_wal_mode() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_db_at_path(&db_path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();

        assert_eq!(mode, "wal");
    }

    /// A reader inherits the journal mode rather than setting it, so it has to
    /// open a database that is still on a rollback journal, as one pulled from
    /// a machine running an older grans would be.
    #[test]
    fn a_read_only_connection_opens_a_rollback_journal_database() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_db_at_path(&db_path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        drop(conn);

        let reader = open_read_only(&db_path).unwrap();
        let count: i64 = reader
            .query_row("SELECT COUNT(*) FROM metadata", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 0);
    }

    #[test]
    fn every_connection_gets_the_busy_timeout() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_db_at_path(&db_path).unwrap();
        let timeout_ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();

        assert_eq!(timeout_ms, BUSY_TIMEOUT.as_millis() as i64);
    }

    #[test]
    fn test_open_or_create_db_creates_schema() {
        // This test uses the real default_db_path which we can't easily override,
        // so we test the underlying migration function directly
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = migrations::open_and_migrate(&db_path).unwrap();

        // Verify key tables exist
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_schema_version_tracked() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = migrations::open_and_migrate(&db_path).unwrap();
        let version = migrations::get_schema_version(&conn).unwrap();

        // After applying all migrations, version should be 17
        assert_eq!(version, 17);
    }
}
