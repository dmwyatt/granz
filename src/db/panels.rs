use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::api::ApiPanel;
use crate::models::Panel;
use crate::query::dates::DateRange;
use crate::tiptap::{extract_chat_url, tiptap_to_markdown};

// ============================================================================
// Read path: SQLite row → Panel
// ============================================================================

struct PanelRow {
    id: Option<String>,
    document_id: Option<String>,
    title: Option<String>,
    content_markdown: Option<String>,
    content_json: Option<String>,
    template_slug: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    deleted_at: Option<String>,
    chat_url: Option<String>,
}

fn row_to_panel(row: PanelRow) -> Panel {
    Panel {
        id: row.id,
        document_id: row.document_id,
        title: row.title,
        content_markdown: row.content_markdown,
        content_json: row.content_json,
        template_slug: row.template_slug,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deleted_at: row.deleted_at,
        chat_url: row.chat_url,
        extra: Default::default(),
    }
}

// ============================================================================
// Write path: ApiPanel → INSERT column values
// ============================================================================

struct PanelWriteRow {
    id: Option<String>,
    title: Option<String>,
    content_json: Option<String>,
    content_markdown: Option<String>,
    original_content_json: Option<String>,
    template_slug: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    deleted_at: Option<String>,
    extra_json: Option<String>,
    chat_url: Option<String>,
    api_snapshot: Option<String>,
}

/// Keys to redact from the panel API snapshot.
///
/// These are bulk content fields already stored in dedicated columns.
/// `content` and `original_content` are explicit `ApiPanel` fields;
/// `generated_lines` and `suggested_questions` land in the `extra` HashMap.
const PANEL_REDACT_KEYS: &[&str] = &[
    "content",
    "original_content",
    "generated_lines",
    "suggested_questions",
];

/// Build a redacted JSON snapshot of an API panel.
///
/// Serializes the panel, then replaces bulk content fields with `"[stored]"`.
/// Returns `None` if serialization fails.
fn redact_panel_snapshot(panel: &ApiPanel) -> Option<String> {
    let mut value = serde_json::to_value(panel).ok()?;
    let obj = value.as_object_mut()?;
    for &key in PANEL_REDACT_KEYS {
        if let Some(v) = obj.get(key) {
            if !v.is_null() {
                obj.insert(key.to_string(), serde_json::Value::String("[stored]".to_string()));
            }
        }
    }
    serde_json::to_string(&value).ok()
}

/// Convert an API panel into INSERT-ready column values.
///
/// Returns `None` for panels without an ID (which should be skipped).
/// Performs TipTap→markdown conversion and chat_url extraction.
fn api_panel_to_write_row(panel: &ApiPanel) -> Option<PanelWriteRow> {
    panel.id.as_deref()?;

    let content_json = panel.content.as_ref().map(|v| v.to_string());
    let raw_markdown = panel.content.as_ref().map(|v| tiptap_to_markdown(v));
    let (content_markdown, chat_url) = match raw_markdown {
        Some(md) => {
            let (cleaned, url) = extract_chat_url(&md);
            (Some(cleaned), url)
        }
        None => (None, None),
    };
    let original_content_json = panel.original_content.as_ref().map(|v| v.to_string());
    let extra_json = if panel.extra.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&panel.extra).unwrap_or_default())
    };

    let api_snapshot = redact_panel_snapshot(panel);

    Some(PanelWriteRow {
        id: panel.id.clone(),
        title: panel.title.clone(),
        content_json,
        content_markdown,
        original_content_json,
        template_slug: panel.template_slug.clone(),
        created_at: panel.created_at.clone(),
        updated_at: panel.updated_at.clone(),
        deleted_at: panel.deleted_at.clone(),
        extra_json,
        chat_url,
        api_snapshot,
    })
}

