//! `grans remote push`.

use std::path::Path;

use anyhow::Result;

use crate::db::wal::checkpoint_file;
use crate::output::format::format_size;
use crate::sync::SyncError;
use crate::sync::config::SyncConfig;
use crate::sync::content_hash::hash_file;
use crate::sync::metadata::SyncMetadata;
use crate::sync::reconcile::{TransferDecision, decide};
use crate::sync::remote::{Remote, connect, upload_verified};
use crate::sync::transfer::no_progress;

use super::progress::TransferProgress;
use super::{REMOTE_DB, REMOTE_METADATA, current_timestamp};

/// Push the database to the remote.
pub(super) fn push(force: bool) -> Result<()> {
    let mut config = SyncConfig::load()?;
    let remote = connect(&config)?;

    // Get local database path
    let db_path = crate::db::connection::default_db_path()?;

    if !db_path.exists() {
        println!("No database found (run a query first to create it)");
        return Ok(());
    }

    // Both the upload and the hash that decides whether to upload read the
    // database file on its own, so it has to hold the whole database first.
    // Unlike the pull side, there is no way to carry on without this: uploading
    // an unflattened file would put an incomplete database on the remote.
    checkpoint_file(&db_path)?;

    let synced_hash = push_file(
        remote.as_ref(),
        &db_path,
        REMOTE_DB,
        "database",
        force,
        config.last_synced_hash.as_deref(),
    )?;

    // Generate and upload metadata
    let metadata = SyncMetadata::from_local_db(Some(&db_path))?;
    upload_metadata(remote.as_ref(), &metadata)?;

    // Record what both sides now hold, so the next sync can tell which one moved.
    config.record_sync(synced_hash);
    config.last_push_time = Some(current_timestamp());
    config.save()?;
    println!("\nPush complete!");

    Ok(())
}

fn upload_metadata(remote: &dyn Remote, metadata: &SyncMetadata) -> Result<()> {
    let json = serde_json::to_string_pretty(metadata)?;
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join("grans_sync_metadata.json");
    std::fs::write(&temp_path, &json)?;

    println!("Uploading sync metadata...");
    let hash = hash_file(remote.provider().hash_kind(), &temp_path)?;
    // A few hundred bytes: a progress bar would flash and vanish.
    upload_verified(remote, &temp_path, REMOTE_METADATA, &hash, no_progress())?;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    Ok(())
}

/// Push a file, unless the remote copy holds changes this machine has not seen.
///
/// Returns the content hash both sides hold afterwards, to record as the
/// reference point for the next sync.
fn push_file(
    remote: &dyn Remote,
    local_path: &Path,
    remote_name: &str,
    what: &str,
    force: bool,
    last_synced: Option<&str>,
) -> Result<String> {
    let provider = remote.provider();
    let local_hash = hash_file(provider.hash_kind(), local_path)?;
    let stored = remote.stat(remote_name)?;
    let remote_hash = stored.as_ref().and_then(|f| f.content_hash.as_deref());

    match decide(&local_hash, remote_hash, last_synced) {
        TransferDecision::UpToDate => {
            println!("{} already has this {}; nothing to upload.", provider, what);
            return Ok(local_hash);
        }
        TransferDecision::Diverged if !force => {
            return Err(SyncError::ConflictRemoteChanged {
                provider,
                what: what.to_string(),
            }
            .into());
        }
        TransferDecision::Unknown if !force => {
            return Err(SyncError::ConflictNoSyncRecord {
                provider,
                what: what.to_string(),
            }
            .into());
        }
        _ => {}
    }

    let size = std::fs::metadata(local_path)?.len();
    println!("Uploading {} ({})...", what, format_size(size));

    let progress = TransferProgress::new(size);
    // Verifying against the hash the decision was made on means a file that
    // changed after hashing fails here, rather than being recorded as synced
    // under a hash it no longer has.
    upload_verified(
        remote,
        local_path,
        remote_name,
        &local_hash,
        progress.reporter(),
    )?;
    drop(progress);

    println!("  Uploaded to {}", provider);

    Ok(local_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::remote::fake::FakeRemote;

    #[test]
    fn pushing_a_database_the_remote_already_has_uploads_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("grans.db");
        std::fs::write(&local, b"meetings").unwrap();
        let remote = FakeRemote::with_file(REMOTE_DB, b"meetings");

        push_file(&remote, &local, REMOTE_DB, "database", false, None).unwrap();

        assert_eq!(remote.uploads(), 0);
    }

    #[test]
    fn pushing_over_an_untouched_remote_copy_replaces_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("grans.db");
        std::fs::write(&local, b"new meetings").unwrap();
        let remote = FakeRemote::with_file(REMOTE_DB, b"old meetings");
        let last_synced = remote.stat(REMOTE_DB).unwrap().unwrap().content_hash;

        push_file(
            &remote,
            &local,
            REMOTE_DB,
            "database",
            false,
            last_synced.as_deref(),
        )
        .unwrap();

        assert_eq!(remote.contents(REMOTE_DB), Some(b"new meetings".to_vec()));
    }

    #[test]
    fn pushing_over_a_remote_copy_changed_elsewhere_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("grans.db");
        std::fs::write(&local, b"our meetings").unwrap();
        let remote = FakeRemote::with_file(REMOTE_DB, b"another machine's meetings");

        let err = push_file(
            &remote,
            &local,
            REMOTE_DB,
            "database",
            false,
            Some("hash of what both held at the last sync"),
        )
        .unwrap_err();

        assert!(
            matches!(
                err.downcast_ref::<SyncError>(),
                Some(SyncError::ConflictRemoteChanged { .. })
            ),
            "{:#}",
            err
        );
    }
}
