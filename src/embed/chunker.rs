use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

use super::chunk::{Chunk, ChunkSourceType, hash_embed_input};

/// How consecutive transcript chunks overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapMode {
    /// Carry the trailing overlap-budget characters of the previous chunk.
    Chars,
    /// Carry the trailing whole utterances that fit the overlap budget, so
    /// chunks always start at an utterance boundary.
    Utterances,
}

impl OverlapMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OverlapMode::Chars => "chars",
            OverlapMode::Utterances => "utterances",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "chars" => Some(OverlapMode::Chars),
            "utterances" => Some(OverlapMode::Utterances),
            _ => None,
        }
    }
}

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
    /// How consecutive transcript chunks overlap.
    pub overlap_mode: OverlapMode,
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
            overlap_mode: OverlapMode::Chars,
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

/// A transcript utterance as read by the transcript window chunker.
struct Utterance {
    document_id: String,
    text: String,
    start_timestamp: Option<String>,
    end_timestamp: Option<String>,
    source: Option<String>,
    speaker_name: Option<String>,
}

/// Distinct speaker names for the utterance window `start..=end`, in
/// first-appearance order. Empty-text utterances contributed nothing to
/// the chunk and are skipped. An empty array is the common case: the
/// pre-cutover corpus has no names at all, and the local user
/// (microphone channel) never carries one. The local user gets no
/// sentinel entry either; the `[You]` label in the chunk text and the
/// window indices already identify them, and an invented name would sit
/// in the same namespace speaker filtering matches display names against.
///
/// Overlap carryover text duplicated from the previous chunk sits outside
/// the window, so its speakers are deliberately not listed here, matching
/// how `window_start_idx`/`window_end_idx` already exclude carryover. The
/// chunk that owns those utterances attributes them; listing them twice
/// would double-attribute every speaker near a chunk boundary.
fn window_speakers(utterances: &[Utterance], start: usize, end: usize) -> Vec<String> {
    let mut speakers: Vec<String> = Vec::new();
    for utt in utterances.get(start..=end).unwrap_or(&[]) {
        if utt.text.trim().is_empty() {
            continue;
        }
        if let Some(name) = &utt.speaker_name {
            if !speakers.iter().any(|s| s == name) {
                speakers.push(name.clone());
            }
        }
    }
    speakers
}

/// Metadata for a transcript window chunk covering `start_idx..=end_idx`.
fn transcript_chunk_metadata(
    utterances: &[Utterance],
    start_idx: usize,
    end_idx: usize,
    start_ts: Option<&str>,
    end_ts: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "window_start_idx": start_idx,
        "window_end_idx": end_idx,
        "start_timestamp": start_ts,
        "end_timestamp": end_ts,
        "speakers": window_speakers(utterances, start_idx, end_idx),
    })
}

/// A finalized transcript window chunk for the buffered text.
fn make_transcript_chunk(
    doc_id: &str,
    chunk_idx: usize,
    text: &str,
    header: Option<&String>,
    utterances: &[Utterance],
    start_idx: usize,
    end_idx: usize,
    start_ts: Option<&str>,
    end_ts: Option<&str>,
) -> Chunk {
    Chunk {
        source_type: ChunkSourceType::TranscriptWindow,
        source_id: format!("{}:c{}", doc_id, chunk_idx),
        document_id: doc_id.to_string(),
        text: text.to_string(),
        content_hash: hash_embed_input(header.map(String::as_str), text),
        header: header.cloned(),
        metadata: Some(transcript_chunk_metadata(
            utterances, start_idx, end_idx, start_ts, end_ts,
        )),
    }
}

