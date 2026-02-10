use anyhow::Result;
use rusqlite::Connection;

use super::chunk::{Chunk, ChunkSourceType};

/// A stored chunk with its database ID.
#[derive(Debug)]
pub struct StoredChunk {
    pub id: i64,
    pub source_type: String,
    pub source_id: String,
    #[allow(dead_code)]
    pub document_id: String,
    pub content_hash: String,
    #[allow(dead_code)]
    pub text: String,
}

/// Get all stored chunks with their content hashes (for diffing).
pub fn get_stored_chunks(conn: &Connection) -> Result<Vec<StoredChunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_type, source_id, document_id, content_hash, text FROM chunks",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(StoredChunk {
            id: row.get(0)?,
            source_type: row.get(1)?,
            source_id: row.get(2)?,
            document_id: row.get(3)?,
            content_hash: row.get(4)?,
            text: row.get(5)?,
        })
    })?;

    let mut chunks = Vec::new();
    for row in rows {
        chunks.push(row?);
    }
    Ok(chunks)
}

/// Insert a chunk and its embedding vector.
/// Note: For production use, prefer `insert_chunks_with_embeddings_batch` for better performance.
#[allow(dead_code)]
pub fn insert_chunk_with_embedding(
    conn: &Connection,
    chunk: &Chunk,
    vector: &[f32],
) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    let metadata_str = chunk
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());

    conn.execute(
        "INSERT OR REPLACE INTO chunks (source_type, source_id, document_id, content_hash, text, metadata_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            chunk.source_type.as_str(),
            chunk.source_id,
            chunk.document_id,
            chunk.content_hash,
            chunk.text,
            metadata_str,
            now,
        ],
    )?;

    let chunk_id = conn.last_insert_rowid();

    let blob = vector_to_blob(vector);
    conn.execute(
        "INSERT OR REPLACE INTO embeddings (chunk_id, vector) VALUES (?1, ?2)",
        rusqlite::params![chunk_id, blob],
    )?;

    Ok(chunk_id)
}

/// Insert multiple chunks and their embeddings in a single transaction.
/// Returns a vector of results, one per chunk (chunk_id on success, error on failure).
pub fn insert_chunks_with_embeddings_batch(
    conn: &Connection,
    items: &[(&Chunk, &[f32])],
) -> Vec<Result<i64>> {
    if items.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::with_capacity(items.len());

    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(e) => {
            return items
                .iter()
                .map(|_| Err(anyhow::anyhow!("Transaction start failed: {}", e)))
                .collect();
        }
    };

    let chunk_sql = "INSERT OR REPLACE INTO chunks (source_type, source_id, document_id, content_hash, text, metadata_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";
    let embedding_sql = "INSERT OR REPLACE INTO embeddings (chunk_id, vector) VALUES (?1, ?2)";

    let mut chunk_stmt = match tx.prepare(chunk_sql) {
        Ok(s) => s,
        Err(e) => {
            return items
                .iter()
                .map(|_| Err(anyhow::anyhow!("Prepare failed: {}", e)))
                .collect();
        }
    };
    let mut embedding_stmt = match tx.prepare(embedding_sql) {
        Ok(s) => s,
        Err(e) => {
            return items
                .iter()
                .map(|_| Err(anyhow::anyhow!("Prepare failed: {}", e)))
                .collect();
        }
    };

    let now = chrono::Utc::now().to_rfc3339();

    for (chunk, vector) in items {
        let result = (|| -> Result<i64> {
            let metadata_str = chunk
                .metadata
                .as_ref()
                .map(|m| serde_json::to_string(m).unwrap_or_default());

            chunk_stmt.execute(rusqlite::params![
                chunk.source_type.as_str(),
                chunk.source_id,
                chunk.document_id,
                chunk.content_hash,
                chunk.text,
                metadata_str,
                now,
            ])?;

            let chunk_id = tx.last_insert_rowid();
            let blob = vector_to_blob(vector);
            embedding_stmt.execute(rusqlite::params![chunk_id, blob])?;

            Ok(chunk_id)
        })();

        results.push(result);
    }

    // Drop statements before committing to release borrows
    drop(chunk_stmt);
    drop(embedding_stmt);

    if let Err(e) = tx.commit() {
        return items
            .iter()
            .map(|_| Err(anyhow::anyhow!("Commit failed: {}", e)))
            .collect();
    }

    results
}

