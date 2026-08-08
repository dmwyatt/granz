//! The shared pre-model snapshot behind embedding status and embedding.
//!
//! Computing the desired chunk set is the expensive part of both the
//! status check and `ensure_embeddings`: it re-reads every source table
//! and re-runs all chunkers, about a second on a large database. A single
//! search needs both a status (for the confirmation prompt) and an
//! embedding pass; `EmbeddingPlan` computes the snapshot once so the
//! search pays that cost once instead of twice (#126).

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::Connection;

use super::chunk::{Chunk, ChunkSourceType};
use super::config::EmbedSpec;
use super::{
    EmbeddingStatus, SourceTypeBreakdown, calculate_chunk_size_stats, chunker, headers, store,
};

/// Everything the embedding phase needs that can be computed without
/// loading a model: the chunks the database should contain under `spec`,
/// and the chunks it currently stores. Both status and
/// `ensure_embeddings_with_plan` diff the same snapshot, so the prompt a
/// user confirms and the work that then runs always agree.
pub struct EmbeddingPlan {
    pub(super) spec: EmbedSpec,
    pub(super) desired_chunks: Vec<Chunk>,
    pub(super) stored: Vec<store::StoredChunk>,
}

impl EmbeddingPlan {
    /// Run all chunkers under `spec` and read the stored chunk state.
    pub fn compute(conn: &Connection, spec: EmbedSpec) -> Result<Self> {
        let desired_chunks = desired_chunks_for_spec(conn, &spec)?;
        let stored = store::get_stored_chunks(conn)?;
        Ok(EmbeddingPlan {
            spec,
            desired_chunks,
            stored,
        })
    }

    /// Derive the embedding status from this snapshot. `current_model`
    /// identifies the embedder that will be used; stored embeddings from a
    /// different model are unusable and reported as pending.
    pub fn status(&self, conn: &Connection, current_model: &str) -> Result<EmbeddingStatus> {
        let stored_map: HashMap<(&str, &str), &store::StoredChunk> = self
            .stored
            .iter()
            .map(|s| ((s.source_type.as_str(), s.source_id.as_str()), s))
            .collect();

        // Build set of desired keys and count by type
        let mut desired_keys: HashSet<(String, String)> = HashSet::new();
        let mut pending_count = 0;
        let mut total_by_type = SourceTypeBreakdown::default();
        let mut pending_by_type = SourceTypeBreakdown::default();

        for chunk in &self.desired_chunks {
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

        // Embeddings written by a different model are unusable: everything must
        // be re-embedded, regardless of content hashes.
        let model_name = store::get_model_name(conn);
        let model_changed_warning =
            model_name.is_some() && model_name.as_deref() != Some(current_model);
        if model_changed_warning {
            pending_count = self.desired_chunks.len();
            pending_by_type = total_by_type.clone();
        }

        // Calculate embedded by type (total - pending)
        let embedded_by_type = SourceTypeBreakdown {
            transcript_window: total_by_type.transcript_window - pending_by_type.transcript_window,
            panel_section: total_by_type.panel_section - pending_by_type.panel_section,
            notes_paragraph: total_by_type.notes_paragraph - pending_by_type.notes_paragraph,
        };

        // Count orphans (stored but not in desired)
        let orphan_count = self
            .stored
            .iter()
            .filter(|s| !desired_keys.contains(&(s.source_type.clone(), s.source_id.clone())))
            .count();

        // Get chunk size statistics
        let chunk_size_stats = calculate_chunk_size_stats(conn)?;

        // Get max_length setting
        let max_length = store::get_max_length(conn);

        // Determine if we should show legacy warning:
        // Model exists but max_length is missing (legacy embeddings)
        let legacy_max_length_warning = model_name.is_some() && max_length.is_none();

        let chunking_changed_warning = self.spec.differs_from_stored(conn);

        Ok(EmbeddingStatus {
            total_chunks: self.desired_chunks.len(),
            embedded_chunks: self.desired_chunks.len() - pending_count,
            pending_chunks: pending_count,
            orphaned_chunks: orphan_count,
            total_by_type,
            embedded_by_type,
            pending_by_type,
            model_name,
            chunk_size_stats,
            max_length,
            legacy_max_length_warning,
            model_changed_warning,
            chunking_changed_warning,
        })
    }
}

/// Run all chunkers under `spec`, building contextual headers when the
/// spec asks for them. This is the single definition of which chunks a
/// database "should" contain; status and embedding both use it so their
/// hashes always agree.
fn desired_chunks_for_spec(conn: &Connection, spec: &EmbedSpec) -> Result<Vec<Chunk>> {
    let doc_headers = if spec.contextual_headers {
        Some(headers::build_doc_headers(conn)?)
    } else {
        None
    };
    let doc_headers = doc_headers.as_ref();

    let mut chunks =
        chunker::transcript_window_chunker_adaptive(conn, &spec.chunking, doc_headers)?;
    chunks.extend(chunker::panel_section_chunker(
        conn,
        &spec.chunking,
        doc_headers,
    )?);
    // Notes keep their historical 20-char minimum; cap and header budget
    // come from the shared chunking spec.
    let notes_config = chunker::ChunkingConfig {
        min_chars: 20,
        ..spec.chunking.clone()
    };
    chunks.extend(chunker::notes_paragraph_chunker(
        conn,
        &notes_config,
        doc_headers,
    )?);
    Ok(chunks)
}
