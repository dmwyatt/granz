//! Embed command: explicit control over embedding generation.
//!
//! This module implements the `grans embed` command which gives users
//! control over when embeddings are built for semantic search.

use std::io::{self, Write};

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::args::EmbedAction;
use crate::embed::config::{EmbedOverrides, EmbedSpec};
use crate::embed::{self, EmbeddingStatus};
use crate::output::format::OutputMode;

/// Run the embed command. `overrides` carries the hidden experiment
/// flags; they win over the stored scheme, which wins over defaults.
pub fn run(
    conn: &Connection,
    action: Option<&EmbedAction>,
    yes: bool,
    batch_size: usize,
    mode: OutputMode,
    overrides: &EmbedOverrides,
) -> Result<()> {
    let spec =
        EmbedSpec::resolve_stored(conn, embed::MODEL_MAX_TOKENS).with_overrides(overrides)?;
    match action {
        Some(EmbedAction::Status) => show_status(conn, mode, &spec),
        Some(EmbedAction::Clear { count }) => clear_embeddings(conn, *count, yes, mode, &spec),
        None => embed_with_prompt(conn, yes, batch_size, mode, &spec),
    }
}

/// Show embedding status without triggering embedding.
fn show_status(conn: &Connection, mode: OutputMode, spec: &EmbedSpec) -> Result<()> {
    let status = embed::get_embedding_status(conn, embed::model::MODEL_NAME, spec)?;

    match mode {
        OutputMode::Json => print_status_json(&status, spec),
        OutputMode::Tty => print_status_tty(&status, spec),
    }

    Ok(())
}

