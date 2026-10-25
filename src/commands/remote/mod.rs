//! `grans remote`: share the database between machines.

mod init;
mod progress;
mod pull;
mod push;
mod status;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::FixedOffset;

use crate::cli::args::RemoteAction;
use crate::output::format::OutputMode;
use crate::sync::config::{SyncConfig, config_path};

/// What the shared copy of the database is called on the remote.
const REMOTE_DB: &str = "grans.db";
/// Database statistics pushed alongside it, so status can compare without a download.
const REMOTE_METADATA: &str = "sync_metadata.json";

/// Run a `grans remote` subcommand.
pub fn run(action: &RemoteAction, output_mode: OutputMode, tz: &FixedOffset) -> Result<()> {
    match action {
        RemoteAction::Init { provider } => init::init(provider)?,
        RemoteAction::Push { force } => push::push(*force)?,
        RemoteAction::Pull { force } => pull::pull(*force)?,
        RemoteAction::Status => status::status(output_mode, tz)?,
        RemoteAction::Logout => logout()?,
    }
    Ok(())
}

fn logout() -> Result<()> {
    let mut config = SyncConfig::load()?;

    if !config.is_authenticated() {
        println!("Not currently authenticated.");
        return Ok(());
    }

    let provider = config.provider();
    config.clear_auth();
    config.save()?;

    // Also try to remove the config file
    if let Ok(path) = config_path() {
        let _ = std::fs::remove_file(path);
    }

    println!("Logged out from {}.", provider);
    println!("Your database on {} has not been deleted.", provider);

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
