//! Transcript sync: fetch transcripts from the Granola API for documents that don't have them.

use anyhow::Result;
use chrono::Utc;
use log::debug;
use rusqlite::Connection;

use crate::api::ApiError;
use crate::db::sync;
use crate::db::transcripts::{
    clear_transcript_sync_log_entry, count_transcript_sync_failures,
    find_documents_without_transcripts, insert_transcript_from_api, log_transcript_sync_failure,
};
use crate::output::format::OutputMode;
use crate::output::progress::SyncProgress;
use crate::query::dates::build_date_range;

pub(super) fn sync_transcripts(
    conn: &Connection,
    limit: Option<usize>,
    since: Option<&str>,
    delay_ms: u64,
    retry: bool,
    dry_run: bool,
    token: Option<&str>,
    mode: OutputMode,
) -> Result<()> {
    debug!(
        "sync_transcripts (limit={:?}, since={:?}, retry={}, dry_run={})",
        limit, since, retry, dry_run
    );
    // Parse since date if provided
    let since_date = if let Some(s) = since {
        let utc_tz = chrono::FixedOffset::east_opt(0).unwrap();
        let date_range = build_date_range(Some(s), None, None, Utc::now(), &utc_tz);
        date_range.and_then(|r| r.start).map(|dt| dt.to_rfc3339())
    } else {
        None
    };

    if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        eprintln!("[grans] Finding documents that need transcripts...");
    }

    let skip_failures = !retry;
    let skipped = if skip_failures {
        count_transcript_sync_failures(conn, since_date.as_deref())?
    } else {
        0
    };

    let documents =
        find_documents_without_transcripts(conn, since_date.as_deref(), limit, skip_failures)?;
    debug!(
        "Found {} documents without transcripts (skipped {} failures)",
        documents.len(),
        skipped
    );

    if documents.is_empty() {
        match mode {
            OutputMode::Json => {
                println!(
                    "{}",
                    serde_json::json!({
                        "action": "sync_transcripts",
                        "dry_run": dry_run,
                        "documents_found": 0,
                        "skipped": skipped,
                        "message": "No documents without transcripts found",
                    })
                );
            }
            _ => {
                if skipped > 0 {
                    println!(
                        "No new documents to sync transcripts for ({} skipped from previous failures, use --retry to include).",
                        skipped
                    );
                } else {
                    println!("No documents without transcripts found.");
                }
            }
        }
        return Ok(());
    }

    if skipped > 0 && !dry_run && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        eprintln!(
            "[grans] Skipping {} documents with previous sync failures (use --retry to include)",
            skipped
        );
    }

    if dry_run {
        match mode {
            OutputMode::Json => {
                let doc_info: Vec<_> = documents
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "id": d.id,
                            "title": d.title,
                            "created_at": d.created_at,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({
                        "action": "sync_transcripts",
                        "dry_run": true,
                        "documents_found": documents.len(),
                        "skipped": skipped,
                        "documents": doc_info,
                    })
                );
            }
            OutputMode::Tty => {
                println!(
                    "[dry-run] Would sync {} document(s):\n",
                    documents.len()
                );
                for doc in &documents {
                    let title = doc.title.as_deref().unwrap_or("(untitled)");
                    let date = doc.created_at.as_deref().unwrap_or("unknown");
                    println!("  {} - {} ({})", doc.id, title, date);
                }
                if skipped > 0 {
                    println!(
                        "\n  ({} skipped from previous failures, use --retry to include)",
                        skipped
                    );
                }
            }
        }
        return Ok(());
    }

    let resolved_token = crate::api::resolve_token(token)?;

    let mut fetched = 0;
    let mut errors = 0;
    let mut not_found = 0;
    let total = documents.len();
    let mut progress = SyncProgress::new(total as u64);

    for (i, doc) in documents.iter().enumerate() {
        let title = doc.title.as_deref().unwrap_or("(untitled)");
        let date = doc
            .created_at
            .as_deref()
            .and_then(|s| s.get(..10))
            .unwrap_or("unknown date");
        progress.println(&format!(
            "[{}/{}] Fetching: {} ({}) [{}]",
            i + 1,
            total,
            title,
            doc.id,
            date
        ));

        match crate::api::fetch_transcript(&resolved_token, &doc.id) {
            Ok(response) => {
                if response.transcript.is_empty() {
                    progress.println("  -> No transcript available");
                    not_found += 1;
                    log_transcript_sync_failure(conn, &doc.id, "not_found").ok();
                } else {
                    match insert_transcript_from_api(conn, &doc.id, &response.transcript) {
                        Ok(count) => {
                            progress.println(&format!("  -> Stored {} utterances", count));
                            fetched += 1;
                            clear_transcript_sync_log_entry(conn, &doc.id).ok();
                        }
                        Err(e) => {
                            progress.println(&format!("  -> Error storing: {}", e));
                            errors += 1;
                            log_transcript_sync_failure(conn, &doc.id, "error").ok();
                        }
                    }
                }
            }
            Err(ApiError::NotFound) => {
                progress.println("  -> Not found on server");
                not_found += 1;
                log_transcript_sync_failure(conn, &doc.id, "not_found").ok();
            }
            Err(ApiError::Unauthorized) => {
                progress.finish();
                anyhow::bail!("Authentication failed. Please re-login to Granola.");
            }
            Err(ApiError::RateLimited) => {
                progress.println("  -> Rate limited, stopping sync");
                break;
            }
            Err(e) => {
                progress.println(&format!("  -> Error: {}", e));
                errors += 1;
                log_transcript_sync_failure(conn, &doc.id, "error").ok();
            }
        }

        progress.inc();

        // Rate limiting
        if i < total - 1 {
            crate::api::client::sleep_with_jitter(delay_ms);
        }
    }

    progress.finish();

    match mode {
        OutputMode::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "action": "sync_transcripts",
                    "total_attempted": total,
                    "fetched": fetched,
                    "not_found": not_found,
                    "errors": errors,
                    "skipped": skipped,
                })
            );
        }
        _ => {
            println!();
            println!("Transcript sync complete:");
            println!("  Fetched:   {}", fetched);
            println!("  Not found: {}", not_found);
            println!("  Errors:    {}", errors);
            if skipped > 0 {
                println!("  Skipped:   {} (previously failed)", skipped);
            }
        }
    }

    sync::set_last_sync_time(conn, "transcripts")?;

    Ok(())
}
