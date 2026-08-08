//! Read-only loading of the stored vector index for search.
//!
//! Search never repairs the embedding store (#126): it searches what is
//! embedded and reports what is not. Freshness is judged from two
//! timestamps that both live inside the database file (so a synced copy
//! of the database stays self-consistent): the `last_sync_*` stamps the
//! sync command writes, and the `last_embedded_at` stamp
//! `ensure_embeddings` writes on completion.

use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use rusqlite::Connection;

use super::{EmbeddingIndex, store};
use crate::db;

/// Sync entities whose data feeds chunks; a sync of any of these after the
/// last embed means the index may be missing content.
const CHUNK_SOURCE_ENTITIES: [&str; 3] = ["documents", "transcripts", "panels"];

/// What search can say about the stored vector index before using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexFreshness {
    /// Usable and no chunk-source sync since the last embed.
    Fresh,
    /// Usable, but chunk sources synced after the last embed (or the
    /// embed time is unknown); recent content may be missing.
    Stale,
    /// No embeddings stored; the semantic half of search has nothing.
    Empty,
    /// Stored embeddings were built by a different model and are
    /// unusable with the current one.
    ModelMismatch { stored_model: String },
}

/// Load the stored vectors for search, read-only, with a freshness
/// verdict. Never chunks, embeds, deletes orphans, or wipes on model
/// change; an unusable store yields an empty index instead.
pub fn load_search_index(
    conn: &Connection,
    current_model: &str,
) -> Result<(EmbeddingIndex, IndexFreshness)> {
    let empty = || EmbeddingIndex {
        vectors: Vec::new(),
        stats: None,
    };

    match store::get_model_name(conn) {
        None => return Ok((empty(), IndexFreshness::Empty)),
        Some(m) if m != current_model => {
            return Ok((empty(), IndexFreshness::ModelMismatch { stored_model: m }));
        }
        Some(_) => {}
    }

    let vectors = store::load_all_vectors(conn)?;
    if vectors.is_empty() {
        return Ok((empty(), IndexFreshness::Empty));
    }

    let freshness = if synced_since_last_embed(conn) {
        IndexFreshness::Stale
    } else {
        IndexFreshness::Fresh
    };
    Ok((
        EmbeddingIndex {
            vectors,
            stats: None,
        },
        freshness,
    ))
}

/// True when any chunk-source entity synced after the last embed, or when
/// the last embed time is unknown or unparseable (conservative: warn).
fn synced_since_last_embed(conn: &Connection) -> bool {
    let last_synced = CHUNK_SOURCE_ENTITIES
        .iter()
        .filter_map(|entity| db::sync::get_last_sync_time(conn, entity))
        .filter_map(|ts| parse_rfc3339(&ts))
        .max();
    let Some(last_synced) = last_synced else {
        // Never synced: nothing can have drifted.
        return false;
    };

    match store::get_last_embedded_at(conn).and_then(|ts| parse_rfc3339(&ts)) {
        Some(last_embedded) => last_synced > last_embedded,
        None => true,
    }
}

fn parse_rfc3339(ts: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(ts).ok()
}

#[cfg(test)]
mod tests {
    use super::super::model::MockEmbedder;
    use super::super::{DEFAULT_BATCH_SIZE, config, ensure_embeddings};
    use super::*;

    fn setup_embedded_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
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
        ensure_embeddings(
            &conn,
            &embedder,
            DEFAULT_BATCH_SIZE,
            &config::EmbedSpec::default_for(512),
        )
        .unwrap();
        conn
    }

    fn set_sync_time(conn: &Connection, entity: &str, ts: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
            rusqlite::params![format!("last_sync_{}", entity), ts],
        )
        .unwrap();
    }

    fn set_embedded_at(conn: &Connection, ts: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO embedding_metadata (key, value) VALUES ('last_embedded_at', ?1)",
            [ts],
        )
        .unwrap();
    }

    #[test]
    fn empty_db_loads_empty_index() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();

        let (index, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert!(index.is_empty());
        assert_eq!(freshness, IndexFreshness::Empty);
    }

    #[test]
    fn embedded_and_never_synced_is_fresh() {
        let conn = setup_embedded_db();

        let (index, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert!(!index.is_empty());
        assert_eq!(freshness, IndexFreshness::Fresh);
    }

    #[test]
    fn embed_after_sync_is_fresh() {
        let conn = setup_embedded_db();
        set_sync_time(&conn, "transcripts", "2025-06-01T00:00:00Z");
        set_embedded_at(&conn, "2025-06-02T00:00:00Z");

        let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert_eq!(freshness, IndexFreshness::Fresh);
    }

    #[test]
    fn sync_after_embed_is_stale() {
        let conn = setup_embedded_db();
        set_embedded_at(&conn, "2025-06-01T00:00:00Z");
        set_sync_time(&conn, "transcripts", "2025-06-02T00:00:00Z");

        let (index, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert!(!index.is_empty(), "stale index still searches");
        assert_eq!(freshness, IndexFreshness::Stale);
    }

    #[test]
    fn any_chunk_source_entity_counts_for_staleness() {
        for entity in ["documents", "transcripts", "panels"] {
            let conn = setup_embedded_db();
            set_embedded_at(&conn, "2025-06-01T00:00:00Z");
            set_sync_time(&conn, entity, "2025-06-02T00:00:00Z");

            let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
            assert_eq!(freshness, IndexFreshness::Stale, "entity {}", entity);
        }
    }

    #[test]
    fn non_chunk_entity_sync_does_not_go_stale() {
        let conn = setup_embedded_db();
        set_embedded_at(&conn, "2025-06-01T00:00:00Z");
        set_sync_time(&conn, "people", "2025-06-02T00:00:00Z");

        let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert_eq!(freshness, IndexFreshness::Fresh);
    }

    #[test]
    fn synced_with_missing_embed_stamp_is_stale() {
        // Databases embedded before the stamp existed must warn, not
        // silently claim freshness.
        let conn = setup_embedded_db();
        conn.execute(
            "DELETE FROM embedding_metadata WHERE key = 'last_embedded_at'",
            [],
        )
        .unwrap();
        set_sync_time(&conn, "documents", "2025-06-01T00:00:00Z");

        let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert_eq!(freshness, IndexFreshness::Stale);
    }

    #[test]
    fn model_mismatch_yields_empty_index_without_wiping() {
        let conn = setup_embedded_db();

        let (index, freshness) = load_search_index(&conn, "some-other-model").unwrap();
        assert!(index.is_empty());
        assert_eq!(
            freshness,
            IndexFreshness::ModelMismatch {
                stored_model: "mock-embedder".to_string()
            }
        );

        // Read-only: the stored vectors must survive untouched.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
