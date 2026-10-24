//! `grans dropbox push`.

use std::path::Path;

use anyhow::Result;

use crate::db::wal::checkpoint_file;
use crate::output::format::format_size;
use crate::sync::SyncError;
use crate::sync::config::SyncConfig;
use crate::sync::content_hash::hash_file;
use crate::sync::dropbox::DropboxClient;
use crate::sync::metadata::SyncMetadata;
use crate::sync::reconcile::{TransferDecision, decide};
use crate::sync::transfer::no_progress;

use super::progress::TransferProgress;
use super::{REMOTE_DB_PATH, REMOTE_METADATA_PATH, current_timestamp, get_access_token};

/// Push database to Dropbox
pub(super) fn push(force: bool) -> Result<()> {
    let mut config = SyncConfig::load()?;

    if !config.is_authenticated() {
        return Err(SyncError::NotAuthenticated.into());
    }

    // Get access token
    let access_token = get_access_token(&config)?;
    let client = DropboxClient::new(access_token)?;

    // Get local database path
    let db_path = crate::db::connection::default_db_path()?;

    if !db_path.exists() {
        println!("No database found (run a query first to create it)");
        return Ok(());
    }

    // Both the upload and the hash that decides whether to upload read the
    // database file on its own, so it has to hold the whole database first.
    // Unlike the pull side, there is no way to carry on without this: uploading
    // an unflattened file would put an incomplete database on Dropbox.
    checkpoint_file(&db_path)?;

    let synced_hash = push_file(
        &client,
        &db_path,
        REMOTE_DB_PATH,
        "database",
        force,
        config.last_synced_hash.as_deref(),
    )?;

    // Generate and upload metadata
    let metadata = SyncMetadata::from_local_db(Some(&db_path))?;
    upload_metadata(&client, &metadata)?;

    // Record what both sides now hold, so the next sync can tell which one moved.
    config.record_sync(synced_hash);
    config.last_push_time = Some(current_timestamp());
    config.save()?;
    println!("\nPush complete!");

    Ok(())
}

fn upload_metadata(client: &DropboxClient, metadata: &SyncMetadata) -> Result<()> {
    let json = serde_json::to_string_pretty(metadata)?;
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join("grans_sync_metadata.json");
    std::fs::write(&temp_path, &json)?;

    println!("Uploading sync metadata...");
    // A few hundred bytes: a progress bar would flash and vanish.
    client.upload(&temp_path, REMOTE_METADATA_PATH, no_progress())?;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    Ok(())
}

/// Push a file, unless the remote copy holds changes this machine has not seen.
///
/// Returns the content hash both sides hold afterwards, to record as the
/// reference point for the next sync.
fn push_file(
    client: &DropboxClient,
    local_path: &Path,
    remote_path: &str,
    name: &str,
    force: bool,
    last_synced: Option<&str>,
) -> Result<String> {
    let local_hash = hash_file(local_path)?;
    let remote = client.get_metadata(remote_path)?;
    let remote_hash = remote.as_ref().and_then(|m| m.content_hash.as_deref());

    match decide(&local_hash, remote_hash, last_synced) {
        TransferDecision::UpToDate => {
            println!("Dropbox already has this {}; nothing to upload.", name);
            return Ok(local_hash);
        }
        TransferDecision::Diverged if !force => {
            return Err(SyncError::ConflictRemoteChanged {
                what: name.to_string(),
            }
            .into());
        }
        TransferDecision::Unknown if !force => {
            return Err(SyncError::ConflictNoSyncRecord {
                what: name.to_string(),
            }
            .into());
        }
        _ => {}
    }

    let size = std::fs::metadata(local_path)?.len();
    println!("Uploading {} ({})...", name, format_size(size));

    let progress = TransferProgress::new(size);
    client.upload(local_path, remote_path, progress.reporter())?;
    drop(progress);

    println!("  Uploaded to {}", remote_path);

    Ok(local_hash)
}
