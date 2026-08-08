pub mod chunk;
pub mod chunker;
pub mod config;
pub mod headers;
pub mod model;
pub mod plan;
pub mod progress;
pub mod rerank;
pub mod search;
pub mod store;

pub use plan::EmbeddingPlan;

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
    /// Warning: embeddings were created by a different model and must be
    /// fully rebuilt.
    pub model_changed_warning: bool,
    /// Warning: the resolved spec differs from the chunking scheme the
    /// database's embeddings were built with (re-embedding will migrate
    /// them to the new scheme).
    pub chunking_changed_warning: bool,
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
    let chunks_over_limit = lengths
        .iter()
        .filter(|&&len| len > max_chars_for_limit)
        .count();

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
/// `current_model` identifies the embedder that will be used; stored embeddings
/// from a different model are unusable and reported as pending.
/// `spec` is the resolved embedding spec the caller intends to embed with.
pub fn get_embedding_status(
    conn: &Connection,
    current_model: &str,
    spec: &config::EmbedSpec,
) -> Result<EmbeddingStatus> {
    EmbeddingPlan::compute(conn, spec.clone())?.status(conn, current_model)
}

/// True when a chunk's desired metadata no longer matches what is stored.
/// Compared as parsed JSON so key order and formatting don't count;
/// unparseable stored metadata counts as drifted so a refresh repairs it.
fn chunk_metadata_differs(stored: Option<&str>, desired: Option<&serde_json::Value>) -> bool {
    let stored_value = stored.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    match (&stored_value, desired) {
        (None, None) => false,
        (Some(s), Some(d)) => s != d,
        _ => true,
    }
}

