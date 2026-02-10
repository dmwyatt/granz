use anyhow::Result;
use rusqlite::Connection;

use super::chunk::{hash_content, Chunk, ChunkSourceType};

/// Configuration for adaptive token-based chunking.
#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    /// Target number of tokens per chunk (soft limit).
    pub target_tokens: usize,
    /// Maximum number of tokens per chunk (hard limit).
    pub max_tokens: usize,
    /// Number of tokens to overlap between consecutive chunks.
    pub overlap_tokens: usize,
    /// Minimum character count for a chunk to be kept.
    pub min_chars: usize,
    /// Approximate characters per token for the model.
    pub chars_per_token: f64,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        // Use from_max_length to ensure consistency between
        // get_embedding_status() and ensure_embeddings()
        Self::from_max_length(512)
    }
}

impl ChunkingConfig {
    /// Create a config based on model's max_length.
    /// Uses ratios: target = 68% of max, overlap = 20% of max.
    pub fn from_max_length(max_length: usize) -> Self {
        Self {
            target_tokens: (max_length as f64 * 0.68) as usize,
            max_tokens: max_length,
            overlap_tokens: (max_length as f64 * 0.20) as usize,
            min_chars: 50,
            chars_per_token: 4.0,
        }
    }

    /// Target chunk size in characters.
    pub fn target_chars(&self) -> usize {
        (self.target_tokens as f64 * self.chars_per_token) as usize
    }

    /// Maximum chunk size in characters.
    pub fn max_chars(&self) -> usize {
        (self.max_tokens as f64 * self.chars_per_token) as usize
    }

    /// Overlap size in characters.
    pub fn overlap_chars(&self) -> usize {
        (self.overlap_tokens as f64 * self.chars_per_token) as usize
    }
}

