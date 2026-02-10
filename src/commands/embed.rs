//! Embed command: explicit control over embedding generation.
//!
//! This module implements the `grans embed` command which gives users
//! control over when embeddings are built for semantic search.

use std::io::{self, Write};

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::args::EmbedAction;
use crate::embed::{self, EmbeddingStatus};
use crate::output::format::OutputMode;

/// Run the embed command.
pub fn run(
    conn: &Connection,
    action: Option<&EmbedAction>,
    yes: bool,
    batch_size: usize,
    mode: OutputMode,
) -> Result<()> {
    match action {
        Some(EmbedAction::Status) => show_status(conn, mode),
        Some(EmbedAction::Clear { count }) => clear_embeddings(conn, *count, yes, mode),
        None => embed_with_prompt(conn, yes, batch_size, mode),
    }
}

/// Show embedding status without triggering embedding.
fn show_status(conn: &Connection, mode: OutputMode) -> Result<()> {
    let status = embed::get_embedding_status(conn)?;

    match mode {
        OutputMode::Json => print_status_json(&status),
        OutputMode::Tty => print_status_tty(&status),
    }

    Ok(())
}

fn print_status_json(status: &EmbeddingStatus) {
    let mut json = serde_json::json!({
        "total_chunks": status.total_chunks,
        "embedded_chunks": status.embedded_chunks,
        "pending_chunks": status.pending_chunks,
        "orphaned_chunks": status.orphaned_chunks,
        "total_by_type": {
            "transcript_window": status.total_by_type.transcript_window,
            "panel_section": status.total_by_type.panel_section,
            "notes_paragraph": status.total_by_type.notes_paragraph,
        },
        "embedded_by_type": {
            "transcript_window": status.embedded_by_type.transcript_window,
            "panel_section": status.embedded_by_type.panel_section,
            "notes_paragraph": status.embedded_by_type.notes_paragraph,
        },
        "pending_by_type": {
            "transcript_window": status.pending_by_type.transcript_window,
            "panel_section": status.pending_by_type.panel_section,
            "notes_paragraph": status.pending_by_type.notes_paragraph,
        },
        "model": status.model_name,
        "max_length": status.max_length,
        "legacy_max_length_warning": status.legacy_max_length_warning,
    });

    if let Some(stats) = &status.chunk_size_stats {
        json["chunk_size_stats"] = serde_json::json!({
            "total_chunks": stats.total_chunks,
            "characters": {
                "avg": stats.avg_chars,
                "min": stats.min_chars,
                "max": stats.max_chars,
                "median": stats.median_chars,
                "p10": stats.p10_chars,
                "p90": stats.p90_chars,
                "p99": stats.p99_chars,
            },
            "tokens_estimated": {
                "avg": stats.avg_tokens_est,
                "median": stats.median_tokens_est,
                "max": stats.max_tokens_est,
            },
            "warnings": {
                "chunks_over_limit": stats.chunks_over_limit,
                "chunks_very_small": stats.chunks_very_small,
            },
        });
    }

    println!("{}", json);
}

