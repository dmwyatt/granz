//! Handling the database as a file when it has a write-ahead log beside it.
//!
//! In WAL mode a committed write can live entirely in the `-wal` file until a
//! checkpoint folds it into the database. Anything that treats the database as
//! bytes has to account for that: a backup copy, a Dropbox upload, the content
//! hash that decides whether to upload at all, and the deletes that replace or
//! clear the file.

use std::path::Path;

use anyhow::{Context, Result, bail};
use log::debug;
use rusqlite::{Connection, OpenFlags};

use crate::db::connection::apply_pragmas;

/// The files SQLite keeps beside a database in WAL mode.
const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// Fold the write-ahead log into the database file and truncate it.
///
/// Until this runs, the file on disk is not the whole database. Copying it,
/// uploading it, or hashing it in that state silently drops the most recent
/// commits.
pub fn checkpoint(conn: &Connection) -> Result<()> {
    // On a database in rollback-journal mode this reports (0, -1, -1) and
    // changes nothing.
    let (blocked, checkpointed): (i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(2)?))
        })
        .context("checkpointing the write-ahead log")?;

    if blocked != 0 {
        bail!(
            "the write-ahead log could not be folded into the database file because \
             another process is using the database; close any other grans command \
             (and the Granola app if it is syncing) and try again"
        );
    }

    debug!("checkpointed {} pages into the database file", checkpointed);

    Ok(())
}

/// Checkpoint the database at `path`, if there is one there.
///
/// Opens without `CREATE`, so a database that is not there stays not there.
pub fn checkpoint_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    // Read-write without CREATE: a checkpoint must never bring a database into
    // existence.
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .with_context(|| format!("opening {} to checkpoint it", path.display()))?;
    apply_pragmas(&conn)?;

    checkpoint(&conn)
}

/// Put `replacement` in place of the database at `db_path`.
///
/// A `-wal` belongs to the exact database file it was written for, so the old
/// one goes first: left beside the new file, SQLite would replay it into a
/// database it was never written for. Removing it before the rename rather than
/// after leaves no window in which the two are beside each other.
pub fn replace_database(replacement: &Path, db_path: &Path) -> Result<()> {
    remove_sidecars(db_path)?;
    std::fs::rename(replacement, db_path).with_context(|| {
        format!(
            "moving {} into place at {}",
            replacement.display(),
            db_path.display()
        )
    })?;

    Ok(())
}

/// Delete the database at `db_path` along with its write-ahead log.
///
/// A `-wal` outliving its database is replayed into the next database to take
/// that name.
pub fn remove_database(db_path: &Path) -> Result<()> {
    std::fs::remove_file(db_path).with_context(|| format!("deleting {}", db_path.display()))?;

    remove_sidecars(db_path)
}

/// Delete the write-ahead log files beside a database, if any are there.
fn remove_sidecars(db_path: &Path) -> Result<()> {
    for path in sidecar_paths(db_path) {
        match std::fs::remove_file(&path) {
            Ok(()) => debug!("removed {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("deleting {}", path.display())),
        }
    }

    Ok(())
}

/// The `-wal` and `-shm` paths SQLite would use for a database.
fn sidecar_paths(db_path: &Path) -> impl Iterator<Item = std::path::PathBuf> + '_ {
    let name = db_path.as_os_str().to_owned();
    SIDECAR_SUFFIXES.iter().map(move |suffix| {
        let mut path = name.clone();
        path.push(suffix);
        std::path::PathBuf::from(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Open a database in WAL mode with one row written but not checkpointed.
    fn wal_database(db_path: &Path) -> Connection {
        let conn = crate::db::connection::open_db_at_path(db_path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");

        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('written', 'yes')",
            [],
        )
        .unwrap();

        conn
    }

    fn stored_value(db_path: &Path) -> Option<String> {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(
            "SELECT value FROM metadata WHERE key = 'written'",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    /// What `grans dropbox push` uploads and what the migration backup copies is
    /// the database file on its own, so every commit has to be in it.
    #[test]
    fn a_checkpointed_database_file_carries_the_latest_writes() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("grans.db");
        let held_open = wal_database(&db_path);

        checkpoint_file(&db_path).unwrap();

        let copy = dir.path().join("copy.db");
        std::fs::copy(&db_path, &copy).unwrap();
        assert_eq!(stored_value(&copy), Some("yes".to_string()));

        drop(held_open);
    }

    #[test]
    fn checkpointing_a_database_that_is_not_there_does_nothing() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("absent.db");

        checkpoint_file(&db_path).unwrap();

        assert!(!db_path.exists(), "no database should have been created");
    }

    /// A pulled database arrives whole. The write-ahead log of the database it
    /// replaces describes different pages, so replaying it would corrupt the
    /// file that just arrived.
    #[test]
    fn replacing_a_database_discards_the_old_write_ahead_log() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("grans.db");
        drop(wal_database(&db_path));
        std::fs::write(db_path.with_file_name("grans.db-wal"), b"stale").unwrap();

        let replacement = dir.path().join("grans.db.tmp");
        std::fs::copy(&db_path, &replacement).unwrap();

        replace_database(&replacement, &db_path).unwrap();

        assert!(!db_path.with_file_name("grans.db-wal").exists());
        assert!(!db_path.with_file_name("grans.db-shm").exists());
        assert!(!replacement.exists());
        assert!(db_path.exists());
    }

    #[test]
    fn removing_a_database_takes_its_write_ahead_log_with_it() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("grans.db");
        let held_open = wal_database(&db_path);
        assert!(db_path.with_file_name("grans.db-wal").exists());

        drop(held_open);
        std::fs::write(db_path.with_file_name("grans.db-wal"), b"stale").unwrap();
        std::fs::write(db_path.with_file_name("grans.db-shm"), b"stale").unwrap();

        remove_database(&db_path).unwrap();

        assert!(!db_path.exists());
        assert!(!db_path.with_file_name("grans.db-wal").exists());
        assert!(!db_path.with_file_name("grans.db-shm").exists());
    }
}