/// Generate transcript window chunks using adaptive token-based chunking.
/// This normalizes chunk sizes to be within the model's token limits.
pub fn transcript_window_chunker_adaptive(
    index_conn: &Connection,
    config: &ChunkingConfig,
) -> Result<Vec<Chunk>> {
    let mut stmt = index_conn.prepare(
        "SELECT document_id, id, text, start_timestamp, end_timestamp, source
         FROM transcript_utterances
         ORDER BY document_id, start_timestamp, rowid",
    )?;

    struct Utterance {
        document_id: String,
        text: String,
        start_timestamp: Option<String>,
        end_timestamp: Option<String>,
        source: Option<String>,
    }

    let rows = stmt.query_map([], |row| {
        Ok(Utterance {
            document_id: row.get(0)?,
            text: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            start_timestamp: row.get(3)?,
            end_timestamp: row.get(4)?,
            source: row.get(5)?,
        })
    })?;

    // Group by document_id
    let mut docs: std::collections::HashMap<String, Vec<Utterance>> =
        std::collections::HashMap::new();
    for row in rows {
        let utt = row?;
        docs.entry(utt.document_id.clone()).or_default().push(utt);
    }

    let mut chunks = Vec::new();
    let target_chars = config.target_chars();
    let max_chars = config.max_chars();
    let overlap_chars = config.overlap_chars();

    for (doc_id, utterances) in &docs {
        if utterances.is_empty() {
            continue;
        }

        let mut buffer = String::new();
        let mut buffer_start_idx = 0;
        let mut buffer_end_idx = 0;
        let mut buffer_start_ts: Option<&str> = None;
        let mut buffer_end_ts: Option<&str> = None;
        let mut carryover = String::new();
        let mut chunk_idx = 0;

        for (i, utt) in utterances.iter().enumerate() {
            // Format utterance with speaker label
            let formatted_text = format_utterance_text(&utt.text, utt.source.as_deref());

            // Combine carryover with current utterance
            let text_to_add = if carryover.is_empty() {
                formatted_text
            } else {
                let combined = format!("{}\n{}", carryover, formatted_text);
                carryover.clear();
                combined
            };

            if text_to_add.trim().is_empty() {
                continue;
            }

            let combined_len = if buffer.is_empty() {
                text_to_add.len()
            } else {
                buffer.len() + 1 + text_to_add.len() // +1 for newline
            };

            // Check if adding this would exceed max
            if combined_len > max_chars && !buffer.is_empty() {
                // Finalize current buffer as a chunk
                if buffer.len() >= config.min_chars {
                    chunks.push(Chunk {
                        source_type: ChunkSourceType::TranscriptWindow,
                        source_id: format!("{}:c{}", doc_id, chunk_idx),
                        document_id: doc_id.clone(),
                        text: buffer.clone(),
                        content_hash: hash_content(&buffer),
                        metadata: Some(serde_json::json!({
                            "window_start_idx": buffer_start_idx,
                            "window_end_idx": buffer_end_idx,
                            "start_timestamp": buffer_start_ts,
                            "end_timestamp": buffer_end_ts,
                        })),
                    });
                    chunk_idx += 1;
                }

                // Start new buffer with overlap
                let overlap_start = buffer.len().saturating_sub(overlap_chars);
                buffer = buffer[overlap_start..].to_string();
                buffer_start_idx = i;
            }

            // Handle text_to_add that might be too large by itself
            let mut remaining = text_to_add;
            while remaining.len() > max_chars {
                // Split the oversized text
                let (fits, rest) = split_text_at_limit(&remaining, max_chars);

                if buffer.is_empty() {
                    buffer = fits.to_string();
                    buffer_start_idx = i;
                } else {
                    buffer.push('\n');
                    buffer.push_str(fits);
                }
                buffer_end_idx = i;
                buffer_start_ts = buffer_start_ts.or(utt.start_timestamp.as_deref());
                buffer_end_ts = utt.end_timestamp.as_deref();

                // Finalize this chunk
                if buffer.len() >= config.min_chars {
                    chunks.push(Chunk {
                        source_type: ChunkSourceType::TranscriptWindow,
                        source_id: format!("{}:c{}", doc_id, chunk_idx),
                        document_id: doc_id.clone(),
                        text: buffer.clone(),
                        content_hash: hash_content(&buffer),
                        metadata: Some(serde_json::json!({
                            "window_start_idx": buffer_start_idx,
                            "window_end_idx": buffer_end_idx,
                            "start_timestamp": buffer_start_ts,
                            "end_timestamp": buffer_end_ts,
                        })),
                    });
                    chunk_idx += 1;
                }

                // Start fresh buffer with overlap
                let overlap_start = buffer.len().saturating_sub(overlap_chars);
                buffer = buffer[overlap_start..].to_string();
                buffer_start_idx = i;
                buffer_start_ts = None;
                remaining = rest.to_string();
            }

            // Add remaining text to buffer
            if !remaining.is_empty() {
                let new_combined_len = if buffer.is_empty() {
                    remaining.len()
                } else {
                    buffer.len() + 1 + remaining.len()
                };

                // Check if adding would exceed target (but not max)
                if new_combined_len > target_chars && !buffer.is_empty() {
                    // Finalize current buffer
                    if buffer.len() >= config.min_chars {
                        chunks.push(Chunk {
                            source_type: ChunkSourceType::TranscriptWindow,
                            source_id: format!("{}:c{}", doc_id, chunk_idx),
                            document_id: doc_id.clone(),
                            text: buffer.clone(),
                            content_hash: hash_content(&buffer),
                            metadata: Some(serde_json::json!({
                                "window_start_idx": buffer_start_idx,
                                "window_end_idx": buffer_end_idx,
                                "start_timestamp": buffer_start_ts,
                                "end_timestamp": buffer_end_ts,
                            })),
                        });
                        chunk_idx += 1;
                    }

                    // Start new buffer with overlap
                    let overlap_start = buffer.len().saturating_sub(overlap_chars);
                    buffer = buffer[overlap_start..].to_string();
                    buffer_start_idx = i;
                    buffer_start_ts = None;
                }

                // Add to buffer
                if buffer.is_empty() {
                    buffer = remaining;
                    buffer_start_idx = i;
                    buffer_start_ts = utt.start_timestamp.as_deref();
                } else {
                    buffer.push('\n');
                    buffer.push_str(&remaining);
                }
                buffer_end_idx = i;
                buffer_end_ts = utt.end_timestamp.as_deref();
            }
        }

        // Finalize any remaining buffer + carryover
        if !carryover.is_empty() {
            if !buffer.is_empty() {
                buffer.push('\n');
            }
            buffer.push_str(&carryover);
        }

        if buffer.len() >= config.min_chars {
            chunks.push(Chunk {
                source_type: ChunkSourceType::TranscriptWindow,
                source_id: format!("{}:c{}", doc_id, chunk_idx),
                document_id: doc_id.clone(),
                text: buffer.clone(),
                content_hash: hash_content(&buffer),
                metadata: Some(serde_json::json!({
                    "window_start_idx": buffer_start_idx,
                    "window_end_idx": buffer_end_idx,
                    "start_timestamp": buffer_start_ts,
                    "end_timestamp": buffer_end_ts,
                })),
            });
        }
    }

    Ok(chunks)
}