/// Generate transcript window chunks using adaptive token-based chunking.
/// This normalizes chunk sizes to be within the model's token limits.
/// When `headers` is provided, each document's contextual header is
/// attached to its chunks (embed input only) and the per-document char
/// budgets shrink by the header length so header + chunk stays within the
/// model limit.
pub fn transcript_window_chunker_adaptive(
    index_conn: &Connection,
    config: &ChunkingConfig,
    headers: Option<&HashMap<String, String>>,
) -> Result<Vec<Chunk>> {
    let mut stmt = index_conn.prepare(
        "SELECT document_id, id, text, start_timestamp, end_timestamp, source, speaker_name
         FROM transcript_utterances
         ORDER BY document_id, start_timestamp, rowid",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Utterance {
            document_id: row.get(0)?,
            text: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            start_timestamp: row.get(3)?,
            end_timestamp: row.get(4)?,
            source: row.get(5)?,
            speaker_name: row.get(6)?,
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
    let base_target_chars = config.target_chars();
    let base_max_chars = config.max_chars();
    let overlap_chars = config.overlap_chars();

    for (doc_id, utterances) in &docs {
        if utterances.is_empty() {
            continue;
        }

        let header = headers.and_then(|h| h.get(doc_id.as_str()));
        let header_len = header.map_or(0, |h| h.len());
        let target_chars = base_target_chars.saturating_sub(header_len);
        let max_chars = base_max_chars.saturating_sub(header_len);

        let mut buffer = String::new();
        // Whole utterances currently in the buffer, tracked for
        // utterance-boundary overlap. Oversized-split fragments are not
        // utterances and are deliberately never tracked.
        let mut buffer_utts: Vec<String> = Vec::new();
        // False while the buffer holds only overlap carryover. A buffer
        // is only ever finalized once fresh content lands on top of the
        // carryover; finalizing before that would emit a chunk that is
        // 100% text duplicated from its neighbor (#123).
        let mut buffer_has_new_content = false;
        let mut buffer_start_idx = 0;
        let mut buffer_end_idx = 0;
        let mut buffer_start_ts: Option<&str> = None;
        let mut buffer_end_ts: Option<&str> = None;
        let mut chunk_idx = 0;

        for (i, utt) in utterances.iter().enumerate() {
            // Format utterance with speaker label
            let text_to_add = format_utterance_text(&utt.text, utt.source.as_deref());

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
                    chunks.push(make_transcript_chunk(
                        doc_id,
                        chunk_idx,
                        &buffer,
                        header,
                        utterances,
                        buffer_start_idx,
                        buffer_end_idx,
                        buffer_start_ts,
                        buffer_end_ts,
                    ));
                    chunk_idx += 1;
                }

                // Start new buffer with overlap
                buffer = overlap_carryover(
                    config.overlap_mode,
                    &buffer,
                    &mut buffer_utts,
                    overlap_chars,
                );
                buffer_has_new_content = false;
                buffer_start_idx = i;
                buffer_start_ts = None;
            }

            // Handle text_to_add that might be too large by itself
            let mut remaining = text_to_add;
            while remaining.len() > max_chars {
                // Split the oversized text. The split budget subtracts
                // whatever already sits in the buffer (overlap carryover),
                // so carryover + fragment stays within the hard cap.
                let budget = if buffer.is_empty() {
                    max_chars
                } else {
                    max_chars.saturating_sub(buffer.len() + 1)
                };
                let (fits, rest) = split_text_at_limit(&remaining, budget);

                if buffer.is_empty() {
                    buffer = fits.to_string();
                    buffer_start_idx = i;
                } else {
                    buffer.push('\n');
                    buffer.push_str(fits);
                }
                buffer_has_new_content = true;
                buffer_end_idx = i;
                buffer_start_ts = buffer_start_ts.or(utt.start_timestamp.as_deref());
                buffer_end_ts = utt.end_timestamp.as_deref();

                // Finalize this chunk
                if buffer.len() >= config.min_chars {
                    chunks.push(make_transcript_chunk(
                        doc_id,
                        chunk_idx,
                        &buffer,
                        header,
                        utterances,
                        buffer_start_idx,
                        buffer_end_idx,
                        buffer_start_ts,
                        buffer_end_ts,
                    ));
                    chunk_idx += 1;
                }

                // Start fresh buffer with overlap. The buffer ends in a
                // split fragment here, so utterance mode carries nothing.
                buffer_utts.clear();
                buffer = overlap_carryover(
                    config.overlap_mode,
                    &buffer,
                    &mut buffer_utts,
                    overlap_chars,
                );
                buffer_has_new_content = false;
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

                // Check if adding would exceed target (but not max). A
                // buffer holding only carryover is never finalized: that
                // would duplicate its neighbor's text wholesale (#123).
                if new_combined_len > target_chars && buffer_has_new_content {
                    // Finalize current buffer
                    if buffer.len() >= config.min_chars {
                        chunks.push(make_transcript_chunk(
                            doc_id,
                            chunk_idx,
                            &buffer,
                            header,
                            utterances,
                            buffer_start_idx,
                            buffer_end_idx,
                            buffer_start_ts,
                            buffer_end_ts,
                        ));
                        chunk_idx += 1;
                    }

                    // Start new buffer with overlap
                    buffer = overlap_carryover(
                        config.overlap_mode,
                        &buffer,
                        &mut buffer_utts,
                        overlap_chars,
                    );
                    buffer_has_new_content = false;
                    buffer_start_idx = i;
                    buffer_start_ts = None;
                }

                // If the carried-over text alone would push this utterance
                // past the hard cap, shrink the carryover to fit.
                if !buffer_has_new_content
                    && !buffer.is_empty()
                    && buffer.len() + 1 + remaining.len() > max_chars
                {
                    trim_carryover(
                        config.overlap_mode,
                        &mut buffer,
                        &mut buffer_utts,
                        max_chars.saturating_sub(remaining.len() + 1),
                    );
                }

                // Add to buffer
                buffer_utts.push(remaining.clone());
                if buffer.is_empty() {
                    buffer = remaining;
                    buffer_start_idx = i;
                    buffer_start_ts = utt.start_timestamp.as_deref();
                } else {
                    buffer.push('\n');
                    buffer.push_str(&remaining);
                    // After a reseed the carryover has no timestamp; the
                    // first appended utterance starts the window.
                    buffer_start_ts = buffer_start_ts.or(utt.start_timestamp.as_deref());
                }
                buffer_has_new_content = true;
                buffer_end_idx = i;
                buffer_end_ts = utt.end_timestamp.as_deref();
            }
        }

        // Finalize any remaining buffer
        if buffer.len() >= config.min_chars {
            chunks.push(make_transcript_chunk(
                doc_id,
                chunk_idx,
                &buffer,
                header,
                utterances,
                buffer_start_idx,
                buffer_end_idx,
                buffer_start_ts,
                buffer_end_ts,
            ));
        }
    }

    Ok(chunks)
}

/// Largest byte index <= `max` that lies on a char boundary of `s`.
/// If that index is 0 (i.e. `max` falls inside the first character), the
/// end of the first character is returned instead, so callers slicing at
/// the result always make progress.
pub(crate) fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    if i == 0 {
        i = max;
        while !s.is_char_boundary(i) {
            i += 1;
        }
    }
    i
}

/// Build the carryover that seeds the next chunk after a finalize.
/// Chars mode slices the trailing overlap budget off the old buffer;
/// Utterances mode carries the trailing whole utterances that fit the
/// budget (`buffer_utts` is trimmed to exactly the carried utterances).
fn overlap_carryover(
    mode: OverlapMode,
    buffer: &str,
    buffer_utts: &mut Vec<String>,
    overlap_chars: usize,
) -> String {
    match mode {
        OverlapMode::Chars => {
            buffer_utts.clear();
            let overlap_start =
                floor_char_boundary(buffer, buffer.len().saturating_sub(overlap_chars));
            buffer[overlap_start..].to_string()
        }
        OverlapMode::Utterances => {
            let mut total = 0;
            let mut keep = 0;
            for utt in buffer_utts.iter().rev() {
                let addition = utt.len() + if keep == 0 { 0 } else { 1 }; // +1 newline
                if total + addition > overlap_chars {
                    break;
                }
                total += addition;
                keep += 1;
            }
            buffer_utts.drain(..buffer_utts.len() - keep);
            buffer_utts.join("\n")
        }
    }
}

/// Split `text` into fragments of at most `budget` bytes, preferring
/// sentence boundaries. Fragments are non-empty; the caller applies its
/// own minimum-length policy.
fn split_to_cap(text: &str, budget: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    loop {
        let (fits, remainder) = split_text_at_limit(rest, budget);
        parts.push(fits);
        if remainder.is_empty() {
            break;
        }
        rest = remainder;
    }
    parts
}

/// Shrink an all-carryover buffer to at most `allowed` bytes so that
/// carryover + incoming content stays within the hard cap. Overlap is a
/// soft budget; the model cap is not. Chars mode keeps the trailing
/// characters; Utterances mode drops whole utterances from the front
/// (`buffer_utts` mirrors the survivors) so chunks still start at
/// utterance boundaries.
fn trim_carryover(
    mode: OverlapMode,
    buffer: &mut String,
    buffer_utts: &mut Vec<String>,
    allowed: usize,
) {
    if buffer.len() <= allowed {
        return;
    }
    match mode {
        OverlapMode::Chars => {
            let mut start = buffer.len() - allowed;
            while !buffer.is_char_boundary(start) {
                start += 1;
            }
            *buffer = buffer[start..].to_string();
        }
        OverlapMode::Utterances => {
            while !buffer_utts.is_empty() && joined_len(buffer_utts) > allowed {
                buffer_utts.remove(0);
            }
            *buffer = buffer_utts.join("\n");
        }
    }
}

