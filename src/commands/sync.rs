//! Sync command implementations.

use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;

use chrono::FixedOffset;

use crate::cli::args::DropboxAction;
use crate::output::format::OutputMode;
use crate::sync::config::{config_path, SyncConfig};
use crate::sync::dropbox::{format_timestamp, parse_dropbox_time, DropboxClient, FileMetadata};
use crate::sync::metadata::SyncMetadata;
use crate::sync::oauth::{build_auth_url, exchange_code, refresh_access_token, PkceChallenge};
use crate::sync::SyncError;

/// Remote paths on Dropbox (within app folder)
const REMOTE_DB_PATH: &str = "/grans.db";
const REMOTE_METADATA_PATH: &str = "/sync_metadata.json";

/// Run Dropbox commands (init, push, pull, status, logout)
pub fn run_dropbox(action: &DropboxAction, output_mode: OutputMode, tz: &FixedOffset) -> Result<()> {
    match action {
        DropboxAction::Init => init()?,
        DropboxAction::Push { force } => push(*force)?,
        DropboxAction::Pull { force } => pull(*force)?,
        DropboxAction::Status => status(output_mode, tz)?,
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
    let pkce = PkceChallenge::generate();
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
    let client = DropboxClient::new(access_token);

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
    let client = DropboxClient::new(access_token);

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
        if let Some(remote_ts) = remote_mtime {
            if local_mtime > remote_ts {
                return Err(SyncError::ConflictLocalNewer {
                    what: name.to_string(),
                    local_time: format_timestamp(local_mtime),
                    remote_time: format_timestamp(remote_ts),
                }
                .into());
            }
        }
    }

    println!(
        "Downloading {} ({:.2} MB)...",
        name,
        remote.size as f64 / 1_048_576.0
    );

    let content = client.download(remote_path)?;

    // Ensure parent directory exists
    if let Some(parent) = local_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write to temp file first for atomic operation
    let temp_path = local_path.with_extension("db.tmp");
    std::fs::write(&temp_path, &content)?;
    std::fs::rename(&temp_path, local_path)?;

    println!("  Downloaded to {}", local_path.display());

    Ok(())
}

/// File information for status display
#[derive(Debug, Clone, Serialize)]
struct FileInfo {
    exists: bool,
    size_bytes: Option<u64>,
    modified_time: Option<u64>,
}

impl FileInfo {
    fn from_local(path: &Path) -> Self {
        if !path.exists() {
            return Self {
                exists: false,
                size_bytes: None,
                modified_time: None,
            };
        }

        let meta = std::fs::metadata(path).ok();
        Self {
            exists: true,
            size_bytes: meta.as_ref().map(|m| m.len()),
            modified_time: get_file_mtime(path).ok(),
        }
    }

    fn from_remote(meta: Option<&FileMetadata>) -> Self {
        match meta {
            Some(m) => Self {
                exists: true,
                size_bytes: Some(m.size),
                modified_time: parse_dropbox_time(&m.server_modified),
            },
            None => Self {
                exists: false,
                size_bytes: None,
                modified_time: None,
            },
        }
    }
}

/// Complete sync status data for JSON output
#[derive(Debug, Serialize)]
struct SyncStatusData {
    authenticated: bool,
    last_push_time: Option<u64>,
    last_pull_time: Option<u64>,
    local: Option<SyncMetadata>,
    remote: Option<SyncMetadata>,
    local_db: FileInfo,
    remote_db: FileInfo,
}

fn status(output_mode: OutputMode, tz: &FixedOffset) -> Result<()> {
    let config = SyncConfig::load()?;

    // Get local database path
    let db_path = crate::db::connection::default_db_path()?;

    // Collect local metadata
    let local_metadata = SyncMetadata::from_local_db(
        db_path.exists().then_some(&db_path),
    )
    .ok();

    // Local file info
    let local_db_info = FileInfo::from_local(&db_path);

    // Remote info (only if authenticated)
    let (remote_metadata, remote_db_info) =
        if config.is_authenticated() {
            match get_access_token(&config) {
                Ok(access_token) => {
                    let client = DropboxClient::new(access_token);
                    let remote_meta = fetch_remote_metadata(&client);
                    let db_meta = client.get_metadata(REMOTE_DB_PATH).ok().flatten();

                    (
                        remote_meta,
                        FileInfo::from_remote(db_meta.as_ref()),
                    )
                }
                Err(_) => (
                    None,
                    FileInfo::from_remote(None),
                ),
            }
        } else {
            (
                None,
                FileInfo::from_remote(None),
            )
        };

    let status_data = SyncStatusData {
        authenticated: config.is_authenticated(),
        last_push_time: config.last_push_time,
        last_pull_time: config.last_pull_time,
        local: local_metadata.clone(),
        remote: remote_metadata.clone(),
        local_db: local_db_info.clone(),
        remote_db: remote_db_info.clone(),
    };

    match output_mode {
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&status_data)?);
        }
        OutputMode::Tty => {
            print_status_tty(&status_data, tz);
        }
    }

    Ok(())
}

