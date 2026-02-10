//! Database schema management.
//!
//! Schema versioning is now handled by the migrations module using rusqlite_migration.
//! This module provides helper functions for test databases and schema inspection.

use anyhow::Result;
use rusqlite::Connection;

use crate::db::migrations;

/// Get the current schema version.
/// Delegates to the migrations module.
pub fn get_schema_version(conn: &Connection) -> Result<usize> {
    migrations::get_schema_version(conn)
}

/// Creates all tables in an in-memory database for testing.
/// Uses the same schema as the migration system.
#[cfg(test)]
pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("migrations/v001_initial_schema.sql"))?;
    conn.execute_batch(include_str!("migrations/v002_capture_missing_fields.sql"))?;
    conn.execute_batch(include_str!("migrations/v003_utterance_metadata.sql"))?;
    conn.execute_batch(include_str!("migrations/v004_make_title_not_null.sql"))?;
    conn.execute_batch(include_str!("migrations/v005_transcript_sync_log.sql"))?;
    conn.execute_batch(include_str!("migrations/v006_panels.sql"))?;
    conn.execute_batch(include_str!("migrations/v007_transcript_utterance_index.sql"))?;
    conn.execute_batch(include_str!("migrations/v008_panel_chat_url.sql"))?;
    conn.execute_batch(include_str!("migrations/v009_document_raw_json.sql"))?;
    conn.execute_batch(include_str!("migrations/v010_rename_audio_source_to_source.sql"))?;
    conn.execute_batch(include_str!("migrations/v011_raw_json_templates_recipes_events.sql"))?;
    conn.execute_batch(include_str!("migrations/v012_rename_is_primary_to_primary.sql"))?;
    conn.execute_batch(include_str!("migrations/v013_api_snapshot.sql"))?;
    Ok(())
}
