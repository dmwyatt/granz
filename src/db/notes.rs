use anyhow::Result;
use rusqlite::Connection;

use crate::query::dates::DateRange;
use crate::query::search::{build_text_context_windows, TextContextWindow, TextSegment};
use crate::query::text::split_into_paragraphs;

/// Search notes with context windows.
///
/// Returns `(document_id, document_title, context_windows)` for each matching document.
pub fn search_notes_with_context(
    conn: &Connection,
    query: &str,
    meeting_filter: Option<&str>,
    context_size: usize,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<(String, String, Vec<TextContextWindow>)>> {
    let matching_docs = find_matching_note_documents(conn, query, meeting_filter, date_range, include_deleted)?;

    let mut results = Vec::new();
    for (doc_id, doc_title) in &matching_docs {
        let notes_plain: Option<String> = conn
            .query_row(
                "SELECT notes_plain FROM documents WHERE id = ?1",
                [doc_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let notes = match notes_plain {
            Some(ref n) if !n.is_empty() => n,
            _ => continue,
        };

        let paragraphs = split_into_paragraphs(notes);
        let segments: Vec<TextSegment> = paragraphs
            .into_iter()
            .map(|p| TextSegment {
                label: None,
                text: p.to_string(),
            })
            .collect();

        let windows = build_text_context_windows(&segments, query, context_size);
        if !windows.is_empty() {
            results.push((doc_id.clone(), doc_title.clone(), windows));
        }
    }

    Ok(results)
}

fn find_matching_note_documents(
    conn: &Connection,
    query: &str,
    meeting_filter: Option<&str>,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<(String, String)>> {
    let fts_query = sanitize_fts_query(query);
    let deleted_filter = if include_deleted { "" } else { " AND d.deleted_at IS NULL" };

    let mut sql = format!(
        "SELECT DISTINCT d.id, COALESCE(d.title, '(untitled)')
         FROM notes_fts
         JOIN documents d ON notes_fts.rowid = d.rowid
         WHERE notes_fts MATCH ?1{}",
        deleted_filter
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use serde_json::json;

    fn notes_state() -> serde_json::Value {
        json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Strategy Meeting",
                    "created_at": "2026-01-20T10:00:00Z",
                    "notes_plain": "We discussed the roadmap for Q1.\n\nThe team agreed on priorities.\n\nAction items were assigned to each member."
                },
                "doc-2": {
                    "id": "doc-2",
                    "title": "Weekly Standup",
                    "created_at": "2026-01-21T10:00:00Z",
                    "notes_plain": "Nothing notable."
                }
            }
        })
    }

    #[test]
    fn test_search_notes_with_context_basic() {
        let conn = build_test_db(&notes_state());
        let results =
            search_notes_with_context(&conn, "roadmap", None, 1, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "doc-1");
        assert_eq!(results[0].2.len(), 1);
        // The matched paragraph should contain "roadmap"
        assert!(results[0].2[0].matched.text.contains("roadmap"));
        // Should have context after (the "priorities" paragraph)
        assert_eq!(results[0].2[0].after.len(), 1);
    }

    #[test]
    fn test_search_notes_with_context_no_match() {
        let conn = build_test_db(&notes_state());
        let results =
            search_notes_with_context(&conn, "quantum", None, 1, None, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_notes_with_context_meeting_filter() {
        let conn = build_test_db(&notes_state());
        // Filter to "Standup" meeting - "roadmap" is not in standup notes
        let results =
            search_notes_with_context(&conn, "roadmap", Some("Standup"), 1, None, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_notes_paragraphs_have_no_label() {
        let conn = build_test_db(&notes_state());
        let results =
            search_notes_with_context(&conn, "roadmap", None, 0, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].2[0].matched.label.is_none());
    }
}