/// Byte length of `parts` joined with single newlines.
fn joined_len(parts: &[String]) -> usize {
    let text: usize = parts.iter().map(String::len).sum();
    text + parts.len().saturating_sub(1)
}

/// Split text to fit within max_chars, returning (fits, remainder).
/// Strategy: prefer sentence boundaries, fall back to word boundaries.
/// If text <= max_chars, returns (text, "").
fn split_text_at_limit(text: &str, max_chars: usize) -> (&str, &str) {
    if text.len() <= max_chars {
        return (text, "");
    }

    // Find the last sentence boundary (., !, ?) within max_chars.
    // The budget is clamped to at least one character: a zero budget must
    // still yield a non-empty fits, or the oversized-split loop that calls
    // this in a `while remaining.len() > max_chars` never terminates.
    let limit = floor_char_boundary(text, max_chars.max(1));
    let search_area = &text[..limit];
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

    // No good boundary - hard split at the char boundary nearest max_chars
    (&text[..limit], &text[limit..])
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
    headers: Option<&HashMap<String, String>>,
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

            let header = headers.and_then(|h| h.get(panel.document_id.as_str()));
            // Sections longer than the model cap are split; header + text
            // must stay within the cap (#123). The first part keeps the
            // unsuffixed id so within-cap sections keep their identity.
            let budget = config
                .max_chars()
                .saturating_sub(header.map_or(0, |h| h.len()));

            for (part, fragment) in split_to_cap(&text, budget).into_iter().enumerate() {
                if fragment.len() < config.min_chars {
                    continue;
                }
                let source_id = if part == 0 {
                    format!("{}:s{}", panel.id, section_idx)
                } else {
                    format!("{}:s{}p{}", panel.id, section_idx, part)
                };
                chunks.push(Chunk {
                    source_type: ChunkSourceType::PanelSection,
                    source_id,
                    document_id: panel.document_id.clone(),
                    text: fragment.to_string(),
                    content_hash: hash_embed_input(header.map(String::as_str), fragment),
                    header: header.cloned(),
                    metadata: Some(serde_json::json!({
                        "panel_id": panel.id,
                        "section_heading": heading,
                        "section_idx": section_idx,
                    })),
                });
            }
        }
    }

    Ok(chunks)
}

