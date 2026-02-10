use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::models::TranscriptUtterance;
use crate::query::dates::DateRange;
use crate::query::search::ContextWindow;

/// Raw SQLite row for a transcript utterance.
pub(crate) struct TranscriptUtteranceRow {
    pub id: Option<String>,
    pub document_id: Option<String>,
    pub start_timestamp: Option<String>,
    pub end_timestamp: Option<String>,
    pub text: Option<String>,
    pub source: Option<String>,
    pub is_final: Option<bool>,
}

/// Convert a raw database row into a domain `TranscriptUtterance`.
///
/// The `extra` field is always empty for database-sourced utterances since
/// we don't store arbitrary extra fields for transcripts.
pub(crate) fn row_to_utterance(row: TranscriptUtteranceRow) -> TranscriptUtterance {
    TranscriptUtterance {
        id: row.id,
        document_id: row.document_id,
        start_timestamp: row.start_timestamp,
        end_timestamp: row.end_timestamp,
        text: row.text,
        source: row.source,
        is_final: row.is_final,
        extra: Default::default(),
    }
}

pub fn search_transcripts(
    conn: &Connection,
    query: &str,
    meeting: Option<&str>,
    context_size: usize,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<(String, Vec<ContextWindow>)>> {
    let matching_doc_ids = find_matching_documents(conn, query, meeting, date_range, include_deleted)?;

    let mut results = Vec::new();
    for (doc_id, doc_title) in &matching_doc_ids {
        let utterances = load_transcript(conn, doc_id)?;
        if utterances.is_empty() {
            continue;
        }

        let windows = build_context_windows(&utterances, query, context_size);
        if !windows.is_empty() {
            results.push((doc_title.clone(), windows));
        }
    }

    Ok(results)
}

fn find_matching_documents(
    conn: &Connection,
    query: &str,
    meeting: Option<&str>,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<(String, String)>> {
    let fts_query = sanitize_fts_query(query);
    let deleted_filter = if include_deleted { "" } else { " AND d.deleted_at IS NULL" };

    let mut sql = format!(
        "SELECT DISTINCT tu.document_id, COALESCE(d.title, '(untitled)')
         FROM transcript_fts
         JOIN transcript_utterances tu ON transcript_fts.rowid = tu.rowid
         JOIN documents d ON tu.document_id = d.id
         WHERE transcript_fts MATCH ?1{}",
        deleted_filter
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(fts_query)];

    if let Some(meeting_q) = meeting {
        sql.push_str(" AND (d.id = ?2 OR d.id LIKE ?3 OR d.title LIKE ?3 COLLATE NOCASE)");
        let pattern = format!("%{}%", meeting_q);
        params.push(Box::new(meeting_q.to_string()));
        params.push(Box::new(pattern));
    }

    if let Some(range) = date_range {
        if let Some(start) = &range.start {
            sql.push_str(" AND d.created_at >= ?");
            params.push(Box::new(start.to_rfc3339()));
        }
        if let Some(end) = &range.end {
            sql.push_str(" AND d.created_at < ?");
            params.push(Box::new(end.to_rfc3339()));
        }
    }

    sql.push_str(" ORDER BY d.created_at DESC");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Load all transcript utterances for a document, ordered by timestamp.
///
/// Note: `source` and `is_final` may be None for historical transcripts synced before
/// migration v003. These fields are only populated for transcripts fetched after that
/// migration was applied. See v003_utterance_metadata.sql for details.
pub fn load_transcript(conn: &Connection, document_id: &str) -> Result<Vec<TranscriptUtterance>> {
    let mut stmt = conn.prepare(
        "SELECT id, document_id, start_timestamp, end_timestamp, text, source, is_final FROM transcript_utterances WHERE document_id = ?1 ORDER BY start_timestamp",
    )?;

    let rows = stmt.query_map([document_id], |row| {
        Ok(TranscriptUtteranceRow {
            id: row.get(0)?,
            document_id: row.get(1)?,
            start_timestamp: row.get(2)?,
            end_timestamp: row.get(3)?,
            text: row.get(4)?,
            source: row.get(5)?,
            is_final: row.get(6)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_utterance).collect())
}

fn build_context_windows(
    utterances: &[TranscriptUtterance],
    query: &str,
    context_size: usize,
) -> Vec<ContextWindow> {
    use crate::query::search::contains_ignore_case;

    let mut results = Vec::new();

    for (i, utt) in utterances.iter().enumerate() {
        let text = match &utt.text {
            Some(t) => t,
            None => continue,
        };

        if !contains_ignore_case(text, query) {
            continue;
        }

        let before_start = i.saturating_sub(context_size);
        let after_end = (i + 1 + context_size).min(utterances.len());

        let before: Vec<TranscriptUtterance> = utterances[before_start..i]
            .iter()
            .cloned()
            .collect();
        let matched = utt.clone();
        let after: Vec<TranscriptUtterance> = utterances[i + 1..after_end]
            .iter()
            .cloned()
            .collect();

        results.push(ContextWindow {
            before,
            matched,
            after,
        });
    }

    results
}

fn sanitize_fts_query(query: &str) -> String {
    format!("\"{}\"", query.replace('"', ""))
}

/// Build a context window from semantic search result indices.
///
/// This is used to display context around semantic search matches. The window_start_idx
/// and window_end_idx represent the range of utterances that were embedded together.
/// We center on the middle utterance of that window and add additional_context
/// utterances before and after.
pub fn build_context_window_from_indices(
    utterances: &[TranscriptUtterance],
    window_start_idx: usize,
    window_end_idx: usize,
    additional_context: usize,
) -> Option<ContextWindow> {
    if utterances.is_empty() {
        return None;
    }

    // Clamp indices to valid range
    let start = window_start_idx.min(utterances.len().saturating_sub(1));
    let end = window_end_idx.min(utterances.len().saturating_sub(1));

    // Find the center of the matched window
    let center_idx = (start + end) / 2;

    // Calculate context bounds
    let before_start = center_idx.saturating_sub(additional_context);
    let after_end = (center_idx + 1 + additional_context).min(utterances.len());

    let before: Vec<TranscriptUtterance> = utterances[before_start..center_idx]
        .iter()
        .cloned()
        .collect();
    let matched = utterances[center_idx].clone();
    let after: Vec<TranscriptUtterance> = utterances[center_idx + 1..after_end]
        .iter()
        .cloned()
        .collect();

    Some(ContextWindow {
        before,
        matched,
        after,
    })
}

/// Document info for transcript sync
#[derive(Debug, Clone, Serialize)]
pub struct DocumentWithoutTranscript {
    pub id: String,
    pub title: Option<String>,
    pub created_at: Option<String>,
}

/// Find documents that need transcript syncing.
///
/// Returns documents that either have no transcripts at all, or have transcripts
/// where every utterance has NULL `source` (indicating they were synced before
/// the source column was populated).
///
/// When `skip_logged_failures` is true, documents with entries in `transcript_sync_log`
/// are excluded from the results. Pass `false` (retry mode) to include them.
pub fn find_documents_without_transcripts(
    conn: &Connection,
    since: Option<&str>,
    limit: Option<usize>,
    skip_logged_failures: bool,
) -> Result<Vec<DocumentWithoutTranscript>> {
    let mut sql = String::from(
        "SELECT d.id, d.title, d.created_at
         FROM documents d
         WHERE d.deleted_at IS NULL
           AND (
               NOT EXISTS (
                   SELECT 1 FROM transcript_utterances t WHERE t.document_id = d.id
               )
               OR NOT EXISTS (
                   SELECT 1 FROM transcript_utterances t
                   WHERE t.document_id = d.id AND t.source IS NOT NULL
               )
           )",
    );

    if skip_logged_failures {
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM transcript_sync_log l WHERE l.document_id = d.id)",
        );
    }

    if since.is_some() {
        sql.push_str(" AND d.created_at >= ?1");
    }

    sql.push_str(" ORDER BY d.created_at DESC");

    if let Some(n) = limit {
        sql.push_str(&format!(" LIMIT {}", n));
    }

    let mut stmt = conn.prepare(&sql)?;

    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<DocumentWithoutTranscript> {
        Ok(DocumentWithoutTranscript {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
        })
    };

    let results: Vec<DocumentWithoutTranscript> = if let Some(since_date) = since {
        stmt.query_map([since_date], map_row)?
            .filter_map(|r| r.ok())
            .collect()
    } else {
        stmt.query_map([], map_row)?
            .filter_map(|r| r.ok())
            .collect()
    };

    Ok(results)
}

/// Record a transcript sync failure for a document.
///
/// On conflict (document already logged), updates the status, timestamp, and
/// increments the attempt counter.
pub fn log_transcript_sync_failure(
    conn: &Connection,
    document_id: &str,
    status: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO transcript_sync_log (document_id, status, last_attempted_at, attempts)
         VALUES (?1, ?2, ?3, 1)
         ON CONFLICT(document_id) DO UPDATE SET
             status = excluded.status,
             last_attempted_at = excluded.last_attempted_at,
             attempts = transcript_sync_log.attempts + 1",
        rusqlite::params![document_id, status, now],
    )?;
    Ok(())
}

/// Remove a transcript sync log entry for a document.
///
/// Called when a retry succeeds so the document won't be skipped in future syncs.
pub fn clear_transcript_sync_log_entry(conn: &Connection, document_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM transcript_sync_log WHERE document_id = ?1",
        [document_id],
    )?;
    Ok(())
}

/// Count how many documents have logged transcript sync failures.
///
/// When `since` is provided, only counts failures for documents created on or after
/// that date, matching the filter used by `find_documents_without_transcripts`.
pub fn count_transcript_sync_failures(conn: &Connection, since: Option<&str>) -> Result<usize> {
    let count: i64 = if let Some(since_date) = since {
        conn.query_row(
            "SELECT COUNT(*) FROM transcript_sync_log l
             JOIN documents d ON d.id = l.document_id
             WHERE d.deleted_at IS NULL AND d.created_at >= ?1",
            [since_date],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM transcript_sync_log l
             JOIN documents d ON d.id = l.document_id
             WHERE d.deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?
    };
    Ok(count as usize)
}

/// Build a redacted JSON snapshot of a transcript utterance.
///
/// Serializes the utterance, then replaces the `text` field with `"[stored]"`
/// since it's already stored in a dedicated column.
/// Returns `None` if serialization fails.
fn redact_utterance_snapshot(utt: &crate::models::TranscriptUtterance) -> Option<String> {
    let mut value = serde_json::to_value(utt).ok()?;
    let obj = value.as_object_mut()?;
    if let Some(v) = obj.get("text") {
        if !v.is_null() {
            obj.insert("text".to_string(), serde_json::Value::String("[stored]".to_string()));
        }
    }
    serde_json::to_string(&value).ok()
}

/// Insert a transcript fetched from the API
/// This will delete any existing transcript for the document and insert the new one
pub fn insert_transcript_from_api(
    conn: &Connection,
    document_id: &str,
    utterances: &[crate::models::TranscriptUtterance],
) -> Result<usize> {
    // Check if document exists
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ?1)",
            [document_id],
            |row| row.get(0),
        )
        .context("Failed to check document existence")?;

    if !exists {
        anyhow::bail!("Document {} not found in database", document_id);
    }

    // Delete from FTS index first (before deleting from source table)
    // The FTS table is content-synced, so we need to handle this specially
    conn.execute(
        "DELETE FROM transcript_fts WHERE rowid IN (
            SELECT rowid FROM transcript_utterances WHERE document_id = ?1
        )",
        [document_id],
    ).ok(); // Ignore errors if already deleted

    // Delete existing transcripts for this document
    conn.execute(
        "DELETE FROM transcript_utterances WHERE document_id = ?1",
        [document_id],
    )?;

    // Insert new utterances with 'api' source
    let mut stmt = conn.prepare(
        "INSERT INTO transcript_utterances (id, document_id, start_timestamp, end_timestamp, text, transcript_source, source, is_final, api_snapshot)
         VALUES (?1, ?2, ?3, ?4, ?5, 'api', ?6, ?7, ?8)",
    )?;

    let mut inserted = 0;
    for utt in utterances {
        let Some(_utt_id) = utt.id.as_deref() else {
            eprintln!("Warning: skipping utterance without ID");
            continue;
        };

        let api_snapshot = redact_utterance_snapshot(utt);

        stmt.execute(rusqlite::params![
            &utt.id,
            document_id,
            &utt.start_timestamp,
            &utt.end_timestamp,
            &utt.text,
            utt.source.as_deref(),
            utt.is_final,
            &api_snapshot,
        ])?;
        inserted += 1;
    }

    // Update FTS index for the new utterances
    conn.execute(
        "INSERT INTO transcript_fts(rowid, text)
         SELECT rowid, text FROM transcript_utterances WHERE document_id = ?1",
        [document_id],
    )?;

    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, transcripts_state};

    #[test]
    fn test_search_transcripts_basic() {
        let conn = build_test_db(&transcripts_state());
        let results = search_transcripts(&conn, "neural networks", None, 1, None, false).unwrap();
        assert_eq!(results.len(), 1);
        let (title, windows) = &results[0];
        assert_eq!(title, "AI Meeting");
        assert_eq!(windows.len(), 1);
        assert_eq!(
            windows[0].matched.text.as_deref(),
            Some("Let's talk about neural networks today")
        );
    }

    #[test]
    fn test_search_transcripts_context() {
        let conn = build_test_db(&transcripts_state());
        let results = search_transcripts(&conn, "neural networks", None, 1, None, false).unwrap();
        let (_, windows) = &results[0];
        assert_eq!(windows[0].before.len(), 1);
        assert_eq!(
            windows[0].before[0].text.as_deref(),
            Some("Hello everyone")
        );
        assert_eq!(windows[0].after.len(), 1);
        assert_eq!(windows[0].after[0].text.as_deref(), Some("Great idea"));
    }

    #[test]
    fn test_search_transcripts_no_match() {
        let conn = build_test_db(&transcripts_state());
        let results =
            search_transcripts(&conn, "quantum computing", None, 1, None, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_transcripts_filter_by_meeting() {
        let conn = build_test_db(&transcripts_state());
        // "neural" appears in both doc-1 and doc-2, but filter by meeting
        let results =
            search_transcripts(&conn, "neural", Some("AI Meeting"), 0, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "AI Meeting");
    }

    #[test]
    fn test_search_transcripts_with_date_range() {
        let conn = build_test_db(&transcripts_state());
        use chrono::{TimeZone, Utc};
        let range = DateRange {
            start: Some(Utc.with_ymd_and_hms(2026, 1, 21, 0, 0, 0).unwrap()),
            end: Some(Utc.with_ymd_and_hms(2026, 1, 22, 0, 0, 0).unwrap()),
        };
        let results =
            search_transcripts(&conn, "neural", None, 0, Some(&range), false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "Other Meeting");
    }

    #[test]
    fn test_find_documents_without_transcripts() {
        let conn = build_test_db(&transcripts_state());
        // transcripts_state has doc-1 and doc-2, both with transcripts
        let docs = find_documents_without_transcripts(&conn, None, None, false).unwrap();
        assert!(docs.is_empty());

        // Add a document without transcripts
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-3', 'No Transcript', '2026-01-22T10:00:00Z')",
            [],
        ).unwrap();

        let docs = find_documents_without_transcripts(&conn, None, None, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc-3");
    }

    #[test]
    fn test_find_documents_without_transcripts_with_since() {
        let conn = build_test_db(&transcripts_state());

        // Add documents without transcripts at different dates
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-old', 'Old Doc', '2026-01-10T10:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-new', 'New Doc', '2026-01-25T10:00:00Z')",
            [],
        ).unwrap();

        let docs = find_documents_without_transcripts(&conn, Some("2026-01-20T00:00:00Z"), None, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc-new");
    }

    #[test]
    fn test_find_documents_without_transcripts_with_limit() {
        let conn = build_test_db(&transcripts_state());

        // Add multiple documents without transcripts
        for i in 3..8 {
            conn.execute(
                &format!("INSERT INTO documents (id, title, created_at) VALUES ('doc-{}', 'Doc {}', '2026-01-{}T10:00:00Z')", i, i, 20 + i),
                [],
            ).unwrap();
        }

        let docs = find_documents_without_transcripts(&conn, None, Some(2), false).unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_insert_transcript_from_api() {
        let conn = build_test_db(&transcripts_state());

        // Add a document without transcript
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-api', 'API Test', '2026-01-25T10:00:00Z')",
            [],
        ).unwrap();

        let utterances = vec![
            crate::models::TranscriptUtterance {
                id: Some("api-u1".to_string()),
                document_id: Some("doc-api".to_string()),
                start_timestamp: Some("2026-01-25T10:00:00Z".to_string()),
                end_timestamp: Some("2026-01-25T10:00:30Z".to_string()),
                text: Some("Hello from API".to_string()),
                source: None,
                is_final: None,
                extra: Default::default(),
            },
            crate::models::TranscriptUtterance {
                id: Some("api-u2".to_string()),
                document_id: Some("doc-api".to_string()),
                start_timestamp: Some("2026-01-25T10:00:30Z".to_string()),
                end_timestamp: Some("2026-01-25T10:01:00Z".to_string()),
                text: Some("Second utterance".to_string()),
                source: None,
                is_final: None,
                extra: Default::default(),
            },
        ];

        let inserted = insert_transcript_from_api(&conn, "doc-api", &utterances).unwrap();
        assert_eq!(inserted, 2);

        // Verify insertion
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcript_utterances WHERE document_id = 'doc-api'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 2);

        // Verify source is 'api'
        let source: String = conn.query_row(
            "SELECT transcript_source FROM transcript_utterances WHERE id = 'api-u1'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(source, "api");
    }

    #[test]
    fn test_insert_transcript_from_api_replaces_existing() {
        let conn = build_test_db(&transcripts_state());

        // doc-1 already has transcripts from transcripts_state
        let old_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcript_utterances WHERE document_id = 'doc-1'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert!(old_count > 0);

        let utterances = vec![
            crate::models::TranscriptUtterance {
                id: Some("new-u1".to_string()),
                document_id: Some("doc-1".to_string()),
                start_timestamp: None,
                end_timestamp: None,
                text: Some("Replaced transcript".to_string()),
                source: None,
                is_final: None,
                extra: Default::default(),
            },
        ];

        let inserted = insert_transcript_from_api(&conn, "doc-1", &utterances).unwrap();
        assert_eq!(inserted, 1);

        // Verify only the new transcript exists
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcript_utterances WHERE document_id = 'doc-1'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_insert_transcript_from_api_updates_fts_index() {
        let conn = build_test_db(&transcripts_state());

        // transcripts_state has doc-1 with text "neural networks" - verify it's searchable
        let old_search = search_transcripts(&conn, "neural networks", None, 0, None, false).unwrap();
        assert_eq!(old_search.len(), 1, "Should find 'neural networks' before replacement");

        // Replace with completely different text
        let utterances = vec![
            crate::models::TranscriptUtterance {
                id: Some("new-u1".to_string()),
                document_id: Some("doc-1".to_string()),
                start_timestamp: None,
                end_timestamp: None,
                text: Some("quantum computing breakthroughs".to_string()),
                source: None,
                is_final: None,
                extra: Default::default(),
            },
        ];

        insert_transcript_from_api(&conn, "doc-1", &utterances).unwrap();

        // OLD text should NOT be searchable anymore (FTS index should be cleaned)
        let old_search_after = search_transcripts(&conn, "neural networks", Some("doc-1"), 0, None, false).unwrap();
        assert!(
            old_search_after.is_empty(),
            "Should NOT find 'neural networks' after replacement - FTS index has stale entries"
        );

        // NEW text SHOULD be searchable
        let new_search = search_transcripts(&conn, "quantum computing", None, 0, None, false).unwrap();
        assert_eq!(new_search.len(), 1, "Should find 'quantum computing' after replacement");
        assert_eq!(new_search[0].0, "AI Meeting");
    }

    #[test]
    fn test_insert_transcript_from_api_nonexistent_document() {
        let conn = build_test_db(&transcripts_state());

        let utterances = vec![
            crate::models::TranscriptUtterance {
                id: Some("u1".to_string()),
                document_id: Some("nonexistent".to_string()),
                start_timestamp: None,
                end_timestamp: None,
                text: Some("test".to_string()),
                source: None,
                is_final: None,
                extra: Default::default(),
            },
        ];

        let result = insert_transcript_from_api(&conn, "nonexistent", &utterances);
        assert!(result.is_err());
    }

    #[test]
    fn test_insert_transcript_from_api_with_source_and_is_final() {
        let conn = build_test_db(&transcripts_state());

        // Add a document without transcript
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-metadata', 'Metadata Test', '2026-01-25T10:00:00Z')",
            [],
        ).unwrap();

        let utterances = vec![
            crate::models::TranscriptUtterance {
                id: Some("meta-u1".to_string()),
                document_id: Some("doc-metadata".to_string()),
                start_timestamp: Some("2026-01-25T10:00:00Z".to_string()),
                end_timestamp: Some("2026-01-25T10:00:30Z".to_string()),
                text: Some("Hello from me".to_string()),
                source: Some("microphone".to_string()),
                is_final: Some(true),
                extra: Default::default(),
            },
            crate::models::TranscriptUtterance {
                id: Some("meta-u2".to_string()),
                document_id: Some("doc-metadata".to_string()),
                start_timestamp: Some("2026-01-25T10:00:30Z".to_string()),
                end_timestamp: Some("2026-01-25T10:01:00Z".to_string()),
                text: Some("Response from others".to_string()),
                source: Some("system".to_string()),
                is_final: Some(false),
                extra: Default::default(),
            },
        ];

        let inserted = insert_transcript_from_api(&conn, "doc-metadata", &utterances).unwrap();
        assert_eq!(inserted, 2);

        // Verify the metadata was stored correctly
        let (source1, is_final1): (Option<String>, Option<bool>) = conn.query_row(
            "SELECT source, is_final FROM transcript_utterances WHERE id = 'meta-u1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(source1, Some("microphone".to_string()));
        assert_eq!(is_final1, Some(true));

        let (source2, is_final2): (Option<String>, Option<bool>) = conn.query_row(
            "SELECT source, is_final FROM transcript_utterances WHERE id = 'meta-u2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(source2, Some("system".to_string()));
        assert_eq!(is_final2, Some(false));
    }

    #[test]
    fn test_load_transcript_includes_source_and_is_final() {
        let conn = build_test_db(&transcripts_state());

        // Add a document and insert transcript with metadata directly
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-load', 'Load Test', '2026-01-25T10:00:00Z')",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source, is_final, transcript_source)
             VALUES ('load-u1', 'doc-load', 'My utterance', 'microphone', 1, 'api')",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source, is_final, transcript_source)
             VALUES ('load-u2', 'doc-load', 'Their utterance', 'system', 0, 'api')",
            [],
        ).unwrap();

        // Load the transcript and verify the metadata is included
        let utterances = load_transcript(&conn, "doc-load").unwrap();
        assert_eq!(utterances.len(), 2);

        assert_eq!(utterances[0].source, Some("microphone".to_string()));
        assert_eq!(utterances[0].is_final, Some(true));

        assert_eq!(utterances[1].source, Some("system".to_string()));
        assert_eq!(utterances[1].is_final, Some(false));
    }

    #[test]
    fn test_build_context_window_from_indices_basic() {
        use crate::models::TranscriptUtterance;

        let utterances: Vec<TranscriptUtterance> = (0..10)
            .map(|i| TranscriptUtterance {
                id: Some(format!("u{}", i)),
                document_id: Some("doc1".to_string()),
                start_timestamp: Some(format!("2026-01-01T10:{:02}:00Z", i)),
                end_timestamp: None,
                text: Some(format!("Utterance {}", i)),
                source: None,
                is_final: None,
                extra: Default::default(),
            })
            .collect();

        // Window from index 3 to 6, additional context of 2
        // Center is (3+6)/2 = 4
        // Before: 2, 3 (idx 4 - 2 to idx 4)
        // Matched: 4
        // After: 5, 6 (idx 5 to idx 4 + 1 + 2 = 7)
        let window = build_context_window_from_indices(&utterances, 3, 6, 2).unwrap();

        assert_eq!(window.before.len(), 2);
        assert_eq!(window.before[0].text.as_deref(), Some("Utterance 2"));
        assert_eq!(window.before[1].text.as_deref(), Some("Utterance 3"));
        assert_eq!(window.matched.text.as_deref(), Some("Utterance 4"));
        assert_eq!(window.after.len(), 2);
        assert_eq!(window.after[0].text.as_deref(), Some("Utterance 5"));
        assert_eq!(window.after[1].text.as_deref(), Some("Utterance 6"));
    }

    #[test]
    fn test_build_context_window_from_indices_at_start() {
        use crate::models::TranscriptUtterance;

        let utterances: Vec<TranscriptUtterance> = (0..5)
            .map(|i| TranscriptUtterance {
                id: Some(format!("u{}", i)),
                document_id: Some("doc1".to_string()),
                start_timestamp: None,
                end_timestamp: None,
                text: Some(format!("Utterance {}", i)),
                source: None,
                is_final: None,
                extra: Default::default(),
            })
            .collect();

        // Window at start, center = 0, context = 2
        let window = build_context_window_from_indices(&utterances, 0, 1, 2).unwrap();

        // Center = (0+1)/2 = 0, so before is empty
        assert!(window.before.is_empty());
        assert_eq!(window.matched.text.as_deref(), Some("Utterance 0"));
        assert_eq!(window.after.len(), 2);
    }

    #[test]
    fn test_build_context_window_from_indices_at_end() {
        use crate::models::TranscriptUtterance;

        let utterances: Vec<TranscriptUtterance> = (0..5)
            .map(|i| TranscriptUtterance {
                id: Some(format!("u{}", i)),
                document_id: Some("doc1".to_string()),
                start_timestamp: None,
                end_timestamp: None,
                text: Some(format!("Utterance {}", i)),
                source: None,
                is_final: None,
                extra: Default::default(),
            })
            .collect();

        // Window at end, center = 4, context = 2
        let window = build_context_window_from_indices(&utterances, 3, 4, 2).unwrap();

        // Center = (3+4)/2 = 3
        assert_eq!(window.before.len(), 2);
        assert_eq!(window.matched.text.as_deref(), Some("Utterance 3"));
        assert_eq!(window.after.len(), 1); // Only utterance 4 is after
    }

    #[test]
    fn test_build_context_window_from_indices_empty() {
        let utterances: Vec<TranscriptUtterance> = vec![];
        let result = build_context_window_from_indices(&utterances, 0, 0, 2);
        assert!(result.is_none());
    }

    #[test]
    fn test_log_transcript_sync_failure_insert() {
        let conn = build_test_db(&transcripts_state());

        // Add a document without transcripts
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-3', 'No Transcript', '2026-01-22T10:00:00Z')",
            [],
        ).unwrap();

        log_transcript_sync_failure(&conn, "doc-3", "not_found").unwrap();

        let (status, attempts): (String, i64) = conn.query_row(
            "SELECT status, attempts FROM transcript_sync_log WHERE document_id = 'doc-3'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();

        assert_eq!(status, "not_found");
        assert_eq!(attempts, 1);
    }

    #[test]
    fn test_log_transcript_sync_failure_upsert_increments_attempts() {
        let conn = build_test_db(&transcripts_state());

        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-3', 'No Transcript', '2026-01-22T10:00:00Z')",
            [],
        ).unwrap();

        log_transcript_sync_failure(&conn, "doc-3", "not_found").unwrap();
        log_transcript_sync_failure(&conn, "doc-3", "error").unwrap();

        let (status, attempts): (String, i64) = conn.query_row(
            "SELECT status, attempts FROM transcript_sync_log WHERE document_id = 'doc-3'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();

        assert_eq!(status, "error");
        assert_eq!(attempts, 2);
    }

    #[test]
    fn test_clear_transcript_sync_log_entry() {
        let conn = build_test_db(&transcripts_state());

        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-3', 'No Transcript', '2026-01-22T10:00:00Z')",
            [],
        ).unwrap();

        log_transcript_sync_failure(&conn, "doc-3", "not_found").unwrap();

        // Verify it exists
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcript_sync_log WHERE document_id = 'doc-3'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1);

        clear_transcript_sync_log_entry(&conn, "doc-3").unwrap();

        // Verify it's gone
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcript_sync_log WHERE document_id = 'doc-3'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_clear_transcript_sync_log_entry_nonexistent_is_ok() {
        let conn = build_test_db(&transcripts_state());
        // Clearing a non-existent entry should not error
        clear_transcript_sync_log_entry(&conn, "doc-nonexistent").unwrap();
    }

    #[test]
    fn test_find_documents_without_transcripts_skips_logged_failures() {
        let conn = build_test_db(&transcripts_state());

        // Add documents without transcripts
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-3', 'Doc Three', '2026-01-22T10:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-4', 'Doc Four', '2026-01-23T10:00:00Z')",
            [],
        ).unwrap();

        // Log a failure for doc-3
        log_transcript_sync_failure(&conn, "doc-3", "not_found").unwrap();

        // With skip_logged_failures=true, doc-3 should be excluded
        let docs = find_documents_without_transcripts(&conn, None, None, true).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc-4");
    }

    #[test]
    fn test_find_documents_without_transcripts_includes_logged_failures_on_retry() {
        let conn = build_test_db(&transcripts_state());

        // Add documents without transcripts
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-3', 'Doc Three', '2026-01-22T10:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-4', 'Doc Four', '2026-01-23T10:00:00Z')",
            [],
        ).unwrap();

        // Log a failure for doc-3
        log_transcript_sync_failure(&conn, "doc-3", "not_found").unwrap();

        // With skip_logged_failures=false (retry mode), doc-3 should be included
        let docs = find_documents_without_transcripts(&conn, None, None, false).unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_count_transcript_sync_failures() {
        let conn = build_test_db(&transcripts_state());

        // Add documents and log failures
        for i in 3..6 {
            conn.execute(
                &format!("INSERT INTO documents (id, title, created_at) VALUES ('doc-{}', 'Doc {}', '2026-01-{}T10:00:00Z')", i, i, 20 + i),
                [],
            ).unwrap();
            log_transcript_sync_failure(&conn, &format!("doc-{}", i), "not_found").unwrap();
        }

        assert_eq!(count_transcript_sync_failures(&conn, None).unwrap(), 3);
    }

    #[test]
    fn test_count_transcript_sync_failures_with_since() {
        let conn = build_test_db(&transcripts_state());

        // doc-3 created 2026-01-23, doc-4 created 2026-01-24, doc-5 created 2026-01-25
        for i in 3..6 {
            conn.execute(
                &format!("INSERT INTO documents (id, title, created_at) VALUES ('doc-{}', 'Doc {}', '2026-01-{}T10:00:00Z')", i, i, 20 + i),
                [],
            ).unwrap();
            log_transcript_sync_failure(&conn, &format!("doc-{}", i), "not_found").unwrap();
        }

        // Only count failures for documents created since 2026-01-24
        assert_eq!(count_transcript_sync_failures(&conn, Some("2026-01-24T00:00:00Z")).unwrap(), 2);
    }

    #[test]
    fn test_build_context_window_from_indices_out_of_bounds() {
        use crate::models::TranscriptUtterance;

        let utterances: Vec<TranscriptUtterance> = (0..3)
            .map(|i| TranscriptUtterance {
                id: Some(format!("u{}", i)),
                document_id: Some("doc1".to_string()),
                start_timestamp: None,
                end_timestamp: None,
                text: Some(format!("Utterance {}", i)),
                source: None,
                is_final: None,
                extra: Default::default(),
            })
            .collect();

        // Out of bounds indices should be clamped
        let window = build_context_window_from_indices(&utterances, 100, 200, 1).unwrap();

        // Center should be clamped to last valid index (2)
        assert_eq!(window.matched.text.as_deref(), Some("Utterance 2"));
    }

    #[test]
    fn test_find_documents_without_transcripts_includes_null_source() {
        let conn = build_test_db(&transcripts_state());

        // Add a document with transcripts that have NULL source (pre-migration data)
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-premigration', 'Old Sync', '2026-01-15T10:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source, is_final) VALUES ('u-old1', 'doc-premigration', 'Hello', NULL, NULL)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source, is_final) VALUES ('u-old2', 'doc-premigration', 'World', NULL, NULL)",
            [],
        ).unwrap();

        // This document should be returned because all its utterances have NULL source
        let docs = find_documents_without_transcripts(&conn, None, None, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc-premigration");
    }

    #[test]
    fn test_find_documents_without_transcripts_excludes_complete_transcripts() {
        let conn = build_test_db(&transcripts_state());

        // transcripts_state already has doc-1 and doc-2 with source values populated
        // They should NOT appear in the results
        let docs = find_documents_without_transcripts(&conn, None, None, false).unwrap();
        assert!(docs.is_empty());
    }

    // --- redact_utterance_snapshot tests ---

    #[test]
    fn test_redact_utterance_snapshot_replaces_text() {
        let utt = crate::models::TranscriptUtterance {
            id: Some("u1".to_string()),
            document_id: Some("doc-1".to_string()),
            start_timestamp: Some("2026-01-20T10:00:00Z".to_string()),
            end_timestamp: Some("2026-01-20T10:00:30Z".to_string()),
            text: Some("Hello everyone, welcome to the meeting.".to_string()),
            source: Some("microphone".to_string()),
            is_final: Some(true),
            extra: Default::default(),
        };

        let snapshot = redact_utterance_snapshot(&utt).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&snapshot).unwrap();

        assert_eq!(parsed["text"], "[stored]");
        assert_eq!(parsed["id"], "u1");
        assert_eq!(parsed["source"], "microphone");
        assert_eq!(parsed["is_final"], true);
        assert_eq!(parsed["start_timestamp"], "2026-01-20T10:00:00Z");
    }

    #[test]
    fn test_redact_utterance_snapshot_preserves_absent_text() {
        let utt = crate::models::TranscriptUtterance {
            id: Some("u1".to_string()),
            document_id: None,
            start_timestamp: None,
            end_timestamp: None,
            text: None,
            source: None,
            is_final: None,
            extra: Default::default(),
        };

        let snapshot = redact_utterance_snapshot(&utt).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&snapshot).unwrap();

        assert_eq!(parsed["id"], "u1");
        // None text serializes as null, not "[stored]"
        assert!(parsed["text"].is_null());
    }

    // --- api_snapshot integration tests ---

    #[test]
    fn test_insert_transcript_from_api_stores_snapshot() {
        let conn = build_test_db(&transcripts_state());

        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-snap', 'Snapshot Test', '2026-01-25T10:00:00Z')",
            [],
        ).unwrap();

        let utterances = vec![
            crate::models::TranscriptUtterance {
                id: Some("snap-u1".to_string()),
                document_id: Some("doc-snap".to_string()),
                start_timestamp: Some("2026-01-25T10:00:00Z".to_string()),
                end_timestamp: Some("2026-01-25T10:00:30Z".to_string()),
                text: Some("Hello from snapshot test".to_string()),
                source: Some("microphone".to_string()),
                is_final: Some(true),
                extra: Default::default(),
            },
        ];

        insert_transcript_from_api(&conn, "doc-snap", &utterances).unwrap();

        let snapshot: Option<String> = conn.query_row(
            "SELECT api_snapshot FROM transcript_utterances WHERE id = 'snap-u1'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(snapshot.is_some());

        let parsed: serde_json::Value = serde_json::from_str(&snapshot.unwrap()).unwrap();
        assert_eq!(parsed["text"], "[stored]");
        assert_eq!(parsed["id"], "snap-u1");
        assert_eq!(parsed["source"], "microphone");
    }

    #[test]
    fn test_find_documents_without_transcripts_mixed_source_not_resynced() {
        let conn = build_test_db(&transcripts_state());

        // Add a document where some utterances have source and some don't.
        // This is a partially-populated document — should NOT be re-synced,
        // since the mix indicates it was synced after the migration.
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-mixed', 'Mixed Doc', '2026-01-18T10:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source) VALUES ('u-m1', 'doc-mixed', 'Hello', 'microphone')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source) VALUES ('u-m2', 'doc-mixed', 'World', NULL)",
            [],
        ).unwrap();

        let docs = find_documents_without_transcripts(&conn, None, None, false).unwrap();
        assert!(docs.is_empty());
    }

    #[test]
    fn test_row_to_utterance_all_fields() {
        let row = TranscriptUtteranceRow {
            id: Some("u1".to_string()),
            document_id: Some("doc-1".to_string()),
            start_timestamp: Some("2026-01-20T10:00:00Z".to_string()),
            end_timestamp: Some("2026-01-20T10:01:00Z".to_string()),
            text: Some("Hello world".to_string()),
            source: Some("microphone".to_string()),
            is_final: Some(true),
        };
        let utt = row_to_utterance(row);
        assert_eq!(utt.id.as_deref(), Some("u1"));
        assert_eq!(utt.document_id.as_deref(), Some("doc-1"));
        assert_eq!(utt.start_timestamp.as_deref(), Some("2026-01-20T10:00:00Z"));
        assert_eq!(utt.end_timestamp.as_deref(), Some("2026-01-20T10:01:00Z"));
        assert_eq!(utt.text.as_deref(), Some("Hello world"));
        assert_eq!(utt.source.as_deref(), Some("microphone"));
        assert_eq!(utt.is_final, Some(true));
        assert!(utt.extra.is_empty());
    }

    #[test]
    fn test_row_to_utterance_none_fields() {
        let row = TranscriptUtteranceRow {
            id: None,
            document_id: None,
            start_timestamp: None,
            end_timestamp: None,
            text: None,
            source: None,
            is_final: None,
        };
        let utt = row_to_utterance(row);
        assert!(utt.id.is_none());
        assert!(utt.source.is_none());
        assert!(utt.is_final.is_none());
        assert!(utt.extra.is_empty());
    }
}
