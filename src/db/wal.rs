//! The database as a file: journal mode, checkpoints, and the logs beside it.
//!
//! In write-ahead logging mode a committed write can live entirely in the
//! `-wal` file until a checkpoint folds it into the database. Anything that
//! treats the database as bytes has to account for that: the Dropbox upload,
//! the content hash that decides whether to upload at all, and the deletes that
//! replace or clear the file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use log::debug;
use rusqlite::{Connection, ErrorCode};

use crate::db::connection::{is_missing_database, open_existing};

/// The files SQLite keeps beside a database in WAL mode.
const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// Move a database to write-ahead logging, reporting whether it moved.
///
/// Under a rollback journal a commit has to lock the whole file, so a `grans
/// sync` writing while a `grans search` reads fails with "database is locked"
/// however long it is willing to wait: SQLite returns busy without consulting
/// the busy handler when waiting could deadlock the two. WAL lets one writer
/// and any number of readers work at once, which is the shape of every
/// collision grans has.
///
/// The mode is a property of the file, so this is a one-time conversion rather
/// than something every open repeats. It needs the database to itself, and
/// SQLite waits out the busy timeout and then raises "database is locked" when
/// another connection is mid-transaction (verified on SQLite 3.51.1). Every
/// grans command opens the database, so failing there would lock the user out
/// of the tool while a sync runs. A busy database stays in the mode it has and
/// the next command tries again.
pub fn convert_to_wal(conn: &Connection) -> Result<bool> {
    match conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0)) {
        Ok(mode) if mode == "wal" => Ok(true),
        Ok(mode) => {
            debug!("database stayed in {} journal mode rather than WAL", mode);
            Ok(false)
        }
        Err(e) if is_busy(&e) => {
            debug!("another connection is using the database; leaving its journal mode alone");
            Ok(false)
        }
        Err(e) => Err(e).context("switching the database to write-ahead logging"),
    }
}

/// The journal mode the database is currently in.
pub fn journal_mode(conn: &Connection) -> Result<String> {
    conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .context("reading the database journal mode")
}

/// Fold the write-ahead log into the database file and truncate it.
///
/// Until this runs, the file on disk is not the whole database. Copying it,
/// uploading it, or hashing it in that state silently drops the most recent
/// commits, so a checkpoint that could not finish is an error rather than a
/// warning.
pub fn checkpoint(conn: &Connection) -> Result<()> {
    // On a database in rollback-journal mode this reports (0, -1, -1) and
    // changes nothing. Under a concurrent read transaction it reports busy
    // after waiting out the busy timeout rather than failing (SQLite 3.51.1).
    let (blocked, checkpointed): (i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(2)?))
        })
        .context("checkpointing the write-ahead log")?;

    if blocked != 0 {
        bail!(
            "the write-ahead log could not be folded into the database file because \
             another grans command is reading or writing it; wait for it to finish \
             and try again"
        );
    }

    debug!("checkpointed {} pages into the database file", checkpointed);

    Ok(())
}

/// Checkpoint the database at `path`.
///
/// Fails distinctly when there is no database there, which callers that treat a
/// missing file as nothing to do can ask about with `is_missing_database`.
pub fn checkpoint_file(path: &Path) -> Result<()> {
    let conn = open_existing(path)?;

    checkpoint(&conn)
}

/// Put `replacement` in place of the database at `db_path`.
///
/// The database being replaced is checkpointed first, so its log holds nothing
/// its own file does not. That matters because the rename can fail: on Windows
/// it fails whenever another process has the destination open (verified: the
/// rename returns "Access is denied"). Deleting the log first, as the obvious
/// ordering would, strips the database of every commit since the last
/// checkpoint and then leaves it in place when the rename gives up.
///
/// The logs are cleared afterwards instead. A `-wal` belongs to the exact file
/// it was written for, so a stale one beside a database it did not come from
/// would be replayed into it; truncating it during the checkpoint means a
/// leftover has no frames to replay even if the delete does not go through.
pub fn replace_database(replacement: &Path, db_path: &Path) -> Result<()> {
    flatten_replaced_database(db_path)?;

    std::fs::rename(replacement, db_path).with_context(|| {
        format!(
            "moving {} into place at {}",
            replacement.display(),
            db_path.display()
        )
    })?;

    clear_stale_logs(db_path);
    // The integrity check opens the downloaded file read-only, and a read-only
    // open of a WAL database creates its `-wal` and `-shm` and leaves them
    // behind (verified on SQLite 3.51.1). Renaming the file away orphans them.
    clear_stale_logs(replacement);

    Ok(())
}

/// Checkpoint the database about to be replaced, if there is one.
fn flatten_replaced_database(db_path: &Path) -> Result<()> {
    match checkpoint_file(db_path) {
        Err(e) if is_missing_database(&e) => {
            debug!("no database at {} to replace", db_path.display());
            Ok(())
        }
        other => other,
    }
}

