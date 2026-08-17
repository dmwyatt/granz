//! Complete sync pipeline: the `grans sync --all` orchestration.
//!
//! Runs every stage that brings the local database fully up to date, in
//! dependency order: entity sync (documents, people, calendars, templates,
//! recipes), then transcripts, then panels (both iterate over the synced
//! document list), then embeddings (which pick up the new transcript and
//! panel chunks).

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::args::DEFAULT_SYNC_DELAY_MS;
use crate::output::format::OutputMode;

use super::sync_panels::sync_panels;
use super::sync_transcripts::sync_transcripts;

/// Run the four sync stages in order. `retry` re-attempts documents whose
/// transcript or panel fetch previously failed or came back empty.
pub fn run_complete_sync(
    conn: &Connection,
    retry: bool,
    dry_run: bool,
    token: Option<&str>,
    mode: OutputMode,
) -> Result<()> {
    super::sync_granola::sync_all(conn, dry_run, token, mode)?;

    eprintln!("[grans] Syncing transcripts...");
    sync_transcripts(
        conn,
        None,
        None,
        DEFAULT_SYNC_DELAY_MS,
        retry,
        dry_run,
        token,
        mode,
    )?;

    eprintln!("[grans] Syncing panels...");
    sync_panels(
        conn,
        None,
        None,
        DEFAULT_SYNC_DELAY_MS,
        retry,
        dry_run,
        token,
        mode,
    )?;

    if dry_run {
        match mode {
            OutputMode::Json => println!(
                "{}",
                serde_json::json!({
                    "action": "embed",
                    "dry_run": true,
                    "message": "Embeddings would be built for content added by the sync stages",
                })
            ),
            _ => println!("[dry-run] Would build embeddings for content added by the sync stages."),
        }
        return Ok(());
    }

    eprintln!("[grans] Building embeddings...");
    super::embed::run_after_sync(conn, mode)
}
