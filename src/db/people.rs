use anyhow::Result;
use rusqlite::Connection;

use super::common::{DocumentRow, row_to_document};
use crate::models::{Document, Person};

struct PersonRow {
    id: Option<String>,
    name: Option<String>,
    email: Option<String>,
    company_name: Option<String>,
    job_title: Option<String>,
    extra_json: Option<String>,
}

fn row_to_person(row: PersonRow) -> Person {
    let extra = row
        .extra_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    Person {
        id: row.id,
        name: row.name,
        email: row.email,
        company_name: row.company_name,
        job_title: row.job_title,
        extra,
        ..Default::default()
    }
}

pub fn list_people(conn: &Connection, company: Option<&str>) -> Result<Vec<Person>> {
    let mut sql =
        String::from("SELECT id, name, email, company_name, job_title, extra_json FROM people WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(company_q) = company {
        sql.push_str(" AND company_name LIKE ?");
        params.push(Box::new(format!("%{}%", company_q)));
    }

    sql.push_str(" ORDER BY name");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(PersonRow {
            id: row.get(0)?,
            name: row.get(1)?,
            email: row.get(2)?,
            company_name: row.get(3)?,
            job_title: row.get(4)?,
            extra_json: row.get(5)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_person).collect())
}

pub fn find_person(conn: &Connection, query: &str) -> Result<Vec<Person>> {
    let pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT id, name, email, company_name, job_title, extra_json FROM people WHERE name LIKE ?1 OR email LIKE ?1",
    )?;

    let rows = stmt.query_map([&pattern], |row| {
        Ok(PersonRow {
            id: row.get(0)?,
            name: row.get(1)?,
            email: row.get(2)?,
            company_name: row.get(3)?,
            job_title: row.get(4)?,
            extra_json: row.get(5)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).map(row_to_person).collect())
}

pub fn find_meetings_by_person(
    conn: &Connection,
    query: &str,
    include_deleted: bool,
) -> Result<Vec<Document>> {
    let pattern = format!("%{}%", query);
    let deleted_filter = if include_deleted { "" } else { " AND d.deleted_at IS NULL" };
    let sql = format!(
        "SELECT DISTINCT d.id, d.title, d.created_at, d.updated_at, d.deleted_at, d.doc_type, d.notes_plain, d.notes_markdown, d.summary, d.people_json, d.google_calendar_event_json
         FROM documents d
         JOIN document_people dp ON d.id = dp.document_id
         WHERE (dp.email LIKE ?1 OR dp.full_name LIKE ?1){}
         ORDER BY d.created_at DESC",
        deleted_filter
    );
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([&pattern], |row| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, people_state};

    #[test]
    fn test_list_people_all() {
        let conn = build_test_db(&people_state());
        let people = list_people(&conn, None).unwrap();
        assert_eq!(people.len(), 2);
    }

    #[test]
    fn test_list_people_by_company() {
        let conn = build_test_db(&people_state());
        let people = list_people(&conn, Some("Acme")).unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].name.as_deref(), Some("Alice Smith"));
    }

    #[test]
    fn test_find_person_by_name() {
        let conn = build_test_db(&people_state());
        let people = find_person(&conn, "alice").unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].email.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn test_find_person_by_email() {
        let conn = build_test_db(&people_state());
        let people = find_person(&conn, "bob@").unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].name.as_deref(), Some("Bob Jones"));
    }

    #[test]
    fn test_find_meetings_by_person() {
        let conn = build_test_db(&people_state());
        let docs = find_meetings_by_person(&conn, "alice", false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title.as_deref(), Some("AI Meeting"));
    }

    #[test]
    fn test_find_meetings_by_person_no_match() {
        let conn = build_test_db(&people_state());
        let docs = find_meetings_by_person(&conn, "nobody", false).unwrap();
        assert!(docs.is_empty());
    }

    #[test]
    fn test_find_meetings_by_person_attendee() {
        let conn = build_test_db(&people_state());
        let docs = find_meetings_by_person(&conn, "bob", false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title.as_deref(), Some("AI Meeting"));
    }

    #[test]
    fn test_row_to_person_with_extra_json() {
        let row = PersonRow {
            id: Some("p-1".to_string()),
            name: Some("Alice".to_string()),
            email: Some("alice@example.com".to_string()),
            company_name: Some("Acme".to_string()),
            job_title: Some("Engineer".to_string()),
            extra_json: Some(r#"{"linkedin":"https://linkedin.com/in/alice"}"#.to_string()),
        };
        let person = row_to_person(row);
        assert_eq!(person.id.as_deref(), Some("p-1"));
        assert_eq!(person.name.as_deref(), Some("Alice"));
        assert_eq!(person.email.as_deref(), Some("alice@example.com"));
        assert_eq!(person.company_name.as_deref(), Some("Acme"));
        assert_eq!(person.job_title.as_deref(), Some("Engineer"));
        assert_eq!(person.extra["linkedin"], "https://linkedin.com/in/alice");
    }

    #[test]
    fn test_row_to_person_without_extra_json() {
        let row = PersonRow {
            id: Some("p-2".to_string()),
            name: Some("Bob".to_string()),
            email: None,
            company_name: None,
            job_title: None,
            extra_json: None,
        };
        let person = row_to_person(row);
        assert_eq!(person.id.as_deref(), Some("p-2"));
        assert!(person.extra.is_empty());
    }

    #[test]
    fn test_row_to_person_invalid_extra_json() {
        let row = PersonRow {
            id: Some("p-3".to_string()),
            name: None,
            email: None,
            company_name: None,
            job_title: None,
            extra_json: Some("not valid json".to_string()),
        };
        let person = row_to_person(row);
        assert!(person.extra.is_empty());
    }

    #[test]
    fn test_find_meetings_by_person_include_deleted() {
        let state = serde_json::json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Active Meeting",
                    "created_at": "2026-01-20T10:00:00Z",
                    "people": {
                        "creator": {"name": "Alice Smith", "email": "alice@example.com"}
                    }
                },
                "doc-deleted": {
                    "id": "doc-deleted",
                    "title": "Deleted Meeting",
                    "created_at": "2026-01-21T10:00:00Z",
                    "deleted_at": "2026-01-22T10:00:00Z",
                    "people": {
                        "creator": {"name": "Alice Smith", "email": "alice@example.com"}
                    }
                }
            },
            "people": [
                {"id": "p-1", "name": "Alice Smith", "email": "alice@example.com"}
            ]
        });
        let conn = build_test_db(&state);

        let docs = find_meetings_by_person(&conn, "alice", false).unwrap();
        assert_eq!(docs.len(), 1);

        let docs = find_meetings_by_person(&conn, "alice", true).unwrap();
        assert_eq!(docs.len(), 2);
    }
}
