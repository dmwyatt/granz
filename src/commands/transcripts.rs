use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;

use crate::api::ApiError;
use crate::cli::args::AdminTranscriptsAction;
use crate::db::transcripts::{
    clear_transcript_sync_log_entry, count_transcript_sync_failures,
    find_documents_without_transcripts, insert_transcript_from_api, log_transcript_sync_failure,
    DocumentWithoutTranscript,
};
use crate::output::format::OutputMode;
use crate::query::dates::build_date_range;

/// Run admin transcripts commands (fetch, sync)
pub fn run_admin(conn: &Connection, action: &AdminTranscriptsAction, token: Option<&str>, mode: OutputMode) -> Result<()> {
    match action {
        AdminTranscriptsAction::Fetch { document_id, dry_run } => {
            fetch(conn, document_id, *dry_run, token, mode)
        }
        AdminTranscriptsAction::Sync {
            limit,
            since,
            delay_ms,
            dry_run,
            retry,
        } => sync(conn, *limit, since.as_deref(), *delay_ms, *retry, *dry_run, token, mode),
    }
}

fn fetch(conn: &Connection, document_id: &str, dry_run: bool, token: Option<&str>, mode: OutputMode) -> Result<()> {
    // Check if document exists
    let title: Option<String> = conn
        .query_row(
            "SELECT title FROM documents WHERE id = ?1",
            [document_id],
            |row| row.get(0),
        )
        .ok();

    let title_display = title.as_deref().unwrap_or("(untitled)");

    if title.is_none() {
        anyhow::bail!("Document not found: {}", document_id);
    }

    if dry_run {
        match mode {
            OutputMode::Json => {
                println!(
                    "{}",
                    serde_json::json!({
                        "action": "fetch",
                        "dry_run": true,
                        "document_id": document_id,
                        "title": title_display,
                    })
                );
            }
            _ => {
                println!("[dry-run] Would fetch transcript for: {} ({})", title_display, document_id);
            }
        }
        return Ok(());
    }

    let resolved_token = crate::api::resolve_token(token)?;

    eprintln!("Fetching transcript for: {} ({})", title_display, document_id);

    match crate::api::fetch_transcript(&resolved_token, document_id) {
        Ok(response) => {
            if response.transcript.is_empty() {
                match mode {
                    OutputMode::Json => {
                        println!(
                            "{}",
                            serde_json::json!({
                                "action": "fetch",
                                "document_id": document_id,
                                "title": title_display,
                                "result": "empty",
                                "utterances": 0,
                            })
                        );
                    }
                    _ => {
                        println!("No transcript available for this document.");
                    }
                }
                return Ok(());
            }

            let count = insert_transcript_from_api(conn, document_id, &response.transcript)?;

            match mode {
                OutputMode::Json => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "action": "fetch",
                            "document_id": document_id,
                            "title": title_display,
                            "result": "success",
                            "utterances": count,
                        })
                    );
                }
                _ => {
                    println!("Fetched and stored {} utterances.", count);
                }
            }
        }
        Err(ApiError::NotFound) => {
            match mode {
                OutputMode::Json => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "action": "fetch",
                            "document_id": document_id,
                            "title": title_display,
                            "result": "not_found",
                        })
                    );
                }
                _ => {
                    println!("Transcript not found on server.");
                }
            }
        }
        Err(e) => {
            anyhow::bail!("API error: {}", e);
        }
    }

    Ok(())
}

fn sync(
    conn: &Connection,
    limit: Option<usize>,
    since: Option<&str>,
    delay_ms: u64,
    retry: bool,
    dry_run: bool,
    token: Option<&str>,
    mode: OutputMode,
) -> Result<()> {
    // Parse since date if provided
    let since_date = if let Some(s) = since {
        // Try to parse as date or relative date
        let utc_tz = chrono::FixedOffset::east_opt(0).unwrap();
        let date_range = build_date_range(Some(s), None, None, Utc::now(), &utc_tz);
        date_range.and_then(|r| r.start).map(|dt| dt.to_rfc3339())
    } else {
        None
    };

    let skip_failures = !retry;
    let skipped = if skip_failures {
        count_transcript_sync_failures(conn, since_date.as_deref())?
    } else {
        0
    };

    let documents =
        find_documents_without_transcripts(conn, since_date.as_deref(), limit, skip_failures)?;

    if documents.is_empty() {
        match mode {
            OutputMode::Json => {
                println!(
                    "{}",
                    serde_json::json!({
                        "action": "sync",
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

    if skipped > 0 && !dry_run {
        eprintln!(
            "[grans] Skipping {} documents with previous sync failures (use --retry to include)",
            skipped
        );
    }

    if dry_run {
        print_sync_dry_run(&documents, skipped, mode);
        return Ok(());
    }

    let resolved_token = crate::api::resolve_token(token)?;

    let mut fetched = 0;
    let mut errors = 0;
    let mut not_found = 0;
    let total = documents.len();

    for (i, doc) in documents.iter().enumerate() {
        let title = doc.title.as_deref().unwrap_or("(untitled)");
        eprintln!(
            "[{}/{}] Fetching: {} ({})",
            i + 1,
            total,
            title,
            doc.id
        );

        match crate::api::fetch_transcript(&resolved_token, &doc.id) {
            Ok(response) => {
                if response.transcript.is_empty() {
                    eprintln!("  -> No transcript available");
                    not_found += 1;
                    log_transcript_sync_failure(conn, &doc.id, "not_found").ok();
                } else {
                    match insert_transcript_from_api(conn, &doc.id, &response.transcript) {
                        Ok(count) => {
                            eprintln!("  -> Stored {} utterances", count);
                            fetched += 1;
                            clear_transcript_sync_log_entry(conn, &doc.id).ok();
                        }
                        Err(e) => {
                            eprintln!("  -> Error storing: {}", e);
                            errors += 1;
                            log_transcript_sync_failure(conn, &doc.id, "error").ok();
                        }
                    }
                }
            }
            Err(ApiError::NotFound) => {
                eprintln!("  -> Not found on server");
                not_found += 1;
                log_transcript_sync_failure(conn, &doc.id, "not_found").ok();
            }
            Err(ApiError::Unauthorized) => {
                anyhow::bail!("Authentication failed. Please re-login to Granola.");
            }
            Err(ApiError::RateLimited) => {
                eprintln!("  -> Rate limited, stopping sync");
                break;
            }
            Err(e) => {
                eprintln!("  -> Error: {}", e);
                errors += 1;
                log_transcript_sync_failure(conn, &doc.id, "error").ok();
            }
        }

        // Rate limiting: sleep between requests (except for the last one)
        if i < total - 1 {
            crate::api::client::sleep_with_jitter(delay_ms);
        }
    }

    match mode {
        OutputMode::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "action": "sync",
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
            println!("Sync complete:");
            println!("  Fetched:   {}", fetched);
            println!("  Not found: {}", not_found);
            println!("  Errors:    {}", errors);
            if skipped > 0 {
                println!("  Skipped:   {} (previously failed)", skipped);
            }
        }
    }

    Ok(())
}

fn print_sync_dry_run(documents: &[DocumentWithoutTranscript], skipped: usize, mode: OutputMode) {
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
                    "action": "sync",
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
            for doc in documents {
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
}