/// Load all panels for a document.
pub fn load_panels(conn: &Connection, document_id: &str) -> Result<Vec<Panel>> {
    let mut stmt = conn.prepare(
        "SELECT id, document_id, title, content_markdown, content_json, template_slug,
                created_at, updated_at, deleted_at, chat_url
         FROM panels
         WHERE document_id = ?1 AND deleted_at IS NULL
         ORDER BY created_at",
    )?;

    let rows = stmt.query_map([document_id], |row| {
        Ok(PanelRow {
            id: row.get(0)?,
            document_id: row.get(1)?,
            title: row.get(2)?,
            content_markdown: row.get(3)?,
            content_json: row.get(4)?,
            template_slug: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            deleted_at: row.get(8)?,
            chat_url: row.get(9)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_panel).collect())
}

/// Search panels with context windows.
///
/// Returns `(document_id, document_title, context_windows)` for each matching document.
/// Panels are split into sections (on the most frequent heading level) and context is
/// shown as surrounding sections.
pub fn search_panels_with_context(
    conn: &Connection,
    query: &str,
    meeting_filter: Option<&str>,
    context_size: usize,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<(String, String, Vec<crate::query::search::TextContextWindow>)>> {
    use crate::query::search::{build_text_context_windows, TextSegment};
    use crate::query::text::{split_markdown_sections, strip_panel_footer};

    let matching_docs = find_matching_panel_documents(conn, query, meeting_filter, date_range, include_deleted)?;

    let mut results = Vec::new();
    for (doc_id, doc_title) in &matching_docs {
        let panels = load_panels(conn, doc_id)?;
        for panel in &panels {
            let markdown = match panel.content_markdown.as_deref() {
                Some(md) if !md.is_empty() => md,
                _ => continue,
            };

            let cleaned = strip_panel_footer(markdown);
            let sections = split_markdown_sections(cleaned);
            let segments: Vec<TextSegment> = sections
                .into_iter()
                .map(|(heading, body)| TextSegment {
                    label: heading.map(String::from),
                    text: body.to_string(),
                })
                .collect();

            let windows = build_text_context_windows(&segments, query, context_size);
            if !windows.is_empty() {
                results.push((doc_id.clone(), doc_title.clone(), windows));
            }
        }
    }

    Ok(results)
}

fn find_matching_panel_documents(
    conn: &Connection,
    query: &str,
    meeting_filter: Option<&str>,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<(String, String)>> {
    let fts_query = sanitize_fts_query(query);
    let d_deleted_filter = if include_deleted { "" } else { " AND d.deleted_at IS NULL" };

    let mut sql = format!(
        "SELECT DISTINCT p.document_id, COALESCE(d.title, '(untitled)')
         FROM panels_fts
         JOIN panels p ON panels_fts.rowid = p.rowid
         JOIN documents d ON p.document_id = d.id
         WHERE p.deleted_at IS NULL{} AND panels_fts MATCH ?1",
        d_deleted_filter
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(fts_query)];

    if let Some(meeting_q) = meeting_filter {
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

fn sanitize_fts_query(query: &str) -> String {
    format!("\"{}\"", query.replace('"', ""))
}

// ============================================================================
// Sync functions
// ============================================================================

/// Document info for panel sync
#[derive(Debug, Clone, Serialize)]
pub struct DocumentWithoutPanels {
    pub id: String,
    pub title: Option<String>,
    pub created_at: Option<String>,
}

/// Find documents that don't have any panels.
///
/// When `skip_logged_failures` is true, documents with entries in `panel_sync_log`
/// are excluded. Pass `false` (retry mode) to include them.
pub fn find_documents_without_panels(
    conn: &Connection,
    since: Option<&str>,
    limit: Option<usize>,
    skip_logged_failures: bool,
) -> Result<Vec<DocumentWithoutPanels>> {
    let mut sql = String::from(
        "SELECT d.id, d.title, d.created_at
         FROM documents d
         WHERE d.deleted_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM panels p WHERE p.document_id = d.id
           )",
    );

    if skip_logged_failures {
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM panel_sync_log l WHERE l.document_id = d.id)",
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

    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<DocumentWithoutPanels> {
        Ok(DocumentWithoutPanels {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
        })
    };

    let results: Vec<DocumentWithoutPanels> = if let Some(since_date) = since {
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

/// Insert panels fetched from the API.
/// Deletes any existing panels for the document, then inserts the new ones
/// with TipTap-to-markdown conversion.
pub fn insert_panels_from_api(
    conn: &Connection,
    document_id: &str,
    panels: &[ApiPanel],
) -> Result<usize> {
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

    // Delete from FTS index first
    conn.execute(
        "DELETE FROM panels_fts WHERE rowid IN (
            SELECT rowid FROM panels WHERE document_id = ?1
        )",
        [document_id],
    )
    .ok();

    // Delete existing panels for this document
    conn.execute(
        "DELETE FROM panels WHERE document_id = ?1",
        [document_id],
    )?;

    let mut stmt = conn.prepare(
        "INSERT INTO panels (id, document_id, title, content_json, content_markdown,
                             original_content_json, template_slug,
                             created_at, updated_at, deleted_at, extra_json, chat_url,
                             api_snapshot)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;

    let mut inserted = 0;
    for panel in panels {
        let Some(write_row) = api_panel_to_write_row(panel) else {
            eprintln!("Warning: skipping panel without ID");
            continue;
        };

        stmt.execute(rusqlite::params![
            &write_row.id,
            document_id,
            &write_row.title,
            &write_row.content_json,
            &write_row.content_markdown,
            &write_row.original_content_json,
            &write_row.template_slug,
            &write_row.created_at,
            &write_row.updated_at,
            &write_row.deleted_at,
            &write_row.extra_json,
            &write_row.chat_url,
            &write_row.api_snapshot,
        ])?;
        inserted += 1;
    }

    // Update FTS index for the new panels
    conn.execute(
        "INSERT INTO panels_fts(rowid, content_markdown)
         SELECT rowid, content_markdown FROM panels WHERE document_id = ?1",
        [document_id],
    )?;

    Ok(inserted)
}

/// Record a panel sync failure for a document.
pub fn log_panel_sync_failure(
    conn: &Connection,
    document_id: &str,
    status: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO panel_sync_log (document_id, status, last_attempted_at, attempts)
         VALUES (?1, ?2, ?3, 1)
         ON CONFLICT(document_id) DO UPDATE SET
             status = excluded.status,
             last_attempted_at = excluded.last_attempted_at,
             attempts = panel_sync_log.attempts + 1",
        rusqlite::params![document_id, status, now],
    )?;
    Ok(())
}

/// Remove a panel sync log entry (on successful retry).
pub fn clear_panel_sync_log_entry(conn: &Connection, document_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM panel_sync_log WHERE document_id = ?1",
        [document_id],
    )?;
    Ok(())
}

/// Count how many documents have logged panel sync failures.
pub fn count_panel_sync_failures(conn: &Connection, since: Option<&str>) -> Result<usize> {
    let count: i64 = if let Some(since_date) = since {
        conn.query_row(
            "SELECT COUNT(*) FROM panel_sync_log l
             JOIN documents d ON d.id = l.document_id
             WHERE d.deleted_at IS NULL AND d.created_at >= ?1",
            [since_date],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM panel_sync_log l
             JOIN documents d ON d.id = l.document_id
             WHERE d.deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?
    };
    Ok(count as usize)
}

/// Count total panels in the database.
pub fn count_panels(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM panels WHERE deleted_at IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, panels_state, transcripts_state};
    use serde_json::json;

    #[test]
    fn test_load_panels() {
        let conn = build_test_db(&panels_state());
        let panels = load_panels(&conn, "doc-1").unwrap();
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].title.as_deref(), Some("Summary"));
        assert!(panels[0].content_markdown.is_some());
    }

    #[test]
    fn test_load_panels_empty() {
        let conn = build_test_db(&panels_state());
        let panels = load_panels(&conn, "doc-2").unwrap();
        assert!(panels.is_empty());
    }

    #[test]
    fn test_find_matching_panel_documents() {
        let conn = build_test_db(&panels_state());
        let results = find_matching_panel_documents(&conn, "roadmap", None, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "doc-1");
    }

    #[test]
    fn test_find_matching_panel_documents_no_match() {
        let conn = build_test_db(&panels_state());
        let results = find_matching_panel_documents(&conn, "quantum computing", None, None, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_find_documents_without_panels() {
        let conn = build_test_db(&panels_state());
        // doc-1 has panels, doc-2 does not
        let docs = find_documents_without_panels(&conn, None, None, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc-2");
    }

    #[test]
    fn test_find_documents_without_panels_with_since() {
        let conn = build_test_db(&panels_state());
        // doc-2 is at 2026-01-21, filter since 2026-01-21
        let docs =
            find_documents_without_panels(&conn, Some("2026-01-21T00:00:00Z"), None, false)
                .unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc-2");

        // Filter since 2026-01-22, should find nothing
        let docs =
            find_documents_without_panels(&conn, Some("2026-01-22T00:00:00Z"), None, false)
                .unwrap();
        assert!(docs.is_empty());
    }

    #[test]
    fn test_find_documents_without_panels_with_limit() {
        let conn = build_test_db(&transcripts_state());
        // No panels for any doc
        let docs = find_documents_without_panels(&conn, None, Some(1), false).unwrap();
        assert_eq!(docs.len(), 1);
    }

    #[test]
    fn test_insert_panels_from_api() {
        let conn = build_test_db(&transcripts_state());

        let api_panels = vec![crate::api::ApiPanel {
            id: Some("panel-new".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Action Items".to_string()),
            content: Some(json!({
                "type": "doc",
                "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Review the proposal"}]}
                ]
            })),
            original_content: None,
            template_slug: Some("action-items".to_string()),
            created_at: Some("2026-01-20T11:00:00Z".to_string()),
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        }];

        let inserted = insert_panels_from_api(&conn, "doc-1", &api_panels).unwrap();
        assert_eq!(inserted, 1);

        // Verify stored correctly
        let panels = load_panels(&conn, "doc-1").unwrap();
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].title.as_deref(), Some("Action Items"));
        assert_eq!(
            panels[0].content_markdown.as_deref(),
            Some("Review the proposal")
        );
    }

    #[test]
    fn test_insert_panels_from_api_replaces_existing() {
        let conn = build_test_db(&panels_state());

        // doc-1 already has a panel
        let old_panels = load_panels(&conn, "doc-1").unwrap();
        assert_eq!(old_panels.len(), 1);

        let api_panels = vec![
            crate::api::ApiPanel {
                id: Some("panel-replaced-1".to_string()),
                document_id: Some("doc-1".to_string()),
                title: Some("New Panel".to_string()),
                content: Some(json!({"type": "doc", "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Replaced content"}]}
                ]})),
                original_content: None,
                template_slug: None,
                created_at: None,
                updated_at: None,
                deleted_at: None,
                extra: Default::default(),
            },
        ];

        let inserted = insert_panels_from_api(&conn, "doc-1", &api_panels).unwrap();
        assert_eq!(inserted, 1);

        let panels = load_panels(&conn, "doc-1").unwrap();
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].id.as_deref(), Some("panel-replaced-1"));
    }

    #[test]
    fn test_insert_panels_from_api_updates_fts() {
        let conn = build_test_db(&panels_state());

        // Verify existing panel content is searchable
        let results = find_matching_panel_documents(&conn, "roadmap", None, None, false).unwrap();
        assert_eq!(results.len(), 1);

        // Replace with different content
        let api_panels = vec![crate::api::ApiPanel {
            id: Some("panel-new".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Summary".to_string()),
            content: Some(json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "quantum mechanics discussion"}]}
            ]})),
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        }];

        insert_panels_from_api(&conn, "doc-1", &api_panels).unwrap();

        // Old content should not be searchable
        let results = find_matching_panel_documents(&conn, "roadmap", None, None, false).unwrap();
        assert!(results.is_empty());

        // New content should be searchable
        let results = find_matching_panel_documents(&conn, "quantum mechanics", None, None, false).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_insert_panels_nonexistent_document() {
        let conn = build_test_db(&transcripts_state());

        let api_panels = vec![crate::api::ApiPanel {
            id: Some("panel-1".to_string()),
            document_id: Some("nonexistent".to_string()),
            title: None,
            content: None,
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        }];

        let result = insert_panels_from_api(&conn, "nonexistent", &api_panels);
        assert!(result.is_err());
    }

    #[test]
    fn test_log_panel_sync_failure() {
        let conn = build_test_db(&transcripts_state());

        log_panel_sync_failure(&conn, "doc-1", "not_found").unwrap();

        let (status, attempts): (String, i64) = conn
            .query_row(
                "SELECT status, attempts FROM panel_sync_log WHERE document_id = 'doc-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(status, "not_found");
        assert_eq!(attempts, 1);
    }

    #[test]
    fn test_log_panel_sync_failure_increments() {
        let conn = build_test_db(&transcripts_state());

        log_panel_sync_failure(&conn, "doc-1", "not_found").unwrap();
        log_panel_sync_failure(&conn, "doc-1", "error").unwrap();

        let (status, attempts): (String, i64) = conn
            .query_row(
                "SELECT status, attempts FROM panel_sync_log WHERE document_id = 'doc-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(status, "error");
        assert_eq!(attempts, 2);
    }

    #[test]
    fn test_clear_panel_sync_log_entry() {
        let conn = build_test_db(&transcripts_state());

        log_panel_sync_failure(&conn, "doc-1", "not_found").unwrap();
        clear_panel_sync_log_entry(&conn, "doc-1").unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM panel_sync_log WHERE document_id = 'doc-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_clear_panel_sync_log_entry_nonexistent_is_ok() {
        let conn = build_test_db(&transcripts_state());
        clear_panel_sync_log_entry(&conn, "doc-nonexistent").unwrap();
    }

    #[test]
    fn test_find_documents_without_panels_skips_logged_failures() {
        let conn = build_test_db(&transcripts_state());

        log_panel_sync_failure(&conn, "doc-1", "not_found").unwrap();

        let docs = find_documents_without_panels(&conn, None, None, true).unwrap();
        // doc-1 should be skipped, only doc-2 remains
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "doc-2");
    }

    #[test]
    fn test_find_documents_without_panels_includes_failures_on_retry() {
        let conn = build_test_db(&transcripts_state());

        log_panel_sync_failure(&conn, "doc-1", "not_found").unwrap();

        let docs = find_documents_without_panels(&conn, None, None, false).unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_count_panel_sync_failures() {
        let conn = build_test_db(&transcripts_state());

        log_panel_sync_failure(&conn, "doc-1", "not_found").unwrap();
        log_panel_sync_failure(&conn, "doc-2", "error").unwrap();

        assert_eq!(count_panel_sync_failures(&conn, None).unwrap(), 2);
    }

    #[test]
    fn test_count_panel_sync_failures_with_since() {
        let conn = build_test_db(&transcripts_state());

        // doc-1 created 2026-01-20, doc-2 created 2026-01-21
        log_panel_sync_failure(&conn, "doc-1", "not_found").unwrap();
        log_panel_sync_failure(&conn, "doc-2", "error").unwrap();

        assert_eq!(
            count_panel_sync_failures(&conn, Some("2026-01-21T00:00:00Z")).unwrap(),
            1
        );
    }

    #[test]
    fn test_count_panels() {
        let conn = build_test_db(&panels_state());
        assert_eq!(count_panels(&conn).unwrap(), 1);
    }

    #[test]
    fn test_find_matching_panel_documents_excludes_soft_deleted() {
        let conn = build_test_db(&panels_state());

        // Insert a soft-deleted panel with searchable content
        conn.execute(
            "INSERT INTO panels (id, document_id, title, content_markdown, deleted_at)
             VALUES ('panel-deleted', 'doc-2', 'Deleted Panel', 'unique_deleted_content', '2026-01-22T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO panels_fts(panels_fts) VALUES('rebuild')",
            [],
        )
        .unwrap();

        // Searching for the deleted panel's content should return nothing
        let results = find_matching_panel_documents(&conn, "unique_deleted_content", None, None, false).unwrap();
        assert!(
            results.is_empty(),
            "Soft-deleted panels should not appear in search results"
        );
    }

    #[test]
    fn test_insert_panels_extracts_chat_url() {
        let conn = build_test_db(&transcripts_state());

        let api_panels = vec![crate::api::ApiPanel {
            id: Some("panel-chat".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Summary".to_string()),
            content: Some(json!({
                "type": "doc",
                "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Key decisions made."}]},
                    {"type": "paragraph", "content": [
                        {"type": "text", "text": "Chat with meeting transcript: "},
                        {"type": "text", "text": "https://notes.granola.ai/t/abc123", "marks": [
                            {"type": "link", "attrs": {"href": "https://notes.granola.ai/t/abc123"}}
                        ]}
                    ]}
                ]
            })),
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        }];

        insert_panels_from_api(&conn, "doc-1", &api_panels).unwrap();

        let panels = load_panels(&conn, "doc-1").unwrap();
        assert_eq!(panels.len(), 1);
        assert_eq!(
            panels[0].chat_url.as_deref(),
            Some("https://notes.granola.ai/t/abc123")
        );
        // Chat footer should be stripped from markdown
        let md = panels[0].content_markdown.as_deref().unwrap();
        assert!(!md.contains("notes.granola.ai"));
        assert!(md.contains("Key decisions made."));
    }

    #[test]
    fn test_insert_panels_no_chat_url() {
        let conn = build_test_db(&transcripts_state());

        let api_panels = vec![crate::api::ApiPanel {
            id: Some("panel-nochat".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Notes".to_string()),
            content: Some(json!({
                "type": "doc",
                "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Just regular content."}]}
                ]
            })),
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        }];

        insert_panels_from_api(&conn, "doc-1", &api_panels).unwrap();

        let panels = load_panels(&conn, "doc-1").unwrap();
        assert_eq!(panels.len(), 1);
        assert!(panels[0].chat_url.is_none());
        assert_eq!(
            panels[0].content_markdown.as_deref(),
            Some("Just regular content.")
        );
    }

    // --- search_panels_with_context tests ---

    #[test]
    fn test_search_panels_with_context_basic() {
        let conn = build_test_db(&panels_state());
        let results =
            search_panels_with_context(&conn, "roadmap", None, 1, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "doc-1"); // document_id
        assert_eq!(results[0].2.len(), 1); // one match window

        let window = &results[0].2[0];
        assert!(window.matched.text.contains("roadmap"));
        // "Key Decisions" section matched, with context_size=1 should have Action Items after
        assert_eq!(window.after.len(), 1);
    }

    #[test]
    fn test_search_panels_with_context_section_labels() {
        let conn = build_test_db(&panels_state());
        let results =
            search_panels_with_context(&conn, "deployment", None, 1, None, false).unwrap();
        assert_eq!(results.len(), 1);

        let window = &results[0].2[0];
        // "deployment" is in "Action Items" section
        assert_eq!(window.matched.label.as_deref(), Some("Action Items"));
    }

    #[test]
    fn test_search_panels_with_context_no_match() {
        let conn = build_test_db(&panels_state());
        let results =
            search_panels_with_context(&conn, "quantum computing", None, 1, None, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_panels_with_context_meeting_filter() {
        let conn = build_test_db(&panels_state());
        // doc-1 has panels, but filter to "Other Meeting" (doc-2) which has none
        let results =
            search_panels_with_context(&conn, "roadmap", Some("Other"), 1, None, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_panels_with_context_zero_context() {
        let conn = build_test_db(&panels_state());
        let results =
            search_panels_with_context(&conn, "roadmap", None, 0, None, false).unwrap();
        assert_eq!(results.len(), 1);
        let window = &results[0].2[0];
        assert!(window.before.is_empty());
        assert!(window.after.is_empty());
    }

    /// Regression test: --in panels --context should return panel content, not transcripts.
    /// This is the core bug from issue #197.
    #[test]
    fn test_search_panels_with_context_returns_panel_content_not_transcripts() {
        // Build a DB with both transcripts and panels containing different content
        let state = json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Test Meeting",
                    "created_at": "2026-01-20T10:00:00Z"
                }
            },
            "transcripts": {
                "doc-1": [
                    {"id": "u1", "document_id": "doc-1", "text": "This is transcript content only"}
                ]
            },
            "panels": {
                "doc-1": [
                    {
                        "id": "panel-1",
                        "document_id": "doc-1",
                        "title": "Summary",
                        "content_markdown": "### Key Decisions\n\nThis is panel BKE content only."
                    }
                ]
            }
        });
        let conn = build_test_db(&state);

        // Search for "BKE" in panels - should find it in panel, not transcript
        let results =
            search_panels_with_context(&conn, "BKE", None, 1, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].2[0].matched.text.contains("BKE"));
    }

    #[test]
    fn test_search_panels_with_context_h1_headers() {
        let state = json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Team Standup",
                    "created_at": "2026-01-20T10:00:00Z"
                }
            },
            "panels": {
                "doc-1": [
                    {
                        "id": "panel-h1",
                        "document_id": "doc-1",
                        "title": "Notes",
                        "content_markdown": "# Announcements\n\nNew hire starting Monday.\n\n# Updates\n\nProject deployment is on track.\n\n# Action Items\n\n- Send welcome email"
                    }
                ]
            }
        });
        let conn = build_test_db(&state);

        // Search for "deployment" — should match "Updates" section
        let results =
            search_panels_with_context(&conn, "deployment", None, 1, None, false).unwrap();
        assert_eq!(results.len(), 1);

        let window = &results[0].2[0];
        assert_eq!(window.matched.label.as_deref(), Some("Updates"));
        assert!(window.matched.text.contains("deployment"));

        // With context_size=1, should have Announcements before and Action Items after
        assert_eq!(window.before.len(), 1);
        assert_eq!(window.before[0].label.as_deref(), Some("Announcements"));
        assert_eq!(window.after.len(), 1);
        assert_eq!(window.after[0].label.as_deref(), Some("Action Items"));
    }

    #[test]
    fn test_row_to_panel_all_fields() {
        let row = PanelRow {
            id: Some("panel-1".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Summary".to_string()),
            content_markdown: Some("# Key Points\n\nDecisions made.".to_string()),
            content_json: Some(r#"{"type":"doc"}"#.to_string()),
            template_slug: Some("summary".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T11:00:00Z".to_string()),
            deleted_at: None,
            chat_url: Some("https://notes.granola.ai/t/abc123".to_string()),
        };
        let panel = row_to_panel(row);
        assert_eq!(panel.id.as_deref(), Some("panel-1"));
        assert_eq!(panel.document_id.as_deref(), Some("doc-1"));
        assert_eq!(panel.title.as_deref(), Some("Summary"));
        assert!(panel.content_markdown.unwrap().contains("Key Points"));
        assert_eq!(panel.template_slug.as_deref(), Some("summary"));
        assert_eq!(panel.chat_url.as_deref(), Some("https://notes.granola.ai/t/abc123"));
        assert!(panel.deleted_at.is_none());
        assert!(panel.extra.is_empty());
    }

    #[test]
    fn test_api_panel_to_write_row_with_content() {
        let panel = crate::api::ApiPanel {
            id: Some("panel-1".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Action Items".to_string()),
            content: Some(json!({
                "type": "doc",
                "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Review the proposal"}]}
                ]
            })),
            original_content: Some(json!({"type": "doc", "content": []})),
            template_slug: Some("action-items".to_string()),
            created_at: Some("2026-01-20T11:00:00Z".to_string()),
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        };
        let write_row = api_panel_to_write_row(&panel).unwrap();
        assert_eq!(write_row.id.as_deref(), Some("panel-1"));
        assert_eq!(write_row.title.as_deref(), Some("Action Items"));
        assert_eq!(write_row.content_markdown.as_deref(), Some("Review the proposal"));
        assert!(write_row.content_json.is_some());
        assert!(write_row.original_content_json.is_some());
        assert!(write_row.chat_url.is_none());
        assert!(write_row.extra_json.is_none());
    }

    #[test]
    fn test_api_panel_to_write_row_without_id() {
        let panel = crate::api::ApiPanel {
            id: None,
            document_id: Some("doc-1".to_string()),
            title: Some("No ID".to_string()),
            content: None,
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        };
        assert!(api_panel_to_write_row(&panel).is_none());
    }

    #[test]
    fn test_api_panel_to_write_row_extracts_chat_url() {
        let panel = crate::api::ApiPanel {
            id: Some("panel-chat".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Summary".to_string()),
            content: Some(json!({
                "type": "doc",
                "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Key decisions."}]},
                    {"type": "paragraph", "content": [
                        {"type": "text", "text": "Chat with meeting transcript: "},
                        {"type": "text", "text": "https://notes.granola.ai/t/abc123", "marks": [
                            {"type": "link", "attrs": {"href": "https://notes.granola.ai/t/abc123"}}
                        ]}
                    ]}
                ]
            })),
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        };
        let write_row = api_panel_to_write_row(&panel).unwrap();
        assert_eq!(write_row.chat_url.as_deref(), Some("https://notes.granola.ai/t/abc123"));
        let md = write_row.content_markdown.unwrap();
        assert!(!md.contains("notes.granola.ai"));
        assert!(md.contains("Key decisions."));
    }

    #[test]
    fn test_api_panel_to_write_row_with_extra() {
        let mut extra = std::collections::HashMap::new();
        extra.insert("custom".to_string(), json!("value"));

        let panel = crate::api::ApiPanel {
            id: Some("panel-extra".to_string()),
            document_id: None,
            title: None,
            content: None,
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra,
        };
        let write_row = api_panel_to_write_row(&panel).unwrap();
        assert!(write_row.extra_json.is_some());
        let parsed: serde_json::Value = serde_json::from_str(&write_row.extra_json.unwrap()).unwrap();
        assert_eq!(parsed["custom"], "value");
    }

    // --- redact_panel_snapshot tests ---

    #[test]
    fn test_redact_panel_snapshot_replaces_bulk_fields() {
        let mut extra = std::collections::HashMap::new();
        extra.insert("generated_lines".to_string(), json!(["line1", "line2"]));
        extra.insert("suggested_questions".to_string(), json!(["q1"]));

        let panel = crate::api::ApiPanel {
            id: Some("panel-1".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Summary".to_string()),
            content: Some(json!({"type": "doc", "content": [{"type": "paragraph"}]})),
            original_content: Some(json!({"type": "doc", "content": []})),
            template_slug: Some("meeting-notes".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: None,
            deleted_at: None,
            extra,
        };

        let snapshot = redact_panel_snapshot(&panel).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&snapshot).unwrap();

        // Bulk fields should be replaced with "[stored]"
        assert_eq!(parsed["content"], "[stored]");
        assert_eq!(parsed["original_content"], "[stored]");
        assert_eq!(parsed["generated_lines"], "[stored]");
        assert_eq!(parsed["suggested_questions"], "[stored]");

        // Metadata fields should be preserved
        assert_eq!(parsed["id"], "panel-1");
        assert_eq!(parsed["title"], "Summary");
        assert_eq!(parsed["template_slug"], "meeting-notes");
        assert_eq!(parsed["created_at"], "2026-01-20T10:00:00Z");
    }

    #[test]
    fn test_redact_panel_snapshot_preserves_absent_fields() {
        let panel = crate::api::ApiPanel {
            id: Some("panel-1".to_string()),
            document_id: None,
            title: None,
            content: None,
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        };

        let snapshot = redact_panel_snapshot(&panel).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&snapshot).unwrap();

        assert_eq!(parsed["id"], "panel-1");
        // None fields should serialize as null, not "[stored]"
        assert!(parsed["content"].is_null());
        assert!(parsed["original_content"].is_null());
    }

    // --- api_snapshot integration tests ---

    #[test]
    fn test_insert_panels_from_api_stores_snapshot() {
        let conn = build_test_db(&transcripts_state());

        let mut extra = std::collections::HashMap::new();
        extra.insert("generated_lines".to_string(), json!(["line1"]));

        let api_panels = vec![crate::api::ApiPanel {
            id: Some("panel-snap".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("Summary".to_string()),
            content: Some(json!({
                "type": "doc",
                "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "Key decisions made."}]}
                ]
            })),
            original_content: None,
            template_slug: Some("meeting-notes".to_string()),
            created_at: Some("2026-01-20T11:00:00Z".to_string()),
            updated_at: None,
            deleted_at: None,
            extra,
        }];

        insert_panels_from_api(&conn, "doc-1", &api_panels).unwrap();

        let snapshot: Option<String> = conn
            .query_row(
                "SELECT api_snapshot FROM panels WHERE id = 'panel-snap'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(snapshot.is_some());

        let parsed: serde_json::Value = serde_json::from_str(&snapshot.unwrap()).unwrap();
        assert_eq!(parsed["content"], "[stored]");
        assert_eq!(parsed["generated_lines"], "[stored]");
        assert_eq!(parsed["id"], "panel-snap");
        assert_eq!(parsed["title"], "Summary");
    }

    #[test]
    fn test_insert_panels_from_api_re_insert_updates_snapshot() {
        let conn = build_test_db(&panels_state());

        // First insert
        let api_panels = vec![crate::api::ApiPanel {
            id: Some("panel-re".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("V1".to_string()),
            content: Some(json!({"type": "doc", "content": []})),
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        }];
        insert_panels_from_api(&conn, "doc-1", &api_panels).unwrap();

        // Re-insert with different title
        let api_panels_v2 = vec![crate::api::ApiPanel {
            id: Some("panel-re".to_string()),
            document_id: Some("doc-1".to_string()),
            title: Some("V2".to_string()),
            content: Some(json!({"type": "doc", "content": []})),
            original_content: None,
            template_slug: None,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            extra: Default::default(),
        }];
        insert_panels_from_api(&conn, "doc-1", &api_panels_v2).unwrap();

        let snapshot: Option<String> = conn
            .query_row(
                "SELECT api_snapshot FROM panels WHERE id = 'panel-re'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&snapshot.unwrap()).unwrap();
        assert_eq!(parsed["title"], "V2");
    }
}
