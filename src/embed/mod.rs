pub mod chunk;
pub mod chunker;
pub mod model;
pub mod progress;
pub mod search;
pub mod store;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use self::chunk::Chunk;
use self::model::Embedder;
use self::search::SemanticSearchResult;
use self::store::StoredVector;

/// Approximate characters per token for nomic-embed-text.
/// This is a rough estimate; actual tokenization varies by content.
const CHARS_PER_TOKEN: f64 = 4.0;

/// Maximum token length for the embedding model.
pub const MODEL_MAX_TOKENS: usize = 512;

/// Per-source-type chunk counts.
#[derive(Debug, Clone, Default)]
pub struct SourceTypeBreakdown {
    pub transcript_window: usize,
    pub panel_section: usize,
    pub notes_paragraph: usize,
}

impl SourceTypeBreakdown {
    pub fn total(&self) -> usize {
        self.transcript_window + self.panel_section + self.notes_paragraph
    }
}

/// Statistics about chunk sizes.
#[derive(Debug, Clone, Serialize)]
pub struct ChunkSizeStats {
    pub avg_chars: f64,
    pub min_chars: usize,
    pub max_chars: usize,
    pub median_chars: usize,
    /// Percentile values (p10, p90, p99)
    pub p10_chars: usize,
    pub p90_chars: usize,
    pub p99_chars: usize,
    /// Estimated token statistics (chars / CHARS_PER_TOKEN)
    pub avg_tokens_est: usize,
    pub median_tokens_est: usize,
    pub max_tokens_est: usize,
    /// Chunks exceeding the model's max token limit (512 tokens)
    pub chunks_over_limit: usize,
    /// Chunks with very few characters (< 50 chars, ~12 tokens)
    pub chunks_very_small: usize,
    /// Total chunk count (for percentage calculations)
    pub total_chunks: usize,
}

/// Status of embeddings in the database.
#[derive(Debug)]
pub struct EmbeddingStatus {
    /// Total number of chunks that should exist (from transcripts).
    pub total_chunks: usize,
    /// Number of chunks already embedded.
    pub embedded_chunks: usize,
    /// Number of chunks that need embedding (new or changed).
    pub pending_chunks: usize,
    /// Number of orphaned chunks (embedded but source deleted).
    pub orphaned_chunks: usize,
    /// Per-source-type breakdown of total chunks.
    pub total_by_type: SourceTypeBreakdown,
    /// Per-source-type breakdown of embedded chunks.
    pub embedded_by_type: SourceTypeBreakdown,
    /// Per-source-type breakdown of pending chunks.
    pub pending_by_type: SourceTypeBreakdown,
    /// The current embedding model name (if any).
    pub model_name: Option<String>,
    /// Chunk size statistics (if chunks exist).
    pub chunk_size_stats: Option<ChunkSizeStats>,
    /// The max_length setting used when embeddings were created (None for legacy).
    pub max_length: Option<usize>,
    /// Warning: embeddings were created with unknown max_length settings.
    pub legacy_max_length_warning: bool,
}

/// Calculate a percentile value from a sorted slice.
fn percentile(sorted: &[usize], p: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p / 100.0).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Estimate tokens from character count.
fn estimate_tokens(chars: usize) -> usize {
    (chars as f64 / CHARS_PER_TOKEN).round() as usize
}

/// Calculate chunk size statistics from stored chunks.
pub fn calculate_chunk_size_stats(conn: &Connection) -> Result<Option<ChunkSizeStats>> {
    let mut stmt = conn.prepare("SELECT LENGTH(text) as len FROM chunks ORDER BY len")?;

    let lengths: Vec<usize> = stmt
        .query_map([], |row| row.get::<_, i64>(0).map(|v| v as usize))?
        .filter_map(Result::ok)
        .collect();

    if lengths.is_empty() {
        return Ok(None);
    }

    let total_chunks = lengths.len();
    let total_chars: usize = lengths.iter().sum();
    let avg_chars = total_chars as f64 / total_chunks as f64;
    let min_chars = *lengths.first().unwrap();
    let max_chars = *lengths.last().unwrap();

    let median_chars = if total_chunks % 2 == 0 {
        (lengths[total_chunks / 2 - 1] + lengths[total_chunks / 2]) / 2
    } else {
        lengths[total_chunks / 2]
    };

    // Calculate percentiles
    let p10_chars = percentile(&lengths, 10.0);
    let p90_chars = percentile(&lengths, 90.0);
    let p99_chars = percentile(&lengths, 99.0);

    // Estimate tokens
    let avg_tokens_est = estimate_tokens(avg_chars.round() as usize);
    let median_tokens_est = estimate_tokens(median_chars);
    let max_tokens_est = estimate_tokens(max_chars);

    // Count problematic chunks
    let max_chars_for_limit = (MODEL_MAX_TOKENS as f64 * CHARS_PER_TOKEN) as usize;
    let chunks_over_limit = lengths.iter().filter(|&&len| len > max_chars_for_limit).count();

    // Very small chunks (< 50 chars, roughly < 12 tokens)
    let chunks_very_small = lengths.iter().filter(|&&len| len < 50).count();

    Ok(Some(ChunkSizeStats {
        avg_chars,
        min_chars,
        max_chars,
        median_chars,
        p10_chars,
        p90_chars,
        p99_chars,
        avg_tokens_est,
        median_tokens_est,
        max_tokens_est,
        chunks_over_limit,
        chunks_very_small,
        total_chunks,
    }))
}

