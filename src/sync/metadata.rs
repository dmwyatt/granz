//! Sync metadata for remote database statistics.
//!
//! This module defines the metadata structure that gets uploaded alongside databases
//! during `sync push`. This allows `sync status` to show database statistics without
//! downloading the full databases.

use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::schema::get_schema_version;

/// Metadata about synced databases, uploaded as a small JSON file alongside the databases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMetadata {
    /// ISO 8601 timestamp when this metadata was generated
    pub generated_at: String,
    /// Statistics from the database (now includes embeddings)
    pub index_db: Option<IndexDbStats>,
    /// Legacy field for backwards compatibility with older remote metadata
    /// New pushes will not include this field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeddings_db: Option<EmbeddingsDbStats>,
}

/// Statistics extracted from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDbStats {
    pub document_count: i64,
    pub documents_with_transcripts: i64,
    pub transcript_utterance_count: i64,
    pub people_count: i64,
    pub earliest_document: Option<String>,
    pub latest_document: Option<String>,
    pub schema_version: usize,
    /// Embedding stats (now part of main database)
    #[serde(default)]
    pub embedding_count: i64,
    #[serde(default)]
    pub embedding_model: Option<String>,
}

/// Legacy embeddings database stats for backwards compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsDbStats {
    pub embedding_count: i64,
    pub model: Option<String>,
}

impl SyncMetadata {
    /// Generate metadata from local database file.
    pub fn from_local_db(db_path: Option<&Path>) -> Result<Self> {
        let generated_at = chrono::Utc::now().to_rfc3339();

        let index_db = if let Some(path) = db_path {
            if path.exists() {
                let conn = Connection::open(path)?;
                Some(IndexDbStats::from_db(&conn)?)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            generated_at,
            index_db,
            embeddings_db: None, // No longer using separate embeddings.db
        })
    }

}

impl IndexDbStats {
    /// Extract statistics from an open database connection.
    pub fn from_db(conn: &Connection) -> Result<Self> {
        // Total documents (excluding deleted)
        let document_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE deleted_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Documents with transcripts
        let documents_with_transcripts: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT document_id) FROM transcript_utterances",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Total utterances
        let transcript_utterance_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transcript_utterances", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        // People count
        let people_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))
            .unwrap_or(0);

        // Date range
        let (earliest_document, latest_document): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT MIN(created_at), MAX(created_at) FROM documents WHERE deleted_at IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((None, None));

        // Schema version from migrations
        let schema_version = get_schema_version(conn).unwrap_or(0);

        // Embedding stats (now in main database)
        let embedding_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
            .unwrap_or(0);

        let embedding_model: Option<String> = conn
            .query_row(
                "SELECT value FROM embedding_metadata WHERE key = 'model_name'",
                [],
                |row| row.get(0),
            )
            .ok();

        Ok(Self {
            document_count,
            documents_with_transcripts,
            transcript_utterance_count,
            people_count,
            earliest_document,
            latest_document,
            schema_version,
            embedding_count,
            embedding_model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_db(dir: &TempDir) -> std::path::PathBuf {
        let db_path = dir.path().join("grans.db");
        let conn = Connection::open(&db_path).unwrap();
        crate::db::schema::create_tables(&conn).unwrap();

        // Insert test data
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
            "INSERT INTO transcript_utterances (id, document_id, text) VALUES ('u2', 'd1', 'world')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO people (id, name, email) VALUES ('p1', 'Alice', 'alice@test.com')",
            [],
        )
        .unwrap();

        // Add some embedding data
        conn.execute(
            "INSERT INTO embedding_metadata (key, value) VALUES ('model_name', 'all-MiniLM-L6-v2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
             VALUES ('transcript', 'd1:w0', 'd1', 'hash1', 'test', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let chunk_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, X'0001')",
            [chunk_id],
        )
        .unwrap();

        db_path
    }

    #[test]
    fn test_index_db_stats() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);

        let conn = Connection::open(&db_path).unwrap();
        let stats = IndexDbStats::from_db(&conn).unwrap();

        assert_eq!(stats.document_count, 2);
        assert_eq!(stats.documents_with_transcripts, 1);
        assert_eq!(stats.transcript_utterance_count, 2);
        assert_eq!(stats.people_count, 1);
        assert_eq!(
            stats.earliest_document,
            Some("2024-01-15T10:00:00Z".to_string())
        );
        assert_eq!(
            stats.latest_document,
            Some("2024-06-20T10:00:00Z".to_string())
        );
        assert_eq!(stats.embedding_count, 1);
        assert_eq!(stats.embedding_model, Some("all-MiniLM-L6-v2".to_string()));
    }

    #[test]
    fn test_sync_metadata_from_local_db() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);

        let metadata = SyncMetadata::from_local_db(Some(&db_path)).unwrap();

        assert!(metadata.index_db.is_some());
        assert!(metadata.embeddings_db.is_none()); // No longer used
        assert!(!metadata.generated_at.is_empty());

        let index = metadata.index_db.unwrap();
        assert_eq!(index.document_count, 2);
        assert_eq!(index.embedding_count, 1);
    }

    #[test]
    fn test_sync_metadata_with_missing_db() {
        let metadata = SyncMetadata::from_local_db(None).unwrap();

        assert!(metadata.index_db.is_none());
        assert!(metadata.embeddings_db.is_none());
    }

    #[test]
    fn test_sync_metadata_serialization() {
        let metadata = SyncMetadata {
            generated_at: "2025-01-27T10:00:00Z".to_string(),
            index_db: Some(IndexDbStats {
                document_count: 100,
                documents_with_transcripts: 80,
                transcript_utterance_count: 5000,
                people_count: 50,
                earliest_document: Some("2023-06-01T00:00:00Z".to_string()),
                latest_document: Some("2025-01-27T00:00:00Z".to_string()),
                schema_version: 1,
                embedding_count: 5000,
                embedding_model: Some("all-MiniLM-L6-v2".to_string()),
            }),
            embeddings_db: None,
        };

        let json = serde_json::to_string_pretty(&metadata).unwrap();
        assert!(json.contains("document_count"));
        assert!(json.contains("embedding_count"));
        assert!(!json.contains("embeddings_db")); // Should be skipped

        // Verify round-trip
        let parsed: SyncMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.index_db.as_ref().unwrap().document_count,
            metadata.index_db.as_ref().unwrap().document_count
        );
    }

    #[test]
    fn test_backwards_compatible_deserialization() {
        // Old format with separate embeddings_db
        let old_json = r#"{
            "generated_at": "2025-01-27T10:00:00Z",
            "index_db": {
                "document_count": 100,
                "documents_with_transcripts": 80,
                "transcript_utterance_count": 5000,
                "people_count": 50,
                "earliest_document": "2023-06-01T00:00:00Z",
                "latest_document": "2025-01-27T00:00:00Z",
                "schema_version": 3
            },
            "embeddings_db": {
                "embedding_count": 5000,
                "model": "all-MiniLM-L6-v2"
            }
        }"#;

        let parsed: SyncMetadata = serde_json::from_str(old_json).unwrap();
        assert!(parsed.index_db.is_some());
        assert!(parsed.embeddings_db.is_some());
        assert_eq!(parsed.embeddings_db.unwrap().embedding_count, 5000);
    }
}