fn print_status_tty(status: &EmbeddingStatus) {
    println!("\x1b[1mEmbedding Status\x1b[0m");
    println!("\x1b[2m────────────────\x1b[0m");

    let model = status.model_name.as_deref().unwrap_or("(not set)");
    println!("Model:      {}", model);
    if let Some(max_len) = status.max_length {
        println!("Max length: {} tokens", format_number(max_len));
    }
    println!();

    println!(
        "Total:     {} chunks",
        format_number(status.total_chunks)
    );
    if status.total_chunks > 0 {
        println!(
            "  Transcripts:  {} chunks ({}%)",
            format_number(status.total_by_type.transcript_window),
            percentage(status.total_by_type.transcript_window, status.total_chunks)
        );
        println!(
            "  Panels:       {} chunks ({}%)",
            format_number(status.total_by_type.panel_section),
            percentage(status.total_by_type.panel_section, status.total_chunks)
        );
        println!(
            "  Notes:        {} chunks ({}%)",
            format_number(status.total_by_type.notes_paragraph),
            percentage(status.total_by_type.notes_paragraph, status.total_chunks)
        );
    }
    println!();

    println!(
        "Embedded:  {} chunks",
        format_number(status.embedded_chunks)
    );
    if status.embedded_chunks > 0 {
        println!(
            "  Transcripts:  {} chunks",
            format_number(status.embedded_by_type.transcript_window)
        );
        println!(
            "  Panels:       {} chunks",
            format_number(status.embedded_by_type.panel_section)
        );
        println!(
            "  Notes:        {} chunks",
            format_number(status.embedded_by_type.notes_paragraph)
        );
    }
    println!();

    println!(
        "Pending:   {} chunks",
        format_number(status.pending_chunks)
    );
    if status.pending_chunks > 0 {
        println!(
            "  Transcripts:  {} chunks",
            format_number(status.pending_by_type.transcript_window)
        );
        println!(
            "  Panels:       {} chunks",
            format_number(status.pending_by_type.panel_section)
        );
        println!(
            "  Notes:        {} chunks",
            format_number(status.pending_by_type.notes_paragraph)
        );
    }

    if status.orphaned_chunks > 0 {
        println!();
        println!(
            "Orphaned:  {} chunks (will be cleaned up)",
            format_number(status.orphaned_chunks)
        );
    }

    if let Some(stats) = &status.chunk_size_stats {
        println!();
        println!("\x1b[1mChunk Sizes\x1b[0m");
        println!("\x1b[2m───────────\x1b[0m");
        println!(
            "  Characters:  {} avg, {} median (range: {} - {})",
            format_number(stats.avg_chars.round() as usize),
            format_number(stats.median_chars),
            format_number(stats.min_chars),
            format_number(stats.max_chars)
        );
        println!(
            "  Tokens (est): {} avg, {} median, {} max",
            format_number(stats.avg_tokens_est),
            format_number(stats.median_tokens_est),
            format_number(stats.max_tokens_est)
        );
        println!();
        println!("\x1b[1mDistribution (characters)\x1b[0m");
        println!("\x1b[2m─────────────────────────\x1b[0m");
        println!(
            "  p10: {}  |  p50: {}  |  p90: {}  |  p99: {}",
            format_number(stats.p10_chars),
            format_number(stats.median_chars),
            format_number(stats.p90_chars),
            format_number(stats.p99_chars)
        );

        // Show warnings if there are problematic chunks
        if stats.chunks_over_limit > 0 || stats.chunks_very_small > 0 {
            println!();
            println!("\x1b[1;33mWarnings\x1b[0m");
            println!("\x1b[2m────────\x1b[0m");
            if stats.chunks_over_limit > 0 {
                let pct = 100.0 * stats.chunks_over_limit as f64 / stats.total_chunks as f64;
                println!(
                    "  \x1b[33m⚠\x1b[0m {} chunks ({:.1}%) exceed {}-token limit — content truncated during embedding",
                    format_number(stats.chunks_over_limit),
                    pct,
                    crate::embed::MODEL_MAX_TOKENS
                );
            }
            if stats.chunks_very_small > 0 {
                let pct = 100.0 * stats.chunks_very_small as f64 / stats.total_chunks as f64;
                println!(
                    "  \x1b[33m⚠\x1b[0m {} chunks ({:.1}%) are < 50 chars — may lack semantic meaning",
                    format_number(stats.chunks_very_small),
                    pct
                );
            }
        }
    }

    // Show legacy warning if embeddings exist but max_length is unknown
    if status.legacy_max_length_warning {
        println!();
        println!("\x1b[1;33mWarning\x1b[0m");
        println!("\x1b[2m───────\x1b[0m");
        println!(
            "  \x1b[33m⚠\x1b[0m Embeddings were created with unknown max_length settings."
        );
        println!(
            "    Run `grans embed` to re-embed with current settings."
        );
    }

    if status.pending_chunks > 0 {
        println!();
        println!("Run `grans embed` to build embeddings for pending chunks.");
    }
}

/// Clear embeddings (all or most recent N).
fn clear_embeddings(
    conn: &Connection,
    count: Option<usize>,
    yes: bool,
    mode: OutputMode,
) -> Result<()> {
    let status = embed::get_embedding_status(conn)?;

    if status.embedded_chunks == 0 && status.orphaned_chunks == 0 {
        match mode {
            OutputMode::Json => {
                println!(
                    "{}",
                    serde_json::json!({
                        "action": "clear",
                        "message": "No embeddings to clear",
                        "cleared": 0,
                    })
                );
            }
            _ => {
                println!("No embeddings to clear.");
            }
        }
        return Ok(());
    }

    let to_clear = count.unwrap_or(status.embedded_chunks);
    let actual_clear = to_clear.min(status.embedded_chunks);

    // Prompt unless --yes or non-TTY
    if !yes && mode == OutputMode::Tty {
        if count.is_some() {
            eprintln!(
                "\nThis will clear {} most recent embeddings.",
                format_number(actual_clear)
            );
        } else {
            let total = status.embedded_chunks + status.orphaned_chunks;
            eprintln!(
                "\nThis will clear all {} embeddings.",
                format_number(total)
            );
        }
        eprint!("Proceed? [y/N] ");
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let cleared = if let Some(n) = count {
        embed::store::delete_recent_chunks(conn, n)?
    } else {
        embed::wipe_all_embeddings(conn)?;
        status.embedded_chunks + status.orphaned_chunks
    };

    match mode {
        OutputMode::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "action": "clear",
                    "cleared": cleared,
                })
            );
        }
        _ => {
            println!("Cleared {} embeddings.", format_number(cleared));
        }
    }

    Ok(())
}