fn fetch_remote_metadata(client: &DropboxClient) -> Option<SyncMetadata> {
    match client.download(REMOTE_METADATA_PATH) {
        Ok(bytes) => {
            let json = String::from_utf8(bytes).ok()?;
            match serde_json::from_str(&json) {
                Ok(meta) => Some(meta),
                Err(e) => {
                    eprintln!("Warning: Could not parse remote metadata: {}", e);
                    None
                }
            }
        }
        Err(_) => None, // File doesn't exist or couldn't be downloaded
    }
}

fn print_status_tty(data: &SyncStatusData, tz: &FixedOffset) {
    use colored::Colorize;

    println!("{}", "Sync Status".bold());
    println!("{}", "───────────".dimmed());

    // Authentication
    if data.authenticated {
        println!("Authentication: {}", "Connected".green());
    } else {
        println!("Authentication: {}", "Not connected".red());
        println!(
            "\nRun '{}' to connect to Dropbox.",
            "grans dropbox init".cyan()
        );
        return;
    }

    // Last sync times
    if let Some(ts) = data.last_push_time {
        println!("Last push: {}", format_timestamp(ts).dimmed());
    } else {
        println!("Last push: {}", "Never".dimmed());
    }
    if let Some(ts) = data.last_pull_time {
        println!("Last pull: {}", format_timestamp(ts).dimmed());
    } else {
        println!("Last pull: {}", "Never".dimmed());
    }

    println!();

    // Column headers
    let header = format!(
        "{:28} {:>18}    {:>18}",
        "",
        "Local".bold(),
        "Remote".bold()
    );
    println!("{}", header);
    let separator = format!(
        "{:28} {:>18}    {:>18}",
        "",
        "─────".dimmed(),
        "──────".dimmed()
    );
    println!("{}", separator);

    // Get local and remote stats
    let local_idx = data.local.as_ref().and_then(|m| m.index_db.as_ref());
    let remote_idx = data.remote.as_ref().and_then(|m| m.index_db.as_ref());

    // Documents
    print_comparison_row(
        "Documents:",
        local_idx.map(|i| format_number(i.document_count)),
        remote_idx.map(|i| format_number(i.document_count)),
    );

    // With transcripts
    print_comparison_row(
        "With transcripts:",
        local_idx.map(|i| format_number(i.documents_with_transcripts)),
        remote_idx.map(|i| format_number(i.documents_with_transcripts)),
    );

    // Utterances
    print_comparison_row(
        "Utterances:",
        local_idx.map(|i| format_number(i.transcript_utterance_count)),
        remote_idx.map(|i| format_number(i.transcript_utterance_count)),
    );

    // People
    print_comparison_row(
        "People:",
        local_idx.map(|i| format_number(i.people_count)),
        remote_idx.map(|i| format_number(i.people_count)),
    );

    // Date range
    let local_range = local_idx.map(|i| format_date_range(&i.earliest_document, &i.latest_document));
    let remote_range =
        remote_idx.map(|i| format_date_range(&i.earliest_document, &i.latest_document));
    print_comparison_row("Date range:", local_range, remote_range);

    // Schema version
    let local_schema = local_idx.map(|i| i.schema_version);
    let remote_schema = remote_idx.map(|i| i.schema_version);
    let schema_mismatch =
        local_schema.is_some() && remote_schema.is_some() && local_schema != remote_schema;
    print_comparison_row_with_warning(
        "Schema version:",
        local_idx.map(|i| i.schema_version.to_string()),
        remote_idx.map(|i| i.schema_version.to_string()),
        schema_mismatch,
    );

    // Database size
    print_comparison_row(
        "Database size:",
        data.local_db.size_bytes.map(format_size),
        data.remote_db.size_bytes.map(format_size),
    );

    // Database modified
    print_comparison_row(
        "Database modified:",
        data.local_db.modified_time.map(|ts| format_short_time(ts, tz)),
        data.remote_db.modified_time.map(|ts| format_short_time(ts, tz)),
    );

    // Embeddings
    let local_emb_count = local_idx.map(|i| i.embedding_count).unwrap_or(0);
    let remote_emb_count = remote_idx.map(|i| i.embedding_count)
        .or_else(|| data.remote.as_ref().and_then(|m| m.embeddings_db.as_ref()).map(|e| e.embedding_count))
        .unwrap_or(0);
    print_comparison_row(
        "Embeddings:",
        Some(format_number(local_emb_count)),
        Some(format_number(remote_emb_count)),
    );

    // Embedding model
    let local_model = local_idx.and_then(|i| i.embedding_model.clone());
    let remote_model = remote_idx.and_then(|i| i.embedding_model.clone())
        .or_else(|| data.remote.as_ref().and_then(|m| m.embeddings_db.as_ref()).and_then(|e| e.model.clone()));
    print_comparison_row(
        "Embedding model:",
        local_model,
        remote_model,
    );

    if schema_mismatch {
        println!();
        println!(
            "{}",
            "Warning: Schema version mismatch between local and remote databases.".yellow()
        );
    }
}