/// Generate chunks from document notes paragraphs.
pub fn notes_paragraph_chunker(
    conn: &Connection,
    config: &ChunkingConfig,
    headers: Option<&HashMap<String, String>>,
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
            .filter(|p| p.len() >= config.min_chars)
            .collect();

        let header = headers.and_then(|h| h.get(doc.id.as_str()));
        // Paragraphs longer than the model cap are split; header + text
        // must stay within the cap (#123). The first part keeps the
        // unsuffixed id so within-cap paragraphs keep their identity.
        let budget = config
            .max_chars()
            .saturating_sub(header.map_or(0, |h| h.len()));
        for (para_idx, para) in paragraphs.into_iter().enumerate() {
            for (part, fragment) in split_to_cap(para, budget).into_iter().enumerate() {
                if fragment.len() < config.min_chars {
                    continue;
                }
                let source_id = if part == 0 {
                    format!("{}:n{}", doc.id, para_idx)
                } else {
                    format!("{}:n{}p{}", doc.id, para_idx, part)
                };
                chunks.push(Chunk {
                    source_type: ChunkSourceType::NotesParagraph,
                    source_id,
                    document_id: doc.id.clone(),
                    text: fragment.to_string(),
                    content_hash: hash_embed_input(header.map(String::as_str), fragment),
                    header: header.cloned(),
                    metadata: Some(serde_json::json!({
                        "paragraph_idx": para_idx,
                    })),
                });
            }
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
    fn test_split_text_zero_limit_still_makes_progress() {
        // A zero budget must not return an empty fits for non-empty text:
        // the oversized-split loop would never shrink `remaining` and spin
        // forever. Splitting must be total for any input.
        let (fits, remainder) = split_text_at_limit("abcdef", 0);
        assert!(!fits.is_empty());
        assert_eq!(format!("{}{}", fits, remainder), "abcdef");

        let multibyte = "\u{2019}\u{2019}\u{2019}";
        let (fits, remainder) = split_text_at_limit(multibyte, 0);
        assert!(!fits.is_empty());
        assert_eq!(format!("{}{}", fits, remainder), multibyte);
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
        let with_speakers: Vec<_> = utterances
            .iter()
            .map(|&(doc_id, ts, text, source)| (doc_id, ts, text, source, None))
            .collect();
        setup_test_db_with_speakers(&with_speakers)
    }

    fn setup_test_db_with_speakers(
        utterances: &[(&str, &str, &str, Option<&str>, Option<&str>)],
    ) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE transcript_utterances (
                id TEXT PRIMARY KEY,
                document_id TEXT NOT NULL,
                start_timestamp TEXT,
                end_timestamp TEXT,
                text TEXT,
                source TEXT,
                speaker_name TEXT
            );",
        )
        .unwrap();

        for (i, (doc_id, timestamp, text, source, speaker_name)) in utterances.iter().enumerate() {
            conn.execute(
                "INSERT INTO transcript_utterances (id, document_id, start_timestamp, end_timestamp, text, source, speaker_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    format!("u-{}", i),
                    doc_id,
                    timestamp,
                    timestamp,
                    text,
                    source,
                    speaker_name,
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
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_adaptive_single_small_utterance() {
        // A small utterance should become one chunk
        let text = "Hello world, this is a test with enough content to meet minimum chunk size requirements.";
        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", text, None)]);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
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
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("Short one"));
        assert!(chunks[0].text.contains("Short three"));
    }

    #[test]
    fn test_adaptive_splits_at_target() {
        // Create utterances that together exceed target but not max
        // Use a config with small target to force splits
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "First utterance with some content.",
                None,
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "Second utterance with more content.",
                None,
            ),
            (
                "doc1",
                "2025-01-01T10:02:00Z",
                "Third utterance continues on.",
                None,
            ),
        ];
        let conn = setup_test_db(&utts);
        // Small target: ~20 tokens = ~80 chars
        let config = ChunkingConfig {
            target_tokens: 15,
            max_tokens: 100,
            overlap_tokens: 5,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        // Should have multiple chunks
        assert!(
            chunks.len() >= 2,
            "Expected multiple chunks, got {}",
            chunks.len()
        );
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
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        // The huge text should result in multiple chunks
        assert!(
            chunks.len() > 1,
            "Expected split chunks, got {}",
            chunks.len()
        );
        // Each chunk should not exceed max_chars
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= config.max_chars(),
                "Chunk too large: {} chars, max was {}",
                chunk.text.len(),
                config.max_chars()
            );
        }
    }

    #[test]
    fn test_adaptive_split_path_counts_carryover_against_max() {
        // #123 defect 1: an oversized utterance arriving on a non-empty
        // buffer used to be split at max_chars and appended to the overlap
        // carryover, producing chunks up to overlap + 1 + max chars.
        let filler = "a".repeat(90);
        let huge = "x".repeat(400); // no split boundaries: forces hard splits
        let conn = setup_test_db(&[
            ("doc1", "2025-01-01T10:00:00Z", filler.as_str(), None),
            ("doc1", "2025-01-01T10:01:00Z", huge.as_str(), None),
        ]);
        let config = ChunkingConfig {
            target_tokens: 100,
            max_tokens: 150,
            overlap_tokens: 30,
            min_chars: 10,
            chars_per_token: 1.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert!(
            chunks.len() >= 4,
            "expected several chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= config.max_chars(),
                "chunk {} is {} chars, max is {}",
                chunk.source_id,
                chunk.text.len(),
                config.max_chars()
            );
        }
        // The tail of the oversized utterance survives the splits.
        assert!(chunks.last().unwrap().text.ends_with("xxx"));
    }

    #[test]
    fn test_adaptive_no_pure_carryover_duplicate_chunk() {
        // #123 defect 2: a large utterance arriving at a max boundary used
        // to double-finalize, emitting a chunk that was 100% overlap
        // carryover duplicated from its neighbor, with an inverted window
        // (window_start_idx > window_end_idx). The live database had 149
        // of these, every one exactly overlap_chars long.
        let first = "a".repeat(140);
        let second = "b".repeat(120);
        let conn = setup_test_db(&[
            ("doc1", "2025-01-01T10:00:00Z", first.as_str(), None),
            ("doc1", "2025-01-01T10:01:00Z", second.as_str(), None),
        ]);
        let config = ChunkingConfig {
            target_tokens: 100,
            max_tokens: 150,
            overlap_tokens: 30,
            min_chars: 10,
            chars_per_token: 1.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 2, "duplicate carryover chunk emitted");
        for chunk in &chunks {
            let meta = chunk.metadata.as_ref().unwrap();
            assert!(
                meta["window_start_idx"].as_u64() <= meta["window_end_idx"].as_u64(),
                "inverted window in {}: {:?}",
                chunk.source_id,
                meta
            );
            assert!(chunk.text.len() <= config.max_chars());
        }
        // The second chunk keeps the whole utterance, with the carryover
        // trimmed to fit the cap, and its window points at it.
        assert!(chunks[1].text.ends_with(second.as_str()));
        let meta = chunks[1].metadata.as_ref().unwrap();
        assert_eq!(meta["window_start_idx"], 1);
    }

    #[test]
    fn test_adaptive_utterance_overlap_dropped_at_max_boundary() {
        // Utterances-mode variant of the same boundary: carried whole
        // utterances are dropped from the front rather than char-sliced
        // when they would push the incoming utterance past the cap, so
        // chunks still start at utterance boundaries.
        let u0 = "a".repeat(60);
        let u1 = "b".repeat(70);
        let u2 = "c".repeat(120);
        let conn = setup_test_db(&[
            ("doc1", "2025-01-01T10:00:00Z", u0.as_str(), None),
            ("doc1", "2025-01-01T10:01:00Z", u1.as_str(), None),
            ("doc1", "2025-01-01T10:02:00Z", u2.as_str(), None),
        ]);
        let config = ChunkingConfig {
            target_tokens: 100,
            max_tokens: 150,
            overlap_tokens: 80,
            min_chars: 10,
            chars_per_token: 1.0,
            overlap_mode: OverlapMode::Utterances,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        for chunk in &chunks {
            assert!(chunk.text.len() <= config.max_chars());
            let meta = chunk.metadata.as_ref().unwrap();
            assert!(
                meta["window_start_idx"].as_u64() <= meta["window_end_idx"].as_u64(),
                "inverted window in {}: {:?}",
                chunk.source_id,
                meta
            );
        }
        // No carried utterance fits next to u2, so the last chunk starts
        // clean at the utterance boundary.
        assert_eq!(chunks.last().unwrap().text, u2);
    }

    #[test]
    fn test_adaptive_start_timestamp_survives_chunk_boundaries() {
        // #123 defect 3: the target-path reseed cleared buffer_start_ts and
        // nothing restored it on the normal append path, so every chunk
        // after a document's first carried a null start_timestamp (97% of
        // the live corpus). The max-path reseed had the mirror bug: it kept
        // the previous chunk's stale value. Every chunk's start_timestamp
        // must match the utterance its window starts at. The 250-char
        // utterance routes one boundary through the max path so both
        // reseed sites are exercised.
        let timestamps = [
            "2025-01-01T10:00:00Z",
            "2025-01-01T10:01:00Z",
            "2025-01-01T10:02:00Z",
            "2025-01-01T10:03:00Z",
            "2025-01-01T10:04:00Z",
        ];
        let texts = [
            "a".repeat(80),
            "b".repeat(80),
            "c".repeat(80),
            "d".repeat(250),
            "e".repeat(80),
        ];
        let utts: Vec<(&str, &str, &str, Option<&str>)> = timestamps
            .iter()
            .zip(texts.iter())
            .map(|(&ts, text)| ("doc1", ts, text.as_str(), None))
            .collect();
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 100,
            max_tokens: 300,
            overlap_tokens: 30,
            min_chars: 10,
            chars_per_token: 1.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert!(chunks.len() >= 3);
        for chunk in &chunks {
            let meta = chunk.metadata.as_ref().unwrap();
            let start_idx = meta["window_start_idx"].as_u64().unwrap() as usize;
            assert_eq!(
                meta["start_timestamp"],
                serde_json::json!(timestamps[start_idx]),
                "chunk {} start_timestamp should match its window start",
                chunk.source_id
            );
        }
    }

    #[test]
    fn test_adaptive_very_small_chunks_dropped() {
        // Chunks below min_chars should be dropped
        let conn = setup_test_db(&[
            ("doc1", "2025-01-01T10:00:00Z", "ok", None), // 2 chars - too small
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "This is adequate content for a chunk.",
                None,
            ),
        ]);
        let config = ChunkingConfig {
            target_tokens: 100,
            max_tokens: 200,
            overlap_tokens: 10,
            min_chars: 20,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        // The "ok" text alone would be too small, but combined they're fine
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.text.len() >= config.min_chars);
        }
    }

    #[test]
    fn test_adaptive_multiple_documents() {
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "Document one content that is long enough to meet the minimum chunk size requirements.",
                None,
            ),
            (
                "doc2",
                "2025-01-01T11:00:00Z",
                "Document two content that is also long enough to meet the minimum chunk size requirements.",
                None,
            ),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

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
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["window_start_idx"], 0);
        assert_eq!(meta["window_end_idx"], 2);
    }

    #[test]
    fn test_adaptive_metadata_tracks_timestamps() {
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "First utterance with enough content to pass the minimum.",
                None,
            ),
            (
                "doc1",
                "2025-01-01T10:05:00Z",
                "Last utterance with additional content to meet requirements.",
                None,
            ),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["start_timestamp"], "2025-01-01T10:00:00Z");
        assert_eq!(meta["end_timestamp"], "2025-01-01T10:05:00Z");
    }

    #[test]
    fn test_adaptive_source_id_format() {
        let conn = setup_test_db(&[(
            "doc1",
            "2025-01-01T10:00:00Z",
            "Content here that is long enough to meet minimum chunk size requirements for the test.",
            None,
        )]);
        let config = ChunkingConfig::default();
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

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
        )
        .unwrap();
    }

    #[test]
    fn test_panel_section_chunker_empty_db() {
        let conn = setup_panel_test_db();
        let config = ChunkingConfig::default();
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_panel_section_chunker_creates_chunks() {
        let conn = setup_panel_test_db();
        let markdown = "### Action Items\n\nWe need to complete the deployment process for the new release version.\n\n### Key Decisions\n\nThe team agreed to postpone the feature release until after testing is complete.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig {
            min_chars: 20,
            ..ChunkingConfig::default()
        };
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();

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

        let config = ChunkingConfig {
            min_chars: 20,
            ..ChunkingConfig::default()
        };
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].text.contains("Chat with"));
    }

    #[test]
    fn test_panel_section_chunker_skips_short_sections() {
        let conn = setup_panel_test_db();
        let markdown = "### Action Items\n\nOk.\n\n### Key Decisions\n\nWe decided to postpone the feature release until after quality testing is complete.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig {
            min_chars: 50,
            ..ChunkingConfig::default()
        };
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();

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

        let config = ChunkingConfig {
            min_chars: 20,
            ..ChunkingConfig::default()
        };
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_panel_section_chunker_metadata() {
        let conn = setup_panel_test_db();
        let markdown = "### Budget Review\n\nThe quarterly budget needs revision for the marketing department.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig {
            min_chars: 20,
            ..ChunkingConfig::default()
        };
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();

        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["panel_id"], "panel1");
        assert_eq!(meta["section_heading"], "Budget Review");
        assert_eq!(meta["section_idx"], 0);
    }

    // Tests for notes_paragraph_chunker

    /// The production notes config: 20-char minimum on the default spec.
    fn notes_test_config() -> ChunkingConfig {
        ChunkingConfig {
            min_chars: 20,
            ..ChunkingConfig::default()
        }
    }
    #[test]
    fn test_notes_paragraph_chunker_empty_db() {
        let conn = setup_panel_test_db();
        let chunks = notes_paragraph_chunker(&conn, &notes_test_config(), None).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_notes_paragraph_chunker_creates_chunks() {
        let conn = setup_panel_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', ?1)",
            ["First paragraph with enough content.\n\nSecond paragraph also with enough content."],
        ).unwrap();

        let chunks = notes_paragraph_chunker(&conn, &notes_test_config(), None).unwrap();

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

        let chunks = notes_paragraph_chunker(&conn, &notes_test_config(), None).unwrap();

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

        let chunks = notes_paragraph_chunker(&conn, &notes_test_config(), None).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_notes_paragraph_chunker_metadata() {
        let conn = setup_panel_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', ?1)",
            ["A paragraph that is long enough to be embedded."],
        ).unwrap();

        let chunks = notes_paragraph_chunker(&conn, &notes_test_config(), None).unwrap();

        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["paragraph_idx"], 0);
    }

    #[test]
    fn test_panel_section_chunker_splits_oversized_section() {
        // #123 defect 4: a section longer than the model cap became one
        // chunk of whatever length, and the embedder truncated its tail
        // (the live outlier was a 2,843-char panel section).
        let conn = setup_panel_test_db();
        let body = "This sentence pads the section out to a useful length. ".repeat(60);
        let markdown = format!("### Big Section\n\n{}", body);
        insert_test_panel(&conn, "panel1", "doc1", &markdown);

        let config = ChunkingConfig::default();
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();

        assert!(
            chunks.len() >= 2,
            "oversized section not split: {} chunk(s)",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= config.max_chars(),
                "chunk {} is {} chars, max is {}",
                chunk.source_id,
                chunk.text.len(),
                config.max_chars()
            );
            let meta = chunk.metadata.as_ref().unwrap();
            assert_eq!(meta["section_idx"], 0);
        }
        // Parts get distinct source ids (UNIQUE constraint), and the first
        // part keeps the unsuffixed id so within-cap sections keep their
        // identity and never re-embed.
        let ids: std::collections::HashSet<_> = chunks.iter().map(|c| &c.source_id).collect();
        assert_eq!(ids.len(), chunks.len());
        assert_eq!(chunks[0].source_id, "panel1:s0");
    }

    #[test]
    fn test_notes_paragraph_chunker_splits_oversized_paragraph() {
        // #123 defect 4, notes flavor: one huge paragraph became one huge
        // chunk. Same cap, same split.
        let conn = setup_panel_test_db();
        let para = "Another sentence keeps this paragraph growing longer. ".repeat(60);
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', ?1)",
            [para.trim()],
        )
        .unwrap();

        let config = notes_test_config();
        let chunks = notes_paragraph_chunker(&conn, &config, None).unwrap();

        assert!(
            chunks.len() >= 2,
            "oversized paragraph not split: {} chunk(s)",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= config.max_chars(),
                "chunk {} is {} chars, max is {}",
                chunk.source_id,
                chunk.text.len(),
                config.max_chars()
            );
        }
        let ids: std::collections::HashSet<_> = chunks.iter().map(|c| &c.source_id).collect();
        assert_eq!(ids.len(), chunks.len());
        assert_eq!(chunks[0].source_id, "doc1:n0");
    }

    #[test]
    fn test_panel_header_counts_against_cap() {
        // Header + section must stay within the model cap, matching the
        // budget the transcript chunker already applies.
        let conn = setup_panel_test_db();
        let body = "This sentence pads the section out to a useful length. ".repeat(35);
        let markdown = format!("### Big Section\n\n{}", body);
        insert_test_panel(&conn, "panel1", "doc1", &markdown);

        let header = format!("Meeting: {}\n\n", "x".repeat(300));
        let mut headers = HashMap::new();
        headers.insert("doc1".to_string(), header);

        let config = ChunkingConfig::default();
        let chunks = panel_section_chunker(&conn, &config, Some(&headers)).unwrap();

        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                chunk.embed_input().len() <= config.max_chars(),
                "embed input for {} is {} chars, max is {}",
                chunk.source_id,
                chunk.embed_input().len(),
                config.max_chars()
            );
        }
    }

    #[test]
    fn test_panel_section_chunker_h1_headers() {
        let conn = setup_panel_test_db();
        let markdown = "# Announcements\n\nNew hire starting Monday and onboarding schedule is ready.\n\n# Updates\n\nProject is on track for the quarterly deadline.\n\n# Action Items\n\n- Send welcome email to the new team member";
        insert_test_panel(&conn, "panel1", "doc1", markdown);

        let config = ChunkingConfig {
            min_chars: 20,
            ..ChunkingConfig::default()
        };
        let chunks = panel_section_chunker(&conn, &config, None).unwrap();

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

    // Tests for utterance-boundary overlap mode
    #[test]
    fn test_utterance_overlap_carries_whole_utterances() {
        // Three ~50-char utterances; target forces a split after the
        // second. In utterance mode the next chunk starts with the full
        // second utterance, not a mid-utterance character slice.
        let utt_a = "Utterance alpha talks about the quarterly budget.";
        let utt_b = "Utterance bravo covers the deployment timeline ok.";
        let utt_c = "Utterance charlie wraps up with the action items.";
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", utt_a, None),
            ("doc1", "2025-01-01T10:01:00Z", utt_b, None),
            ("doc1", "2025-01-01T10:02:00Z", utt_c, None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 25,
            max_tokens: 100,
            overlap_tokens: 15,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Utterances,
        };

        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert!(
            chunks.len() >= 2,
            "expected a split, got {} chunks",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(
                [utt_a, utt_b, utt_c]
                    .iter()
                    .any(|u| chunk.text.starts_with(u)),
                "chunk must start at an utterance boundary, got: {:?}",
                &chunk.text[..chunk.text.len().min(60)]
            );
        }
        // The overlap actually carries content: some chunk beyond the first
        // repeats an utterance already emitted.
        let all_text: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert!(all_text.matches(utt_b).count() >= 2 || all_text.matches(utt_a).count() >= 2);

        // Contrast: chars mode slices mid-utterance under the same config.
        let chars_config = ChunkingConfig {
            overlap_mode: OverlapMode::Chars,
            ..config
        };
        let chars_chunks = transcript_window_chunker_adaptive(&conn, &chars_config, None).unwrap();
        assert!(
            chars_chunks
                .iter()
                .any(|c| ![utt_a, utt_b, utt_c].iter().any(|u| c.text.starts_with(u))),
            "chars mode should produce at least one mid-utterance chunk here"
        );
    }

    #[test]
    fn test_utterance_overlap_skips_oversized_tail() {
        // The trailing utterance is bigger than the overlap budget, so
        // nothing is carried: the next chunk starts with the new utterance.
        let utt_a = "Utterance alpha talks about the quarterly budget and it keeps going for a while longer.";
        let utt_b =
            "Utterance bravo is the next one and stands alone with plenty of content of its own.";
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", utt_a, None),
            ("doc1", "2025-01-01T10:01:00Z", utt_b, None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 20,
            max_tokens: 100,
            overlap_tokens: 5, // 20 chars: smaller than either utterance
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Utterances,
        };

        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, utt_a);
        assert_eq!(chunks[1].text, utt_b);
    }

    #[test]
    fn test_overlap_mode_roundtrip() {
        assert_eq!(
            OverlapMode::parse(OverlapMode::Chars.as_str()),
            Some(OverlapMode::Chars)
        );
        assert_eq!(
            OverlapMode::parse(OverlapMode::Utterances.as_str()),
            Some(OverlapMode::Utterances)
        );
        assert_eq!(OverlapMode::parse("bogus"), None);
    }

    // Tests for contextual headers threading through the chunkers
    fn headers_for(doc_id: &str, header: &str) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        map.insert(doc_id.to_string(), header.to_string());
        map
    }

    #[test]
    fn test_adaptive_header_on_chunk_not_in_text() {
        let text = "Hello world, this is a test with enough content to meet minimum chunk size requirements.";
        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", text, None)]);
        let config = ChunkingConfig::default();
        let headers = headers_for("doc1", "Meeting: Sync\n\n");

        let chunks = transcript_window_chunker_adaptive(&conn, &config, Some(&headers)).unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].header.as_deref(), Some("Meeting: Sync\n\n"));
        assert!(!chunks[0].text.contains("Meeting:"));
        assert!(chunks[0].embed_input().starts_with("Meeting: Sync\n\n"));
    }

    #[test]
    fn test_adaptive_header_changes_content_hash() {
        let text = "Hello world, this is a test with enough content to meet minimum chunk size requirements.";
        let config = ChunkingConfig::default();

        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", text, None)]);
        let without = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        let with_a = transcript_window_chunker_adaptive(
            &conn,
            &config,
            Some(&headers_for("doc1", "Meeting: A\n\n")),
        )
        .unwrap();
        let with_b = transcript_window_chunker_adaptive(
            &conn,
            &config,
            Some(&headers_for("doc1", "Meeting: B\n\n")),
        )
        .unwrap();

        assert_ne!(without[0].content_hash, with_a[0].content_hash);
        assert_ne!(with_a[0].content_hash, with_b[0].content_hash);
        // Text itself is identical in all three.
        assert_eq!(without[0].text, with_a[0].text);
    }

    #[test]
    fn test_adaptive_header_shrinks_chunk_budget() {
        // With a header, header + text must stay within max_chars.
        let long_text =
            "This is a fairly long sentence that keeps going with more words. ".repeat(10);
        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", &long_text, None)]);
        let config = ChunkingConfig {
            target_tokens: 30,
            max_tokens: 50,
            overlap_tokens: 10,
            min_chars: 20,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let header = "Meeting: Budget Review Session\nDate: 2026-02-01\n\n";
        let headers = headers_for("doc1", header);

        let chunks = transcript_window_chunker_adaptive(&conn, &config, Some(&headers)).unwrap();

        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(
                chunk.embed_input().len() <= config.max_chars() + 50,
                "embed input too large: {} bytes vs max {}",
                chunk.embed_input().len(),
                config.max_chars()
            );
        }
    }

    #[test]
    fn test_adaptive_no_header_entry_means_no_header() {
        let text = "Hello world, this is a test with enough content to meet minimum chunk size requirements.";
        let conn = setup_test_db(&[("doc1", "2025-01-01T10:00:00Z", text, None)]);
        let config = ChunkingConfig::default();
        let headers = headers_for("other-doc", "Meeting: Other\n\n");

        let chunks = transcript_window_chunker_adaptive(&conn, &config, Some(&headers)).unwrap();

        assert_eq!(chunks[0].header, None);
    }

    #[test]
    fn test_panel_chunker_carries_header() {
        let conn = setup_panel_test_db();
        let markdown = "### Action Items\n\nComplete the deployment process for the entire team.";
        insert_test_panel(&conn, "panel1", "doc1", markdown);
        let config = ChunkingConfig {
            min_chars: 20,
            ..ChunkingConfig::default()
        };
        let headers = headers_for("doc1", "Meeting: Sync\n\n");

        let chunks = panel_section_chunker(&conn, &config, Some(&headers)).unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].header.as_deref(), Some("Meeting: Sync\n\n"));
        assert!(!chunks[0].text.contains("Meeting: Sync"));
    }

    #[test]
    fn test_notes_chunker_carries_header() {
        let conn = setup_panel_test_db();
        conn.execute(
            "INSERT INTO documents (id, title, created_at, notes_plain) VALUES ('doc1', 'Test', '2025-01-01T00:00:00Z', ?1)",
            ["A paragraph that is long enough to be embedded."],
        ).unwrap();
        let headers = headers_for("doc1", "Meeting: Sync\n\n");

        let chunks = notes_paragraph_chunker(&conn, &notes_test_config(), Some(&headers)).unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].header.as_deref(), Some("Meeting: Sync\n\n"));
    }

    // Regression tests: transcripts contain multi-byte UTF-8 (curly
    // apostrophes from transcription); byte-offset slicing must not panic.
    #[test]
    fn test_split_text_multibyte_hard_split_no_panic() {
        // 27 three-byte chars, no sentence or word boundaries: forces the
        // hard-split path with max_chars landing mid-character.
        let text = "\u{2019}".repeat(27);
        let (fits, remainder) = split_text_at_limit(&text, 10);
        assert!(!fits.is_empty());
        assert!(fits.len() <= 10);
        assert_eq!(format!("{}{}", fits, remainder), text);
    }

    #[test]
    fn test_adaptive_target_overlap_multibyte_no_panic() {
        // Two 81-byte utterances of 3-byte chars; finalizing at the target
        // boundary computes overlap_start = 81 - 20 = 61, which is not a
        // char boundary.
        let utt = "\u{2019}".repeat(27);
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", utt.as_str(), None),
            ("doc1", "2025-01-01T10:01:00Z", utt.as_str(), None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 15,
            max_tokens: 100,
            overlap_tokens: 5,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_adaptive_max_overlap_multibyte_no_panic() {
        // Same shape, but the second utterance pushes past max_chars so the
        // max-limit finalization path computes the overlap slice.
        let utt = "\u{2019}".repeat(27);
        let utts = vec![
            ("doc1", "2025-01-01T10:00:00Z", utt.as_str(), None),
            ("doc1", "2025-01-01T10:01:00Z", utt.as_str(), None),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 15,
            max_tokens: 25,
            overlap_tokens: 5,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_adaptive_oversized_multibyte_utterance_no_panic() {
        // A single 600-byte utterance of 3-byte chars exercises the
        // oversized-split loop and its overlap carryover.
        let utt = "\u{2019}".repeat(200);
        let utts = vec![("doc1", "2025-01-01T10:00:00Z", utt.as_str(), None)];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 15,
            max_tokens: 25,
            overlap_tokens: 5,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        assert!(chunks.len() > 1);
    }

    // Tests for format_utterance_text
    #[test]
    fn test_format_utterance_text_microphone() {
        assert_eq!(
            format_utterance_text("hello", Some("microphone")),
            "[You] hello"
        );
    }

    #[test]
    fn test_format_utterance_text_system() {
        assert_eq!(
            format_utterance_text("hello", Some("system")),
            "[Other] hello"
        );
    }

    #[test]
    fn test_format_utterance_text_none() {
        assert_eq!(format_utterance_text("hello", None), "hello");
    }

    #[test]
    fn test_format_utterance_text_unknown() {
        assert_eq!(
            format_utterance_text("hello", Some("unknown_source")),
            "hello"
        );
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
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();
        assert!(
            chunks.is_empty(),
            "Empty text with source should produce no chunks, got {}",
            chunks.len()
        );
    }

    // Integration tests for speaker labels in chunkers
    #[test]
    fn test_adaptive_speaker_labels_in_chunks() {
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "I think we should proceed with the plan.",
                Some("microphone"),
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "That sounds good, let me check the timeline.",
                Some("system"),
            ),
            (
                "doc1",
                "2025-01-01T10:02:00Z",
                "Great, I will send the details after this meeting.",
                Some("microphone"),
            ),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("[You] I think we should proceed"));
        assert!(chunks[0].text.contains("[Other] That sounds good"));
        assert!(chunks[0].text.contains("[You] Great, I will send"));
    }

    #[test]
    fn test_adaptive_no_labels_when_source_null() {
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "First utterance with enough content for chunking.",
                None,
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "Second utterance also with enough content here.",
                None,
            ),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].text.contains("[You]"));
        assert!(!chunks[0].text.contains("[Other]"));
        assert!(chunks[0].text.contains("First utterance"));
        assert!(chunks[0].text.contains("Second utterance"));
    }

    #[test]
    fn test_adaptive_mixed_sources_and_null() {
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "Labeled utterance from the user.",
                Some("microphone"),
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "Unlabeled utterance with no source.",
                None,
            ),
            (
                "doc1",
                "2025-01-01T10:02:00Z",
                "Labeled utterance from other person.",
                Some("system"),
            ),
        ];
        let conn = setup_test_db(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(
            chunks[0]
                .text
                .contains("[You] Labeled utterance from the user.")
        );
        assert!(
            chunks[0]
                .text
                .contains("Unlabeled utterance with no source.")
        );
        assert!(!chunks[0].text.contains("[You] Unlabeled"));
        assert!(!chunks[0].text.contains("[Other] Unlabeled"));
        assert!(
            chunks[0]
                .text
                .contains("[Other] Labeled utterance from other person.")
        );
    }

    // Tests for the speakers array in chunk metadata (#110)
    #[test]
    fn test_adaptive_speakers_in_metadata() {
        // Distinct names, first-appearance order, duplicates collapsed.
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "Jane opens the meeting with the agenda for today.",
                Some("system"),
                Some("Jane Doe"),
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "John responds with an update on the deployment.",
                Some("system"),
                Some("John Smith"),
            ),
            (
                "doc1",
                "2025-01-01T10:02:00Z",
                "Jane wraps up with the action items for everyone.",
                Some("system"),
                Some("Jane Doe"),
            ),
        ];
        let conn = setup_test_db_with_speakers(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(
            meta["speakers"],
            serde_json::json!(["Jane Doe", "John Smith"])
        );
    }

    #[test]
    fn test_adaptive_speakers_empty_when_unattributed() {
        // Pre-cutover data: no names anywhere. The array is present and
        // empty, not absent and not an error.
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "An old utterance from before speaker attribution existed.",
                Some("system"),
                None,
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "Another unattributed utterance with plenty of content.",
                Some("system"),
                None,
            ),
        ];
        let conn = setup_test_db_with_speakers(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["speakers"], serde_json::json!([]));
    }

    #[test]
    fn test_adaptive_speakers_exclude_local_user() {
        // Microphone utterances are the local user: no name, no sentinel.
        // Only named speakers from the system channel appear.
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                "I think we should go ahead with the migration plan.",
                Some("microphone"),
                None,
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                "Agreed, let me pull up the timeline for that work.",
                Some("system"),
                Some("Jane Doe"),
            ),
        ];
        let conn = setup_test_db_with_speakers(&utts);
        let config = ChunkingConfig {
            target_tokens: 500,
            max_tokens: 1000,
            overlap_tokens: 100,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 1);
        let meta = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta["speakers"], serde_json::json!(["Jane Doe"]));
    }

    #[test]
    fn test_adaptive_speakers_follow_chunk_windows() {
        // When a document splits into multiple chunks, each chunk carries
        // only the speakers of its own window.
        let utt_a = "Utterance alpha talks about the quarterly budget plan.";
        let utt_b = "Utterance bravo covers the deployment timeline today.";
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                utt_a,
                Some("system"),
                Some("Jane Doe"),
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                utt_b,
                Some("system"),
                Some("John Smith"),
            ),
        ];
        let conn = setup_test_db_with_speakers(&utts);
        let config = ChunkingConfig {
            target_tokens: 20,
            max_tokens: 100,
            overlap_tokens: 1,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Utterances,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 2);
        let meta0 = chunks[0].metadata.as_ref().unwrap();
        let meta1 = chunks[1].metadata.as_ref().unwrap();
        assert_eq!(meta0["speakers"], serde_json::json!(["Jane Doe"]));
        assert_eq!(meta1["speakers"], serde_json::json!(["John Smith"]));
    }

    #[test]
    fn test_adaptive_speakers_exclude_overlap_carryover() {
        // Chars-mode overlap copies the tail of the previous chunk into
        // the next one, so a prior speaker's words can appear in a chunk
        // whose window doesn't include them. The speakers array follows
        // the window, exactly like window_start_idx/window_end_idx: the
        // chunk that owns the utterance attributes it, and adjacent
        // chunks don't double-attribute shared overlap text.
        let utt_a = "Jane spends a while walking through the quarterly budget figures";
        let utt_b = "John follows up with a question about the deployment schedule now";
        let utts = vec![
            (
                "doc1",
                "2025-01-01T10:00:00Z",
                utt_a,
                Some("system"),
                Some("Jane Doe"),
            ),
            (
                "doc1",
                "2025-01-01T10:01:00Z",
                utt_b,
                Some("system"),
                Some("John Smith"),
            ),
        ];
        let conn = setup_test_db_with_speakers(&utts);
        let config = ChunkingConfig {
            target_tokens: 25,
            max_tokens: 100,
            overlap_tokens: 15,
            min_chars: 10,
            chars_per_token: 4.0,
            overlap_mode: OverlapMode::Chars,
        };
        let chunks = transcript_window_chunker_adaptive(&conn, &config, None).unwrap();

        assert_eq!(chunks.len(), 2);
        // The second chunk really does carry Jane's words via overlap...
        assert!(chunks[1].text.contains("budget figures"));
        // ...but its window is utterance 1 only, and speakers match it.
        let meta = chunks[1].metadata.as_ref().unwrap();
        assert_eq!(meta["window_start_idx"], 1);
        assert_eq!(meta["speakers"], serde_json::json!(["John Smith"]));
        // Jane is attributed by the chunk that owns her utterance.
        let meta0 = chunks[0].metadata.as_ref().unwrap();
        assert_eq!(meta0["speakers"], serde_json::json!(["Jane Doe"]));
    }

    #[test]
    fn test_speaker_name_does_not_change_content_hash() {
        // The core #110 invariant: attaching speaker names is metadata
        // only. Identical text with and without names must hash the same,
        // or the whole corpus re-embeds.
        let text = "This utterance has enough content to form a chunk on its own for the test.";
        let config = ChunkingConfig::default();

        let conn_named = setup_test_db_with_speakers(&[(
            "doc1",
            "2025-01-01T10:00:00Z",
            text,
            Some("system"),
            Some("Jane Doe"),
        )]);
        let named = transcript_window_chunker_adaptive(&conn_named, &config, None).unwrap();

        let conn_unnamed = setup_test_db_with_speakers(&[(
            "doc1",
            "2025-01-01T10:00:00Z",
            text,
            Some("system"),
            None,
        )]);
        let unnamed = transcript_window_chunker_adaptive(&conn_unnamed, &config, None).unwrap();

        assert_eq!(named[0].text, unnamed[0].text);
        assert_eq!(named[0].content_hash, unnamed[0].content_hash);
        // Only the metadata differs.
        let named_meta = named[0].metadata.as_ref().unwrap();
        let unnamed_meta = unnamed[0].metadata.as_ref().unwrap();
        assert_ne!(named_meta["speakers"], unnamed_meta["speakers"]);
    }

    #[test]
    fn test_content_hash_changes_with_speaker_label() {
        // Same text but different source → different hash
        let text = "This is a test utterance with enough content to meet minimum chunk size requirements for the test.";
        let utts_mic = vec![("doc1", "2025-01-01T10:00:00Z", text, Some("microphone"))];
        let utts_sys = vec![("doc1", "2025-01-01T10:00:00Z", text, Some("system"))];
        let utts_none = vec![("doc1", "2025-01-01T10:00:00Z", text, None)];
        let config = ChunkingConfig::default();

        let conn_mic = setup_test_db(&utts_mic);
        let chunks_mic = transcript_window_chunker_adaptive(&conn_mic, &config, None).unwrap();

        let conn_sys = setup_test_db(&utts_sys);
        let chunks_sys = transcript_window_chunker_adaptive(&conn_sys, &config, None).unwrap();

        let conn_none = setup_test_db(&utts_none);
        let chunks_none = transcript_window_chunker_adaptive(&conn_none, &config, None).unwrap();

        // All three should produce different hashes
        assert_ne!(chunks_mic[0].content_hash, chunks_sys[0].content_hash);
        assert_ne!(chunks_mic[0].content_hash, chunks_none[0].content_hash);
        assert_ne!(chunks_sys[0].content_hash, chunks_none[0].content_hash);
    }
}