/// Get embedding status without triggering embedding.
/// Returns counts of total, embedded, pending, and orphaned chunks from all sources.
pub fn get_embedding_status(conn: &Connection) -> Result<EmbeddingStatus> {
    use chunk::ChunkSourceType;

    // Get desired chunks from all sources using adaptive chunker with default config
    let config = chunker::ChunkingConfig::default();
    let mut desired_chunks = chunker::transcript_window_chunker_adaptive(conn, &config)?;
    desired_chunks.extend(chunker::panel_section_chunker(conn, &config)?);
    desired_chunks.extend(chunker::notes_paragraph_chunker(conn, 20)?);

    // Get stored chunks
    let stored = store::get_stored_chunks(conn)?;
    let stored_map: HashMap<(&str, &str), &store::StoredChunk> = stored
        .iter()
        .map(|s| ((s.source_type.as_str(), s.source_id.as_str()), s))
        .collect();

    // Build set of desired keys and count by type
    let mut desired_keys: HashSet<(String, String)> = HashSet::new();
    let mut pending_count = 0;
    let mut total_by_type = SourceTypeBreakdown::default();
    let mut pending_by_type = SourceTypeBreakdown::default();

    for chunk in &desired_chunks {
        let key = (chunk.source_type.as_str(), chunk.source_id.as_str());
        desired_keys.insert((chunk.source_type.to_string(), chunk.source_id.clone()));

        // Count total by type
        match chunk.source_type {
            ChunkSourceType::TranscriptWindow => total_by_type.transcript_window += 1,
            ChunkSourceType::PanelSection => total_by_type.panel_section += 1,
            ChunkSourceType::NotesParagraph => total_by_type.notes_paragraph += 1,
        }

        match stored_map.get(&key) {
            Some(existing) if existing.content_hash == chunk.content_hash => {
                // Unchanged — already embedded
            }
            _ => {
                // New or changed — needs embedding
                pending_count += 1;
                match chunk.source_type {
                    ChunkSourceType::TranscriptWindow => pending_by_type.transcript_window += 1,
                    ChunkSourceType::PanelSection => pending_by_type.panel_section += 1,
                    ChunkSourceType::NotesParagraph => pending_by_type.notes_paragraph += 1,
                }
            }
        }
    }

    // Calculate embedded by type (total - pending)
    let embedded_by_type = SourceTypeBreakdown {
        transcript_window: total_by_type.transcript_window - pending_by_type.transcript_window,
        panel_section: total_by_type.panel_section - pending_by_type.panel_section,
        notes_paragraph: total_by_type.notes_paragraph - pending_by_type.notes_paragraph,
    };

    // Count orphans (stored but not in desired)
    let orphan_count = stored
        .iter()
        .filter(|s| !desired_keys.contains(&(s.source_type.clone(), s.source_id.clone())))
        .count();

    // Get model name
    let model_name = store::get_model_name(conn);

    // Get chunk size statistics
    let chunk_size_stats = calculate_chunk_size_stats(conn)?;

    // Get max_length setting
    let max_length = store::get_max_length(conn);

    // Determine if we should show legacy warning:
    // Model exists but max_length is missing (legacy embeddings)
    let legacy_max_length_warning = model_name.is_some() && max_length.is_none();

    Ok(EmbeddingStatus {
        total_chunks: desired_chunks.len(),
        embedded_chunks: desired_chunks.len() - pending_count,
        pending_chunks: pending_count,
        orphaned_chunks: orphan_count,
        total_by_type,
        embedded_by_type,
        pending_by_type,
        model_name,
        chunk_size_stats,
        max_length,
        legacy_max_length_warning,
    })
}