fn print_comparison_row(label: &str, local: Option<String>, remote: Option<String>) {
    print_comparison_row_with_warning(label, local, remote, false);
}

fn print_comparison_row_with_warning(
    label: &str,
    local: Option<String>,
    remote: Option<String>,
    warn: bool,
) {
    use colored::Colorize;

    let local_str = local.unwrap_or_else(|| "—".to_string());
    let remote_str = remote.unwrap_or_else(|| "unknown".to_string());

    if warn {
        println!(
            "{:28} {:>18}    {:>18} {}",
            label,
            local_str,
            remote_str.yellow(),
            "⚠".yellow()
        );
    } else {
        println!("{:28} {:>18}    {:>18}", label, local_str, remote_str);
    }
}

fn format_number(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn format_short_time(ts: u64, tz: &FixedOffset) -> String {
    use chrono::DateTime;
    let dt = DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
    dt.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string()
}

fn format_date_range(earliest: &Option<String>, latest: &Option<String>) -> String {
    match (earliest, latest) {
        (Some(e), Some(l)) => {
            let e_short = e.get(..7).unwrap_or(e);
            let l_short = l.get(..7).unwrap_or(l);
            format!("{} → {}", e_short, l_short)
        }
        _ => "—".to_string(),
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
fn get_access_token(config: &SyncConfig) -> Result<String> {
    let refresh_token = config
        .refresh_token
        .as_ref()
        .ok_or(SyncError::NotAuthenticated)?;

    let tokens = refresh_access_token(refresh_token)?;
    Ok(tokens.access_token)
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

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1.0K");
        assert_eq!(format_number(1500), "1.5K");
        assert_eq!(format_number(52847), "52.8K");
        assert_eq!(format_number(1000000), "1.0M");
        assert_eq!(format_number(1500000), "1.5M");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(47185920), "45.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
    }

    #[test]
    fn test_format_date_range() {
        assert_eq!(
            format_date_range(
                &Some("2023-06-01T00:00:00Z".to_string()),
                &Some("2025-01-27T00:00:00Z".to_string())
            ),
            "2023-06 → 2025-01"
        );
        assert_eq!(format_date_range(&None, &None), "—");
    }

    #[test]
    fn test_file_info_from_local_nonexistent() {
        let info = FileInfo::from_local(Path::new("/nonexistent/path/file.db"));
        assert!(!info.exists);
        assert!(info.size_bytes.is_none());
        assert!(info.modified_time.is_none());
    }

    #[test]
    fn test_file_info_from_local_existing() {
        let info = FileInfo::from_local(Path::new("Cargo.toml"));
        assert!(info.exists);
        assert!(info.size_bytes.is_some());
        assert!(info.modified_time.is_some());
    }

    #[test]
    fn test_sync_status_data_serialization() {
        let data = SyncStatusData {
            authenticated: true,
            last_push_time: Some(1737973800),
            last_pull_time: None,
            local: None,
            remote: None,
            local_db: FileInfo {
                exists: true,
                size_bytes: Some(1048576),
                modified_time: Some(1737973800),
            },
            remote_db: FileInfo {
                exists: true,
                size_bytes: Some(1048000),
                modified_time: Some(1737970000),
            },
        };

        let json = serde_json::to_string_pretty(&data).unwrap();
        assert!(json.contains("authenticated"));
        assert!(json.contains("local_db"));
        assert!(json.contains("remote_db"));
    }
}