/// Delete the database at `db_path` along with its logs.
///
/// The logs go first. A `-wal` that outlives its database is replayed into the
/// next database to take that name, so the log must never be the thing left
/// behind: if it cannot be deleted, because another grans process still has the
/// database open, nothing is deleted at all.
pub fn remove_database(db_path: &Path) -> Result<()> {
    remove_sidecars(db_path)?;

    std::fs::remove_file(db_path).with_context(|| format!("deleting {}", db_path.display()))
}

/// Delete a database file and its logs, ignoring whatever is not there.
///
/// For abandoning a download: nothing depends on the result, and a failure to
/// clean up must not replace the error that caused the cleanup.
pub fn discard(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        debug!("could not remove {}: {}", path.display(), e);
    }

    clear_stale_logs(path);
}

/// Delete the log files beside a database, if any are there.
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

/// Clear log files whose database is already gone, reporting rather than
/// failing.
///
/// They are inert by the time this runs, and a completed transfer should not be
/// reported as a failure because Windows would not let a leftover file be
/// unlinked.
fn clear_stale_logs(db_path: &Path) {
    if let Err(e) = remove_sidecars(db_path) {
        debug!("{:#}", e);
    }
}

/// The write-ahead log SQLite keeps beside a database.
pub fn log_path(db_path: &Path) -> PathBuf {
    sidecar_paths(db_path)
        .next()
        .expect("the sidecar list is never empty")
}

/// The `-wal` and `-shm` paths SQLite would use for a database.
fn sidecar_paths(db_path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let name = db_path.as_os_str().to_owned();
    SIDECAR_SUFFIXES.iter().map(move |suffix| {
        let mut path = name.clone();
        path.push(suffix);
        PathBuf::from(path)
    })
}