/// Wipe all embeddings from the database.
/// Used by `grans embed --force` to force re-embedding.
pub fn wipe_all_embeddings(conn: &Connection) -> Result<()> {
    conn.execute_batch("DELETE FROM embeddings; DELETE FROM chunks; DELETE FROM embedding_metadata;")?;
    Ok(())
}

/// Speed statistics from an embedding run.
#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingStats {
    /// Number of chunks that were embedded in this run.
    pub chunks_embedded: usize,
    /// Total wall-clock time spent embedding (seconds).
    pub elapsed_secs: f64,
    /// Throughput: chunks per second.
    pub chunks_per_sec: f64,
}

/// In-memory index of all embedded vectors, ready for search.
pub struct EmbeddingIndex {
    pub vectors: Vec<StoredVector>,
    /// Stats from the embedding run, if any chunks were embedded.
    pub stats: Option<EmbeddingStats>,
}

impl EmbeddingIndex {
    pub fn search(&self, query_vec: &[f32], min_score: f32, source_type_filter: Option<&[&str]>) -> Vec<SemanticSearchResult> {
        search::rank_results(query_vec, &self.vectors, min_score, source_type_filter)
    }

    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
}

/// Default batch size for embedding. Can be overridden via --batch-size flag.
pub const DEFAULT_BATCH_SIZE: usize = 16;

