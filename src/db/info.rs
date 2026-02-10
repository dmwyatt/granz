use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::db::schema::get_schema_version;
use crate::embed::{calculate_chunk_size_stats, ChunkSizeStats};

#[derive(Debug, Serialize)]
pub struct DbInfo {
    // Content stats (from DB)
    pub total_documents: i64,
    pub documents_with_transcripts: i64,
    pub documents_without_transcripts: i64,
    pub earliest_document: Option<String>,
    pub latest_document: Option<String>,
    pub total_people: i64,
    pub total_calendars: i64,
    pub total_events: i64,
    pub total_templates: i64,
    pub total_recipes: i64,
    pub total_panels: i64,
    pub total_utterances: i64,

    // Embedding stats
    pub total_chunks: i64,
    pub total_embeddings: i64,
    pub embedding_model: Option<String>,
    pub chunk_size_stats: Option<ChunkSizeStats>,

    // Database info
    pub db_path: PathBuf,
    pub db_size_bytes: u64,
    pub schema_version: usize,
}

pub fn get_info_db_only(conn: &Connection, db_path: &Path) -> Result<DbInfo> {
    // Total documents (excluding deleted)
    let total_documents: i64 = conn.query_row(
        "SELECT COUNT(*) FROM documents WHERE deleted_at IS NULL",
        [],
        |row| row.get(0),
    )?;

    // Documents with transcripts
    let documents_with_transcripts: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT document_id) FROM transcript_utterances",
        [],
        |row| row.get(0),
    )?;

    // Documents without transcripts
    let documents_without_transcripts: i64 = total_documents - documents_with_transcripts;

    // Date range of documents
    let (earliest_document, latest_document): (Option<String>, Option<String>) = conn.query_row(
        "SELECT MIN(created_at), MAX(created_at) FROM documents WHERE deleted_at IS NULL",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    // Other counts
    let total_people: i64 =
        conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;

    let total_calendars: i64 =
        conn.query_row("SELECT COUNT(*) FROM calendars", [], |row| row.get(0))?;

    let total_events: i64 =
        conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;

    let total_templates: i64 = conn.query_row(
        "SELECT COUNT(*) FROM templates WHERE deleted_at IS NULL",
        [],
        |row| row.get(0),
    )?;

    let total_recipes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM recipes WHERE deleted_at IS NULL",
        [],
        |row| row.get(0),
    )?;

    let total_panels: i64 = crate::db::panels::count_panels(conn).unwrap_or(0);

    let total_utterances: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transcript_utterances",
        [],
        |row| row.get(0),
    )?;

    // Embedding stats
    let total_chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .unwrap_or(0);

    let total_embeddings: i64 = conn
        .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
        .unwrap_or(0);

    let embedding_model: Option<String> = conn
        .query_row(
            "SELECT value FROM embedding_metadata WHERE key = 'model_name'",
            [],
            |row| row.get(0),
        )
        .ok();

    let chunk_size_stats = calculate_chunk_size_stats(conn).unwrap_or(None);

    let db_size_bytes = std::fs::metadata(db_path)
        .map(|m| m.len())
        .unwrap_or(0);

    let schema_version = get_schema_version(conn).unwrap_or(0);

    Ok(DbInfo {
        total_documents,
        documents_with_transcripts,
        documents_without_transcripts,
        earliest_document,
        latest_document,
        total_people,
        total_calendars,
        total_events,
        total_templates,
        total_recipes,
        total_panels,
        total_utterances,
        total_chunks,
        total_embeddings,
        embedding_model,
        chunk_size_stats,
        db_path: db_path.to_path_buf(),
        db_size_bytes,
        schema_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_db(dir: &TempDir) -> (Connection, PathBuf) {
        let db_path = dir.path().join("index.db");

        let conn = Connection::open(&db_path).unwrap();
        crate::db::schema::create_tables(&conn).unwrap();

        // Insert some test data
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('d1', 'Test', '2024-01-15T10:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('d2', 'Test 2', '2024-06-20T10:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text) VALUES ('u1', 'd1', 'hello')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO people (id, name, email) VALUES ('p1', 'Alice', 'alice@test.com')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO calendars (id, provider, summary) VALUES ('c1', 'google', 'Work')",
            [],
        )
        .unwrap();

        (conn, db_path)
    }

    #[test]
    fn test_get_info_returns_stats() {
        let dir = TempDir::new().unwrap();
        let (conn, db_path) = create_test_db(&dir);

        let info = get_info_db_only(&conn, &db_path).unwrap();

        assert_eq!(info.total_documents, 2);
        assert_eq!(info.documents_with_transcripts, 1);
        assert_eq!(info.documents_without_transcripts, 1);
        assert_eq!(info.earliest_document, Some("2024-01-15T10:00:00Z".to_string()));
        assert_eq!(info.latest_document, Some("2024-06-20T10:00:00Z".to_string()));
        assert_eq!(info.total_people, 1);
        assert_eq!(info.total_calendars, 1);
        assert_eq!(info.total_events, 0);
        assert_eq!(info.total_templates, 0);
        assert_eq!(info.total_recipes, 0);
        assert_eq!(info.total_utterances, 1);
        assert_eq!(info.total_chunks, 0);
        assert_eq!(info.total_embeddings, 0);
        assert!(info.embedding_model.is_none());
        assert!(info.chunk_size_stats.is_none());
        assert!(info.db_size_bytes > 0);
    }

    #[test]
    fn test_get_info_excludes_deleted_documents() {
        let dir = TempDir::new().unwrap();
        let (conn, db_path) = create_test_db(&dir);

        // Add a deleted document
        conn.execute(
            "INSERT INTO documents (id, title, created_at, deleted_at) VALUES ('d3', 'Deleted', '2024-01-01T00:00:00Z', '2024-01-02T00:00:00Z')",
            [],
        )
        .unwrap();

        let info = get_info_db_only(&conn, &db_path).unwrap();

        // Should still be 2, not 3
        assert_eq!(info.total_documents, 2);
    }

    #[test]
    fn test_get_info_empty_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");

        let conn = Connection::open(&db_path).unwrap();
        crate::db::schema::create_tables(&conn).unwrap();

        let info = get_info_db_only(&conn, &db_path).unwrap();

        assert_eq!(info.total_documents, 0);
        assert_eq!(info.documents_with_transcripts, 0);
        assert_eq!(info.documents_without_transcripts, 0);
        assert!(info.earliest_document.is_none());
        assert!(info.latest_document.is_none());
    }

    #[test]
    fn test_get_info_includes_embedding_stats() {
        let dir = TempDir::new().unwrap();
        let (conn, db_path) = create_test_db(&dir);

        // Add embedding data
        conn.execute(
            "INSERT INTO embedding_metadata (key, value) VALUES ('model_name', 'test-model')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
             VALUES ('transcript', 'd1:w0', 'd1', 'hash1', 'test text', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let chunk_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, X'00010203')",
            [chunk_id],
        )
        .unwrap();

        let info = get_info_db_only(&conn, &db_path).unwrap();

        assert_eq!(info.total_chunks, 1);
        assert_eq!(info.total_embeddings, 1);
        assert_eq!(info.embedding_model, Some("test-model".to_string()));
        assert!(info.chunk_size_stats.is_some());
    }
}
