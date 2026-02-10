//! Database migration system using rusqlite_migration.
//!
//! This module handles schema versioning and migrations for the grans database.
//! Migrations are embedded SQL files that are run in order to bring the database
//! up to the current schema version.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

/// All migrations, in order. Each migration brings the schema from version N to N+1.
/// The `user_version` pragma is used automatically by rusqlite_migration to track
/// which migrations have been applied.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("v001_initial_schema.sql")),
        M::up(include_str!("v002_capture_missing_fields.sql")),
        M::up(include_str!("v003_utterance_metadata.sql")),
        M::up(include_str!("v004_make_title_not_null.sql")),
        M::up(include_str!("v005_transcript_sync_log.sql")),
        M::up(include_str!("v006_panels.sql")),
        M::up(include_str!("v007_transcript_utterance_index.sql")),
        M::up(include_str!("v008_panel_chat_url.sql")),
        M::up(include_str!("v009_document_raw_json.sql")),
        M::up(include_str!("v010_rename_audio_source_to_source.sql")),
        M::up(include_str!("v011_raw_json_templates_recipes_events.sql")),
        M::up(include_str!("v012_rename_is_primary_to_primary.sql")),
        M::up(include_str!("v013_api_snapshot.sql")),
    ])
}

/// Open the database, running any pending migrations.
/// Backs up the database before applying migrations if it already exists.
pub fn open_and_migrate(db_path: &Path) -> Result<Connection> {
    let db_exists = db_path.exists();

    let mut conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

    let m = migrations();

    // Check if there are pending migrations
    let current_version = m.current_version(&conn)
        .context("Failed to check current schema version")?;

    // Check if we need to apply migrations by comparing current vs target
    let needs_migration = match current_version {
        rusqlite_migration::SchemaVersion::NoneSet => true,
        rusqlite_migration::SchemaVersion::Inside(v) => {
            // Check if current version is less than the number of migrations
            let current = v.get();
            let total = 13; // We have 13 migrations (v001-v013)
            current < total
        }
        rusqlite_migration::SchemaVersion::Outside(_) => false,
    };

    if needs_migration && db_exists && !matches!(current_version, rusqlite_migration::SchemaVersion::NoneSet) {
        backup_database(db_path)?;
        eprintln!("[grans] Applying database migration(s)...");
    } else if needs_migration && !db_exists {
        eprintln!("[grans] Creating new database at {}", db_path.display());
    }

    m.to_latest(&mut conn)
        .context("Failed to apply database migrations")?;

    Ok(conn)
}

/// Get the current schema version from the database.
pub fn get_schema_version(conn: &Connection) -> Result<usize> {
    let m = migrations();
    let version = m.current_version(conn)
        .context("Failed to get schema version")?;

    Ok(match version {
        rusqlite_migration::SchemaVersion::NoneSet => 0,
        rusqlite_migration::SchemaVersion::Inside(v) => v.get(),
        rusqlite_migration::SchemaVersion::Outside(v) => v.get(),
    })
}