/// Split text to fit within max_chars, returning (fits, remainder).
/// Strategy: prefer sentence boundaries, fall back to word boundaries.
/// If text <= max_chars, returns (text, "").
fn split_text_at_limit(text: &str, max_chars: usize) -> (&str, &str) {
    if text.len() <= max_chars {
        return (text, "");
    }

    // Find the last sentence boundary (., !, ?) within max_chars
    let search_area = &text[..max_chars];
    let sentence_end = search_area
        .rfind(|c| c == '.' || c == '!' || c == '?')
        .map(|pos| pos + 1); // Include the punctuation

    if let Some(pos) = sentence_end {
        // Check that there's actually content before the split
        if pos > 0 {
            return (&text[..pos], text[pos..].trim_start());
        }
    }

    // Fall back to word boundary (last space)
    if let Some(pos) = search_area.rfind(' ') {
        if pos > 0 {
            return (&text[..pos], text[pos..].trim_start());
        }
    }

    // No good boundary - hard split at max_chars
    (&text[..max_chars], &text[max_chars..])
}

/// Format utterance text with a speaker label prefix based on source.
/// "microphone" = user's voice → `[You]`, "system" = others → `[Other]`.
/// NULL or unknown source gets no prefix (backward compatible with old data).
fn format_utterance_text(text: &str, source: Option<&str>) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    match source {
        Some("microphone") => format!("[You] {}", text),
        Some("system") => format!("[Other] {}", text),
        _ => text.to_string(),
    }
}

use crate::query::text::{split_markdown_sections, strip_panel_footer};

/// Generate chunks from panel markdown sections.
pub fn panel_section_chunker(
    conn: &Connection,
    config: &ChunkingConfig,
) -> Result<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.document_id, p.content_markdown
         FROM panels p
         WHERE p.deleted_at IS NULL
           AND p.content_markdown IS NOT NULL
           AND p.content_markdown != ''",
    )?;

    struct PanelRow {
        id: String,
        document_id: String,
        content_markdown: String,
    }

    let rows = stmt.query_map([], |row| {
        Ok(PanelRow {
            id: row.get(0)?,
            document_id: row.get(1)?,
            content_markdown: row.get(2)?,
        })
    })?;

    let mut chunks = Vec::new();

    for row in rows {
        let panel = row?;
        let stripped = strip_panel_footer(&panel.content_markdown);
        if stripped.is_empty() {
            continue;
        }

        let sections = split_markdown_sections(stripped);

        for (section_idx, (heading, body)) in sections.iter().enumerate() {
            let text = if let Some(h) = heading {
                format!("{}\n\n{}", h, body)
            } else {
                body.to_string()
            };

            if text.len() < config.min_chars {
                continue;
            }

            let content_hash = hash_content(&text);

            chunks.push(Chunk {
                source_type: ChunkSourceType::PanelSection,
                source_id: format!("{}:s{}", panel.id, section_idx),
                document_id: panel.document_id.clone(),
                text,
                content_hash,
                metadata: Some(serde_json::json!({
                    "panel_id": panel.id,
                    "section_heading": heading,
                    "section_idx": section_idx,
                })),
            });
        }
    }

    Ok(chunks)
}

