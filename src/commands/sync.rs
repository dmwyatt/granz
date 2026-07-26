//! Sync command implementations.

use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};

use chrono::FixedOffset;

use crate::cli::args::DropboxAction;
use crate::output::format::{format_size, OutputMode};
use crate::sync::config::{config_path, SyncConfig};
use crate::sync::dropbox::{format_timestamp, parse_dropbox_time, DropboxClient};
use crate::sync::metadata::SyncMetadata;
use crate::pkce::PkceChallenge;
use crate::sync::oauth::{
    build_auth_url, exchange_code, refresh_access_token, PKCE_ENTROPY_BYTES,
};
use crate::sync::transfer::verify_transfer_size;
use crate::sync::SyncError;

/// Remote paths on Dropbox (within app folder)
pub(super) const REMOTE_DB_PATH: &str = "/grans.db";
pub(super) const REMOTE_METADATA_PATH: &str = "/sync_metadata.json";

/// Run Dropbox commands (init, push, pull, status, logout)
pub fn run_dropbox(action: &DropboxAction, output_mode: OutputMode, tz: &FixedOffset) -> Result<()> {
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

    push_file(
        &client,
        &db_path,
        REMOTE_DB_PATH,
        "database",
        force,
    )?;

    // Generate and upload metadata
    let metadata = SyncMetadata::from_local_db(Some(&db_path))?;
    upload_metadata(&client, &metadata)?;

    // Update last push time
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
    client.upload(&temp_path, REMOTE_METADATA_PATH)?;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    Ok(())
}

fn push_file(
    client: &DropboxClient,
    local_path: &Path,
    remote_path: &str,
    name: &str,
    force: bool,
) -> Result<()> {
    let local_mtime = get_file_mtime(local_path)?;

    // Check remote file
    if !force {
        if let Some(remote) = client.get_metadata(remote_path)? {
            if let Some(remote_mtime) = parse_dropbox_time(&remote.server_modified) {
                if remote_mtime > local_mtime {
                    return Err(SyncError::ConflictRemoteNewer {
                        what: name.to_string(),
                        remote_time: format_timestamp(remote_mtime),
                        local_time: format_timestamp(local_mtime),
                    }
                    .into());
                }
            }
        }
    }

    let size = std::fs::metadata(local_path)?.len();
    println!(
        "Uploading {} ({:.2} MB)...",
        name,
        size as f64 / 1_048_576.0
    );

    client.upload(local_path, remote_path)?;
    println!("  Uploaded to {}", remote_path);

    Ok(())
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
        pull_file(
            &client,
            &db_path,
            REMOTE_DB_PATH,
            "database",
            force,
        )?;

        // Update last pull time
        config.last_pull_time = Some(current_timestamp());
        config.save()?;
        println!("\nPull complete!");
    } else {
        println!("No database on Dropbox");
    }

    Ok(())
}

fn pull_file(
    client: &DropboxClient,
    local_path: &Path,
    remote_path: &str,
    name: &str,
    force: bool,
) -> Result<()> {
    // Get remote metadata
    let remote = client
        .get_metadata(remote_path)?
        .ok_or_else(|| SyncError::DropboxApi(format!("{} not found on Dropbox", remote_path)))?;

    let remote_mtime = parse_dropbox_time(&remote.server_modified);

    // Check local file
    if !force && local_path.exists() {
        let local_mtime = get_file_mtime(local_path)?;
        if let Some(remote_ts) = remote_mtime
            && local_mtime > remote_ts
        {
            return Err(SyncError::ConflictLocalNewer {
                what: name.to_string(),
                local_time: format_timestamp(local_mtime),
                remote_time: format_timestamp(remote_ts),
            }
            .into());
        }
    }

    println!("Downloading {} ({})...", name, format_size(remote.size));

    // Ensure parent directory exists
    if let Some(parent) = local_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Download into a temp file so a failed transfer never replaces a good database.
    let temp_path = local_path.with_extension("db.tmp");
    let downloaded = download_to_temp(client, remote_path, &temp_path, remote.size, name);

    match downloaded {
        Ok(()) => {
            std::fs::rename(&temp_path, local_path)?;
            println!("  Downloaded to {}", local_path.display());
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(e)
        }
    }
}

/// Stream a remote file to `temp_path`, showing a progress bar, and verify its size.
fn download_to_temp(
    client: &DropboxClient,
    remote_path: &str,
    temp_path: &Path,
    expected_size: u64,
    name: &str,
) -> Result<()> {
    let progress = DownloadProgress::new(expected_size);

    let mut file = BufWriter::new(std::fs::File::create(temp_path)?);
    let written =
        client.download_to_writer(remote_path, &mut file, &mut |bytes| progress.set(bytes))?;
    file.flush()?;
    drop(file);

    verify_transfer_size(written, expected_size, name)?;

    Ok(())
}

/// Byte-count progress bar, shown only when stderr is a terminal.
struct DownloadProgress {
    bar: Option<ProgressBar>,
}

impl DownloadProgress {
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
}

/// Clear the bar however the download ends, so a failure message is not printed
/// underneath a stale progress line.
impl Drop for DownloadProgress {
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

    #[test]
    fn test_get_file_mtime() {
        // Use Cargo.toml as a known file
        let result = get_file_mtime(Path::new("Cargo.toml"));
        assert!(result.is_ok());
        assert!(result.unwrap() > 0);
    }
}
