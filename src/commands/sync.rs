//! Sync command implementations.

use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use log::debug;

use chrono::FixedOffset;

use crate::cli::args::DropboxAction;
use crate::db::integrity::check_pulled_database;
use crate::db::wal::{checkpoint_file, discard, replace_database};
use crate::output::format::{OutputMode, format_size};
use crate::output::progress::create_spinner;
use crate::pkce::PkceChallenge;
use crate::sync::SyncError;
use crate::sync::config::{SyncConfig, config_path};
use crate::sync::content_hash::{HashingWriter, hash_file};
use crate::sync::dropbox::DropboxClient;
use crate::sync::metadata::SyncMetadata;
use crate::sync::oauth::{PKCE_ENTROPY_BYTES, build_auth_url, exchange_code, refresh_access_token};
use crate::sync::reconcile::{TransferDecision, decide};
use crate::sync::transfer::{ProgressFn, no_progress, verify_content_hash, verify_transfer_size};

/// Remote paths on Dropbox (within app folder)
pub(super) const REMOTE_DB_PATH: &str = "/grans.db";
pub(super) const REMOTE_METADATA_PATH: &str = "/sync_metadata.json";

/// Run Dropbox commands (init, push, pull, status, logout)
pub fn run_dropbox(
    action: &DropboxAction,
    output_mode: OutputMode,
    tz: &FixedOffset,
) -> Result<()> {
    match action {
        DropboxAction::Init => init()?,
        DropboxAction::Push { force } => push(*force)?,
        DropboxAction::Pull { force } => pull(*force)?,
        DropboxAction::Status => super::sync_status::status(output_mode, tz)?,
        DropboxAction::Logout => logout()?,
    }
    Ok(())
}

fn init() -> Result<()> {
    let mut config = SyncConfig::load()?;

    if config.is_authenticated() {
        println!("Already authenticated with Dropbox.");
        print!("Re-authenticate? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Keeping existing authentication.");
            return Ok(());
        }
    }

    // Generate PKCE challenge
    let pkce = PkceChallenge::generate(PKCE_ENTROPY_BYTES);
    let auth_url = build_auth_url(&pkce.challenge);

    println!("\nOpening browser for Dropbox authorization...");
    println!("\nIf the browser doesn't open, visit this URL:");
    println!("{}\n", auth_url);

    // Try to open browser
    if let Err(e) = open::that(&auth_url) {
        eprintln!("Could not open browser: {}", e);
    }

    // Get authorization code from user
    print!("Enter the authorization code from Dropbox: ");
    io::stdout().flush()?;

    let mut code = String::new();
    io::stdin().read_line(&mut code)?;
    let code = code.trim();

    if code.is_empty() {
        anyhow::bail!("No authorization code provided");
    }

    // Exchange code for tokens
    println!("Exchanging code for tokens...");
    let tokens = exchange_code(code, &pkce.verifier)?;

    // Store refresh token
    config.refresh_token = tokens.refresh_token;
    config.save()?;

    println!("\nSuccessfully authenticated with Dropbox!");
    println!("Your database will sync to Apps/grans/ in your Dropbox.");

    Ok(())
}

/// Push database to Dropbox
fn push(force: bool) -> Result<()> {
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

/// Pull database from Dropbox
fn pull(force: bool) -> Result<()> {
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

/// Byte-count progress bar for a transfer in either direction, shown only when
/// stderr is a terminal.
struct TransferProgress {
    bar: Option<ProgressBar>,
}

impl TransferProgress {
    fn new(total: u64) -> Self {
        if !io::stderr().is_terminal() {
            return Self { bar: None };
        }

        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "[grans] {bytes}/{total_bytes} [{bar:30}] {percent}% {bytes_per_sec}, ETA {eta}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=> "),
        );
        Self { bar: Some(pb) }
    }

    fn set(&self, bytes: u64) {
        if let Some(ref pb) = self.bar {
            pb.set_position(bytes);
        }
    }

    /// An owned reporter for callers that hand progress off to the transport.
    ///
    /// The handle is a cheap clone of the same bar, so updates through it land
    /// on the bar this value still controls and clears.
    fn reporter(&self) -> ProgressFn {
        match self.bar {
            Some(ref pb) => {
                let handle = pb.clone();
                Box::new(move |bytes| handle.set_position(bytes))
            }
            None => no_progress(),
        }
    }
}

/// Clear the bar however the transfer ends, so a failure message is not printed
/// underneath a stale progress line.
impl Drop for TransferProgress {
    fn drop(&mut self) {
        if let Some(ref pb) = self.bar {
            pb.finish_and_clear();
        }
    }
}

fn logout() -> Result<()> {
    let mut config = SyncConfig::load()?;

    if !config.is_authenticated() {
        println!("Not currently authenticated.");
        return Ok(());
    }

    config.clear_auth();
    config.save()?;

    // Also try to remove the config file
    if let Ok(path) = config_path() {
        let _ = std::fs::remove_file(path);
    }

    println!("Logged out from Dropbox.");
    println!("Your database on Dropbox has not been deleted.");

    Ok(())
}

/// Get a valid access token, refreshing if necessary.
pub(super) fn get_access_token(config: &SyncConfig) -> Result<String> {
    let refresh_token = config
        .refresh_token
        .as_ref()
        .ok_or(SyncError::NotAuthenticated)?;

    let tokens = refresh_access_token(refresh_token)?;
    Ok(tokens.access_token)
}

/// Get file modification time as Unix timestamp.
pub(super) fn get_file_mtime(path: &Path) -> Result<u64> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified()?;
    Ok(mtime
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs())
}

/// Get current time as Unix timestamp.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_timestamp() {
        let ts = current_timestamp();
        // Should be after 2025-01-01
        assert!(ts > 1735689600);
    }

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

    #[test]
    fn test_get_file_mtime() {
        // Use Cargo.toml as a known file
        let result = get_file_mtime(Path::new("Cargo.toml"));
        assert!(result.is_ok());
        assert!(result.unwrap() > 0);
    }
}