/// Wipe all embeddings from the database.
/// Used by `grans embed --force` to force re-embedding.
pub fn wipe_all_embeddings(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM embeddings; DELETE FROM chunks; DELETE FROM embedding_metadata;",
    )?;
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
    pub fn search(
        &self,
        query_vec: &[f32],
        min_score: f32,
        source_type_filter: Option<&[&str]>,
    ) -> Vec<SemanticSearchResult> {
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
/// `spec` is the resolved embedding spec; callers on the search/benchmark
/// path resolve it from stored metadata (never silently migrating a
/// variant-embedded database), `grans embed` may apply explicit overrides.
pub fn ensure_embeddings(
    conn: &Connection,
    embedder: &dyn Embedder,
    batch_size: usize,
    spec: &config::EmbedSpec,
) -> Result<EmbeddingIndex> {
    let plan = EmbeddingPlan::compute(conn, spec.clone())?;
    ensure_embeddings_with_plan(conn, embedder, batch_size, plan)
}

/// Like [`ensure_embeddings`], but consuming an already-computed
/// [`EmbeddingPlan`]. Callers that also need an [`EmbeddingStatus`] (the
/// search prompt) compute one plan and feed both, instead of paying for
/// the chunk rebuild twice.
pub fn ensure_embeddings_with_plan(
    conn: &Connection,
    embedder: &dyn Embedder,
    batch_size: usize,
    plan: EmbeddingPlan,
) -> Result<EmbeddingIndex> {
    // Check model consistency — if model changed, all embeddings are wiped
    let model_consistent = store::check_model_consistency(conn, embedder.model_name())?;

    let EmbeddingPlan {
        spec,
        desired_chunks,
        stored,
    } = plan;

    // The plan's stored snapshot predates the wipe a model change just
    // triggered; after the wipe nothing is stored, so everything desired
    // must be embedded.
    let stored = if model_consistent { stored } else { Vec::new() };

    if desired_chunks.is_empty() {
        if !model_consistent {
            store::set_model_metadata(
                conn,
                embedder.model_name(),
                embedder.dimension(),
                embedder.max_length(),
            )?;
        }
        eprintln!("[grans] No embeddable content found.");
        return Ok(EmbeddingIndex {
            vectors: Vec::new(),
            stats: None,
        });
    }

    let stored_map: HashMap<(&str, &str), &store::StoredChunk> = stored
        .iter()
        .map(|s| ((s.source_type.as_str(), s.source_id.as_str()), s))
        .collect();

    // Diff: find new/changed chunks, metadata drift, and orphaned chunks
    let mut to_embed: Vec<&Chunk> = Vec::new();
    let mut to_refresh: Vec<(i64, Option<String>)> = Vec::new();
    let mut desired_keys: HashSet<(String, String)> = HashSet::new();

    for chunk in &desired_chunks {
        let key = (chunk.source_type.as_str(), chunk.source_id.as_str());
        desired_keys.insert((chunk.source_type.to_string(), chunk.source_id.clone()));

        match stored_map.get(&key) {
            Some(existing) if existing.content_hash == chunk.content_hash => {
                // Unchanged embed input. Metadata can still drift (e.g.
                // speaker attribution arriving after the chunk was
                // embedded); carry it into the store without re-embedding.
                if chunk_metadata_differs(
                    existing.metadata_json.as_deref(),
                    chunk.metadata.as_ref(),
                ) {
                    let serialized = chunk
                        .metadata
                        .as_ref()
                        .map(|m| serde_json::to_string(m).unwrap_or_default());
                    to_refresh.push((existing.id, serialized));
                }
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

    // Refresh drifted metadata on chunks whose embed input is unchanged
    if !to_refresh.is_empty() {
        eprintln!(
            "[grans] Refreshing metadata for {} unchanged chunks.",
            to_refresh.len()
        );
        store::update_chunk_metadata_batch(conn, &to_refresh)?;
    }

    // Embed new/changed chunks
    let stats = if !to_embed.is_empty() {
        let pb = progress::embedding_progress_bar(to_embed.len() as u64);
        let start = Instant::now();

        for batch in to_embed.chunks(batch_size) {
            let inputs: Vec<std::borrow::Cow<'_, str>> =
                batch.iter().map(|c| c.embed_input()).collect();
            let texts: Vec<&str> = inputs.iter().map(|i| i.as_ref()).collect();

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

    // Store model metadata and the chunking scheme the chunks now reflect
    store::set_model_metadata(
        conn,
        embedder.model_name(),
        embedder.dimension(),
        embedder.max_length(),
    )?;
    store::set_chunking_metadata(conn, &spec)?;

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
    let deleted_filter = if include_deleted {
        ""
    } else {
        " AND deleted_at IS NULL"
    };
    let sql = format!(
        "SELECT id, created_at FROM documents WHERE id IN ({}){}",
        placeholders, deleted_filter
    );

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = doc_ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect();

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

/// Run a semantic search against an already-loaded embedder and index.
/// Callers that issue many queries (hybrid search, benchmarks) load the
/// model and index once and reuse them here.
#[allow(clippy::too_many_arguments)]
pub fn semantic_search_with_index(
    conn: &Connection,
    embedder: &dyn Embedder,
    index: &EmbeddingIndex,
    query: &str,
    date_range: Option<&crate::query::dates::DateRange>,
    limit: usize,
    source_type_filter: Option<&[&str]>,
    include_deleted: bool,
) -> Result<(Vec<SemanticSearchResult>, usize)> {
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
    let spec = config::EmbedSpec::resolve_stored(conn, embedder.max_length());
    let index = ensure_embeddings(conn, embedder, DEFAULT_BATCH_SIZE, &spec)?;
    semantic_search_with_index(conn, embedder, &index, query, None, limit, None, false)
}

#[cfg(test)]
mod tests {
    use super::model::MockEmbedder;
    use super::*;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn insert_utterances(conn: &Connection, doc_id: &str, texts: &[&str]) {
        // First ensure the document exists (for foreign key constraint)
        conn.execute(
            "INSERT OR IGNORE INTO documents (id, title, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                doc_id,
                format!("Test Doc {}", doc_id),
                "2025-01-01T00:00:00Z"
            ],
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

    /// A MockEmbedder that reports a different model name, for exercising
    /// model-change paths.
    struct RenamedEmbedder {
        inner: MockEmbedder,
        name: &'static str,
    }

    impl Embedder for RenamedEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            self.inner.embed_batch(texts)
        }
        fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            self.inner.embed_query(text)
        }
        fn dimension(&self) -> usize {
            self.inner.dimension()
        }
        fn model_name(&self) -> &str {
            self.name
        }
        fn max_length(&self) -> usize {
            self.inner.max_length()
        }
    }

    #[test]
    fn embedding_plan_status_reports_pending_and_embedded() {
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is a longer utterance that contains enough characters to meet the minimum chunk size requirement for embedding.",
            ],
        );
        let embedder = MockEmbedder::default();
        let spec = config::EmbedSpec::default_for(512);
        ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &spec).unwrap();

        insert_utterances(
            &conn,
            "doc2",
            &[
                "A second document whose content is also long enough to clear the minimum chunk size threshold for embedding.",
            ],
        );

        let plan = EmbeddingPlan::compute(&conn, spec).unwrap();
        let status = plan.status(&conn, "mock-embedder").unwrap();
        assert_eq!(status.total_chunks, 2);
        assert_eq!(status.embedded_chunks, 1);
        assert_eq!(status.pending_chunks, 1);
        assert_eq!(status.orphaned_chunks, 0);
        assert!(!status.model_changed_warning);
    }

    #[test]
    fn ensure_embeddings_with_plan_embeds_pending_chunks() {
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is a longer utterance that contains enough characters to meet the minimum chunk size requirement for embedding.",
            ],
        );
        let embedder = MockEmbedder::default();
        let spec = config::EmbedSpec::default_for(512);

        let plan = EmbeddingPlan::compute(&conn, spec.clone()).unwrap();
        let index =
            ensure_embeddings_with_plan(&conn, &embedder, DEFAULT_BATCH_SIZE, plan).unwrap();
        assert_eq!(index.vectors.len(), 1);

        let status = get_embedding_status(&conn, "mock-embedder", &spec).unwrap();
        assert_eq!(status.pending_chunks, 0);
    }

    #[test]
    fn ensure_embeddings_with_plan_model_change_reembeds_all() {
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is a longer utterance that contains enough characters to meet the minimum chunk size requirement for embedding.",
            ],
        );
        let embedder = MockEmbedder::default();
        let spec = config::EmbedSpec::default_for(512);
        ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &spec).unwrap();

        // The plan's stored snapshot predates the wipe that a model change
        // triggers inside ensure; it must not suppress re-embedding.
        let plan = EmbeddingPlan::compute(&conn, spec.clone()).unwrap();
        let other = RenamedEmbedder {
            inner: MockEmbedder::default(),
            name: "other-model",
        };
        let index = ensure_embeddings_with_plan(&conn, &other, DEFAULT_BATCH_SIZE, plan).unwrap();
        assert_eq!(index.vectors.len(), 1);

        let status = get_embedding_status(&conn, "other-model", &spec).unwrap();
        assert_eq!(status.pending_chunks, 0);
        assert!(!status.model_changed_warning);
    }

    #[test]
    fn test_ensure_embeddings_empty_db() {
        let conn = setup_test_db();
        let embedder = MockEmbedder::default();

        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn test_ensure_embeddings_creates_vectors() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold (50 chars)
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is a longer utterance that contains enough characters to meet the minimum chunk size requirement for embedding.",
            ],
        );

        let embedder = MockEmbedder::default();

        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert!(!index.is_empty());
    }

    #[test]
    fn test_ensure_embeddings_incremental() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold
        insert_utterances(
            &conn,
            "doc1",
            &[
                "First document content that is long enough to pass the minimum character threshold for embedding chunks.",
            ],
        );

        let embedder = MockEmbedder::default();

        // First run
        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        let count1 = index.vectors.len();
        assert!(count1 > 0);

        // Second run with same data — should not re-embed
        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
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

        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert!(index.vectors.len() > count1);
    }

    #[test]
    fn test_semantic_search_with_embedder() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold
        insert_utterances(
            &conn,
            "doc1",
            &[
                "We had a detailed deployment strategy discussion today about how to deploy the application to production servers efficiently.",
            ],
        );
        insert_utterances(
            &conn,
            "doc2",
            &[
                "We discussed lunch plans for tomorrow and various options for what we should eat together as a team.",
            ],
        );

        let embedder = MockEmbedder {
            dim: 8,
            max_length: 512,
        };

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
                &[&format!(
                    "This is document {} with content about topic {} that is long enough to meet the minimum character threshold for embedding.",
                    i, i
                )],
            );
        }

        let embedder = MockEmbedder {
            dim: 8,
            max_length: 512,
        };

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
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is document content that is long enough to meet the minimum character threshold for the embedding chunker.",
            ],
        );

        let embedder = MockEmbedder {
            dim: 4,
            ..Default::default()
        };

        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        let stats = index
            .stats
            .expect("stats should be present after embedding");
        assert!(stats.chunks_embedded > 0);
        assert!(stats.elapsed_secs >= 0.0);
        assert!(stats.chunks_per_sec >= 0.0);
    }

    #[test]
    fn test_ensure_embeddings_no_stats_when_already_embedded() {
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is document content that is long enough to meet the minimum character threshold for the embedding chunker.",
            ],
        );

        let embedder = MockEmbedder {
            dim: 4,
            ..Default::default()
        };

        // First run embeds everything
        ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();

        // Second run — nothing to embed
        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert!(
            index.stats.is_none(),
            "stats should be None when no new chunks embedded"
        );
    }

    #[test]
    fn test_ensure_embeddings_no_stats_when_empty_db() {
        let conn = setup_test_db();
        let embedder = MockEmbedder {
            dim: 4,
            ..Default::default()
        };

        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert!(
            index.stats.is_none(),
            "stats should be None when no content exists"
        );
    }

    #[test]
    fn test_orphan_cleanup() {
        let conn = setup_test_db();
        // Use content long enough to pass min_chars threshold
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is document content that is long enough to meet the minimum character threshold for the embedding chunker.",
            ],
        );

        let embedder = MockEmbedder::default();

        // First run creates embeddings for doc1
        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert_eq!(index.vectors.len(), 1);

        // Remove doc1's utterances
        conn.execute("DELETE FROM transcript_utterances", [])
            .unwrap();

        // Re-run — should clean up orphan
        let index = ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn test_chunk_metadata_differs() {
        let desired = serde_json::json!({"speakers": ["Jane Doe"]});

        assert!(!chunk_metadata_differs(None, None));
        assert!(chunk_metadata_differs(None, Some(&desired)));
        assert!(chunk_metadata_differs(Some(r#"{"speakers": []}"#), None));
        assert!(chunk_metadata_differs(
            Some(r#"{"speakers": []}"#),
            Some(&desired)
        ));
        // Key order and whitespace don't count as drift.
        assert!(!chunk_metadata_differs(
            Some(r#"{ "speakers" : [ "Jane Doe" ] }"#),
            Some(&desired)
        ));
        // Unparseable stored metadata counts as drifted so it gets repaired.
        assert!(chunk_metadata_differs(Some("not json"), Some(&desired)));
    }

    #[test]
    fn test_metadata_refresh_without_reembed() {
        // The #110 invariant end to end: speaker names arriving after a
        // corpus is embedded must flow into stored chunk metadata without
        // a single chunk re-embedding.
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "This is document content that is long enough to meet the minimum character threshold for the embedding chunker.",
            ],
        );

        let embedder = MockEmbedder::default();
        let spec = config::EmbedSpec::default_for(512);

        // First run embeds the corpus; nothing is attributed yet.
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &spec).unwrap();
        assert!(index.stats.is_some());
        let meta: serde_json::Value =
            serde_json::from_str(index.vectors[0].metadata_json.as_ref().unwrap()).unwrap();
        assert_eq!(meta["speakers"], serde_json::json!([]));

        // Attribution arrives (post-cutover sync).
        conn.execute(
            "UPDATE transcript_utterances SET source = 'system', speaker_name = 'Jane Doe'",
            [],
        )
        .unwrap();
        // Setting source changes the embed input ([Other] prefix), which
        // WOULD re-embed; this test isolates the metadata path, so keep
        // the input identical by only setting speaker_name.
        conn.execute("UPDATE transcript_utterances SET source = NULL", [])
            .unwrap();

        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &spec).unwrap();
        assert!(index.stats.is_none(), "metadata refresh must not re-embed");
        let meta: serde_json::Value =
            serde_json::from_str(index.vectors[0].metadata_json.as_ref().unwrap()).unwrap();
        assert_eq!(meta["speakers"], serde_json::json!(["Jane Doe"]));
    }

    #[test]
    fn test_variant_spec_survives_search_path_resolution() {
        // The core Phase 4 guarantee: a database embedded with a variant
        // scheme (different chunking + headers) is NOT silently re-embedded
        // back to defaults when a search/benchmark path opens it, because
        // those paths resolve the spec from stored metadata.
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "First utterance about the deployment strategy with enough content to chunk.",
                "Second utterance continuing the discussion with plenty of additional words here.",
            ],
        );

        let embedder = MockEmbedder::default();
        let variant = config::EmbedSpec {
            chunking: chunker::ChunkingConfig {
                target_tokens: 100,
                overlap_tokens: 20,
                overlap_mode: chunker::OverlapMode::Utterances,
                ..chunker::ChunkingConfig::from_max_length(512)
            },
            contextual_headers: true,
        };

        // Variant embed (what `grans embed --flags` will do).
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &variant).unwrap();
        assert!(index.stats.is_some());
        let variant_count = index.vectors.len();

        // Search-path resolution under different binary defaults.
        let resolved = config::EmbedSpec::resolve_stored(&conn, 512);
        assert_eq!(resolved.persisted_fields(), variant.persisted_fields());

        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &resolved).unwrap();
        assert!(index.stats.is_none(), "variant db must not be re-embedded");
        assert_eq!(index.vectors.len(), variant_count);

        // And status agrees: nothing pending, no scheme-change warning.
        let status = get_embedding_status(&conn, embedder.model_name(), &resolved).unwrap();
        assert_eq!(status.pending_chunks, 0);
        assert!(!status.chunking_changed_warning);

        // A default-spec embed WOULD be a scheme change, and status says so.
        let default_spec = config::EmbedSpec::default_for(512);
        let status = get_embedding_status(&conn, embedder.model_name(), &default_spec).unwrap();
        assert!(status.chunking_changed_warning);
        assert!(status.pending_chunks > 0);
    }

    #[test]
    fn test_contextual_headers_flow_into_embed_input() {
        // With headers on, the text sent to the embedder carries the
        // meeting context while the stored chunk text stays raw.
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &["A single utterance long enough to form a chunk for the header test."],
        );

        let embedder = MockEmbedder::default();
        let spec = config::EmbedSpec {
            contextual_headers: true,
            ..config::EmbedSpec::default_for(512)
        };
        ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &spec).unwrap();

        let stored_text: String = conn
            .query_row("SELECT text FROM chunks LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert!(!stored_text.contains("Meeting:"));

        // Re-running with headers off is a real re-embed: hashes differ.
        let spec_off = config::EmbedSpec::default_for(512);
        let index = ensure_embeddings(&conn, &embedder, DEFAULT_BATCH_SIZE, &spec_off).unwrap();
        assert!(index.stats.is_some(), "toggling headers must re-embed");
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
        assert_eq!(percentile(&sorted, 0.0), 10); // idx = 0
        assert_eq!(percentile(&sorted, 10.0), 20); // idx = round(9 * 0.1) = 1
        assert_eq!(percentile(&sorted, 50.0), 60); // idx = round(9 * 0.5) = 5
        assert_eq!(percentile(&sorted, 90.0), 90); // idx = round(9 * 0.9) = 8
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
        insert_utterances(
            &conn,
            "doc1",
            &[
                "Hello world, this is a test with enough content to meet minimum chunk size requirements.",
            ],
        );

        let embedder = MockEmbedder::default();
        ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();

        let status = get_embedding_status(
            &conn,
            embedder.model_name(),
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
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

        let status =
            get_embedding_status(&conn, "mock-embedder", &config::EmbedSpec::default_for(512))
                .unwrap();
        assert_eq!(status.max_length, None);
        // Model exists but no max_length -> warning
        assert!(status.legacy_max_length_warning);
    }

    #[test]
    fn test_embedding_status_no_warning_when_no_model() {
        let conn = setup_test_db();
        // No embeddings at all - no model name, no max_length
        let status =
            get_embedding_status(&conn, "mock-embedder", &config::EmbedSpec::default_for(512))
                .unwrap();
        assert_eq!(status.max_length, None);
        // No model means no warning (nothing to warn about)
        assert!(!status.legacy_max_length_warning);
        // No model also means no mismatch to warn about
        assert!(!status.model_changed_warning);
    }

    #[test]
    fn test_embedding_status_matching_model_no_rebuild() {
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "Hello world, this is a test with enough content to meet minimum chunk size requirements.",
            ],
        );

        let embedder = MockEmbedder::default();
        ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();

        let status = get_embedding_status(
            &conn,
            embedder.model_name(),
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        assert!(!status.model_changed_warning);
        assert_eq!(status.pending_chunks, 0);
    }

    #[test]
    fn test_embedding_status_model_change_marks_all_pending() {
        let conn = setup_test_db();
        insert_utterances(
            &conn,
            "doc1",
            &[
                "Hello world, this is a test with enough content to meet minimum chunk size requirements.",
            ],
        );

        let embedder = MockEmbedder::default();
        ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();

        let status =
            get_embedding_status(&conn, "other-model", &config::EmbedSpec::default_for(512))
                .unwrap();
        assert!(status.model_changed_warning);
        assert_eq!(status.pending_chunks, status.total_chunks);
        assert_eq!(status.embedded_chunks, 0);
        assert_eq!(status.pending_by_type.total(), status.total_chunks);
    }
}
