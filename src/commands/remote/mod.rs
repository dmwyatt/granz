//! `grans dropbox`: share the database between machines.

mod progress;
mod pull;
mod push;
mod status;

use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::FixedOffset;

use crate::cli::args::DropboxAction;
use crate::output::format::OutputMode;
use crate::pkce::PkceChallenge;
use crate::sync::config::{SyncConfig, config_path};
use crate::sync::dropbox::oauth::{PKCE_ENTROPY_BYTES, build_auth_url, exchange_code};

/// What the shared copy of the database is called on the remote.
const REMOTE_DB: &str = "grans.db";
/// Database statistics pushed alongside it, so status can compare without a download.
const REMOTE_METADATA: &str = "sync_metadata.json";

/// Run Dropbox commands (init, push, pull, status, logout)
pub fn run_dropbox(
    action: &DropboxAction,
    output_mode: OutputMode,
    tz: &FixedOffset,
) -> Result<()> {
    match action {
        DropboxAction::Init => init()?,
        DropboxAction::Push { force } => push::push(*force)?,
        DropboxAction::Pull { force } => pull::pull(*force)?,
        DropboxAction::Status => status::status(output_mode, tz)?,
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

/// Get file modification time as Unix timestamp.
fn get_file_mtime(path: &Path) -> Result<u64> {
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
