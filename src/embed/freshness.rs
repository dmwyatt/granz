//! Read-only loading of the stored vector index for search.
//!
//! Search never repairs the embedding store (#126): it searches what is
//! embedded and reports what is not. Freshness is judged entirely within
//! the `last_sync_*` stamp stream (so a synced copy of the database stays
//! self-consistent): an embed run captures the newest chunk-source sync
//! stamp before reading any source data and stores it as the covered
//! watermark on success; the index is stale once any chunk-source stamp
//! is newer than that watermark. Because the watermark is a copied sync
//! stamp, the embedding machine's clock never enters the comparison; the
//! residual assumption is only that sync stamps themselves advance, which
//! holds across machines sharing a database unless their clocks disagree
//! by more than the gap between two consecutive syncs.

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
    /// Usable and no chunk-source sync past the covered watermark.
    Fresh,
    /// Usable, but chunk sources synced past the covered watermark (or
    /// no watermark is stored); recent content may be missing.
    Stale,
    /// No embeddings stored; the semantic half of search has nothing.
    Empty,
    /// Usable, but built before chunking settings were recorded (no
    /// stored max_length): an outdated chunking strategy that degrades
    /// relevance until `grans embed` rebuilds the store.
    LegacyChunking,
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

    let freshness = if store::get_max_length(conn).is_none() {
        IndexFreshness::LegacyChunking
    } else if synced_since_last_embed(conn)? {
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

/// The newest chunk-source sync stamp, raw and parsed. None when no chunk
/// source has ever synced (or no stamp parses).
fn newest_chunk_source_sync(conn: &Connection) -> Result<Option<(String, DateTime<FixedOffset>)>> {
    let mut newest: Option<(String, DateTime<FixedOffset>)> = None;
    for entity in CHUNK_SOURCE_ENTITIES {
        let Some(raw) = db::sync::get_last_sync_time(conn, entity)? else {
            continue;
        };
        let Some(parsed) = parse_rfc3339(&raw) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(_, t)| parsed > *t) {
            newest = Some((raw, parsed));
        }
    }
    Ok(newest)
}

/// The sync watermark an embed run must capture *before* reading source
/// data: whatever it embeds covers at most the syncs recorded so far, so
/// this is the newest stamp the run may claim on success. A sync that
/// completes mid-run writes a newer stamp and correctly reads as stale.
pub fn current_sync_watermark(conn: &Connection) -> Result<Option<String>> {
    Ok(newest_chunk_source_sync(conn)?.map(|(raw, _)| raw))
}

/// True when a chunk-source sync stamp is newer than the watermark the
/// store covers, or when syncs exist but no watermark is stored or it is
/// unparseable (conservative: warn).
fn synced_since_last_embed(conn: &Connection) -> Result<bool> {
    let Some((_, last_synced)) = newest_chunk_source_sync(conn)? else {
        // Never synced: nothing can have drifted.
        return Ok(false);
    };

    match store::get_embedded_watermark(conn)?.and_then(|ts| parse_rfc3339(&ts)) {
        Some(covered) => Ok(last_synced > covered),
        None => Ok(true),
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

    fn set_watermark(conn: &Connection, ts: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO embedding_metadata (key, value) \
             VALUES ('embedded_sync_watermark', ?1)",
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
    fn watermark_covering_latest_sync_is_fresh() {
        // The stored watermark is the sync stamp the embed run captured;
        // a stamp equal to the newest sync means everything is covered.
        let conn = setup_embedded_db();
        set_sync_time(&conn, "transcripts", "2025-06-01T00:00:00Z");
        set_watermark(&conn, "2025-06-01T00:00:00Z");

        let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert_eq!(freshness, IndexFreshness::Fresh);
    }

    #[test]
    fn sync_after_embed_is_stale() {
        let conn = setup_embedded_db();
        set_watermark(&conn, "2025-06-01T00:00:00Z");
        set_sync_time(&conn, "transcripts", "2025-06-02T00:00:00Z");

        let (index, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert!(!index.is_empty(), "stale index still searches");
        assert_eq!(freshness, IndexFreshness::Stale);
    }

    #[test]
    fn any_chunk_source_entity_counts_for_staleness() {
        for entity in ["documents", "transcripts", "panels"] {
            let conn = setup_embedded_db();
            set_watermark(&conn, "2025-06-01T00:00:00Z");
            set_sync_time(&conn, entity, "2025-06-02T00:00:00Z");

            let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
            assert_eq!(freshness, IndexFreshness::Stale, "entity {}", entity);
        }
    }

    #[test]
    fn non_chunk_entity_sync_does_not_go_stale() {
        let conn = setup_embedded_db();
        set_watermark(&conn, "2025-06-01T00:00:00Z");
        set_sync_time(&conn, "people", "2025-06-02T00:00:00Z");

        let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert_eq!(freshness, IndexFreshness::Fresh);
    }

    #[test]
    fn synced_with_missing_watermark_is_stale() {
        // Databases embedded before the watermark existed (or whose
        // watermark was invalidated) must warn, not claim freshness.
        let conn = setup_embedded_db();
        set_sync_time(&conn, "documents", "2025-06-01T00:00:00Z");

        let (_, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert_eq!(freshness, IndexFreshness::Stale);
    }

    #[test]
    fn missing_max_length_is_legacy_chunking() {
        // Regression for #129 review: the read-only path lost main's
        // pre-search rebuild warning for stores that predate persisted
        // chunking settings; the verdict must carry it instead.
        let conn = setup_embedded_db();
        conn.execute(
            "DELETE FROM embedding_metadata WHERE key = 'max_length'",
            [],
        )
        .unwrap();

        let (index, freshness) = load_search_index(&conn, "mock-embedder").unwrap();
        assert!(!index.is_empty(), "legacy index still searches");
        assert_eq!(freshness, IndexFreshness::LegacyChunking);
    }

    #[test]
    fn current_sync_watermark_is_newest_chunk_source_stamp() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        set_sync_time(&conn, "documents", "2025-06-01T00:00:00Z");
        set_sync_time(&conn, "transcripts", "2025-06-03T00:00:00Z");
        set_sync_time(&conn, "panels", "2025-06-02T00:00:00Z");
        // Non-chunk entities never move the watermark.
        set_sync_time(&conn, "people", "2025-06-09T00:00:00Z");

        assert_eq!(
            current_sync_watermark(&conn).unwrap().as_deref(),
            Some("2025-06-03T00:00:00Z")
        );
    }

    #[test]
    fn current_sync_watermark_none_when_never_synced() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        assert_eq!(current_sync_watermark(&conn).unwrap(), None);
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
