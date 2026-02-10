use anyhow::Result;
use rusqlite::Connection;

use super::common::{DocumentRow, row_to_document};
use super::transcripts::{TranscriptUtteranceRow, row_to_utterance};
use crate::models::{Document, TranscriptUtterance};
use crate::query::dates::DateRange;

pub fn list_meetings(
    conn: &Connection,
    person: Option<&str>,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<Document>> {
    if let Some(person_q) = person {
        return list_meetings_by_person(conn, person_q, date_range, include_deleted);
    }

    let mut sql = String::from(
        "SELECT id, title, created_at, updated_at, deleted_at, doc_type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json FROM documents WHERE 1=1",
    );
    if !include_deleted {
        sql.push_str(" AND deleted_at IS NULL");
    }
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(range) = date_range {
        if let Some(start) = &range.start {
            sql.push_str(" AND created_at >= ?");
            params.push(Box::new(start.to_rfc3339()));
        }
        if let Some(end) = &range.end {
            sql.push_str(" AND created_at < ?");
            params.push(Box::new(end.to_rfc3339()));
        }
    }

    sql.push_str(" ORDER BY created_at DESC");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(DocumentRow {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            deleted_at: row.get(4)?,
            doc_type: row.get(5)?,
            notes_plain: row.get(6)?,
            notes_markdown: row.get(7)?,
            summary: row.get(8)?,
            people_json: row.get(9)?,
            google_calendar_event_json: row.get(10)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_document).collect())
}

fn list_meetings_by_person(
    conn: &Connection,
    person_query: &str,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<Document>> {
    let pattern = format!("%{}%", person_query);
    let mut sql = String::from(
        "SELECT DISTINCT d.id, d.title, d.created_at, d.updated_at, d.deleted_at, d.doc_type, d.notes_plain, d.notes_markdown, d.summary, d.people_json, d.google_calendar_event_json
         FROM documents d
         JOIN document_people dp ON d.id = dp.document_id
         WHERE (dp.email LIKE ?1 OR dp.full_name LIKE ?1)",
    );
    if !include_deleted {
        sql.push_str(" AND d.deleted_at IS NULL");
    }
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(pattern)];

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
        Ok(DocumentRow {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            deleted_at: row.get(4)?,
            doc_type: row.get(5)?,
            notes_plain: row.get(6)?,
            notes_markdown: row.get(7)?,
            summary: row.get(8)?,
            people_json: row.get(9)?,
            google_calendar_event_json: row.get(10)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_document).collect())
}

pub fn show_meeting(conn: &Connection, query: &str) -> Result<Option<Document>> {
    let prefix_pattern = format!("{}%", query);
    let title_pattern = format!("%{}%", query);

    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, updated_at, deleted_at, doc_type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json
         FROM documents
         WHERE deleted_at IS NULL
           AND (id = ?1 OR id LIKE ?2 OR title LIKE ?3 COLLATE NOCASE)
         ORDER BY CASE
           WHEN id = ?1 THEN 0
           WHEN id LIKE ?2 THEN 1
           ELSE 2
         END
         LIMIT 1",
    )?;

    let result = stmt
        .query_row(rusqlite::params![query, prefix_pattern, title_pattern], |row| {
            Ok(DocumentRow {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                deleted_at: row.get(4)?,
                doc_type: row.get(5)?,
                notes_plain: row.get(6)?,
                notes_markdown: row.get(7)?,
                summary: row.get(8)?,
                people_json: row.get(9)?,
                google_calendar_event_json: row.get(10)?,
            })
        })
        .ok();

    Ok(result.map(row_to_document))
}

