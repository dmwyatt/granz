//! Contextual chunk headers: meeting title, date, and attendees prepended
//! to the embed input (never to the stored chunk text) so the embedding
//! model sees which meeting a chunk came from.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

/// Hard cap on header length in bytes. Keeps the header from eating the
/// model's token budget on meetings with huge rosters; attendee names that
/// don't fit are dropped.
pub const HEADER_MAX_CHARS: usize = 512;

/// Build a contextual header per document: `Meeting: {title}`,
/// `Date: {YYYY-MM-DD}`, `Attendees: {names}`, ending with a blank line so
/// it can be prepended to chunk text directly. Lines whose data is missing
/// are omitted; documents with no header content get no entry. Attendee
/// names are sorted and deduplicated so headers (and therefore content
/// hashes) are deterministic.
pub fn build_doc_headers(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut names_by_doc: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT document_id, COALESCE(full_name, email) AS name
         FROM document_people
         WHERE COALESCE(full_name, email) IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (doc_id, name) = row?;
        names_by_doc.entry(doc_id).or_default().push(name);
    }
    for names in names_by_doc.values_mut() {
        names.sort_unstable();
        names.dedup();
    }

    let mut stmt = conn.prepare(
        "SELECT id, title, created_at FROM documents WHERE deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut headers = HashMap::new();
    for row in rows {
        let (doc_id, title, created_at) = row?;
        let names = names_by_doc.get(&doc_id).map(Vec::as_slice).unwrap_or(&[]);
        if let Some(header) = assemble_header(&title, created_at.as_deref(), names) {
            headers.insert(doc_id, header);
        }
    }
    Ok(headers)
}