/// Delete chunks by their IDs.
/// Embeddings are automatically deleted via CASCADE.
pub fn delete_chunks(conn: &Connection, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    let mut stmt = tx.prepare("DELETE FROM chunks WHERE id = ?1")?;

    for id in ids {
        stmt.execute([id])?;
    }

    drop(stmt);
    tx.commit()?;
    Ok(())
}

/// Delete the N most recently created chunks.
/// Returns the number of chunks actually deleted.
pub fn delete_recent_chunks(conn: &Connection, count: usize) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT id FROM chunks ORDER BY created_at DESC LIMIT ?1")?;
    let ids: Vec<i64> = stmt
        .query_map([count as i64], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let actual = ids.len();
    delete_chunks(conn, &ids)?;
    Ok(actual)
}

/// A loaded vector with its chunk metadata.
#[derive(Debug, Clone)]
pub struct StoredVector {
    #[allow(dead_code)]
    pub chunk_id: i64,
    pub document_id: String,
    pub source_type: String,
    pub text: String,
    pub vector: Vec<f32>,
    /// JSON metadata from chunk (contains window_start_idx, window_end_idx, etc.)
    pub metadata_json: Option<String>,
}

/// Load all vectors into memory for search.
pub fn load_all_vectors(conn: &Connection) -> Result<Vec<StoredVector>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.document_id, c.source_type, c.text, e.vector, c.metadata_json
         FROM chunks c
         JOIN embeddings e ON e.chunk_id = c.id",
    )?;

    let rows = stmt.query_map([], |row| {
        let blob: Vec<u8> = row.get(4)?;
        Ok(StoredVector {
            chunk_id: row.get(0)?,
            document_id: row.get(1)?,
            source_type: row.get(2)?,
            text: row.get(3)?,
            vector: blob_to_vector(&blob),
            metadata_json: row.get(5)?,
        })
    })?;

    let mut vectors = Vec::new();
    for row in rows {
        vectors.push(row?);
    }
    Ok(vectors)
}

/// Store model metadata.
pub fn set_model_metadata(
    conn: &Connection,
    model_name: &str,
    dim: usize,
    max_length: usize,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO embedding_metadata (key, value) VALUES ('model_name', ?1)",
        [model_name],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO embedding_metadata (key, value) VALUES ('embedding_dim', ?1)",
        [&dim.to_string()],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO embedding_metadata (key, value) VALUES ('max_length', ?1)",
        [&max_length.to_string()],
    )?;
    Ok(())
}

/// Get stored max_length (None for legacy embeddings that don't have it).
pub fn get_max_length(conn: &Connection) -> Option<usize> {
    conn.query_row(
        "SELECT value FROM embedding_metadata WHERE key = 'max_length'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| s.parse().ok())
}

/// Get stored model name (to detect model changes).
pub fn get_model_name(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM embedding_metadata WHERE key = 'model_name'",
        [],
        |row| row.get(0),
    )
    .ok()
}

/// Check if the stored model matches the current one. If not, wipe all embeddings.
pub fn check_model_consistency(conn: &Connection, current_model: &str) -> Result<bool> {
    match get_model_name(conn) {
        Some(stored) if stored == current_model => Ok(true),
        Some(_) => {
            // Model changed — wipe embeddings
            conn.execute_batch(
                "DELETE FROM embeddings; DELETE FROM chunks;",
            )?;
            Ok(false)
        }
        None => Ok(false),
    }
}

fn vector_to_blob(vector: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(vector.len() * 4);
    for &val in vector {
        blob.extend_from_slice(&val.to_le_bytes());
    }
    blob
}

