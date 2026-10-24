//! `grans dropbox pull`.

use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};
use log::debug;

use crate::db::integrity::check_pulled_database;
use crate::db::wal::{checkpoint_file, discard, replace_database};
use crate::output::format::format_size;
use crate::output::progress::create_spinner;
use crate::sync::SyncError;
use crate::sync::config::SyncConfig;
use crate::sync::content_hash::{HashingWriter, hash_file};
use crate::sync::dropbox::DropboxClient;
use crate::sync::reconcile::{TransferDecision, decide};
use crate::sync::transfer::{verify_content_hash, verify_transfer_size};

use super::progress::TransferProgress;
use super::{REMOTE_DB_PATH, current_timestamp, get_access_token};

/// Pull database from Dropbox
pub(super) fn pull(force: bool) -> Result<()> {
    let mut config = SyncConfig::load()?;

    if !config.is_authenticated() {
        return Err(SyncError::NotAuthenticated.into());
    }

    // Get access token
    let access_token = get_access_token(&config)?;
    let client = DropboxClient::new(access_token)?;

    // Get local database path
    let db_path = crate::db::connection::default_db_path()?;

    // Pull database
    if client.get_metadata(REMOTE_DB_PATH)?.is_some() {
        let synced_hash = pull_file(
            &client,
            &db_path,
            REMOTE_DB_PATH,
            "database",
            force,
            // What the local file held when it last agreed with Dropbox, which
            // is not the hash Dropbox holds once a local rewrite that changed
            // no data (the journal mode conversion) has moved the file on.
            config.local_reference(),
        )?;

        // Record what both sides now hold, so the next sync can tell which one moved.
        config.record_sync(synced_hash);
        config.last_pull_time = Some(current_timestamp());
        config.save()?;
        println!("\nPull complete!");
    } else {
        println!("No database on Dropbox");
    }

    Ok(())
}

