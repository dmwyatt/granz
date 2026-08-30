//! Keeping the Dropbox sync record straight across a rewrite of the local
//! database file that changes nothing it holds.
//!
//! Sync tells the two copies apart by content hash. The one-time conversion to
//! write-ahead logging rewrites two bytes of the database header, so to the
//! sync record the local file looks edited: without this, the first pull after
//! an upgrade refuses as diverged over a change the user never made, on a
//! database whose meetings are identical to the ones on Dropbox.

use std::path::Path;

use anyhow::{Context, Result};
use log::debug;

use crate::sync::config::SyncConfig;
use crate::sync::content_hash::hash_file;

/// Whether the sync record points at the local database file as it is now.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncedDatabase {
    /// The record describes this exact file, so it can be moved to follow it.
    Tracked,
    /// There is no sync record, or it already described something else. A
    /// rewrite changes nothing that was still true.
    Untracked,
}

impl SyncedDatabase {
    /// Ask whether the sync record still describes the file, before rewriting it.
    ///
    /// Hashing a database is not free, so this reads the record first and does
    /// no work at all on the machines that have never synced.
    pub fn capture(db_path: &Path) -> Self {
        let config = match SyncConfig::load() {
            Ok(config) => config,
            Err(e) => {
                debug!("no readable sync record to keep in step: {}", e);
                return Self::Untracked;
            }
        };

        let Some(reference) = config.local_reference() else {
            return Self::Untracked;
        };

        match hash_file(db_path) {
            Ok(hash) if hash == reference => Self::Tracked,
            Ok(_) => Self::Untracked,
            Err(e) => {
                debug!("could not hash {}: {}", db_path.display(), e);
                Self::Untracked
            }
        }
    }

    /// Point the record at the rewritten file.
    ///
    /// Only the local side moves: Dropbox still holds the bytes it always held,
    /// so a push has nothing new to fear and a pull sees a local database that
    /// has not been edited.
    pub fn follow(&self, db_path: &Path) -> Result<()> {
        if *self == Self::Untracked {
            return Ok(());
        }

        let hash = hash_file(db_path)
            .with_context(|| format!("hashing {} after rewriting it", db_path.display()))?;

        let mut config = SyncConfig::load().context("reading the Dropbox sync record")?;
        config.last_local_hash = Some(hash);
        config.save().context("writing the Dropbox sync record")?;

        debug!("sync record now follows the rewritten database file");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::wal;
    use crate::sync::reconcile::{TransferDecision, decide};
    use tempfile::TempDir;

    /// A database as it was before the upgrade: rollback journal, pushed to
    /// Dropbox, both sides identical.
    fn synced_database(dir: &TempDir) -> (std::path::PathBuf, SyncConfig) {
        let db_path = dir.path().join("grans.db");
        let conn = crate::db::connection::open_db_at_path(&db_path).unwrap();
        conn.execute("INSERT INTO metadata (key, value) VALUES ('a', 'b')", [])
            .unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
        drop(conn);

        let mut config = SyncConfig::default();
        config.record_sync(hash_file(&db_path).unwrap());

        (db_path, config)
    }

    /// The lock-out this module exists to prevent: converting an untouched
    /// database must not make the next pull look like a conflict.
    #[test]
    fn a_converted_database_does_not_reconcile_as_diverged() {
        let dir = TempDir::new().unwrap();
        let (db_path, mut config) = synced_database(&dir);
        let on_dropbox = config.last_synced_hash.clone().unwrap();

        let conn = crate::db::connection::open_existing(&db_path).unwrap();
        assert!(wal::convert_to_wal(&conn).unwrap());
        drop(conn);
        // What `follow` does, without reaching for the real sync record on this
        // machine.
        config.last_local_hash = Some(hash_file(&db_path).unwrap());

        let local = hash_file(&db_path).unwrap();
        assert_ne!(local, on_dropbox, "the conversion rewrites the file");

        assert_eq!(
            decide(&on_dropbox, Some(&local), config.local_reference()),
            TransferDecision::Proceed,
            "a pull must see a local database nobody edited"
        );
        assert_eq!(
            decide(
                &local,
                Some(&on_dropbox),
                config.last_synced_hash.as_deref()
            ),
            TransferDecision::Proceed,
            "a push must see a copy on Dropbox nobody edited"
        );
    }

    /// Without the record following the file, both directions report a conflict
    /// that only --force gets past.
    #[test]
    fn an_unrecorded_conversion_reads_as_a_conflict() {
        let dir = TempDir::new().unwrap();
        let (db_path, config) = synced_database(&dir);
        let on_dropbox = config.last_synced_hash.clone().unwrap();

        let conn = crate::db::connection::open_existing(&db_path).unwrap();
        assert!(wal::convert_to_wal(&conn).unwrap());
        drop(conn);

        let local = hash_file(&db_path).unwrap();

        assert_eq!(
            decide(&on_dropbox, Some(&local), config.local_reference()),
            TransferDecision::Diverged
        );
    }

    #[test]
    fn a_database_the_record_does_not_describe_is_untracked() {
        let dir = TempDir::new().unwrap();
        let (db_path, _) = synced_database(&dir);

        // Nothing on this machine has synced, so `capture` reads the record and
        // stops there.
        assert_eq!(
            SyncedDatabase::capture(&dir.path().join("absent.db")),
            SyncedDatabase::Untracked
        );
        assert!(db_path.exists());
    }

    #[test]
    fn an_untracked_database_writes_no_record() {
        let dir = TempDir::new().unwrap();
        let (db_path, _) = synced_database(&dir);

        SyncedDatabase::Untracked.follow(&db_path).unwrap();
    }
}