fn blob_to_vector(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Get the source type filter for stored chunks.
#[allow(dead_code)]
pub fn get_stored_chunks_by_source(
    conn: &Connection,
    source_type: &ChunkSourceType,
) -> Result<Vec<StoredChunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_type, source_id, document_id, content_hash, text
         FROM chunks WHERE source_type = ?1",
    )?;

    let rows = stmt.query_map([source_type.as_str()], |row| {
        Ok(StoredChunk {
            id: row.get(0)?,
            source_type: row.get(1)?,
            source_id: row.get(2)?,
            document_id: row.get(3)?,
            content_hash: row.get(4)?,
            text: row.get(5)?,
        })
    })?;

    let mut chunks = Vec::new();
    for row in rows {
        chunks.push(row?);
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::chunk::{hash_content, ChunkSourceType};

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_load_vector() {
        let conn = test_db();
        let chunk = Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:w0".to_string(),
            document_id: "doc1".to_string(),
            text: "hello world".to_string(),
            content_hash: hash_content("hello world"),
            metadata: None,
        };
        let vec = vec![1.0_f32, 2.0, 3.0];

        let id = insert_chunk_with_embedding(&conn, &chunk, &vec).unwrap();
        assert!(id > 0);

        let vectors = load_all_vectors(&conn).unwrap();
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].document_id, "doc1");
        assert_eq!(vectors[0].vector, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_delete_chunks() {
        let conn = test_db();
        let chunk = Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:w0".to_string(),
            document_id: "doc1".to_string(),
            text: "test".to_string(),
            content_hash: hash_content("test"),
            metadata: None,
        };
        let id = insert_chunk_with_embedding(&conn, &chunk, &[0.5, 0.5]).unwrap();
        delete_chunks(&conn, &[id]).unwrap();

        let vectors = load_all_vectors(&conn).unwrap();
        assert!(vectors.is_empty());
    }

    #[test]
    fn test_get_stored_chunks() {
        let conn = test_db();
        let chunk = Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:w0".to_string(),
            document_id: "doc1".to_string(),
            text: "test".to_string(),
            content_hash: hash_content("test"),
            metadata: None,
        };
        insert_chunk_with_embedding(&conn, &chunk, &[1.0]).unwrap();

        let stored = get_stored_chunks(&conn).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].source_id, "doc1:w0");
        assert_eq!(stored[0].content_hash, hash_content("test"));
    }

    #[test]
    fn test_vector_blob_roundtrip() {
        let v = vec![1.0_f32, -2.5, 0.0, 3.14159];
        let blob = vector_to_blob(&v);
        let recovered = blob_to_vector(&blob);
        assert_eq!(v, recovered);
    }

    #[test]
    fn test_model_consistency_check() {
        let conn = test_db();

        // No model stored yet
        assert!(!check_model_consistency(&conn, "model-a").unwrap());

        set_model_metadata(&conn, "model-a", 768, 512).unwrap();
        assert!(check_model_consistency(&conn, "model-a").unwrap());

        // Different model — should wipe
        let chunk = Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:w0".to_string(),
            document_id: "doc1".to_string(),
            text: "test".to_string(),
            content_hash: hash_content("test"),
            metadata: None,
        };
        insert_chunk_with_embedding(&conn, &chunk, &[1.0]).unwrap();

        assert!(!check_model_consistency(&conn, "model-b").unwrap());
        let stored = get_stored_chunks(&conn).unwrap();
        assert!(stored.is_empty());
    }

    #[test]
    fn test_load_all_vectors_includes_metadata() {
        let conn = test_db();
        let metadata = serde_json::json!({
            "window_start_idx": 0,
            "window_end_idx": 5,
        });
        let chunk = Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:w0".to_string(),
            document_id: "doc1".to_string(),
            text: "hello world".to_string(),
            content_hash: hash_content("hello world"),
            metadata: Some(metadata),
        };
        insert_chunk_with_embedding(&conn, &chunk, &[1.0, 2.0]).unwrap();

        let vectors = load_all_vectors(&conn).unwrap();
        assert_eq!(vectors.len(), 1);
        assert!(vectors[0].metadata_json.is_some());

        let meta: serde_json::Value =
            serde_json::from_str(vectors[0].metadata_json.as_ref().unwrap()).unwrap();
        assert_eq!(meta["window_start_idx"], 0);
        assert_eq!(meta["window_end_idx"], 5);
    }

    #[test]
    fn test_load_all_vectors_without_metadata() {
        let conn = test_db();
        let chunk = Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:w0".to_string(),
            document_id: "doc1".to_string(),
            text: "no metadata".to_string(),
            content_hash: hash_content("no metadata"),
            metadata: None,
        };
        insert_chunk_with_embedding(&conn, &chunk, &[1.0]).unwrap();

        let vectors = load_all_vectors(&conn).unwrap();
        assert_eq!(vectors.len(), 1);
        assert!(vectors[0].metadata_json.is_none());
    }

    #[test]
    fn test_delete_recent_chunks() {
        let conn = test_db();

        // Insert 3 chunks with small delays to ensure different created_at times
        for i in 0..3 {
            let chunk = Chunk {
                source_type: ChunkSourceType::TranscriptWindow,
                source_id: format!("doc1:w{}", i),
                document_id: "doc1".to_string(),
                text: format!("chunk {}", i),
                content_hash: hash_content(&format!("chunk {}", i)),
                metadata: None,
            };
            insert_chunk_with_embedding(&conn, &chunk, &[i as f32]).unwrap();
        }

        let stored = get_stored_chunks(&conn).unwrap();
        assert_eq!(stored.len(), 3);

        // Delete 2 most recent chunks
        let deleted = delete_recent_chunks(&conn, 2).unwrap();
        assert_eq!(deleted, 2);

        let remaining = get_stored_chunks(&conn).unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn test_delete_recent_chunks_more_than_exist() {
        let conn = test_db();

        let chunk = Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:w0".to_string(),
            document_id: "doc1".to_string(),
            text: "only one".to_string(),
            content_hash: hash_content("only one"),
            metadata: None,
        };
        insert_chunk_with_embedding(&conn, &chunk, &[1.0]).unwrap();

        // Try to delete 10, but only 1 exists
        let deleted = delete_recent_chunks(&conn, 10).unwrap();
        assert_eq!(deleted, 1);

        let remaining = get_stored_chunks(&conn).unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_insert_chunks_batch() {
        let conn = test_db();

        let chunks: Vec<Chunk> = (0..5)
            .map(|i| Chunk {
                source_type: ChunkSourceType::TranscriptWindow,
                source_id: format!("doc1:w{}", i),
                document_id: "doc1".to_string(),
                text: format!("chunk {}", i),
                content_hash: hash_content(&format!("chunk {}", i)),
                metadata: None,
            })
            .collect();

        let vectors: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32, (i + 1) as f32]).collect();

        let items: Vec<(&Chunk, &[f32])> = chunks
            .iter()
            .zip(vectors.iter())
            .map(|(c, v)| (c, v.as_slice()))
            .collect();

        let results = insert_chunks_with_embeddings_batch(&conn, &items);

        assert_eq!(results.len(), 5);
        for result in &results {
            assert!(result.is_ok());
        }

        let stored = get_stored_chunks(&conn).unwrap();
        assert_eq!(stored.len(), 5);

        // Verify vectors were stored correctly
        let loaded = load_all_vectors(&conn).unwrap();
        assert_eq!(loaded.len(), 5);
    }

    #[test]
    fn test_insert_chunks_batch_empty() {
        let conn = test_db();
        let results = insert_chunks_with_embeddings_batch(&conn, &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_delete_chunks_empty() {
        let conn = test_db();
        // Should not error when deleting empty list
        delete_chunks(&conn, &[]).unwrap();
    }

    #[test]
    fn test_set_and_get_max_length() {
        let conn = test_db();
        set_model_metadata(&conn, "model-a", 768, 512).unwrap();

        let max_length = get_max_length(&conn);
        assert_eq!(max_length, Some(512));
    }

    #[test]
    fn test_get_max_length_missing() {
        let conn = test_db();
        // No metadata set yet - legacy case
        let max_length = get_max_length(&conn);
        assert_eq!(max_length, None);
    }

    #[test]
    fn test_set_model_metadata_updates_existing() {
        let conn = test_db();
        set_model_metadata(&conn, "model-a", 768, 256).unwrap();
        assert_eq!(get_max_length(&conn), Some(256));

        // Update with new max_length
        set_model_metadata(&conn, "model-a", 768, 512).unwrap();
        assert_eq!(get_max_length(&conn), Some(512));
    }
}
