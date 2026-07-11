use anyhow::Result;
use rusqlite::Connection;

use super::common::{DocumentRow, row_to_document};
use super::transcripts::{TranscriptUtteranceRow, row_to_utterance};
use crate::models::{Document, TranscriptUtterance};
use crate::query::dates::DateRange;
use crate::query::fts::sanitize_fts_query;

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

/// Escape SQL LIKE wildcards so a caller-supplied string matches literally
/// under `ESCAPE '\'`.
fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Resolve a full document ID or unique ID prefix to its `(id, title)`.
///
/// Unlike [`show_meeting`], this matches on ID only (no title fallback) and
/// bails when a prefix matches more than one document, because callers use it
/// to gate a write (replacing a transcript) where guessing is unsafe. An exact
/// ID match always wins, even when it is also a prefix of a longer ID. `%` and
/// `_` in `query` are treated literally via `ESCAPE '\'`.
pub fn resolve_document_id(
    conn: &Connection,
    query: &str,
) -> Result<Option<(String, Option<String>)>> {
    let prefix_pattern = format!("{}%", escape_like(query));

    let mut stmt = conn.prepare(
        "SELECT id, title
         FROM documents
         WHERE deleted_at IS NULL
           AND (id = ?1 OR id LIKE ?2 ESCAPE '\\')
         ORDER BY (id = ?1) DESC
         LIMIT 2",
    )?;

    let mut rows = stmt
        .query_map(rusqlite::params![query, prefix_pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    match rows.len() {
        0 => Ok(None),
        1 => Ok(Some(rows.remove(0))),
        // Two matches: an exact ID (sorted first) wins over any longer-ID
        // prefix sibling; otherwise the prefix is genuinely ambiguous.
        _ if rows[0].0 == query => Ok(Some(rows.remove(0))),
        _ => anyhow::bail!(
            "Document ID prefix \"{}\" matches multiple documents; use more characters",
            query
        ),
    }
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
    // Each enabled source contributes (doc_id, title_hit, score) rows; a
    // document's rank is its best (lowest) bm25 score across sources.
    let mut union_parts: Vec<&str> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if search_titles {
        union_parts.push(
            "SELECT id AS doc_id, 1 AS title_hit, NULL AS score FROM documents WHERE title LIKE ? COLLATE NOCASE",
        );
        params.push(Box::new(format!("%{}%", query)));
    }

    let fts_query = sanitize_fts_query(query);

    if search_notes {
        union_parts.push(
            "SELECT d.id AS doc_id, 0 AS title_hit, bm25(notes_fts) AS score FROM notes_fts JOIN documents d ON notes_fts.rowid = d.rowid WHERE notes_fts MATCH ?",
        );
        params.push(Box::new(fts_query.clone()));
    }

    if search_transcripts {
        union_parts.push(
            "SELECT tu.document_id AS doc_id, 0 AS title_hit, bm25(transcript_fts) AS score FROM transcript_fts JOIN transcript_utterances tu ON transcript_fts.rowid = tu.rowid WHERE transcript_fts MATCH ?",
        );
        params.push(Box::new(fts_query.clone()));
    }

    if search_panels {
        union_parts.push(
            "SELECT p.document_id AS doc_id, 0 AS title_hit, bm25(panels_fts) AS score FROM panels_fts JOIN panels p ON panels_fts.rowid = p.rowid WHERE p.deleted_at IS NULL AND panels_fts MATCH ?",
        );
        params.push(Box::new(fts_query));
    }

    if union_parts.is_empty() {
        return Ok(Vec::new());
    }

    // MATERIALIZED stops the query flattener from pulling bm25() into the
    // aggregate context, where FTS5 auxiliary functions cannot run.
    let mut sql = format!(
        "WITH hits AS MATERIALIZED ({})
         SELECT d.id, d.title, d.created_at, d.updated_at, d.deleted_at, d.doc_type, d.notes_plain, d.notes_markdown, d.summary, d.people_json, d.google_calendar_event_json
         FROM documents d
         JOIN (SELECT doc_id, MAX(title_hit) AS title_hit, MIN(score) AS best_score
               FROM hits GROUP BY doc_id) m ON d.id = m.doc_id
         WHERE 1=1",
        union_parts.join(" UNION ALL ")
    );

    if !include_deleted {
        sql.push_str(" AND d.deleted_at IS NULL");
    }
    append_date_filter(&mut sql, &mut params, date_range, "d.created_at");

    // bm25 is lower-is-better. Title matches carry no bm25 score: a title
    // hit (the whole query as a substring of the title) outranks content
    // matches, and within that tier title-only matches sort after scored
    // ones. Recency breaks remaining ties.
    sql.push_str(" ORDER BY m.title_hit DESC, m.best_score ASC NULLS LAST, d.created_at DESC");

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

    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(row_to_document).collect())
}

/// Fetch documents by id, returned in the order the ids were given.
/// Unknown ids are skipped. No deleted filter is applied: callers pass ids
/// from a search that already honored include_deleted.
pub fn get_meetings_by_ids(conn: &Connection, ids: &[String]) -> Result<Vec<Document>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, title, created_at, updated_at, deleted_at, doc_type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json
         FROM documents WHERE id IN ({})",
        placeholders
    );

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), |row| {
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
    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let mut by_id: std::collections::HashMap<String, Document> = rows
        .into_iter()
        .map(row_to_document)
        .filter_map(|d| d.id.clone().map(|id| (id, d)))
        .collect();

    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

fn append_date_filter(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    date_range: Option<&DateRange>,
    column: &str,
) {
    if let Some(range) = date_range {
        if let Some(start) = &range.start {
            sql.push_str(&format!(" AND {} >= ?", column));
            params.push(Box::new(start.to_rfc3339()));
        }
        if let Some(end) = &range.end {
            sql.push_str(&format!(" AND {} < ?", column));
            params.push(Box::new(end.to_rfc3339()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, meetings_state, transcripts_state};
    use chrono::{TimeZone, Utc};

    #[test]
    fn test_list_meetings_all() {
        let conn = build_test_db(&meetings_state());
        let docs = list_meetings(&conn, None, None, false).unwrap();
        // deleted doc excluded
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn test_get_meetings_by_ids_preserves_input_order() {
        let conn = build_test_db(&meetings_state());
        let ids = vec!["doc-2".to_string(), "doc-1".to_string()];
        let docs = get_meetings_by_ids(&conn, &ids).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].id.as_deref(), Some("doc-2"));
        assert_eq!(docs[1].id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn test_get_meetings_by_ids_skips_unknown_ids() {
        let conn = build_test_db(&meetings_state());
        let ids = vec!["doc-1".to_string(), "doc-missing".to_string()];
        let docs = get_meetings_by_ids(&conn, &ids).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn test_get_meetings_by_ids_empty_input() {
        let conn = build_test_db(&meetings_state());
        let docs = get_meetings_by_ids(&conn, &[]).unwrap();
        assert!(docs.is_empty());
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
    fn test_resolve_document_id_exact() {
        let conn = build_test_db(&transcripts_state());
        let (id, title) = resolve_document_id(&conn, "doc-1").unwrap().unwrap();
        assert_eq!(id, "doc-1");
        assert_eq!(title.as_deref(), Some("AI Meeting"));
    }

    #[test]
    fn test_resolve_document_id_unique_prefix() {
        let conn = build_test_db(&transcripts_state());
        // A distinct, longer id so a partial prefix is unambiguous.
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('abc12345', 'Distinct', '2026-01-23T10:00:00Z')",
            [],
        )
        .unwrap();
        let (id, _) = resolve_document_id(&conn, "abc1").unwrap().unwrap();
        assert_eq!(id, "abc12345");
    }

    #[test]
    fn test_resolve_document_id_exact_wins_over_longer_prefix() {
        let conn = build_test_db(&transcripts_state());
        conn.execute(
            "INSERT INTO documents (id, title, created_at) VALUES ('doc-12', 'Longer', '2026-01-23T10:00:00Z')",
            [],
        )
        .unwrap();
        // "doc-1" is an exact id AND a prefix of "doc-12"; exact wins, no bail.
        let (id, _) = resolve_document_id(&conn, "doc-1").unwrap().unwrap();
        assert_eq!(id, "doc-1");
    }

    #[test]
    fn test_resolve_document_id_ambiguous_prefix() {
        let conn = build_test_db(&transcripts_state());
        // Both doc-1 and doc-2 share the "doc-" prefix.
        let err = resolve_document_id(&conn, "doc-").unwrap_err();
        assert!(err.to_string().contains("multiple documents"));
    }

    #[test]
    fn test_resolve_document_id_not_found() {
        let conn = build_test_db(&transcripts_state());
        assert!(resolve_document_id(&conn, "nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_resolve_document_id_excludes_deleted() {
        let conn = build_test_db(&transcripts_state());
        conn.execute(
            "INSERT INTO documents (id, title, created_at, deleted_at) VALUES ('gone-1', 'Gone', '2026-01-23T10:00:00Z', '2026-01-24T10:00:00Z')",
            [],
        )
        .unwrap();
        assert!(resolve_document_id(&conn, "gone-1").unwrap().is_none());
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

    /// Docs where relevance and recency disagree. Filler rows keep the
    /// matched terms under half the corpus so BM25 IDF stays positive.
    fn ranking_state() -> serde_json::Value {
        serde_json::json!({
            "documents": {
                "doc-relevant": {"id": "doc-relevant", "title": "Platform Sync", "created_at": "2026-01-10T10:00:00Z"},
                "doc-mention": {"id": "doc-mention", "title": "Team Catchup", "created_at": "2026-01-20T10:00:00Z"},
                "doc-titled": {"id": "doc-titled", "title": "Kubernetes Migration", "created_at": "2026-01-05T10:00:00Z"},
                "doc-tie-old": {"id": "doc-tie-old", "title": "Metrics Review A", "created_at": "2026-01-08T10:00:00Z"},
                "doc-tie-new": {"id": "doc-tie-new", "title": "Metrics Review B", "created_at": "2026-01-18T10:00:00Z"}
            },
            "transcripts": {
                "doc-relevant": [
                    {"id": "r1", "document_id": "doc-relevant", "text": "kubernetes migration steps kubernetes cluster upgrades and kubernetes networking"}
                ],
                "doc-mention": [
                    {"id": "m1", "document_id": "doc-mention", "text": "someone mentioned kubernetes briefly while we mostly discussed the quarterly budget review"},
                    {"id": "m2", "document_id": "doc-mention", "text": "budget planning discussion continued"}
                ],
                "doc-tie-old": [
                    {"id": "t1", "document_id": "doc-tie-old", "text": "prometheus metrics dashboard review"}
                ],
                "doc-tie-new": [
                    {"id": "t2", "document_id": "doc-tie-new", "text": "prometheus metrics dashboard review"}
                ]
            }
        })
    }

    #[test]
    fn test_search_meetings_ranks_by_relevance_not_recency() {
        // Regression: results were ordered by created_at DESC, so a passing
        // mention in a newer meeting outranked the meeting about the topic.
        let conn = build_test_db(&ranking_state());
        let results =
            search_meetings(&conn, "kubernetes", false, true, false, false, None, false).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id.as_deref(), Some("doc-relevant"));
        assert_eq!(results[1].id.as_deref(), Some("doc-mention"));
    }

    #[test]
    fn test_search_meetings_title_match_ranks_first() {
        let conn = build_test_db(&ranking_state());
        let results =
            search_meetings(&conn, "kubernetes", true, true, false, false, None, false).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].id.as_deref(), Some("doc-titled"));
    }

    #[test]
    fn test_search_meetings_equal_relevance_breaks_ties_by_recency() {
        let conn = build_test_db(&ranking_state());
        let results =
            search_meetings(&conn, "prometheus", false, true, false, false, None, false).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id.as_deref(), Some("doc-tie-new"));
        assert_eq!(results[1].id.as_deref(), Some("doc-tie-old"));
    }

    #[test]
    fn test_search_meetings_surfaces_row_errors() {
        // Regression: row-mapping errors were silently dropped, which turned
        // real failures into empty search results.
        let conn = build_test_db(&meetings_state());
        conn.execute(
            "UPDATE documents SET title = x'DEADBEEF' WHERE id = 'doc-1'",
            [],
        )
        .unwrap();
        let result = search_meetings(&conn, "neural", false, true, false, false, None, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_search_meetings_multi_word_matches_any_order() {
        // Regression: the old sanitizer quoted the whole query as one FTS5
        // phrase, so reversed word order never matched.
        let conn = build_test_db(&meetings_state());
        let results =
            search_meetings(&conn, "networks neural", false, true, false, false, None, false)
                .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn test_search_meetings_user_quotes_force_phrase() {
        let conn = build_test_db(&meetings_state());
        let results = search_meetings(
            &conn,
            "\"networks neural\"",
            false,
            true,
            false,
            false,
            None,
            false,
        )
        .unwrap();
        assert!(results.is_empty());
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