/// Ensure embeddings are up-to-date and return an in-memory search index.
/// `conn` is the main database connection (grans.db), which contains both
/// the transcript data and the embeddings tables.
/// `batch_size` controls how many chunks are embedded per batch (higher values
/// may be faster on GPU but use more memory).
pub fn ensure_embeddings(
    conn: &Connection,
    embedder: &dyn Embedder,
    batch_size: usize,
) -> Result<EmbeddingIndex> {
    // Check model consistency — if model changed, all embeddings are wiped
    let model_consistent = store::check_model_consistency(conn, embedder.model_name())?;

    // Build chunking config from embedder's max_length
    let chunking_config = chunker::ChunkingConfig::from_max_length(embedder.max_length());

    // Run all chunkers to get desired chunks
    let mut desired_chunks = chunker::transcript_window_chunker_adaptive(conn, &chunking_config)?;
    desired_chunks.extend(chunker::panel_section_chunker(conn, &chunking_config)?);
    desired_chunks.extend(chunker::notes_paragraph_chunker(conn, 20)?);

    if desired_chunks.is_empty() {
        if !model_consistent {
            store::set_model_metadata(conn, embedder.model_name(), embedder.dimension(), embedder.max_length())?;
        }
        eprintln!("[grans] No embeddable content found.");
        return Ok(EmbeddingIndex {
            vectors: Vec::new(),
            stats: None,
        });
    }

    // Get stored state
    let stored = store::get_stored_chunks(conn)?;
    let stored_map: HashMap<(&str, &str), &store::StoredChunk> = stored
        .iter()
        .map(|s| ((s.source_type.as_str(), s.source_id.as_str()), s))
        .collect();

    // Diff: find new/changed chunks and orphaned chunks
    let mut to_embed: Vec<&Chunk> = Vec::new();
    let mut desired_keys: HashSet<(String, String)> = HashSet::new();

    for chunk in &desired_chunks {
        let key = (chunk.source_type.as_str(), chunk.source_id.as_str());
        desired_keys.insert((chunk.source_type.to_string(), chunk.source_id.clone()));

        match stored_map.get(&key) {
            Some(existing) if existing.content_hash == chunk.content_hash => {
                // Unchanged — skip
            }
            _ => {
                // New or changed
                to_embed.push(chunk);
            }
        }
    }

    // Find orphans (stored but not in desired)
    let orphan_ids: Vec<i64> = stored
        .iter()
        .filter(|s| !desired_keys.contains(&(s.source_type.clone(), s.source_id.clone())))
        .map(|s| s.id)
        .collect();

    // Delete orphans
    if !orphan_ids.is_empty() {
        store::delete_chunks(conn, &orphan_ids)?;
    }

    // Embed new/changed chunks
    let stats = if !to_embed.is_empty() {
        let pb = progress::embedding_progress_bar(to_embed.len() as u64);
        let start = Instant::now();

        for batch in to_embed.chunks(batch_size) {
            let texts: Vec<&str> = batch.iter().map(|c| c.text.as_str()).collect();

            match embedder.embed_batch(&texts) {
                Ok(vectors) => {
                    let items: Vec<(&Chunk, &[f32])> = batch
                        .iter()
                        .zip(vectors.iter())
                        .map(|(chunk, vec)| (*chunk, vec.as_slice()))
                        .collect();

                    let results = store::insert_chunks_with_embeddings_batch(conn, &items);

                    for (i, result) in results.iter().enumerate() {
                        if let Err(e) = result {
                            eprintln!(
                                "[grans] Warning: failed to store chunk {}: {}",
                                batch[i].source_id, e
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[grans] Warning: batch embedding failed: {}", e);
                }
            }

            pb.inc(batch.len() as u64);
        }

        pb.finish_and_clear();

        let elapsed = start.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();
        let chunks_embedded = to_embed.len();
        let chunks_per_sec = if elapsed_secs > 0.0 {
            chunks_embedded as f64 / elapsed_secs
        } else {
            chunks_embedded as f64
        };

        eprintln!(
            "[grans] Embedded {} chunks in {:.1}s ({:.1} chunks/sec).",
            chunks_embedded, elapsed_secs, chunks_per_sec,
        );

        Some(EmbeddingStats {
            chunks_embedded,
            elapsed_secs,
            chunks_per_sec,
        })
    } else {
        None
    };

    // Store model metadata
    store::set_model_metadata(conn, embedder.model_name(), embedder.dimension(), embedder.max_length())?;

    // Load all vectors for search
    let vectors = store::load_all_vectors(conn)?;

    Ok(EmbeddingIndex { vectors, stats })
}

/// Filter semantic search results by document creation date.
fn filter_results_by_date(
    conn: &Connection,
    results: Vec<SemanticSearchResult>,
    date_range: &crate::query::dates::DateRange,
    include_deleted: bool,
) -> Result<Vec<SemanticSearchResult>> {
    use chrono::DateTime;
    use std::collections::HashSet;

    // Get all document IDs from results
    let doc_ids: Vec<String> = results.iter().map(|r| r.document_id.clone()).collect();

    if doc_ids.is_empty() {
        return Ok(results);
    }

    // Query documents table to get created_at dates
    let placeholders = doc_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let deleted_filter = if include_deleted { "" } else { " AND deleted_at IS NULL" };
    let sql = format!(
        "SELECT id, created_at FROM documents WHERE id IN ({}){}",
        placeholders, deleted_filter
    );

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = doc_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();

    let mut valid_doc_ids = HashSet::new();
    let rows = stmt.query_map(params.as_slice(), |row| {
        let id: String = row.get(0)?;
        let created_at_str: String = row.get(1)?;
        Ok((id, created_at_str))
    })?;

    for row in rows {
        if let Ok((id, created_at_str)) = row {
            if let Ok(created_at) = DateTime::parse_from_rfc3339(&created_at_str) {
                let created_utc = created_at.with_timezone(&chrono::Utc);

                // Check if the date is in range
                let mut in_range = true;
                if let Some(start) = &date_range.start {
                    if &created_utc < start {
                        in_range = false;
                    }
                }
                if let Some(end) = &date_range.end {
                    if &created_utc >= end {
                        in_range = false;
                    }
                }

                if in_range {
                    valid_doc_ids.insert(id);
                }
            }
        }
    }

    // Filter results to only include documents in the date range
    Ok(results
        .into_iter()
        .filter(|r| valid_doc_ids.contains(&r.document_id))
        .collect())
}

/// Run a semantic search: ensure embeddings, embed query, rank results.
/// Returns a tuple of (results, total_count) where total_count is the number of matches
/// before applying the limit.
/// `source_type_filter`: if `Some`, only search vectors of these source types.
/// `None` means search all source types.
pub fn semantic_search(
    conn: &Connection,
    query: &str,
    date_range: Option<&crate::query::dates::DateRange>,
    limit: usize,
    source_type_filter: Option<&[&str]>,
    include_deleted: bool,
) -> Result<(Vec<SemanticSearchResult>, usize)> {
    let embedder = model::FastEmbedModel::new()?;
    let index = ensure_embeddings(conn, &embedder, DEFAULT_BATCH_SIZE)?;

    if index.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let query_vec = embedder.embed_query(query)?;
    let mut results = index.search(&query_vec, 0.0, source_type_filter);

    // Filter results by date range if provided
    if let Some(range) = date_range {
        results = filter_results_by_date(conn, results, range, include_deleted)?;
    }

    // Capture total count before applying limit
    let total_count = results.len();

    // Apply limit (0 = no limit)
    if limit > 0 {
        results.truncate(limit);
    }

    Ok((results, total_count))
}

/// Run a semantic search with a provided embedder (for testing).
/// Returns a tuple of (results, total_count). Uses limit=0 (no limit) by default.
#[cfg(test)]
pub fn semantic_search_with_embedder(
    conn: &Connection,
    query: &str,
    embedder: &dyn Embedder,
) -> Result<(Vec<SemanticSearchResult>, usize)> {
    semantic_search_with_embedder_and_limit(conn, query, embedder, 0)
}

/// Run a semantic search with a provided embedder and limit (for testing).
/// Returns a tuple of (results, total_count).
#[cfg(test)]
pub fn semantic_search_with_embedder_and_limit(
    conn: &Connection,
    query: &str,
    embedder: &dyn Embedder,
    limit: usize,
) -> Result<(Vec<SemanticSearchResult>, usize)> {
    let index = ensure_embeddings(conn, embedder, DEFAULT_BATCH_SIZE)?;

    if index.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let query_vec = embedder.embed_query(query)?;
    let results = index.search(&query_vec, 0.0, None);
    let total_count = results.len();

    let results = if limit > 0 {
        results.into_iter().take(limit).collect()
    } else {
        results
    };

    Ok((results, total_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::model::MockEmbedder;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn insert_utterances(conn: &Connection, doc_id: &str, texts: &[&str]) {
        // First ensure the document exists (for foreign key constraint)
        conn.execute(
            "INSERT OR IGNORE INTO documents (id, title, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![doc_id, format!("Test Doc {}", doc_id), "2025-01-01T00:00:00Z"],
        )
        .unwrap();

        let mut stmt = conn
            .prepare(
                "INSERT INTO transcript_utterances (id, document_id, start_timestamp, end_timestamp, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .unwrap();

        for (i, text) in texts.iter().enumerate() {
            stmt.execute(rusqlite::params![
                format!("{}-u{}", doc_id, i),
                doc_id,
                format!("2025-01-01T10:{:02}:00Z", i),
                format!("2025-01-01T10:{:02}:30Z", i),
                text,
            ])
            .unwrap();
        }
    }

    #[test]
    fn test_ensure_embeddings_empty_db() {
        let conn = setup_test_db();
        let embedder = MockEmbedder::default();

        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn test_ensure_embeddings_creates_vectors() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold (50 chars)
        insert_utterances(&conn, "doc1", &[
            "This is a longer utterance that contains enough characters to meet the minimum chunk size requirement for embedding."
        ]);

        let embedder = MockEmbedder::default();

        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert!(!index.is_empty());
    }

    #[test]
    fn test_ensure_embeddings_incremental() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold
        insert_utterances(&conn, "doc1", &[
            "First document content that is long enough to pass the minimum character threshold for embedding chunks."
        ]);

        let embedder = MockEmbedder::default();

        // First run
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        let count1 = index.vectors.len();
        assert!(count1 > 0);

        // Second run with same data — should not re-embed
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert_eq!(index.vectors.len(), count1);

        // Add more data (with document first for foreign key)
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc2', 'Test Doc 2', '2025-01-02T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, start_timestamp, text)
             VALUES ('new-u', 'doc2', '2025-01-02T10:00:00Z', 'Second document with new content that is long enough to pass the minimum character threshold for chunks.')",
            [],
        )
        .unwrap();

        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert!(index.vectors.len() > count1);
    }

    #[test]
    fn test_semantic_search_with_embedder() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold
        insert_utterances(
            &conn,
            "doc1",
            &["We had a detailed deployment strategy discussion today about how to deploy the application to production servers efficiently."],
        );
        insert_utterances(
            &conn,
            "doc2",
            &["We discussed lunch plans for tomorrow and various options for what we should eat together as a team."],
        );

        let embedder = MockEmbedder { dim: 8, max_length: 512 };

        let (results, total_count) =
            semantic_search_with_embedder(&conn, "deploy", &embedder).unwrap();

        assert!(!results.is_empty());
        assert_eq!(results.len(), total_count);
        // Results should be ranked by similarity
        for i in 1..results.len() {
            assert!(results[i - 1].score >= results[i].score);
        }
    }

    #[test]
    fn test_semantic_search_with_limit() {
        let conn = setup_test_db();
        // Create 5 documents with utterances long enough to pass min_chars threshold
        for i in 1..=5 {
            insert_utterances(
                &conn,
                &format!("doc{}", i),
                &[&format!("This is document {} with content about topic {} that is long enough to meet the minimum character threshold for embedding.", i, i)],
            );
        }

        let embedder = MockEmbedder { dim: 8, max_length: 512 };

        // Search with limit=2
        let (results, total_count) =
            semantic_search_with_embedder_and_limit(&conn, "topic", &embedder, 2).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(total_count, 5);

        // Search with limit=0 (no limit)
        let (results, total_count) =
            semantic_search_with_embedder_and_limit(&conn, "topic", &embedder, 0).unwrap();

        assert_eq!(results.len(), 5);
        assert_eq!(total_count, 5);

        // Search with limit > total results
        let (results, total_count) =
            semantic_search_with_embedder_and_limit(&conn, "topic", &embedder, 100).unwrap();

        assert_eq!(results.len(), 5);
        assert_eq!(total_count, 5);
    }

    #[test]
    fn test_ensure_embeddings_returns_stats_when_embedding() {
        let conn = setup_test_db();
        insert_utterances(&conn, "doc1", &[
            "This is document content that is long enough to meet the minimum character threshold for the embedding chunker."
        ]);

        let embedder = MockEmbedder {
            dim: 4,
            ..Default::default()
        };

        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        let stats = index.stats.expect("stats should be present after embedding");
        assert!(stats.chunks_embedded > 0);
        assert!(stats.elapsed_secs >= 0.0);
        assert!(stats.chunks_per_sec >= 0.0);
    }

    #[test]
    fn test_ensure_embeddings_no_stats_when_already_embedded() {
        let conn = setup_test_db();
        insert_utterances(&conn, "doc1", &[
            "This is document content that is long enough to meet the minimum character threshold for the embedding chunker."
        ]);

        let embedder = MockEmbedder {
            dim: 4,
            ..Default::default()
        };

        // First run embeds everything
        ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();

        // Second run — nothing to embed
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert!(index.stats.is_none(), "stats should be None when no new chunks embedded");
    }

    #[test]
    fn test_ensure_embeddings_no_stats_when_empty_db() {
        let conn = setup_test_db();
        let embedder = MockEmbedder {
            dim: 4,
            ..Default::default()
        };

        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert!(index.stats.is_none(), "stats should be None when no content exists");
    }

    #[test]
    fn test_orphan_cleanup() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold
        insert_utterances(&conn, "doc1", &[
            "This is document content that is long enough to meet the minimum character threshold for the embedding chunker."
        ]);

        let embedder = MockEmbedder::default();

        // First run creates embeddings for doc1
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert_eq!(index.vectors.len(), 1);

        // Remove doc1's utterances
        conn.execute("DELETE FROM transcript_utterances", [])
            .unwrap();

        // Re-run — should clean up orphan
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn test_percentile_calculation() {
        // Empty slice
        assert_eq!(percentile(&[], 50.0), 0);

        // Single element
        assert_eq!(percentile(&[100], 50.0), 100);

        // 10 elements (indices 0-9)
        // Formula: idx = ((len-1) * p / 100).round()
        let sorted = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(percentile(&sorted, 0.0), 10);   // idx = 0
        assert_eq!(percentile(&sorted, 10.0), 20);  // idx = round(9 * 0.1) = 1
        assert_eq!(percentile(&sorted, 50.0), 60);  // idx = round(9 * 0.5) = 5
        assert_eq!(percentile(&sorted, 90.0), 90);  // idx = round(9 * 0.9) = 8
        assert_eq!(percentile(&sorted, 100.0), 100); // idx = 9
    }

    #[test]
    fn test_estimate_tokens() {
        // 4 chars = 1 token
        assert_eq!(estimate_tokens(4), 1);
        // 100 chars = 25 tokens
        assert_eq!(estimate_tokens(100), 25);
        // 2048 chars = 512 tokens (the model limit)
        assert_eq!(estimate_tokens(2048), 512);
    }

    #[test]
    fn test_chunk_size_stats_empty_db() {
        let conn = setup_test_db();
        let stats = calculate_chunk_size_stats(&conn).unwrap();
        assert!(stats.is_none());
    }

    #[test]
    fn test_chunk_size_stats_single_chunk() {
        let conn = setup_test_db();

        // Insert a single chunk with 100 chars
        let text = "a".repeat(100);
        conn.execute(
            "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
             VALUES ('test', 'id1', 'doc1', 'hash1', ?1, '2025-01-01')",
            [&text],
        ).unwrap();

        let stats = calculate_chunk_size_stats(&conn).unwrap().unwrap();
        assert_eq!(stats.total_chunks, 1);
        assert_eq!(stats.min_chars, 100);
        assert_eq!(stats.max_chars, 100);
        assert_eq!(stats.median_chars, 100);
        assert_eq!(stats.avg_tokens_est, 25); // 100/4
        assert_eq!(stats.chunks_over_limit, 0);
        assert_eq!(stats.chunks_very_small, 0);
    }

    #[test]
    fn test_chunk_size_stats_detects_problems() {
        let conn = setup_test_db();

        // Insert a very small chunk (10 chars < 50)
        conn.execute(
            "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
             VALUES ('test', 'small', 'doc1', 'h1', 'tiny text!', '2025-01-01')",
            [],
        ).unwrap();

        // Insert a chunk over the limit (512 tokens * 4 chars = 2048 chars)
        let big_text = "x".repeat(3000); // >2048 chars
        conn.execute(
            "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
             VALUES ('test', 'big', 'doc1', 'h2', ?1, '2025-01-01')",
            [&big_text],
        ).unwrap();

        // Insert a normal chunk
        let normal_text = "y".repeat(500);
        conn.execute(
            "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
             VALUES ('test', 'normal', 'doc1', 'h3', ?1, '2025-01-01')",
            [&normal_text],
        ).unwrap();

        let stats = calculate_chunk_size_stats(&conn).unwrap().unwrap();
        assert_eq!(stats.total_chunks, 3);
        assert_eq!(stats.chunks_very_small, 1);
        assert_eq!(stats.chunks_over_limit, 1);
    }

    #[test]
    fn test_chunk_size_stats_percentiles() {
        let conn = setup_test_db();

        // Insert chunks with lengths: 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000
        for i in 1..=10 {
            let text = "x".repeat(i * 100);
            conn.execute(
                "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
                 VALUES ('test', ?1, 'doc1', ?2, ?3, '2025-01-01')",
                rusqlite::params![format!("id{}", i), format!("hash{}", i), text],
            ).unwrap();
        }

        let stats = calculate_chunk_size_stats(&conn).unwrap().unwrap();
        assert_eq!(stats.total_chunks, 10);
        assert_eq!(stats.min_chars, 100);
        assert_eq!(stats.max_chars, 1000);
        // p10 should be near the low end, p90 near high end
        assert!(stats.p10_chars <= 200);
        assert!(stats.p90_chars >= 800);
    }

    #[test]
    fn test_embedding_status_includes_max_length() {
        let conn = setup_test_db();
        insert_utterances(&conn, "doc1", &["some content that is long enough"]);

        let embedder = MockEmbedder::default();
        ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE).unwrap();

        let status = get_embedding_status(&conn).unwrap();
        assert_eq!(status.max_length, Some(512));
        assert!(!status.legacy_max_length_warning);
    }

    #[test]
    fn test_embedding_status_legacy_warning_when_max_length_missing() {
        let conn = setup_test_db();
        insert_utterances(&conn, "doc1", &["some content that is long enough"]);

        // Manually set model metadata without max_length (simulating legacy)
        conn.execute(
            "INSERT OR REPLACE INTO embedding_metadata (key, value) VALUES ('model_name', 'old-model')",
            [],
        )
        .unwrap();

        let status = get_embedding_status(&conn).unwrap();
        assert_eq!(status.max_length, None);
        // Model exists but no max_length -> warning
        assert!(status.legacy_max_length_warning);
    }

    #[test]
    fn test_embedding_status_no_warning_when_no_model() {
        let conn = setup_test_db();
        // No embeddings at all - no model name, no max_length
        let status = get_embedding_status(&conn).unwrap();
        assert_eq!(status.max_length, None);
        // No model means no warning (nothing to warn about)
        assert!(!status.legacy_max_length_warning);
    }
}