/// Embed with optional confirmation prompt.
fn embed_with_prompt(conn: &Connection, yes: bool, batch_size: usize, mode: OutputMode) -> Result<()> {
    let status = embed::get_embedding_status(conn)?;

    if status.total_chunks == 0 {
        match mode {
            OutputMode::Json => {
                println!(
                    "{}",
                    serde_json::json!({
                        "action": "embed",
                        "message": "No content to embed",
                        "total_chunks": 0,
                    })
                );
            }
            _ => {
                println!("No embeddable content found.");
            }
        }
        return Ok(());
    }

    if status.pending_chunks == 0 && status.orphaned_chunks == 0 {
        match mode {
            OutputMode::Json => {
                println!(
                    "{}",
                    serde_json::json!({
                        "action": "embed",
                        "message": "All chunks already embedded",
                        "total_chunks": status.total_chunks,
                        "embedded_chunks": status.embedded_chunks,
                    })
                );
            }
            _ => {
                println!(
                    "All {} chunks are already embedded.",
                    format_number(status.total_chunks)
                );
            }
        }
        return Ok(());
    }

    // Prompt unless --yes or non-TTY
    if !yes && mode == OutputMode::Tty && (status.pending_chunks > 0 || status.orphaned_chunks > 0) {
        let needs_full_reembed = status.orphaned_chunks > 0 || status.legacy_max_length_warning;

        if needs_full_reembed {
            eprintln!("\nEmbeddings need to be rebuilt:");
            if status.legacy_max_length_warning {
                eprintln!("  - Existing embeddings use an outdated chunking strategy");
            }
            if status.orphaned_chunks > 0 {
                eprintln!(
                    "  - {} existing chunks will be deleted",
                    format_number(status.orphaned_chunks)
                );
            }
            eprintln!(
                "  - {} new chunks will be embedded",
                format_number(status.pending_chunks)
            );
        } else {
            eprintln!(
                "\n{} chunks need embedding.",
                format_number(status.pending_chunks)
            );
        }
        eprint!("Proceed? [y/N] ");
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    do_embed(conn, batch_size, mode)
}

/// Actually perform the embedding.
fn do_embed(conn: &Connection, batch_size: usize, mode: OutputMode) -> Result<()> {
    let embedder = embed::model::FastEmbedModel::new()?;
    let index = embed::ensure_embeddings(conn, &embedder, batch_size)?;

    match mode {
        OutputMode::Json => {
            let mut json = serde_json::json!({
                "action": "embed",
                "success": true,
                "total_vectors": index.vectors.len(),
            });
            if let Some(stats) = &index.stats {
                json["stats"] = serde_json::json!({
                    "chunks_embedded": stats.chunks_embedded,
                    "elapsed_secs": stats.elapsed_secs,
                    "chunks_per_sec": stats.chunks_per_sec,
                });
            }
            println!("{}", json);
        }
        _ => {
            println!(
                "Embedding complete. {} vectors ready for search.",
                format_number(index.vectors.len())
            );
        }
    }

    Ok(())
}

/// Run embedding after sync (called from sync_granola when --embed is set).
/// Does not prompt since user explicitly requested embedding.
pub fn run_after_sync(conn: &Connection, mode: OutputMode) -> Result<()> {
    let status = embed::get_embedding_status(conn)?;

    if status.total_chunks == 0 {
        if mode != OutputMode::Json {
            eprintln!("[grans] No embeddable content found.");
        }
        return Ok(());
    }

    if status.pending_chunks == 0 && status.orphaned_chunks == 0 {
        if mode != OutputMode::Json {
            eprintln!(
                "[grans] All {} chunks already embedded.",
                format_number(status.total_chunks)
            );
        }
        return Ok(());
    }

    if mode != OutputMode::Json {
        eprintln!(
            "[grans] Building embeddings for {} chunks...",
            format_number(status.pending_chunks)
        );
    }

    do_embed(conn, embed::DEFAULT_BATCH_SIZE, mode)
}

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

fn percentage(part: usize, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        (100 * part) / total
    }
}