fn print_status_json(status: &EmbeddingStatus, spec: &EmbedSpec) {
    let mut json = serde_json::json!({
        "chunking": {
            "target_tokens": spec.chunking.target_tokens,
            "overlap_tokens": spec.chunking.overlap_tokens,
            "overlap_mode": spec.chunking.overlap_mode.as_str(),
            "contextual_headers": spec.contextual_headers,
        },
        "chunking_changed_warning": status.chunking_changed_warning,
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
        "model_changed_warning": status.model_changed_warning,
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

fn print_status_tty(status: &EmbeddingStatus, spec: &EmbedSpec) {
    println!("\x1b[1mEmbedding Status\x1b[0m");
    println!("\x1b[2m────────────────\x1b[0m");

    let model = status.model_name.as_deref().unwrap_or("(not set)");
    println!("Model:      {}", model);
    if let Some(max_len) = status.max_length {
        println!("Max length: {} tokens", format_number(max_len));
    }
    println!(
        "Chunking:   {} target / {} overlap tokens, {} overlap, headers {}",
        format_number(spec.chunking.target_tokens),
        format_number(spec.chunking.overlap_tokens),
        spec.chunking.overlap_mode.as_str(),
        if spec.contextual_headers { "on" } else { "off" },
    );
    println!();

    println!("Total:     {} chunks", format_number(status.total_chunks));
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

    println!("Pending:   {} chunks", format_number(status.pending_chunks));
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
        println!("  \x1b[33m⚠\x1b[0m Embeddings were created with unknown max_length settings.");
        println!("    Run `grans embed` to re-embed with current settings.");
    }

    if status.model_changed_warning {
        println!();
        println!("\x1b[1;33mWarning\x1b[0m");
        println!("\x1b[2m───────\x1b[0m");
        println!("  \x1b[33m⚠\x1b[0m Embeddings were created by a different embedding model.");
        println!("    Run `grans embed` to rebuild them.");
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
    spec: &EmbedSpec,
) -> Result<()> {
    let status = embed::get_embedding_status(conn, embed::model::MODEL_NAME, spec)?;

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
            eprintln!("\nThis will clear all {} embeddings.", format_number(total));
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

/// The nothing-needs-the-model outcome of [`reconcile_without_model`],
/// carried back so each caller can phrase it for its output channel.
enum ShortCircuit {
    /// No embeddable content exists; any stored chunks were orphans and
    /// have been deleted.
    NoContent { orphans_removed: usize },
    /// Every desired chunk is already embedded and nothing is orphaned.
    AlreadyEmbedded,
}

/// Compute embedding status with a sync watermark captured before the
/// status read, and when nothing needs the embedding model, reconcile the
/// store (delete orphans) and certify freshness. Centralizing capture,
/// cleanup, and stamp here keeps every short-circuit path honest: a path
/// that skips this helper cannot accidentally certify coverage.
fn reconcile_without_model(
    conn: &Connection,
    spec: &EmbedSpec,
) -> Result<(embed::EmbeddingStatus, Option<ShortCircuit>)> {
    let watermark = embed::freshness::current_sync_watermark(conn)?;
    let status = embed::get_embedding_status(conn, embed::model::MODEL_NAME, spec)?;

    if status.total_chunks == 0 {
        let orphans_removed = status.orphan_ids.len();
        embed::store::delete_chunks(conn, &status.orphan_ids)?;
        embed::store::set_embedded_watermark(conn, watermark.as_deref())?;
        return Ok((status, Some(ShortCircuit::NoContent { orphans_removed })));
    }

    if status.pending_chunks == 0 && status.orphaned_chunks == 0 {
        embed::store::set_embedded_watermark(conn, watermark.as_deref())?;
        return Ok((status, Some(ShortCircuit::AlreadyEmbedded)));
    }

    Ok((status, None))
}

/// Embed with optional confirmation prompt.
fn embed_with_prompt(
    conn: &Connection,
    yes: bool,
    batch_size: usize,
    mode: OutputMode,
    spec: &EmbedSpec,
) -> Result<()> {
    let (status, short_circuit) = reconcile_without_model(conn, spec)?;

    match short_circuit {
        Some(ShortCircuit::NoContent { orphans_removed }) => {
            match mode {
                OutputMode::Json => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "action": "embed",
                            "message": "No content to embed",
                            "total_chunks": 0,
                            "orphans_removed": orphans_removed,
                        })
                    );
                }
                _ => {
                    println!("No embeddable content found.");
                    if orphans_removed > 0 {
                        println!(
                            "Removed {} orphaned embeddings.",
                            format_number(orphans_removed)
                        );
                    }
                }
            }
            return Ok(());
        }
        Some(ShortCircuit::AlreadyEmbedded) => {
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
        None => {}
    }

    // Prompt unless --yes or non-TTY
    if !yes && mode == OutputMode::Tty && (status.pending_chunks > 0 || status.orphaned_chunks > 0)
    {
        let needs_full_reembed = status.orphaned_chunks > 0
            || status.legacy_max_length_warning
            || status.model_changed_warning
            || status.chunking_changed_warning;

        if needs_full_reembed {
            eprintln!("\nEmbeddings need to be rebuilt:");
            if status.legacy_max_length_warning {
                eprintln!("  - Existing embeddings use an outdated chunking strategy");
            }
            if status.model_changed_warning {
                eprintln!("  - Existing embeddings were created by a different embedding model");
            }
            if status.chunking_changed_warning {
                eprintln!("  - Existing embeddings use a different chunking scheme");
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

    do_embed(conn, batch_size, mode, spec)
}

/// Actually perform the embedding.
fn do_embed(
    conn: &Connection,
    batch_size: usize,
    mode: OutputMode,
    spec: &EmbedSpec,
) -> Result<()> {
    let embedder = embed::model::ProductionEmbedder::new()?;
    let index = embed::ensure_embeddings(conn, &embedder, batch_size, spec)?;

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
    let spec = EmbedSpec::resolve_stored(conn, embed::MODEL_MAX_TOKENS);
    let (status, short_circuit) = reconcile_without_model(conn, &spec)?;

    match short_circuit {
        Some(ShortCircuit::NoContent { orphans_removed }) => {
            if mode != OutputMode::Json {
                eprintln!("[grans] No embeddable content found.");
                if orphans_removed > 0 {
                    eprintln!(
                        "[grans] Removed {} orphaned embeddings.",
                        format_number(orphans_removed)
                    );
                }
            }
            return Ok(());
        }
        Some(ShortCircuit::AlreadyEmbedded) => {
            if mode != OutputMode::Json {
                eprintln!(
                    "[grans] All {} chunks already embedded.",
                    format_number(status.total_chunks)
                );
            }
            return Ok(());
        }
        None => {}
    }

    if mode != OutputMode::Json {
        eprintln!(
            "[grans] Building embeddings for {} chunks...",
            format_number(status.pending_chunks)
        );
        if embed::model::should_warn_cpu_only() {
            eprintln!(
                "[grans] Note: CPU-only build; embedding runs ~25x slower than a GPU build \
                 (see README for GPU build features)."
            );
        }
    }

    do_embed(conn, embed::DEFAULT_BATCH_SIZE, mode, &spec)
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
    if total == 0 { 0 } else { (100 * part) / total }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::model::MockEmbedder;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    /// A database fully embedded under the production model name, without
    /// loading the production model: embed with the mock, then relabel.
    /// Content hashes don't involve the model, so the status check sees
    /// nothing pending and the already-embedded short-circuits fire.
    fn setup_fully_embedded_db() -> Connection {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc1', 'Doc', '2025-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, start_timestamp, text)
             VALUES ('u1', 'doc1', '2025-01-01T10:00:00Z',
                     'This is a longer utterance that contains enough characters to meet the minimum chunk size requirement for embedding.')",
            [],
        )
        .unwrap();
        let embedder = MockEmbedder::default();
        let spec = EmbedSpec::default_for(512);
        embed::ensure_embeddings(&conn, &embedder, embed::DEFAULT_BATCH_SIZE, &spec).unwrap();
        conn.execute(
            "UPDATE embedding_metadata SET value = ?1 WHERE key = 'model_name'",
            [embed::model::MODEL_NAME],
        )
        .unwrap();
        conn
    }

    const SYNC_STAMP: &str = "2025-06-01T00:00:00+00:00";

    fn set_sync_stamp(conn: &Connection) {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('last_sync_transcripts', ?1)",
            [SYNC_STAMP],
        )
        .unwrap();
    }

    fn stored_watermark(conn: &Connection) -> Option<String> {
        embed::store::get_embedded_watermark(conn).unwrap()
    }

    fn chunk_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn embed_with_prompt_certifies_when_already_embedded() {
        let conn = setup_fully_embedded_db();
        set_sync_stamp(&conn);

        let spec = EmbedSpec::default_for(512);
        embed_with_prompt(
            &conn,
            false,
            embed::DEFAULT_BATCH_SIZE,
            OutputMode::Json,
            &spec,
        )
        .unwrap();

        assert_eq!(stored_watermark(&conn).as_deref(), Some(SYNC_STAMP));
    }

    #[test]
    fn embed_with_prompt_certifies_when_no_content() {
        let conn = setup_test_db();
        set_sync_stamp(&conn);

        let spec = EmbedSpec::default_for(512);
        embed_with_prompt(
            &conn,
            false,
            embed::DEFAULT_BATCH_SIZE,
            OutputMode::Json,
            &spec,
        )
        .unwrap();

        assert_eq!(stored_watermark(&conn).as_deref(), Some(SYNC_STAMP));
    }

    #[test]
    fn embed_with_prompt_no_content_removes_orphans() {
        // Regression for #129 review: the no-content short-circuit used to
        // certify freshness and return before any orphan cleanup, leaving
        // vectors for deleted sources served to search forever.
        let conn = setup_fully_embedded_db();
        conn.execute("DELETE FROM transcript_utterances", [])
            .unwrap();
        set_sync_stamp(&conn);
        assert!(chunk_count(&conn) > 0);

        let spec = EmbedSpec::default_for(512);
        embed_with_prompt(
            &conn,
            false,
            embed::DEFAULT_BATCH_SIZE,
            OutputMode::Json,
            &spec,
        )
        .unwrap();

        assert_eq!(chunk_count(&conn), 0);
        assert_eq!(stored_watermark(&conn).as_deref(), Some(SYNC_STAMP));
    }

    #[test]
    fn run_after_sync_certifies_when_already_embedded() {
        let conn = setup_fully_embedded_db();
        set_sync_stamp(&conn);

        run_after_sync(&conn, OutputMode::Json).unwrap();

        assert_eq!(stored_watermark(&conn).as_deref(), Some(SYNC_STAMP));
    }

    #[test]
    fn run_after_sync_certifies_when_no_content() {
        let conn = setup_test_db();
        set_sync_stamp(&conn);

        run_after_sync(&conn, OutputMode::Json).unwrap();

        assert_eq!(stored_watermark(&conn).as_deref(), Some(SYNC_STAMP));
    }

    #[test]
    fn run_after_sync_no_content_removes_orphans() {
        let conn = setup_fully_embedded_db();
        conn.execute("DELETE FROM transcript_utterances", [])
            .unwrap();
        set_sync_stamp(&conn);

        run_after_sync(&conn, OutputMode::Json).unwrap();

        assert_eq!(chunk_count(&conn), 0);
        assert_eq!(stored_watermark(&conn).as_deref(), Some(SYNC_STAMP));
    }
}