/// Pull a file, unless the local copy holds changes that are not on Dropbox.
///
/// Returns the content hash both sides hold afterwards, to record as the
/// reference point for the next sync.
fn pull_file(
    client: &DropboxClient,
    local_path: &Path,
    remote_path: &str,
    name: &str,
    force: bool,
    last_synced: Option<&str>,
) -> Result<String> {
    // Get remote metadata
    let remote = client
        .get_metadata(remote_path)?
        .ok_or_else(|| SyncError::DropboxApi(format!("{} not found on Dropbox", remote_path)))?;

    // Verification needs the hash before the transfer starts; refusing early
    // beats discovering it after moving hundreds of megabytes.
    let expected_hash =
        remote
            .content_hash
            .as_deref()
            .ok_or_else(|| SyncError::MissingContentHash {
                path: remote_path.to_string(),
            })?;

    let local_hash = local_content_hash(local_path, force);

    match decide(expected_hash, local_hash.as_deref(), last_synced) {
        TransferDecision::UpToDate => {
            println!(
                "The local {} already matches Dropbox; nothing to download.",
                name
            );
            return Ok(expected_hash.to_string());
        }
        TransferDecision::Diverged if !force => {
            return Err(SyncError::ConflictLocalChanged {
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

    println!("Downloading {} ({})...", name, format_size(remote.size));

    // Ensure parent directory exists
    if let Some(parent) = local_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Download into a temp file so nothing replaces a good database until every
    // check has passed.
    let temp_path = local_path.with_extension("db.tmp");
    let downloaded = download_and_verify(
        client,
        remote_path,
        &temp_path,
        &Expected {
            size: remote.size,
            hash: expected_hash,
            name,
        },
    );

    let installed = downloaded.and_then(|()| replace_database(&temp_path, local_path));

    match installed {
        Ok(()) => {
            println!("  Downloaded to {}", local_path.display());
            Ok(expected_hash.to_string())
        }
        // Every way the download can fail to become the local database ends
        // here, installing included: a several hundred megabyte temp file left
        // behind is removed by nothing, not even `db clear --all`.
        Err(e) => {
            discard(&temp_path);
            Err(e)
        }
    }
}

/// The content hash of the local database, when one can be established.
///
/// `None` means the answer is unknown, not that the file is absent. A local
/// database too broken to open is the usual reason to be pulling in the first
/// place, so nothing about reading it may abort the command that would replace
/// it. Under `--force` the answer changes no decision, and hashing hundreds of
/// megabytes to ignore the result is worth skipping.
fn local_content_hash(local_path: &Path, force: bool) -> Option<String> {
    if force {
        return None;
    }

    // The hash has to cover whatever is still in the write-ahead log, since
    // that is part of the database Dropbox is being compared against. A file
    // that cannot be checkpointed is still worth hashing: the bytes are what
    // they are, and a hash matching neither side stops the pull just as a
    // conflict would.
    if let Err(e) = checkpoint_file(local_path) {
        debug!(
            "{} was not checkpointed before hashing: {:#}",
            local_path.display(),
            e
        );
    }

    match hash_file(local_path) {
        Ok(hash) => Some(hash),
        Err(e) => {
            debug!("could not hash {}: {}", local_path.display(), e);
            None
        }
    }
}

/// What a download has to match before it is allowed to replace the database.
struct Expected<'a> {
    size: u64,
    hash: &'a str,
    name: &'a str,
}

/// Stream a remote file to `temp_path` and prove it is safe to install.
///
/// Checks run cheapest first, so a failure costs as little as possible: the byte
/// count and content hash both come free with the transfer, while the integrity
/// check has to read the file back.
fn download_and_verify(
    client: &DropboxClient,
    remote_path: &str,
    temp_path: &Path,
    expected: &Expected<'_>,
) -> Result<()> {
    let (written, actual_hash) = stream_to_file(client, remote_path, temp_path, expected.size)?;

    verify_transfer_size(written, expected.size, expected.name)?;
    verify_content_hash(&actual_hash, expected.hash, expected.name)?;

    let spinner = create_spinner(&format!("Checking {} integrity...", expected.name));
    let result = check_pulled_database(temp_path);
    spinner.finish_and_clear();

    result.with_context(|| {
        format!(
            "the {} downloaded from Dropbox failed its integrity check; \
             the local file was left untouched",
            expected.name
        )
    })
}

/// Write the download to disk, hashing as it streams, and flush it to the device.
///
/// Returns the byte count and the Dropbox content hash of what was written.
fn stream_to_file(
    client: &DropboxClient,
    remote_path: &str,
    temp_path: &Path,
    expected_size: u64,
) -> Result<(u64, String)> {
    let progress = TransferProgress::new(expected_size);

    let file = std::fs::File::create(temp_path)?;
    let mut writer = HashingWriter::new(BufWriter::new(file));

    let written =
        client.download_to_writer(remote_path, &mut writer, &mut |bytes| progress.set(bytes))?;

    let (buffered, actual_hash) = writer.finish();
    let file = buffered
        .into_inner()
        .map_err(|e| anyhow::anyhow!("flushing the download to disk: {}", e))?;

    // Without this the rename can publish a file whose contents never reached
    // the device, leaving a corrupt database after a crash.
    file.sync_all()?;

    Ok((written, actual_hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file a pull exists to replace is often one that will not open. Its
    /// hash is still readable, and nothing about reading it may abort the
    /// command before the download starts.
    #[test]
    fn a_local_file_that_is_not_a_database_still_hashes() {
        let dir = tempfile::TempDir::new().unwrap();
        let corrupt = dir.path().join("grans.db");
        std::fs::write(&corrupt, b"this is not a SQLite database").unwrap();

        assert!(local_content_hash(&corrupt, false).is_some());
    }

    #[test]
    fn an_absent_local_database_has_no_hash() {
        let dir = tempfile::TempDir::new().unwrap();

        assert_eq!(
            local_content_hash(&dir.path().join("nothing.db"), false),
            None
        );
    }

    /// Under --force the answer changes no decision, so hundreds of megabytes
    /// are not read to produce it.
    #[test]
    fn forcing_skips_hashing_the_local_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let local = dir.path().join("grans.db");
        std::fs::write(&local, b"contents").unwrap();

        assert_eq!(local_content_hash(&local, true), None);
    }
}