/// Create a backup of the database file before migrations.
fn backup_database(db_path: &Path) -> Result<()> {
    if !db_path.exists() {
        return Ok(()); // Nothing to backup
    }

    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let backup_path = db_path.with_extension(format!("db.backup.{}", timestamp));

    std::fs::copy(db_path, &backup_path)
        .with_context(|| format!("Failed to backup database to {}", backup_path.display()))?;

    eprintln!("[grans] Backed up database to {}", backup_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn migrations_are_valid() {
        assert!(migrations().validate().is_ok());
    }

    #[test]
    fn test_fresh_database_creation() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"documents".to_string()));
        assert!(tables.contains(&"transcript_utterances".to_string()));
        assert!(tables.contains(&"chunks".to_string()));
        assert!(tables.contains(&"embeddings".to_string()));
        assert!(tables.contains(&"embedding_metadata".to_string()));
    }

    #[test]
    fn test_migration_idempotent() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // First run
        let conn = open_and_migrate(&db_path).unwrap();
        drop(conn);

        // Second run should not fail
        let conn = open_and_migrate(&db_path).unwrap();

        // Should still have all tables
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='documents'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_schema_version() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();
        let version = get_schema_version(&conn).unwrap();

        // Should be version 13 after all migrations
        assert_eq!(version, 13);
    }

    #[test]
    fn test_backup_created_on_migration() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Create initial database
        let conn = open_and_migrate(&db_path).unwrap();
        drop(conn);

        // Count backup files (should be 0 for fresh db)
        let backup_count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .ok()
                    .map(|e| e.path().to_string_lossy().contains(".backup."))
                    .unwrap_or(false)
            })
            .count();

        assert_eq!(backup_count, 0);
    }

    #[test]
    fn test_schema_includes_embedding_tables() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Insert a chunk
        conn.execute(
            "INSERT INTO chunks (source_type, source_id, document_id, content_hash, text, created_at)
             VALUES ('transcript', 'doc1:w0', 'doc1', 'abc123', 'test text', '2025-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let chunk_id = conn.last_insert_rowid();

        // Insert an embedding
        conn.execute(
            "INSERT INTO embeddings (chunk_id, vector) VALUES (?1, X'00010203')",
            [chunk_id],
        )
        .unwrap();

        // Verify data
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_embedding_metadata_table() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Insert metadata
        conn.execute(
            "INSERT INTO embedding_metadata (key, value) VALUES ('model_name', 'test-model')",
            [],
        )
        .unwrap();

        let model: String = conn
            .query_row(
                "SELECT value FROM embedding_metadata WHERE key = 'model_name'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(model, "test-model");
    }

    #[test]
    fn test_transcript_sync_log_table_exists() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Verify transcript_sync_log table exists
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='transcript_sync_log'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Verify we can insert and query
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc1', 'Test Doc')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO transcript_sync_log (document_id, status, last_attempted_at)
             VALUES ('doc1', 'not_found', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let (status, attempts): (String, i64) = conn
            .query_row(
                "SELECT status, attempts FROM transcript_sync_log WHERE document_id = 'doc1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "not_found");
        assert_eq!(attempts, 1);
    }

    #[test]
    fn test_utterance_metadata_columns() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Insert a document first (foreign key constraint)
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc1', 'Test Doc')",
            [],
        )
        .unwrap();

        // Insert an utterance with source and is_final
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source, is_final)
             VALUES ('utt1', 'doc1', 'Hello world', 'microphone', 1)",
            [],
        )
        .unwrap();

        // Verify the data was stored correctly
        let (source, is_final): (Option<String>, Option<bool>) = conn
            .query_row(
                "SELECT source, is_final FROM transcript_utterances WHERE id = 'utt1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(source, Some("microphone".to_string()));
        assert_eq!(is_final, Some(true));

        // Insert another utterance with system source
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source, is_final)
             VALUES ('utt2', 'doc1', 'Response', 'system', 0)",
            [],
        )
        .unwrap();

        let (source, is_final): (Option<String>, Option<bool>) = conn
            .query_row(
                "SELECT source, is_final FROM transcript_utterances WHERE id = 'utt2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(source, Some("system".to_string()));
        assert_eq!(is_final, Some(false));
    }

    #[test]
    fn test_panels_tables_exist() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Verify panels table exists and accepts data
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc1', 'Test Doc')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO panels (id, document_id, title, content_markdown, template_slug, created_at)
             VALUES ('panel1', 'doc1', 'Summary', 'Key decisions were made.', 'meeting-notes', '2026-01-20T11:00:00Z')",
            [],
        )
        .unwrap();

        let (title, content): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT title, content_markdown FROM panels WHERE id = 'panel1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, Some("Summary".to_string()));
        assert_eq!(content, Some("Key decisions were made.".to_string()));

        // Verify index exists on document_id
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_panels_document_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);

        // Verify panel_sync_log table exists and accepts data
        conn.execute(
            "INSERT INTO panel_sync_log (document_id, status, last_attempted_at)
             VALUES ('doc1', 'not_found', '2026-01-20T12:00:00Z')",
            [],
        )
        .unwrap();

        let (status, attempts): (String, i64) = conn
            .query_row(
                "SELECT status, attempts FROM panel_sync_log WHERE document_id = 'doc1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "not_found");
        assert_eq!(attempts, 1);

        // Verify panels_fts virtual table exists and is searchable
        conn.execute(
            "INSERT INTO panels_fts(panels_fts) VALUES('rebuild')",
            [],
        )
        .unwrap();

        let fts_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM panels_fts WHERE panels_fts MATCH '\"decisions\"'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 1);
    }

    #[test]
    fn test_title_not_null_migration() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Insert a document with a title - should work fine
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc1', 'Test Doc')",
            [],
        )
        .unwrap();

        // Verify title can be retrieved as String (not Option<String>)
        let title: String = conn
            .query_row(
                "SELECT title FROM documents WHERE id = 'doc1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Test Doc");

        // Insert a document with empty string title - should work fine
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc2', '')",
            [],
        )
        .unwrap();

        let title: String = conn
            .query_row(
                "SELECT title FROM documents WHERE id = 'doc2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "");
    }

    #[test]
    fn test_transcript_utterance_index_exists() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_transcript_utterances_document'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }

    #[test]
    fn test_document_raw_json_column() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        conn.execute(
            "INSERT INTO documents (id, title, raw_json) VALUES ('doc1', 'Test Doc', '{\"id\":\"doc1\",\"title\":\"Test Doc\"}')",
            [],
        )
        .unwrap();

        let raw_json: Option<String> = conn
            .query_row(
                "SELECT raw_json FROM documents WHERE id = 'doc1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw_json, Some("{\"id\":\"doc1\",\"title\":\"Test Doc\"}".to_string()));

        // Verify NULL is allowed
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc2', 'No Raw')",
            [],
        )
        .unwrap();

        let raw_json: Option<String> = conn
            .query_row(
                "SELECT raw_json FROM documents WHERE id = 'doc2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(raw_json.is_none());
    }

    #[test]
    fn test_raw_json_templates_recipes_events_columns() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Templates: insert with raw_json
        conn.execute(
            "INSERT INTO templates (id, title, raw_json) VALUES ('t1', 'Test Template', '{\"id\":\"t1\",\"title\":\"Test Template\"}')",
            [],
        )
        .unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM templates WHERE id = 't1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw_json, Some("{\"id\":\"t1\",\"title\":\"Test Template\"}".to_string()));

        // Templates: NULL is allowed
        conn.execute("INSERT INTO templates (id, title) VALUES ('t2', 'No Raw')", []).unwrap();
        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM templates WHERE id = 't2'", [], |row| row.get(0))
            .unwrap();
        assert!(raw_json.is_none());

        // Recipes: insert with raw_json
        conn.execute(
            "INSERT INTO recipes (id, slug, raw_json) VALUES ('r1', 'test-recipe', '{\"id\":\"r1\",\"slug\":\"test-recipe\"}')",
            [],
        )
        .unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM recipes WHERE id = 'r1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw_json, Some("{\"id\":\"r1\",\"slug\":\"test-recipe\"}".to_string()));

        // Recipes: NULL is allowed
        conn.execute("INSERT INTO recipes (id, slug) VALUES ('r2', 'no-raw')", []).unwrap();
        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM recipes WHERE id = 'r2'", [], |row| row.get(0))
            .unwrap();
        assert!(raw_json.is_none());

        // Events: insert with raw_json
        conn.execute(
            "INSERT INTO events (id, summary, raw_json) VALUES ('e1', 'Test Event', '{\"id\":\"e1\",\"summary\":\"Test Event\"}')",
            [],
        )
        .unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM events WHERE id = 'e1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw_json, Some("{\"id\":\"e1\",\"summary\":\"Test Event\"}".to_string()));

        // Events: NULL is allowed
        conn.execute("INSERT INTO events (id, summary) VALUES ('e2', 'No Raw')", []).unwrap();
        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM events WHERE id = 'e2'", [], |row| row.get(0))
            .unwrap();
        assert!(raw_json.is_none());
    }

    #[test]
    fn test_calendars_primary_column_name() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // The column should be named "primary" (matching API and model),
        // not "is_primary" (the old name that was an unnecessary rename).
        // Regression test for #241.
        conn.execute(
            "INSERT INTO calendars (id, \"primary\") VALUES ('cal-test', 1)",
            [],
        )
        .unwrap();

        let is_primary: bool = conn
            .query_row(
                "SELECT \"primary\" FROM calendars WHERE id = 'cal-test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(is_primary);
    }

    #[test]
    fn test_api_snapshot_columns() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        // Insert test document and panel
        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc1', 'Test Doc')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO panels (id, document_id, title, api_snapshot)
             VALUES ('panel1', 'doc1', 'Summary', '{\"id\":\"panel1\",\"content\":\"[stored]\"}')",
            [],
        )
        .unwrap();

        let snapshot: Option<String> = conn
            .query_row(
                "SELECT api_snapshot FROM panels WHERE id = 'panel1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(snapshot.is_some());
        assert!(snapshot.unwrap().contains("[stored]"));

        // Test transcript_utterances api_snapshot column
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, api_snapshot)
             VALUES ('utt1', 'doc1', 'Hello', '{\"id\":\"utt1\",\"text\":\"[stored]\"}')",
            [],
        )
        .unwrap();

        let snapshot: Option<String> = conn
            .query_row(
                "SELECT api_snapshot FROM transcript_utterances WHERE id = 'utt1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(snapshot.is_some());
        assert!(snapshot.unwrap().contains("[stored]"));

        // Verify NULL is allowed
        conn.execute(
            "INSERT INTO panels (id, document_id, title)
             VALUES ('panel2', 'doc1', 'Notes')",
            [],
        )
        .unwrap();

        let snapshot: Option<String> = conn
            .query_row(
                "SELECT api_snapshot FROM panels WHERE id = 'panel2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(snapshot.is_none());
    }

    #[test]
    fn test_panel_chat_url_column() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = open_and_migrate(&db_path).unwrap();

        conn.execute(
            "INSERT INTO documents (id, title) VALUES ('doc1', 'Test Doc')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO panels (id, document_id, title, chat_url)
             VALUES ('panel1', 'doc1', 'Summary', 'https://notes.granola.ai/t/abc123')",
            [],
        )
        .unwrap();

        let chat_url: Option<String> = conn
            .query_row(
                "SELECT chat_url FROM panels WHERE id = 'panel1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chat_url, Some("https://notes.granola.ai/t/abc123".to_string()));

        // Verify NULL is allowed
        conn.execute(
            "INSERT INTO panels (id, document_id, title)
             VALUES ('panel2', 'doc1', 'Notes')",
            [],
        )
        .unwrap();

        let chat_url: Option<String> = conn
            .query_row(
                "SELECT chat_url FROM panels WHERE id = 'panel2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(chat_url.is_none());
    }
}