/// Generate chunks from document notes paragraphs.
pub fn notes_paragraph_chunker(
    conn: &Connection,
    min_chars: usize,
) -> Result<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, notes_plain
         FROM documents
         WHERE deleted_at IS NULL
           AND notes_plain IS NOT NULL
           AND notes_plain != ''",
    )?;

    struct DocRow {
        id: String,
        notes_plain: String,
    }

    let rows = stmt.query_map([], |row| {
        Ok(DocRow {
            id: row.get(0)?,
            notes_plain: row.get(1)?,
        })
    })?;

    let mut chunks = Vec::new();

    for row in rows {
        let doc = row?;
        let paragraphs: Vec<&str> = doc
            .notes_plain
            .split("\n\n")
            .map(|p| p.trim())
            .filter(|p| p.len() >= min_chars)
            .collect();

        for (para_idx, para) in paragraphs.iter().enumerate() {
            let text = para.to_string();
            let content_hash = hash_content(&text);

            chunks.push(Chunk {
                source_type: ChunkSourceType::NotesParagraph,
                source_id: format!("{}:n{}", doc.id, para_idx),
                document_id: doc.id.clone(),
                text,
                content_hash,
                metadata: Some(serde_json::json!({
                    "paragraph_idx": para_idx,
                })),
            });
        }
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for split_text_at_limit
    #[test]
    fn test_split_text_fits() {
        let text = "Short text.";
        let (fits, remainder) = split_text_at_limit(text, 100);
        assert_eq!(fits, "Short text.");
        assert_eq!(remainder, "");
    }

    #[test]
    fn test_split_text_at_sentence() {
        let text = "First sentence. Second sentence. Third sentence.";
        let (fits, remainder) = split_text_at_limit(text, 35);
        assert_eq!(fits, "First sentence. Second sentence.");
        assert_eq!(remainder, "Third sentence.");
    }

    #[test]
    fn test_split_text_at_word() {
        // No sentence boundary fits, so fall back to word boundary
        let text = "one two three four five six seven eight nine ten";
        let (fits, remainder) = split_text_at_limit(text, 25);
        assert_eq!(fits, "one two three four five");
        assert_eq!(remainder, "six seven eight nine ten");
    }

    #[test]
    fn test_split_text_hard_split() {
        // No spaces at all - must hard split
        let text = "abcdefghijklmnopqrstuvwxyz";
        let (fits, remainder) = split_text_at_limit(text, 10);
        assert_eq!(fits, "abcdefghij");
        assert_eq!(remainder, "klmnopqrstuvwxyz");
    }

    #[test]
    fn test_split_text_preserves_content() {
        let text = "Hello world. This is a test. More content here.";
        let (fits, remainder) = split_text_at_limit(text, 30);
        // The joined result should be equivalent to original (possibly with trimmed whitespace)
        let reconstructed = format!("{} {}", fits.trim(), remainder.trim());
        // Check that all words are present
        assert!(reconstructed.contains("Hello"));
        assert!(reconstructed.contains("content"));
        assert!(reconstructed.contains("here"));
    }

    #[test]
    fn test_split_text_with_questions() {
        let text = "What is this? It is a test. More text.";
        let (fits, remainder) = split_text_at_limit(text, 20);
        assert_eq!(fits, "What is this?");
        assert_eq!(remainder, "It is a test. More text.");
    }

    #[test]
    fn test_split_text_empty() {
        let (fits, remainder) = split_text_at_limit("", 100);
        assert_eq!(fits, "");
        assert_eq!(remainder, "");
    }

    #[test]
    fn test_split_text_exact_boundary() {
        // Text exactly at the limit
        let text = "Exact.";
        let (fits, remainder) = split_text_at_limit(text, 6);
        assert_eq!(fits, "Exact.");
        assert_eq!(remainder, "");
    }

    // Tests for ChunkingConfig
    #[test]
    fn test_chunking_config_defaults() {
        let config = ChunkingConfig::default();
        // Default uses from_max_length(512) for consistency
        // target = 512 * 0.68 = 348, overlap = 512 * 0.20 = 102
        assert_eq!(config.target_tokens, 348);
        assert_eq!(config.max_tokens, 512);
        assert_eq!(config.overlap_tokens, 102);
        assert_eq!(config.min_chars, 50);
        assert!((config.chars_per_token - 4.0).abs() < 0.001);
    }

    #[test]
    fn test_chunking_config_char_calculations() {
        let config = ChunkingConfig::default();
        // 348 tokens * 4 chars/token = 1392 chars
        assert_eq!(config.target_chars(), 1392);
        // 512 tokens * 4 chars/token = 2048 chars
        assert_eq!(config.max_chars(), 2048);
        // 102 tokens * 4 chars/token = 408 chars
        assert_eq!(config.overlap_chars(), 408);
    }

    #[test]
    fn test_chunking_config_from_max_length() {
        let config = ChunkingConfig::from_max_length(256);
        assert_eq!(config.max_tokens, 256);
        // target should be ~68% of max (0.68 * 256 ≈ 174)
        assert!(config.target_tokens > 150 && config.target_tokens < 200);
        // overlap should be ~20% of max (0.20 * 256 ≈ 51)
        assert!(config.overlap_tokens > 40 && config.overlap_tokens < 70);
    }

    fn setup_test_db(utterances: &[(&str, &str, &str, Option<&str>)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE transcript_utterances (
                id TEXT PRIMARY KEY,
                document_id TEXT NOT NULL,
                start_timestamp TEXT,
                end_timestamp TEXT,
                text TEXT,
                source TEXT
            );",
        )
        .unwrap();

        for (i, (doc_id, timestamp, text, source)) in utterances.iter().enumerate() {
            conn.execute(
                "INSERT INTO transcript_utterances (id, document_id, start_timestamp, end_timestamp, text, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    format!("u-{}", i),
                    doc_id,
                    timestamp,
                    timestamp,
                    text,
                    source,
                ],
            )
            .unwrap();
        }

        conn
    }

    // Tests for adaptive chunker
    #[test]
    fn test_adaptive_empty_db() {
        let conn = setup_test_db(&[]);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_adaptive_single_small_utterance() {
        // A small utterance should become one chunk
        let text = "Hello world, this is a test with enough content to meet minimum chunk size requirements.";
        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", text, None)]);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].document_id, "doc1");
        assert!(chunks[0].text.contains("Hello world"));
    }

    #[test]
    fn test_adaptive_accumulates_small_utterances() {
        // Multiple small utterances should accumulate into one chunk
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "Short one.", None),
            ("doc1", "2025-01-01T10:01:00Z", "Short two.", None),
            ("doc1", "2025-01-01T10:02:00Z", "Short three.", None),
        ];
        let conn = setup_test_db(&utts);
        // Use config with high target so all fit in one chunk
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("Short one"));
        assert!(chunks[0].text.contains("Short three"));
    }

    #[test]
    fn test_adaptive_splits_at_target() {
        // Create utterances that together exceed target but not max
        // Use a config with small target to force splits
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "First utterance with some content.", None),
            ("doc1", "2025-01-01T10:01:00Z", "Second utterance with more content.", None),
            ("doc1", "2025-01-01T10:02:00Z", "Third utterance continues on.", None),
        ];
        let conn = setup_test_db(&utts);
        // Small target: ~20 tokens = ~80 chars
        let config = ChunkingConfig {
            target_tokens: 15,
            max_tokens: 100,
            overlap_tokens: 5,
            min_chars: 10,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();
        // Should have multiple chunks
        assert!(chunks.len() >= 2, "Expected multiple chunks, got {}", chunks.len());
    }

    #[test]
    fn test_adaptive_splits_oversized_utterance() {
        // A single utterance that exceeds max_chars should be split
        let huge_text = "This is a very long sentence that keeps going on and on. ".repeat(20);
        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", &huge_text, None)]);
        // Very small max to force split: 50 tokens * 4 = 200 chars
        let config = ChunkingConfig {
            target_tokens: 30,
            max_tokens: 50,
            overlap_tokens: 10,
            min_chars: 20,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();
        // The huge text should result in multiple chunks
        assert!(chunks.len() > 1, "Expected split chunks, got {}", chunks.len());
        // Each chunk should not exceed max_chars
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= config.max_chars() + 50, // Allow some margin for sentence boundaries
                "Chunk too large: {} chars, max was {}",
                chunk.text.len(),
                config.max_chars()
            );
        }
    }

    #[test]
    fn test_adaptive_very_small_chunks_dropped() {
        // Chunks below min_chars should be dropped
        let conn = setup_test_db(&[
            ("doc1", "2025-01-01T10:00:00Z", "ok", None),  // 2 chars - too small
            ("doc1", "2025-01-01T10:01:00Z", "This is adequate content for a chunk.", None),
        ]);
        let config = ChunkingConfig {
            target_tokens: 100,
            max_tokens: 200,
            overlap_tokens: 10,
            min_chars: 20,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();
        // The "ok" text alone would be too small, but combined they're fine
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.text.len() >= config.min_chars);
        }
    }

    #[test]
    fn test_adaptive_multiple_documents() {
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "Document one content that is long enough to meet the minimum chunk size requirements.", None),
            ("doc2", "2025-01-01T11:00:00Z", "Document two content that is also long enough to meet the minimum chunk size requirements.", None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 2);
        let doc_ids: Vec<&str> = chunks.iter().map(|c| c.document_id.as_str()).collect();
        assert!(doc_ids.contains(&"doc1"));
        assert!(doc_ids.contains(&"doc2"));
    }

    #[test]
    fn test_adaptive_metadata_tracks_indices() {
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "First utterance.", None),
            ("doc1", "2025-01-01T10:01:00Z", "Second utterance.", None),
            ("doc1", "2025-01-01T10:02:00Z", "Third utterance.", None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 1);
        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["window_start_idx"], 0);
        assert_eq!(meta["window_end_idx"], 2);
    }

    #[test]
    fn test_adaptive_metadata_tracks_timestamps() {
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "First utterance with enough content to pass the minimum.", None),
            ("doc1", "2025-01-01T10:05:00Z", "Last utterance with additional content to meet requirements.", None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();

        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["start_timestamp"], "2025-01-01T10:00:00Z");
        assert_eq!(meta["end_timestamp"], "2025-01-01T10:05:00Z");
    }

    #[test]
    fn test_adaptive_source_id_format() {
        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", "Content here that is long enough to meet minimum chunk size requirements for the test.", None)]);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();

        assert!(chunks[0].source_id.starts_with("doc1:"));
    }

    // Tests for panel_section_chunker
    fn setup_panel_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn insert_test_panel(conn: &Connection, panel_id: &str, doc_id: &str, markdown: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO documents (id, title, created_at) VALUES (?1, 'Test', '2025-01-01T00:00:00Z')",
            [doc_id],
        ).unwrap();
        conn.execute(
            "INSERT INTO panels (id, document_id, content_markdown) VALUES (?1, ?2, ?3)",
            rusqlite::params![panel_id, doc_id, markdown],
        ).unwrap();
    }

    #[test]
    fn test_panel_section_chunker_empty_db() {
        let conn = setup_panel_test_db();
        let config = ChunkingConfig::default();
        let chunks = panel_section_chunker(&conn, &config).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_panel_section_chunker_creates_chunks() {
        let conn = setup_panel_test_db();
        let markdown = "### Action Items\n\nWe need to complete the deployment process for the new release version.\n\n### Key Decisions\n\nThe team agreed to postpone the feature release until after testing is complete.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig { min_chars: 20, ..ChunkingConfig::default() };
        let chunks = panel_section_chunker(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].source_type, ChunkSourceType::PanelSection);
        assert!(chunks[0].source_id.starts_with("panel1:s"));
        assert_eq!(chunks[0].document_id, "doc1");
        // Heading included in text for embedding context
        assert!(chunks[0].text.starts_with("Action Items"));
    }

    #[test]
    fn test_panel_section_chunker_strips_footer() {
        let conn = setup_panel_test_db();
        let markdown = "### Action Items\n\nComplete the deployment process for the entire team.\n\n---\nChat with Granola for more details.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig { min_chars: 20, ..ChunkingConfig::default() };
        let chunks = panel_section_chunker(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].text.contains("Chat with"));
    }

    #[test]
    fn test_panel_section_chunker_skips_short_sections() {
        let conn = setup_panel_test_db();
        let markdown = "### Action Items\n\nOk.\n\n### Key Decisions\n\nWe decided to postpone the feature release until after quality testing is complete.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig { min_chars: 50, ..ChunkingConfig::default() };
        let chunks = panel_section_chunker(&conn, &config).unwrap();

        // "Ok." section is too short, only "Key Decisions" should remain
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("Key Decisions"));
    }

    #[test]
    fn test_panel_section_chunker_skips_deleted() {
        let conn = setup_panel_test_db();
        let markdown = "### Action Items\n\nComplete the deployment process for the entire team.";
        conn.execute(
            "INSERT OR IGNORE INTO documents (id, title, created_at) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO panels (id, document_id, content_markdown, deleted_at) VALUES ('panel1', 'doc1', ?1, '2025-01-02T00:00:00Z')",
            [markdown],
        ).unwrap();

        let config = ChunkingConfig { min_chars: 20, ..ChunkingConfig::default() };
        let chunks = panel_section_chunker(&conn, &config).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_panel_section_chunker_metadata() {
        let conn = setup_panel_test_db();
        let markdown = "### Budget Review\n\nThe quarterly budget needs revision for the marketing department.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig { min_chars: 20, ..ChunkingConfig::default() };
        let chunks = panel_section_chunker(&conn, &config).unwrap();

        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["panel_id"], "panel1");
        assert_eq!(meta["section_heading"], "Budget Review");
        assert_eq!(meta["section_idx"], 0);
    }

    // Tests for notes_paragraph_chunker
    #[test]
    fn test_notes_paragraph_chunker_empty_db() {
        let conn = setup_panel_test_db();
        let chunks = notes_paragraph_chunker(&conn, 20).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_notes_paragraph_chunker_creates_chunks() {
        let conn = setup_panel_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', ?1)",
            ["First paragraph with enough content.\n\nSecond paragraph also with enough content."],
        ).unwrap();

        let chunks = notes_paragraph_chunker(&conn, 20).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].source_type, ChunkSourceType::NotesParagraph);
        assert_eq!(chunks[0].source_id, "doc1:n0");
        assert_eq!(chunks[1].source_id, "doc1:n1");
    }

    #[test]
    fn test_notes_paragraph_chunker_skips_short() {
        let conn = setup_panel_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', ?1)",
            ["ok\n\nThis paragraph is long enough to be included in the embedding."],
        ).unwrap();

        let chunks = notes_paragraph_chunker(&conn, 20).unwrap();

        // "ok" is too short
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("long enough"));
    }

    #[test]
    fn test_notes_paragraph_chunker_skips_deleted() {
        let conn = setup_panel_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain, deleted_at) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', 'Some notes that are long enough.', '2025-01-02T00:00:00Z')",
            [],
        ).unwrap();

        let chunks = notes_paragraph_chunker(&conn, 20).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_notes_paragraph_chunker_metadata() {
        let conn = setup_panel_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', ?1)",
            ["A paragraph that is long enough to be embedded."],
        ).unwrap();

        let chunks = notes_paragraph_chunker(&conn, 20).unwrap();

        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["paragraph_idx"], 0);
    }

    #[test]
    fn test_panel_section_chunker_h1_headers() {
        let conn = setup_panel_test_db();
        let markdown = "# Announcements\n\nNew hire starting Monday and onboarding schedule is ready.\n\n# Updates\n\nProject is on track for the quarterly deadline.\n\n# Action Items\n\n- Send welcome email to the new team member";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig { min_chars: 20, ..ChunkingConfig::default() };
        let chunks = panel_section_chunker(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].source_type, ChunkSourceType::PanelSection);

        // Headings should be extracted correctly
        let meta0 = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta0["section_heading"], "Announcements");
        assert!(chunks[0].text.contains("New hire"));

        let meta1 = chunks[1].metadata.as_ref().unwrap();
        assert_eq!(meta1["section_heading"], "Updates");
        assert!(chunks[1].text.contains("on track"));

        let meta2 = chunks[2].metadata.as_ref().unwrap();
        assert_eq!(meta2["section_heading"], "Action Items");
        assert!(chunks[2].text.contains("welcome email"));
    }

    // Tests for format_utterance_text
    #[test]
    fn test_format_utterance_text_microphone() {
        assert_eq!(format_utterance_text("hello", Some("microphone")), "[You] hello");
    }

    #[test]
    fn test_format_utterance_text_system() {
        assert_eq!(format_utterance_text("hello", Some("system")), "[Other] hello");
    }

    #[test]
    fn test_format_utterance_text_none() {
        assert_eq!(format_utterance_text("hello", None), "hello");
    }

    #[test]
    fn test_format_utterance_text_unknown() {
        assert_eq!(format_utterance_text("hello", Some("unknown_source")), "hello");
    }

    #[test]
    fn test_format_utterance_text_empty_with_source() {
        // Empty/whitespace text should remain empty regardless of source
        assert_eq!(format_utterance_text("", Some("microphone")), "");
        assert_eq!(format_utterance_text("   ", Some("microphone")), "");
        assert_eq!(format_utterance_text("", Some("system")), "");
        assert_eq!(format_utterance_text("   ", Some("system")), "");
        assert_eq!(format_utterance_text("", None), "");
        assert_eq!(format_utterance_text("   ", None), "");
    }

    #[test]
    fn test_adaptive_empty_text_with_source_skipped() {
        // Regression: empty text with non-null source must not produce chunks
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "", Some("microphone")),
            ("doc1", "2025-01-01T10:01:00Z", "   ", Some("system")),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();
        assert!(chunks.is_empty(), "Empty text with source should produce no chunks, got {}", chunks.len());
    }

    // Integration tests for speaker labels in chunkers
    #[test]
    fn test_adaptive_speaker_labels_in_chunks() {
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "I think we should proceed with the plan.", Some("microphone")),
            ("doc1", "2025-01-01T10:01:00Z", "That sounds good, let me check the timeline.", Some("system")),
            ("doc1", "2025-01-01T10:02:00Z", "Great, I will send the details after this meeting.", Some("microphone")),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("[You] I think we should proceed"));
        assert!(chunks[0].text.contains("[Other] That sounds good"));
        assert!(chunks[0].text.contains("[You] Great, I will send"));
    }

    #[test]
    fn test_adaptive_no_labels_when_source_null() {
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "First utterance with enough content for chunking.", None),
            ("doc1", "2025-01-01T10:01:00Z", "Second utterance also with enough content here.", None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].text.contains("[You]"));
        assert!(!chunks[0].text.contains("[Other]"));
        assert!(chunks[0].text.contains("First utterance"));
        assert!(chunks[0].text.contains("Second utterance"));
    }

    #[test]
    fn test_adaptive_mixed_sources_and_null() {
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", "Labeled utterance from the user.", Some("microphone")),
            ("doc1", "2025-01-01T10:01:00Z", "Unlabeled utterance with no source.", None),
            ("doc1", "2025-01-01T10:02:00Z", "Labeled utterance from other person.", Some("system")),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("[You] Labeled utterance from the user."));
        assert!(chunks[0].text.contains("Unlabeled utterance with no source."));
        assert!(!chunks[0].text.contains("[You] Unlabeled"));
        assert!(!chunks[0].text.contains("[Other] Unlabeled"));
        assert!(chunks[0].text.contains("[Other] Labeled utterance from other person."));
    }

    #[test]
    fn test_content_hash_changes_with_speaker_label() {
        // Same text but different source → different hash
        let text = "This is a test utterance with enough content to meet minimum chunk size requirements for the test.";
        let utts_mic = vec![
            ("doc1", "2025-01-01T10:00:00Z", text, Some("microphone")),
        ];
        let utts_sys = vec![
            ("doc1", "2025-01-01T10:00:00Z", text, Some("system")),
        ];
        let utts_none = vec![
            ("doc1", "2025-01-01T10:00:00Z", text, None),
        ];
        let config = ChunkingConfig::default();

        let conn_mic = setup_test_db(&utts_mic);
        let chunks_mic = transcript_window_chunker_adaptive(&conn_mic, &config).unwrap();

        let conn_sys = setup_test_db(&utts_sys);
        let chunks_sys = transcript_window_chunker_adaptive(&conn_sys, &config).unwrap();

        let conn_none = setup_test_db(&utts_none);
        let chunks_none = transcript_window_chunker_adaptive(&conn_none, &config).unwrap();

        // All three should produce different hashes
        assert_ne!(chunks_mic[0].content_hash, chunks_sys[0].content_hash);
        assert_ne!(chunks_mic[0].content_hash, chunks_none[0].content_hash);
        assert_ne!(chunks_sys[0].content_hash, chunks_none[0].content_hash);
    }

}