pub fn get_transcript(conn: &Connection, document_id: &str) -> Result<Vec<TranscriptUtterance>> {
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

pub fn search_meetings(
    conn: &Connection,
    query: &str,
    search_titles: bool,
    search_transcripts: bool,
    search_notes: bool,
    search_panels: bool,
    date_range: Option<&DateRange>,
    include_deleted: bool,
) -> Result<Vec<Document>> {
    let title_pattern = format!("%{}%", query);
    let fts_query = sanitize_fts_query(query);
    let doc_filter = if include_deleted { "" } else { " AND deleted_at IS NULL" };
    let d_doc_filter = if include_deleted { "" } else { " AND d.deleted_at IS NULL" };

    // Build UNION of matching document IDs from requested search types
    let mut union_parts: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if search_titles {
        let mut sub = format!(
            "SELECT id FROM documents WHERE 1=1{} AND title LIKE ?{} COLLATE NOCASE",
            doc_filter, param_idx
        );
        params.push(Box::new(title_pattern));
        param_idx += 1;
        append_date_filter(&mut sub, &mut params, &mut param_idx, date_range, "created_at");
        union_parts.push(sub);
    }

    if search_notes {
        let mut sub = format!(
            "SELECT d.id FROM notes_fts JOIN documents d ON notes_fts.rowid = d.rowid WHERE 1=1{} AND notes_fts MATCH ?{}",
            d_doc_filter, param_idx
        );
        params.push(Box::new(fts_query.clone()));
        param_idx += 1;
        append_date_filter(&mut sub, &mut params, &mut param_idx, date_range, "d.created_at");
        union_parts.push(sub);
    }

    if search_transcripts {
        let mut sub = format!(
            "SELECT DISTINCT tu.document_id FROM transcript_fts JOIN transcript_utterances tu ON transcript_fts.rowid = tu.rowid JOIN documents d ON tu.document_id = d.id WHERE 1=1{} AND transcript_fts MATCH ?{}",
            d_doc_filter, param_idx
        );
        params.push(Box::new(fts_query.clone()));
        param_idx += 1;
        append_date_filter(&mut sub, &mut params, &mut param_idx, date_range, "d.created_at");
        union_parts.push(sub);
    }

    if search_panels {
        let mut sub = format!(
            "SELECT DISTINCT p.document_id FROM panels_fts JOIN panels p ON panels_fts.rowid = p.rowid JOIN documents d ON p.document_id = d.id WHERE p.deleted_at IS NULL{} AND panels_fts MATCH ?{}",
            d_doc_filter, param_idx
        );
        params.push(Box::new(fts_query));
        param_idx += 1;
        append_date_filter(&mut sub, &mut params, &mut param_idx, date_range, "d.created_at");
        union_parts.push(sub);
    }

    if union_parts.is_empty() {
        return Ok(Vec::new());
    }

    let union_sql = union_parts.join(" UNION ");
    let sql = format!(
        "SELECT id, title, created_at, updated_at, deleted_at, doc_type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json FROM documents WHERE 1=1{} AND id IN ({}) ORDER BY created_at DESC",
        doc_filter, union_sql
    );

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(DocumentRow {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            deleted_at: row.get(4)?,
            doc_type: row.get(5)?,
            notes_plain: row.get(6)?,
            notes_markdown: row.get(7)?,
            summary: row.get(8)?,
            people_json: row.get(9)?,
            google_calendar_event_json: row.get(10)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_document).collect())
}

fn append_date_filter(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    param_idx: &mut usize,
    date_range: Option<&DateRange>,
    column: &str,
) {
    if let Some(range) = date_range {
        if let Some(start) = &range.start {
            sql.push_str(&format!(" AND {} >= ?{}", column, param_idx));
            params.push(Box::new(start.to_rfc3339()));
            *param_idx += 1;
        }
        if let Some(end) = &range.end {
            sql.push_str(&format!(" AND {} < ?{}", column, param_idx));
            params.push(Box::new(end.to_rfc3339()));
            *param_idx += 1;
        }
    }
}

fn sanitize_fts_query(query: &str) -> String {
    format!("\"{}\"", query.replace('"', ""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, meetings_state};
    use chrono::{TimeZone, Utc};

    #[test]
    fn test_list_meetings_all() {
        let conn = build_test_db(&meetings_state());
        let docs = list_meetings(&conn, None, None, false).unwrap();
        // deleted doc excluded
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_list_meetings_ordered_by_date_desc() {
        let conn = build_test_db(&meetings_state());
        let docs = list_meetings(&conn, None, None, false).unwrap();
        assert_eq!(docs[0].title.as_deref(), Some("Weekly Standup"));
        assert_eq!(docs[1].title.as_deref(), Some("AI Strategy Meeting"));
    }

    #[test]
    fn test_list_meetings_by_date_range() {
        let conn = build_test_db(&meetings_state());
        let range = DateRange {
            start: Some(Utc.with_ymd_and_hms(2026, 1, 21, 0, 0, 0).unwrap()),
            end: Some(Utc.with_ymd_and_hms(2026, 1, 23, 0, 0, 0).unwrap()),
        };
        let docs = list_meetings(&conn, None, Some(&range), false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title.as_deref(), Some("Weekly Standup"));
    }

    #[test]
    fn test_list_meetings_by_person() {
        let conn = build_test_db(&meetings_state());
        let docs = list_meetings(&conn, Some("alice"), None, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title.as_deref(), Some("AI Strategy Meeting"));
    }

    #[test]
    fn test_show_meeting_by_id() {
        let conn = build_test_db(&meetings_state());
        let doc = show_meeting(&conn, "doc-1").unwrap().unwrap();
        assert_eq!(doc.title.as_deref(), Some("AI Strategy Meeting"));
    }

    #[test]
    fn test_show_meeting_by_prefix() {
        let conn = build_test_db(&meetings_state());
        let doc = show_meeting(&conn, "doc-2").unwrap().unwrap();
        assert_eq!(doc.title.as_deref(), Some("Weekly Standup"));
    }

    #[test]
    fn test_show_meeting_by_title() {
        let conn = build_test_db(&meetings_state());
        let doc = show_meeting(&conn, "Standup").unwrap().unwrap();
        assert_eq!(doc.id.as_deref(), Some("doc-2"));
    }

    #[test]
    fn test_show_meeting_not_found() {
        let conn = build_test_db(&meetings_state());
        let doc = show_meeting(&conn, "nonexistent").unwrap();
        assert!(doc.is_none());
    }

    #[test]
    fn test_show_meeting_excludes_deleted() {
        let conn = build_test_db(&meetings_state());
        let doc = show_meeting(&conn, "doc-deleted").unwrap();
        assert!(doc.is_none());
    }

    #[test]
    fn test_get_transcript() {
        let conn = build_test_db(&meetings_state());
        let transcript = get_transcript(&conn, "doc-1").unwrap();
        assert_eq!(transcript.len(), 2);
    }

    #[test]
    fn test_get_transcript_includes_source() {
        let conn = build_test_db(&meetings_state());
        // Insert utterance with source
        conn.execute(
            "INSERT INTO transcript_utterances (id, document_id, text, source, is_final)
             VALUES ('u-mic', 'doc-1', 'I said this', 'microphone', 1)",
            [],
        )
        .unwrap();
        let transcript = get_transcript(&conn, "doc-1").unwrap();
        let mic_utt = transcript.iter().find(|u| u.id.as_deref() == Some("u-mic")).unwrap();
        assert_eq!(mic_utt.source.as_deref(), Some("microphone"));
        assert_eq!(mic_utt.is_final, Some(true));
    }

    #[test]
    fn test_get_transcript_empty() {
        let conn = build_test_db(&meetings_state());
        let transcript = get_transcript(&conn, "doc-2").unwrap();
        assert!(transcript.is_empty());
    }

    #[test]
    fn test_search_meetings_by_title() {
        let conn = build_test_db(&meetings_state());
        let results =
            search_meetings(&conn, "AI", true, false, false, false, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title.as_deref(), Some("AI Strategy Meeting"));
    }

    #[test]
    fn test_search_meetings_by_transcript() {
        let conn = build_test_db(&meetings_state());
        let results =
            search_meetings(&conn, "neural networks", false, true, false, false, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn test_search_meetings_by_notes() {
        let conn = build_test_db(&meetings_state());
        let results =
            search_meetings(&conn, "machine learning", false, false, true, false, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn test_search_meetings_no_results() {
        let conn = build_test_db(&meetings_state());
        let results =
            search_meetings(&conn, "quantum computing", true, true, true, false, None, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_meetings_with_date_range() {
        let conn = build_test_db(&meetings_state());
        let range = DateRange {
            start: Some(Utc.with_ymd_and_hms(2026, 1, 22, 0, 0, 0).unwrap()),
            end: Some(Utc.with_ymd_and_hms(2026, 1, 23, 0, 0, 0).unwrap()),
        };
        let results =
            search_meetings(&conn, "Standup", true, false, false, false, Some(&range), false).unwrap();
        assert_eq!(results.len(), 1);

        // AI meeting is outside range even though title matches
        let results2 =
            search_meetings(&conn, "AI", true, false, false, false, Some(&range), false).unwrap();
        assert!(results2.is_empty());
    }

    #[test]
    fn test_document_has_people_json() {
        let conn = build_test_db(&meetings_state());
        let doc = show_meeting(&conn, "doc-1").unwrap().unwrap();
        assert!(doc.people.is_some());
        let people = doc.people.unwrap();
        assert_eq!(people.creator.unwrap().name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_search_meetings_by_panels_excludes_soft_deleted() {
        use crate::db::test_fixtures::panels_state;

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

        // Searching for the deleted panel's content via search_meetings should return nothing
        let results =
            search_meetings(&conn, "unique_deleted_content", false, false, false, true, None, false)
                .unwrap();
        assert!(
            results.is_empty(),
            "Soft-deleted panels should not appear in search_meetings results"
        );
    }

    #[test]
    fn test_list_meetings_include_deleted() {
        let state = serde_json::json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "Active Meeting", "created_at": "2026-01-20T10:00:00Z"},
                "doc-deleted": {"id": "doc-deleted", "title": "Deleted Meeting", "created_at": "2026-01-21T10:00:00Z", "deleted_at": "2026-01-22T10:00:00Z"}
            }
        });
        let conn = build_test_db(&state);

        // Default: deleted excluded
        let docs = list_meetings(&conn, None, None, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title.as_deref(), Some("Active Meeting"));

        // With include_deleted: both returned
        let docs = list_meetings(&conn, None, None, true).unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_list_meetings_by_person_include_deleted() {
        let state = serde_json::json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Active Meeting",
                    "created_at": "2026-01-20T10:00:00Z",
                    "people": {
                        "creator": {"name": "Alice", "email": "alice@example.com"}
                    }
                },
                "doc-deleted": {
                    "id": "doc-deleted",
                    "title": "Deleted Meeting",
                    "created_at": "2026-01-21T10:00:00Z",
                    "deleted_at": "2026-01-22T10:00:00Z",
                    "people": {
                        "creator": {"name": "Alice", "email": "alice@example.com"}
                    }
                }
            }
        });
        let conn = build_test_db(&state);

        let docs = list_meetings(&conn, Some("alice"), None, false).unwrap();
        assert_eq!(docs.len(), 1);

        let docs = list_meetings(&conn, Some("alice"), None, true).unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_search_meetings_include_deleted() {
        let state = serde_json::json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "AI Strategy Meeting", "created_at": "2026-01-20T10:00:00Z"},
                "doc-deleted": {"id": "doc-deleted", "title": "AI Deleted Meeting", "created_at": "2026-01-21T10:00:00Z", "deleted_at": "2026-01-22T10:00:00Z"}
            }
        });
        let conn = build_test_db(&state);

        // Default: deleted excluded from title search
        let results = search_meetings(&conn, "AI", true, false, false, false, None, false).unwrap();
        assert_eq!(results.len(), 1);

        // With include_deleted: both returned
        let results = search_meetings(&conn, "AI", true, false, false, false, None, true).unwrap();
        assert_eq!(results.len(), 2);
    }
}