/// Assemble one header, ending with a blank line. The cap covers the
/// whole header: the title is truncated to leave room for the fixed
/// `Meeting: `/`Date: ` framing, and attendee names that would push the
/// header past [`HEADER_MAX_CHARS`] are dropped.
fn assemble_header(title: &str, created_at: Option<&str>, names: &[String]) -> Option<String> {
    let mut header = String::new();

    if !title.is_empty() {
        // "Meeting: " + '\n' (10) + "Date: YYYY-MM-DD\n" (17) + trailing '\n' (1)
        const FRAMING_RESERVE: usize = 28;
        let title_budget = HEADER_MAX_CHARS - FRAMING_RESERVE;
        let title = if title.len() > title_budget {
            &title[..crate::embed::chunker::floor_char_boundary(title, title_budget)]
        } else {
            title
        };
        header.push_str("Meeting: ");
        header.push_str(title);
        header.push('\n');
    }

    let date = created_at
        .and_then(|c| c.split('T').next())
        .filter(|d| !d.is_empty());
    if let Some(date) = date {
        header.push_str("Date: ");
        header.push_str(date);
        header.push('\n');
    }

    if !names.is_empty() {
        let prefix = "Attendees: ";
        let mut line = String::new();
        for name in names {
            let addition = if line.is_empty() { name.len() } else { name.len() + 2 };
            // +2 accounts for this line's '\n' and the trailing blank line.
            if header.len() + prefix.len() + line.len() + addition + 2 > HEADER_MAX_CHARS {
                break;
            }
            if !line.is_empty() {
                line.push_str(", ");
            }
            line.push_str(name);
        }
        if !line.is_empty() {
            header.push_str(prefix);
            header.push_str(&line);
            header.push('\n');
        }
    }

    if header.is_empty() {
        return None;
    }
    header.push('\n');
    Some(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use serde_json::json;

    fn state_with_attendees() -> serde_json::Value {
        json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "AI Strategy Meeting",
                    "created_at": "2026-01-20T10:00:00Z",
                    "people": {
                        "creator": {"name": "Zoe Adams", "email": "zoe@example.com"},
                        "attendees": [
                            {"email": "bob@example.com", "details": {"person": {"name": {"fullName": "Bob Jones"}}}},
                            {"email": "alice@example.com", "details": {"person": {"name": {"fullName": "Alice Smith"}}}}
                        ]
                    }
                }
            }
        })
    }

    #[test]
    fn header_contains_title_date_and_sorted_attendees() {
        let conn = build_test_db(&state_with_attendees());
        let headers = build_doc_headers(&conn).unwrap();

        assert_eq!(
            headers.get("doc-1").map(String::as_str),
            Some("Meeting: AI Strategy Meeting\nDate: 2026-01-20\nAttendees: Alice Smith, Bob Jones, Zoe Adams\n\n")
        );
    }

    #[test]
    fn header_is_deterministic_across_calls() {
        let conn = build_test_db(&state_with_attendees());
        let first = build_doc_headers(&conn).unwrap();
        let second = build_doc_headers(&conn).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn empty_title_omits_meeting_line() {
        // documents.title is NOT NULL; a missing title is the empty string.
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "", "created_at": "2026-02-01T09:00:00Z"}
            }
        }));
        let headers = build_doc_headers(&conn).unwrap();

        assert_eq!(
            headers.get("doc-1").map(String::as_str),
            Some("Date: 2026-02-01\n\n")
        );
    }

    #[test]
    fn no_people_omits_attendees_line() {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "Solo Notes", "created_at": "2026-02-01T09:00:00Z"}
            }
        }));
        let headers = build_doc_headers(&conn).unwrap();

        assert_eq!(
            headers.get("doc-1").map(String::as_str),
            Some("Meeting: Solo Notes\nDate: 2026-02-01\n\n")
        );
    }

    #[test]
    fn attendee_without_name_falls_back_to_email() {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Standup",
                    "created_at": "2026-02-01T09:00:00Z",
                    "people": {"attendees": [{"email": "charlie@example.com"}]}
                }
            }
        }));
        let headers = build_doc_headers(&conn).unwrap();

        assert_eq!(
            headers.get("doc-1").map(String::as_str),
            Some("Meeting: Standup\nDate: 2026-02-01\nAttendees: charlie@example.com\n\n")
        );
    }

    #[test]
    fn duplicate_names_appear_once() {
        // Creator also listed as attendee under the same name.
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Sync",
                    "created_at": "2026-02-01T09:00:00Z",
                    "people": {
                        "creator": {"name": "Alice Smith", "email": "alice@example.com"},
                        "attendees": [
                            {"email": "alice@example.com", "details": {"person": {"name": {"fullName": "Alice Smith"}}}}
                        ]
                    }
                }
            }
        }));
        let headers = build_doc_headers(&conn).unwrap();

        assert_eq!(
            headers.get("doc-1").map(String::as_str),
            Some("Meeting: Sync\nDate: 2026-02-01\nAttendees: Alice Smith\n\n")
        );
    }

    #[test]
    fn header_never_exceeds_cap_and_stays_well_formed() {
        // Enough long names to blow past the cap: names that don't fit are
        // dropped, the header stays under HEADER_MAX_CHARS and keeps its
        // trailing blank line.
        let attendees: Vec<serde_json::Value> = (0..40)
            .map(|i| {
                json!({
                    "email": format!("person{:02}@example.com", i),
                    "details": {"person": {"name": {"fullName": format!("Attendee Number {:02} With A Long Name", i)}}}
                })
            })
            .collect();
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "All Hands",
                    "created_at": "2026-02-01T09:00:00Z",
                    "people": {"attendees": attendees}
                }
            }
        }));
        let headers = build_doc_headers(&conn).unwrap();

        let header = headers.get("doc-1").unwrap();
        assert!(header.len() <= HEADER_MAX_CHARS, "header is {} bytes", header.len());
        assert!(header.starts_with("Meeting: All Hands\nDate: 2026-02-01\nAttendees: Attendee Number 00"));
        assert!(header.ends_with("\n\n"));
    }

    #[test]
    fn pathological_title_is_truncated_to_the_cap() {
        // A huge multi-byte title must not blow the header cap: unchecked,
        // it would consume the chunker's entire per-document budget and
        // stall the oversized-split loop. The cap applies to the whole
        // assembled header, not just the attendees line.
        let title = "Budget\u{2019}s ".repeat(400); // ~3600 bytes, multi-byte
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": title, "created_at": "2026-02-01T09:00:00Z"}
            }
        }));
        let headers = build_doc_headers(&conn).unwrap();

        let header = headers.get("doc-1").unwrap();
        assert!(header.len() <= HEADER_MAX_CHARS, "header is {} bytes", header.len());
        assert!(header.starts_with("Meeting: Budget"));
        assert!(header.contains("\nDate: 2026-02-01\n"));
        assert!(header.ends_with("\n\n"));
    }

    #[test]
    fn deleted_documents_get_no_header() {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Gone",
                    "created_at": "2026-02-01T09:00:00Z",
                    "deleted_at": "2026-02-02T09:00:00Z"
                }
            }
        }));
        let headers = build_doc_headers(&conn).unwrap();

        assert!(headers.is_empty());
    }
}