/// Whether SQLite refused because another connection holds the database.
fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy) | Some(ErrorCode::DatabaseLocked)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A database in WAL mode with one row committed but not checkpointed, and
    /// nothing holding it open: what a `grans sync` leaves behind when it is
    /// killed, and what `pull` finds when it goes to install a download.
    fn database_with_a_hot_log(dir: &TempDir, name: &str) -> PathBuf {
        let source = dir.path().join(format!("source-{name}"));
        let conn = crate::db::connection::open_db_at_path(&source).unwrap();
        assert_eq!(journal_mode(&conn).unwrap(), "wal");
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('committed', 'yes')",
            [],
        )
        .unwrap();

        // Copying the pair out from under the live connection reproduces a log
        // whose frames are not in the database file, without needing a second
        // process to leave one behind.
        let db_path = dir.path().join(name);
        std::fs::copy(&source, &db_path).unwrap();
        for (from, to) in sidecar_paths(&source).zip(sidecar_paths(&db_path)) {
            if from.exists() {
                std::fs::copy(&from, &to).unwrap();
            }
        }
        drop(conn);

        // Reading the copy back here would defeat the point: closing the last
        // connection to a database checkpoints it and removes the log, which is
        // the state these tests exist to construct.
        let log = log_path(&db_path);
        assert!(
            std::fs::metadata(&log).is_ok_and(|m| m.len() > 0),
            "the copy should carry a log with frames in it"
        );

        db_path
    }

    fn committed_row(db_path: &Path) -> Option<String> {
        let conn = open_existing(db_path).unwrap();
        conn.query_row(
            "SELECT value FROM metadata WHERE key = 'committed'",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    /// The data-loss shape: clearing the logs before the replacement is in
    /// place leaves a database stripped of every commit since the last
    /// checkpoint when the replacement step then fails. The rename fails here
    /// because the replacement is not there, which is the same shape as the
    /// Windows failure where the destination is open.
    #[test]
    fn a_failed_replacement_leaves_the_database_whole() {
        let dir = TempDir::new().unwrap();
        let db_path = database_with_a_hot_log(&dir, "grans.db");
        let missing = dir.path().join("grans.db.tmp");

        let result = replace_database(&missing, &db_path);

        assert!(result.is_err(), "a missing replacement cannot be installed");
        assert_eq!(
            committed_row(&db_path),
            Some("yes".to_string()),
            "the database that was not replaced must keep its committed rows"
        );
    }

    /// A pulled database arrives whole. The log of the database it replaces
    /// describes different pages, so it must not be left beside it.
    #[test]
    fn replacing_a_database_clears_the_logs_of_both_files() {
        let dir = TempDir::new().unwrap();
        let db_path = database_with_a_hot_log(&dir, "grans.db");
        let replacement = dir.path().join("grans.db.tmp");
        std::fs::copy(&db_path, &replacement).unwrap();
        // What the read-only integrity check leaves beside the download.
        std::fs::write(dir.path().join("grans.db.tmp-wal"), b"").unwrap();
        std::fs::write(dir.path().join("grans.db.tmp-shm"), b"stale").unwrap();

        replace_database(&replacement, &db_path).unwrap();

        for path in sidecar_paths(&db_path).chain(sidecar_paths(&replacement)) {
            assert!(
                !path.exists(),
                "{} should have been cleared",
                path.display()
            );
        }
        assert!(!replacement.exists());
        assert!(db_path.exists());
    }

    /// Installing the first pull on a machine that has no database yet.
    #[test]
    fn replacing_an_absent_database_installs_the_replacement() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("grans.db");
        let replacement = dir.path().join("grans.db.tmp");
        let seeded = database_with_a_hot_log(&dir, "seed.db");
        std::fs::copy(&seeded, &replacement).unwrap();

        replace_database(&replacement, &db_path).unwrap();

        assert!(db_path.exists());
        assert!(!replacement.exists());
    }

    /// What `grans dropbox push` uploads is the database file on its own, so
    /// every commit has to be in it.
    #[test]
    fn a_checkpointed_database_file_carries_the_latest_writes() {
        let dir = TempDir::new().unwrap();
        let db_path = database_with_a_hot_log(&dir, "grans.db");

        checkpoint_file(&db_path).unwrap();

        let copy = dir.path().join("copy.db");
        std::fs::copy(&db_path, &copy).unwrap();
        assert_eq!(committed_row(&copy), Some("yes".to_string()));
    }

    #[test]
    fn checkpointing_reports_a_database_that_is_not_there() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("absent.db");

        let err = checkpoint_file(&db_path).unwrap_err();

        assert!(is_missing_database(&err), "{:#}", err);
        assert!(!db_path.exists(), "no database should have been created");
    }

    #[test]
    fn removing_a_database_takes_its_logs_with_it() {
        let dir = TempDir::new().unwrap();
        let db_path = database_with_a_hot_log(&dir, "grans.db");
        assert!(sidecar_paths(&db_path).any(|p| p.exists()));

        remove_database(&db_path).unwrap();

        assert!(!db_path.exists());
        for path in sidecar_paths(&db_path) {
            assert!(!path.exists(), "{} survived", path.display());
        }
    }

    /// The log is what must not outlive the database, so a database that cannot
    /// be parted from its log is left alone rather than half deleted.
    #[test]
    fn a_database_whose_log_cannot_be_removed_is_left_alone() {
        let dir = TempDir::new().unwrap();
        let db_path = database_with_a_hot_log(&dir, "grans.db");
        // A directory in place of the -wal cannot be unlinked with remove_file,
        // standing in for the sharing violation Windows raises while another
        // process holds the log open.
        let wal = sidecar_paths(&db_path).next().unwrap();
        std::fs::remove_file(&wal).unwrap();
        std::fs::create_dir(&wal).unwrap();

        let result = remove_database(&db_path);

        assert!(result.is_err());
        assert!(db_path.exists(), "the database must survive a failed clear");
    }

    #[test]
    fn discarding_a_download_removes_it_and_its_logs() {
        let dir = TempDir::new().unwrap();
        let temp = dir.path().join("grans.db.tmp");
        std::fs::write(&temp, b"partial download").unwrap();
        std::fs::write(dir.path().join("grans.db.tmp-wal"), b"").unwrap();
        std::fs::write(dir.path().join("grans.db.tmp-shm"), b"stale").unwrap();

        discard(&temp);

        assert!(!temp.exists());
        for path in sidecar_paths(&temp) {
            assert!(!path.exists());
        }
    }

    /// The lock-out the conversion must not cause: SQLite waits out the busy
    /// timeout and then raises "database is locked" when another connection is
    /// mid-transaction (verified on 3.51.1). Every grans command opens the
    /// database, so a converter that propagated that error would refuse to run
    /// any command while a sync is in a transaction.
    #[test]
    fn a_busy_database_keeps_its_journal_mode() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("grans.db");
        let owner = crate::db::connection::open_db_at_path(&db_path).unwrap();
        let mode: String = owner
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        drop(owner);

        let reader = open_existing(&db_path).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM metadata", [], |row| row.get(0))
            .unwrap();

        let conn = open_existing(&db_path).unwrap();
        // Waiting out the real timeout would cost this test fifteen seconds to
        // reach the same answer.
        conn.busy_timeout(std::time::Duration::from_millis(50))
            .unwrap();

        assert!(
            !convert_to_wal(&conn).unwrap(),
            "a busy database cannot move"
        );
        assert_eq!(journal_mode(&conn).unwrap(), "delete");
    }

    #[test]
    fn discarding_what_is_not_there_is_not_a_failure() {
        let dir = TempDir::new().unwrap();

        discard(&dir.path().join("nothing.db"));
    }
}
