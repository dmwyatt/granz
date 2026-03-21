//! Database sync operations for upserting data from the Granola API.
//!
//! All sync operations are upserts: new records are inserted, existing records
//! are updated based on their primary key (usually `id`).

use anyhow::Result;
use rusqlite::Connection;

use crate::api::types::GetRecipesResponse;
use crate::models::{
    CalendarEvent, Document, DocumentPeople, PanelTemplate, Person, Recipe,
};

// ============================================================================
// Sync Statistics
// ============================================================================

/// Statistics from a sync operation
#[derive(Debug, Default, Clone, Copy)]
pub struct SyncStats {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub errors: usize,
}

// ============================================================================
// Serialization helpers: domain → JSON column values
// ============================================================================

struct DocumentJsonFields {
    people_json: Option<String>,
    event_json: Option<String>,
    extra_json: Option<String>,
    raw_json: Option<String>,
}

fn serialize_document_json(doc: &Document) -> DocumentJsonFields {
    DocumentJsonFields {
        people_json: doc.people.as_ref().and_then(|p| serde_json::to_string(p).ok()),
        event_json: doc.google_calendar_event.as_ref().and_then(|e| serde_json::to_string(e).ok()),
        extra_json: if doc.extra.is_empty() { None } else { serde_json::to_string(&doc.extra).ok() },
        raw_json: serde_json::to_string(doc).ok(),
    }
}

struct EventJsonFields {
    start_time: Option<String>,
    end_time: Option<String>,
    attendees_json: Option<String>,
    conference_data_json: Option<String>,
    extra_json: Option<String>,
    raw_json: Option<String>,
}

fn serialize_event_json(event: &CalendarEvent) -> EventJsonFields {
    EventJsonFields {
        start_time: event.start.as_ref().and_then(|s| s.date_time.clone()),
        end_time: event.end.as_ref().and_then(|e| e.date_time.clone()),
        attendees_json: event.attendees.as_ref().and_then(|a| serde_json::to_string(a).ok()),
        conference_data_json: event.conference_data.as_ref().and_then(|c| serde_json::to_string(c).ok()),
        extra_json: if event.extra.is_empty() { None } else { serde_json::to_string(&event.extra).ok() },
        raw_json: serde_json::to_string(event).ok(),
    }
}

struct TemplateJsonFields {
    sections_json: Option<String>,
    chat_suggestions_json: Option<String>,
    extra_json: Option<String>,
    raw_json: Option<String>,
}

fn serialize_template_json(template: &PanelTemplate) -> TemplateJsonFields {
    TemplateJsonFields {
        sections_json: template.sections.as_ref().and_then(|s| serde_json::to_string(s).ok()),
        chat_suggestions_json: template.chat_suggestions.as_ref().and_then(|c| serde_json::to_string(c).ok()),
        extra_json: if template.extra.is_empty() { None } else { serde_json::to_string(&template.extra).ok() },
        raw_json: serde_json::to_string(template).ok(),
    }
}

struct RecipeJsonFields {
    config_json: Option<String>,
    extra_json: Option<String>,
    raw_json: Option<String>,
}

fn serialize_recipe_json(recipe: &Recipe) -> RecipeJsonFields {
    RecipeJsonFields {
        config_json: recipe.config.as_ref().and_then(|c| serde_json::to_string(c).ok()),
        extra_json: if recipe.extra.is_empty() { None } else { serde_json::to_string(&recipe.extra).ok() },
        raw_json: serde_json::to_string(recipe).ok(),
    }
}

// ============================================================================
// Document Sync
// ============================================================================

/// Upsert documents from the API into the database.
/// Returns counts of inserted, updated, unchanged.
pub fn upsert_documents(conn: &Connection, documents: &[Document]) -> Result<SyncStats> {
    let mut stats = SyncStats::default();

    let initial_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM documents", [], |r| r.get(0),
    )?;

    let mut upsert_stmt = conn.prepare(
        "INSERT INTO documents (id, title, created_at, updated_at, deleted_at, doc_type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json, extra_json, raw_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            deleted_at = excluded.deleted_at,
            doc_type = excluded.doc_type,
            notes_plain = excluded.notes_plain,
            notes_markdown = excluded.notes_markdown,
            summary = excluded.summary,
            people_json = excluded.people_json,
            google_calendar_event_json = excluded.google_calendar_event_json,
            extra_json = excluded.extra_json,
            raw_json = excluded.raw_json
         WHERE excluded.updated_at IS NOT documents.updated_at",
    )?;

    for doc in documents {
        let Some(doc_id) = doc.id.as_deref() else {
            eprintln!("Warning: skipping document without ID");
            continue;
        };
        let json = serialize_document_json(doc);

        upsert_stmt.execute(rusqlite::params![
            doc_id,
            &doc.title,
            &doc.created_at,
            &doc.updated_at,
            &doc.deleted_at,
            &doc.doc_type,
            &doc.notes_plain,
            &doc.notes_markdown,
            &doc.summary,
            &json.people_json,
            &json.event_json,
            &json.extra_json,
            &json.raw_json,
        ])?;

        if conn.changes() > 0 {
            // Upsert document_people on insert or update
            if let Some(people) = &doc.people {
                if let Err(e) = upsert_document_people(conn, doc_id, people) {
                    eprintln!("[grans] Warning: Failed to upsert people for {}: {}", doc_id, e);
                }
            }
        } else {
            stats.unchanged += 1;
        }
    }

    let final_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM documents", [], |r| r.get(0),
    )?;
    stats.inserted = (final_count - initial_count) as usize;
    stats.updated = documents.len() - stats.unchanged - stats.inserted
        - documents.iter().filter(|d| d.id.is_none()).count();

    Ok(stats)
}

/// Upsert document_people entries for a document
fn upsert_document_people(conn: &Connection, document_id: &str, people: &DocumentPeople) -> Result<()> {
    // Delete existing entries for this document
    conn.execute(
        "DELETE FROM document_people WHERE document_id = ?1",
        [document_id],
    )?;

    let mut stmt = conn.prepare(
        "INSERT INTO document_people (document_id, email, full_name, role, source)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    // Insert creator
    if let Some(creator) = &people.creator {
        stmt.execute(rusqlite::params![
            document_id,
            &creator.email,
            &creator.name,
            "creator",
            "people",
        ])?;
    }

    // Insert attendees
    if let Some(attendees) = &people.attendees {
        for attendee in attendees {
            stmt.execute(rusqlite::params![
                document_id,
                &attendee.email,
                &attendee.name,
                "attendee",
                "people",
            ])?;
        }
    }

    Ok(())
}

// ============================================================================
// People Sync
// ============================================================================

/// Upsert people from the API into the database.
pub fn upsert_people(conn: &Connection, people: &[Person]) -> Result<SyncStats> {
    let mut stats = SyncStats::default();

    let initial_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM people", [], |r| r.get(0),
    )?;

    let mut upsert_stmt = conn.prepare(
        "INSERT INTO people (id, name, email, company_name, job_title, extra_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            email = excluded.email,
            company_name = excluded.company_name,
            job_title = excluded.job_title,
            extra_json = excluded.extra_json",
    )?;

    for person in people {
        let Some(person_id) = person.id.as_deref() else {
            eprintln!("Warning: skipping person without ID");
            continue;
        };
        let extra_json = if person.extra.is_empty() { None } else { serde_json::to_string(&person.extra).ok() };

        upsert_stmt.execute(rusqlite::params![
            person_id,
            &person.name,
            &person.email,
            &person.company_name,
            &person.job_title,
            &extra_json,
        ])?;
    }

    let final_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM people", [], |r| r.get(0),
    )?;
    stats.inserted = (final_count - initial_count) as usize;
    stats.updated = people.len() - stats.inserted
        - people.iter().filter(|p| p.id.is_none()).count();

    Ok(stats)
}

// ============================================================================
// Calendar Event Sync
// ============================================================================

/// Upsert calendar events from the API into the database.
pub fn upsert_calendar_events(conn: &Connection, events: &[CalendarEvent]) -> Result<SyncStats> {
    let mut stats = SyncStats::default();

    let initial_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events", [], |r| r.get(0),
    )?;

    let mut upsert_stmt = conn.prepare(
        "INSERT INTO events (id, summary, start_time, end_time, calendar_id, attendees_json, conference_data_json, description, extra_json, raw_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            summary = excluded.summary,
            start_time = excluded.start_time,
            end_time = excluded.end_time,
            calendar_id = excluded.calendar_id,
            attendees_json = excluded.attendees_json,
            conference_data_json = excluded.conference_data_json,
            description = excluded.description,
            extra_json = excluded.extra_json,
            raw_json = excluded.raw_json",
    )?;

    for event in events {
        let Some(event_id) = event.id.as_deref() else {
            eprintln!("Warning: skipping calendar event without ID");
            continue;
        };
        let json = serialize_event_json(event);

        upsert_stmt.execute(rusqlite::params![
            event_id,
            &event.summary,
            &json.start_time,
            &json.end_time,
            &event.calendar_id,
            &json.attendees_json,
            &json.conference_data_json,
            &event.description,
            &json.extra_json,
            &json.raw_json,
        ])?;
    }

    let final_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events", [], |r| r.get(0),
    )?;
    stats.inserted = (final_count - initial_count) as usize;
    stats.updated = events.len() - stats.inserted
        - events.iter().filter(|e| e.id.is_none()).count();

    Ok(stats)
}

// ============================================================================
// Calendar Sync (from selected calendars)
// ============================================================================

/// Upsert calendars into the database.
/// Note: The API returns calendars_selected as a map, not full calendar objects.
/// This function is for storing calendar preferences.
pub fn upsert_calendars_from_selection(
    conn: &Connection,
    calendars_selected: &std::collections::HashMap<String, bool>,
    enabled_calendars: &[String],
) -> Result<SyncStats> {
    let mut stats = SyncStats::default();

    let initial_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM calendars", [], |r| r.get(0),
    )?;

    // For calendars, we store the selection state. The full calendar info comes from events.
    for (calendar_id, selected) in calendars_selected {
        let provider = if enabled_calendars.contains(&"google".to_string()) {
            "google"
        } else if enabled_calendars.contains(&"apple".to_string()) {
            "apple"
        } else {
            "unknown"
        };

        if *selected {
            conn.execute(
                "INSERT INTO calendars (id, provider, \"primary\", access_role, summary, background_color)
                 VALUES (?1, ?2, 0, NULL, ?1, NULL)
                 ON CONFLICT(id) DO UPDATE SET provider = excluded.provider",
                rusqlite::params![calendar_id, provider],
            )?;
        } else {
            // Unselected calendar: update provider if it exists, don't insert
            conn.execute(
                "UPDATE calendars SET provider = ?2 WHERE id = ?1",
                rusqlite::params![calendar_id, provider],
            )?;
        }
    }

    let final_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM calendars", [], |r| r.get(0),
    )?;
    stats.inserted = (final_count - initial_count) as usize;
    stats.updated = calendars_selected.len() - stats.inserted;

    Ok(stats)
}

// ============================================================================
// Template Sync
// ============================================================================

/// Upsert panel templates from the API into the database.
pub fn upsert_templates(conn: &Connection, templates: &[PanelTemplate]) -> Result<SyncStats> {
    let mut stats = SyncStats::default();

    let initial_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM templates", [], |r| r.get(0),
    )?;

    let mut upsert_stmt = conn.prepare(
        "INSERT INTO templates (id, title, category, symbol, color, description, is_granola, owner_id, sections_json, created_at, updated_at, deleted_at, chat_suggestions_json, extra_json, raw_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            category = excluded.category,
            symbol = excluded.symbol,
            color = excluded.color,
            description = excluded.description,
            is_granola = excluded.is_granola,
            owner_id = excluded.owner_id,
            sections_json = excluded.sections_json,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            deleted_at = excluded.deleted_at,
            chat_suggestions_json = excluded.chat_suggestions_json,
            extra_json = excluded.extra_json,
            raw_json = excluded.raw_json
         WHERE excluded.updated_at IS NOT templates.updated_at",
    )?;

    for template in templates {
        let Some(template_id) = template.id.as_deref() else {
            eprintln!("Warning: skipping template without ID");
            continue;
        };
        let json = serialize_template_json(template);
        let is_granola = template.is_granola.map(|b| if b { 1 } else { 0 });

        upsert_stmt.execute(rusqlite::params![
            template_id,
            &template.title,
            &template.category,
            &template.symbol,
            &template.color,
            &template.description,
            is_granola,
            &template.owner_id,
            &json.sections_json,
            &template.created_at,
            &template.updated_at,
            &template.deleted_at,
            &json.chat_suggestions_json,
            &json.extra_json,
            &json.raw_json,
        ])?;

        if conn.changes() == 0 {
            stats.unchanged += 1;
        }
    }

    let final_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM templates", [], |r| r.get(0),
    )?;
    stats.inserted = (final_count - initial_count) as usize;
    stats.updated = templates.len() - stats.unchanged - stats.inserted
        - templates.iter().filter(|t| t.id.is_none()).count();

    Ok(stats)
}

// ============================================================================
// Recipe Sync
// ============================================================================

/// Upsert recipes from the API into the database.
pub fn upsert_recipes(conn: &Connection, response: &GetRecipesResponse) -> Result<SyncStats> {
    let mut stats = SyncStats::default();

    let initial_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM recipes", [], |r| r.get(0),
    )?;

    let mut upsert_stmt = conn.prepare(
        "INSERT INTO recipes (id, slug, visibility, publisher_slug, creator_name, config_json, created_at, updated_at, deleted_at, user_id, workspace_id, extra_json, raw_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET
            slug = excluded.slug,
            visibility = excluded.visibility,
            publisher_slug = excluded.publisher_slug,
            creator_name = excluded.creator_name,
            config_json = excluded.config_json,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            deleted_at = excluded.deleted_at,
            user_id = excluded.user_id,
            workspace_id = excluded.workspace_id,
            extra_json = excluded.extra_json,
            raw_json = excluded.raw_json
         WHERE excluded.updated_at IS NOT recipes.updated_at",
    )?;

    let mut skipped = 0usize;

    // Process all recipes from the response
    let mut process_recipes = |recipes: &[Recipe], visibility: &str, stats: &mut SyncStats, skipped: &mut usize| -> Result<()> {
        for recipe in recipes {
            let Some(recipe_id) = recipe.id.as_deref() else {
                eprintln!("Warning: skipping recipe without ID");
                *skipped += 1;
                continue;
            };
            let json = serialize_recipe_json(recipe);

            // Use the visibility from the response category if recipe.visibility is None
            let vis = recipe.visibility.as_deref().unwrap_or(visibility);

            upsert_stmt.execute(rusqlite::params![
                recipe_id,
                &recipe.slug,
                vis,
                &recipe.publisher_slug,
                &recipe.creator_name,
                &json.config_json,
                &recipe.created_at,
                &recipe.updated_at,
                &recipe.deleted_at,
                &recipe.user_id,
                &recipe.workspace_id,
                &json.extra_json,
                &json.raw_json,
            ])?;

            if conn.changes() == 0 {
                stats.unchanged += 1;
            }
        }
        Ok(())
    };

    process_recipes(&response.default_recipes, "default", &mut stats, &mut skipped)?;
    process_recipes(&response.public_recipes, "public", &mut stats, &mut skipped)?;
    process_recipes(&response.user_recipes, "user", &mut stats, &mut skipped)?;
    process_recipes(&response.shared_recipes, "shared", &mut stats, &mut skipped)?;
    process_recipes(&response.unlisted_recipes, "unlisted", &mut stats, &mut skipped)?;

    let final_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM recipes", [], |r| r.get(0),
    )?;
    let total_recipes = response.default_recipes.len()
        + response.public_recipes.len()
        + response.user_recipes.len()
        + response.shared_recipes.len()
        + response.unlisted_recipes.len();
    stats.inserted = (final_count - initial_count) as usize;
    stats.updated = total_recipes - stats.unchanged - stats.inserted - skipped;

    Ok(stats)
}

/// Set the last sync time for a given entity type
pub fn set_last_sync_time(conn: &Connection, entity_type: &str) -> Result<()> {
    let key = format!("last_sync_{}", entity_type);
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO metadata (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2",
        rusqlite::params![&key, &now],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use serde_json::json;

    fn empty_state() -> serde_json::Value {
        json!({
            "documents": {},
            "people": [],
            "events": [],
            "calendars": [],
            "panelTemplates": [],
            "publicRecipes": []
        })
    }

    #[test]
    fn test_upsert_documents_insert() {
        let conn = build_test_db(&empty_state());

        let docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Test Meeting".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            doc_type: Some("meeting".to_string()),
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: Some("Test notes".to_string()),
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra: Default::default(),
        }];

        let stats = upsert_documents(&conn, &docs).unwrap();
        assert_eq!(stats.inserted, 1);
        assert_eq!(stats.updated, 0);

        // Verify insertion
        let title: String = conn
            .query_row("SELECT title FROM documents WHERE id = 'doc-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "Test Meeting");
    }

    #[test]
    fn test_upsert_documents_update() {
        let conn = build_test_db(&empty_state());

        // Insert initial document
        let docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Original Title".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            doc_type: None,
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: None,
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra: Default::default(),
        }];
        upsert_documents(&conn, &docs).unwrap();

        // Update with newer timestamp
        let updated_docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Updated Title".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T11:00:00Z".to_string()), // newer
            deleted_at: None,
            doc_type: None,
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: None,
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra: Default::default(),
        }];

        let stats = upsert_documents(&conn, &updated_docs).unwrap();
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.updated, 1);

        let title: String = conn
            .query_row("SELECT title FROM documents WHERE id = 'doc-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "Updated Title");
    }

    #[test]
    fn test_upsert_documents_unchanged() {
        let conn = build_test_db(&empty_state());

        let docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Test Meeting".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            doc_type: None,
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: None,
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra: Default::default(),
        }];

        upsert_documents(&conn, &docs).unwrap();

        // Upsert same document again
        let stats = upsert_documents(&conn, &docs).unwrap();
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.unchanged, 1);
    }

    #[test]
    fn test_upsert_people() {
        let conn = build_test_db(&empty_state());

        let people = vec![Person {
            id: Some("p-1".to_string()),
            user_id: None,
            created_at: None,
            name: Some("Alice".to_string()),
            email: Some("alice@example.com".to_string()),
            avatar: None,
            job_title: Some("Engineer".to_string()),
            company_name: Some("Acme".to_string()),
            company_description: None,
            user_type: None,
            subscription_name: None,
            links: None,
            favorite_panel_templates: None,
            extra: Default::default(),
        }];

        let stats = upsert_people(&conn, &people).unwrap();
        assert_eq!(stats.inserted, 1);

        let name: String = conn
            .query_row("SELECT name FROM people WHERE id = 'p-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Alice");
    }

    #[test]
    fn test_upsert_calendar_events() {
        let conn = build_test_db(&empty_state());

        let events = vec![CalendarEvent {
            id: Some("e-1".to_string()),
            summary: Some("Team Meeting".to_string()),
            description: None,
            start: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T10:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            end: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T11:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            attendees: None,
            creator: None,
            organizer: None,
            conference_data: None,
            recurring_event_id: None,
            ical_uid: None,
            calendar_id: Some("cal-1".to_string()),
            status: None,
            html_link: None,
            extra: Default::default(),
        }];

        let stats = upsert_calendar_events(&conn, &events).unwrap();
        assert_eq!(stats.inserted, 1);

        let summary: String = conn
            .query_row("SELECT summary FROM events WHERE id = 'e-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(summary, "Team Meeting");
    }

    #[test]
    fn test_upsert_templates() {
        let conn = build_test_db(&empty_state());

        let templates = vec![PanelTemplate {
            id: Some("t-1".to_string()),
            title: Some("Meeting Notes".to_string()),
            description: Some("Standard meeting template".to_string()),
            category: Some("General".to_string()),
            color: Some("blue".to_string()),
            symbol: Some("📝".to_string()),
            is_granola: Some(true),
            owner_id: None,
            sections: None,
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            shared_with: None,
            copied_from: None,
            chat_suggestions: None,
            user_types: None,
            extra: Default::default(),
        }];

        let stats = upsert_templates(&conn, &templates).unwrap();
        assert_eq!(stats.inserted, 1);

        let title: String = conn
            .query_row("SELECT title FROM templates WHERE id = 't-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "Meeting Notes");
    }

    #[test]
    fn test_upsert_recipes() {
        let conn = build_test_db(&empty_state());

        let response = GetRecipesResponse {
            default_recipes: vec![],
            public_recipes: vec![Recipe {
                id: Some("r-1".to_string()),
                slug: Some("test-recipe".to_string()),
                user_id: None,
                workspace_id: None,
                config: None,
                created_at: Some("2026-01-20T10:00:00Z".to_string()),
                updated_at: Some("2026-01-20T10:00:00Z".to_string()),
                deleted_at: None,
                visibility: Some("public".to_string()),
                creation_context: None,
                source_recipe_id: None,
                publisher_slug: Some("granola".to_string()),
                creator_name: Some("Test User".to_string()),
                creator_avatar: None,
                creator_info: None,
                shared_with: None,
                extra: Default::default(),
            }],
            user_recipes: vec![],
            shared_recipes: vec![],
            unlisted_recipes: vec![],
        };

        let stats = upsert_recipes(&conn, &response).unwrap();
        assert_eq!(stats.inserted, 1);

        let slug: String = conn
            .query_row("SELECT slug FROM recipes WHERE id = 'r-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(slug, "test-recipe");
    }

    #[test]
    fn test_sync_metadata() {
        let conn = build_test_db(&empty_state());

        // Set sync time
        set_last_sync_time(&conn, "documents").unwrap();

        // Verify it was written to the database
        let time: String = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'last_sync_documents'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!time.is_empty());
    }

    #[test]
    fn test_upsert_documents_skips_missing_id() {
        let conn = build_test_db(&empty_state());

        let docs = vec![
            Document {
                id: None,
                title: Some("No ID Doc".to_string()),
                ..Default::default()
            },
            Document {
                id: Some("doc-1".to_string()),
                title: Some("Has ID".to_string()),
                updated_at: Some("2026-01-20T10:00:00Z".to_string()),
                ..Default::default()
            },
        ];

        let stats = upsert_documents(&conn, &docs).unwrap();
        assert_eq!(stats.inserted, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_upsert_documents_stores_raw_json() {
        let conn = build_test_db(&empty_state());

        let docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Test Meeting".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            doc_type: Some("meeting".to_string()),
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: Some("Test notes".to_string()),
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra: Default::default(),
        }];

        upsert_documents(&conn, &docs).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM documents WHERE id = 'doc-1'", [], |r| r.get(0))
            .unwrap();
        assert!(raw_json.is_some());

        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["id"], "doc-1");
        assert_eq!(parsed["title"], "Test Meeting");
        assert_eq!(parsed["notes_plain"], "Test notes");
    }

    #[test]
    fn test_upsert_documents_updates_raw_json() {
        let conn = build_test_db(&empty_state());

        let docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Original Title".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            doc_type: None,
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: None,
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra: Default::default(),
        }];
        upsert_documents(&conn, &docs).unwrap();

        // Update with newer timestamp and different title
        let updated_docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Updated Title".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T11:00:00Z".to_string()),
            deleted_at: None,
            doc_type: None,
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: None,
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra: Default::default(),
        }];
        upsert_documents(&conn, &updated_docs).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM documents WHERE id = 'doc-1'", [], |r| r.get(0))
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["title"], "Updated Title");
    }

    #[test]
    fn test_upsert_documents_extra_json() {
        let conn = build_test_db(&empty_state());

        let mut extra = std::collections::HashMap::new();
        extra.insert("custom_field".to_string(), json!("custom_value"));

        let docs = vec![Document {
            id: Some("doc-1".to_string()),
            title: Some("Test Meeting".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            doc_type: None,
            user_id: None,
            workspace_id: None,
            notes: None,
            notes_plain: None,
            notes_markdown: None,
            summary: None,
            google_calendar_event: None,
            people: None,
            privacy_mode_enabled: None,
            sharing_link_visibility: None,
            creation_source: None,
            visibility: None,
            status: None,
            extra,
        }];

        let stats = upsert_documents(&conn, &docs).unwrap();
        assert_eq!(stats.inserted, 1);

        // Verify extra_json was persisted
        let extra_json: Option<String> = conn
            .query_row("SELECT extra_json FROM documents WHERE id = 'doc-1'", [], |r| r.get(0))
            .unwrap();
        assert!(extra_json.is_some());
        let parsed: serde_json::Value = serde_json::from_str(&extra_json.unwrap()).unwrap();
        assert_eq!(parsed["custom_field"], "custom_value");
    }

    #[test]
    fn test_upsert_calendar_events_with_attendees_and_description() {
        let conn = build_test_db(&empty_state());

        let events = vec![CalendarEvent {
            id: Some("e-1".to_string()),
            summary: Some("Team Meeting".to_string()),
            description: Some("Discuss project updates".to_string()),
            start: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T10:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            end: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T11:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            attendees: Some(vec![
                crate::models::EventAttendee {
                    email: Some("alice@example.com".to_string()),
                    display_name: Some("Alice".to_string()),
                    response_status: Some("accepted".to_string()),
                    is_self: None,
                    organizer: None,
                    extra: Default::default(),
                }
            ]),
            creator: None,
            organizer: None,
            conference_data: Some(json!({"entryPointUri": "https://meet.google.com/abc-def"})),
            recurring_event_id: None,
            ical_uid: None,
            calendar_id: Some("cal-1".to_string()),
            status: None,
            html_link: None,
            extra: Default::default(),
        }];

        let stats = upsert_calendar_events(&conn, &events).unwrap();
        assert_eq!(stats.inserted, 1);

        // Verify new columns were persisted
        let (description, attendees_json, conference_data_json): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT description, attendees_json, conference_data_json FROM events WHERE id = 'e-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            )
            .unwrap();

        assert_eq!(description.as_deref(), Some("Discuss project updates"));

        let attendees: Vec<serde_json::Value> = serde_json::from_str(&attendees_json.unwrap()).unwrap();
        assert_eq!(attendees.len(), 1);
        assert_eq!(attendees[0]["email"], "alice@example.com");

        let conf_data: serde_json::Value = serde_json::from_str(&conference_data_json.unwrap()).unwrap();
        assert_eq!(conf_data["entryPointUri"], "https://meet.google.com/abc-def");
    }

    #[test]
    fn test_upsert_templates_with_chat_suggestions() {
        let conn = build_test_db(&empty_state());

        let templates = vec![PanelTemplate {
            id: Some("t-1".to_string()),
            title: Some("Meeting Notes".to_string()),
            description: Some("Standard meeting template".to_string()),
            category: Some("General".to_string()),
            color: Some("blue".to_string()),
            symbol: Some("📝".to_string()),
            is_granola: Some(true),
            owner_id: None,
            sections: None,
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            shared_with: None,
            copied_from: None,
            chat_suggestions: Some(vec![
                crate::models::ChatSuggestion {
                    label: Some("Summarize".to_string()),
                    message: Some("Please summarize this meeting".to_string()),
                }
            ]),
            user_types: None,
            extra: Default::default(),
        }];

        let stats = upsert_templates(&conn, &templates).unwrap();
        assert_eq!(stats.inserted, 1);

        // Verify chat_suggestions_json was persisted
        let chat_suggestions_json: Option<String> = conn
            .query_row("SELECT chat_suggestions_json FROM templates WHERE id = 't-1'", [], |r| r.get(0))
            .unwrap();
        assert!(chat_suggestions_json.is_some());
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&chat_suggestions_json.unwrap()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["label"], "Summarize");
    }

    #[test]
    fn test_upsert_people_extra_json() {
        let conn = build_test_db(&empty_state());

        let mut extra = std::collections::HashMap::new();
        extra.insert("linkedin_url".to_string(), json!("https://linkedin.com/in/alice"));

        let people = vec![Person {
            id: Some("p-1".to_string()),
            user_id: None,
            created_at: None,
            name: Some("Alice".to_string()),
            email: Some("alice@example.com".to_string()),
            avatar: None,
            job_title: Some("Engineer".to_string()),
            company_name: Some("Acme".to_string()),
            company_description: None,
            user_type: None,
            subscription_name: None,
            links: None,
            favorite_panel_templates: None,
            extra,
        }];

        let stats = upsert_people(&conn, &people).unwrap();
        assert_eq!(stats.inserted, 1);

        // Verify extra_json was persisted
        let extra_json: Option<String> = conn
            .query_row("SELECT extra_json FROM people WHERE id = 'p-1'", [], |r| r.get(0))
            .unwrap();
        assert!(extra_json.is_some());
        let parsed: serde_json::Value = serde_json::from_str(&extra_json.unwrap()).unwrap();
        assert_eq!(parsed["linkedin_url"], "https://linkedin.com/in/alice");
    }

    #[test]
    fn test_upsert_templates_stores_raw_json() {
        let conn = build_test_db(&empty_state());

        let templates = vec![PanelTemplate {
            id: Some("t-1".to_string()),
            title: Some("Meeting Notes".to_string()),
            description: Some("Standard meeting template".to_string()),
            category: Some("General".to_string()),
            color: Some("blue".to_string()),
            symbol: Some("M".to_string()),
            is_granola: Some(true),
            owner_id: None,
            sections: None,
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            deleted_at: None,
            shared_with: None,
            copied_from: None,
            chat_suggestions: None,
            user_types: None,
            extra: Default::default(),
        }];

        upsert_templates(&conn, &templates).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM templates WHERE id = 't-1'", [], |r| r.get(0))
            .unwrap();
        assert!(raw_json.is_some());

        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["id"], "t-1");
        assert_eq!(parsed["title"], "Meeting Notes");
    }

    #[test]
    fn test_upsert_templates_updates_raw_json() {
        let conn = build_test_db(&empty_state());

        let templates = vec![PanelTemplate {
            id: Some("t-1".to_string()),
            title: Some("Original Title".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T10:00:00Z".to_string()),
            ..Default::default()
        }];
        upsert_templates(&conn, &templates).unwrap();

        let updated_templates = vec![PanelTemplate {
            id: Some("t-1".to_string()),
            title: Some("Updated Title".to_string()),
            created_at: Some("2026-01-20T10:00:00Z".to_string()),
            updated_at: Some("2026-01-20T11:00:00Z".to_string()),
            ..Default::default()
        }];
        upsert_templates(&conn, &updated_templates).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM templates WHERE id = 't-1'", [], |r| r.get(0))
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["title"], "Updated Title");
    }

    #[test]
    fn test_upsert_calendar_events_stores_raw_json() {
        let conn = build_test_db(&empty_state());

        let events = vec![CalendarEvent {
            id: Some("e-1".to_string()),
            summary: Some("Team Meeting".to_string()),
            description: None,
            start: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T10:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            end: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T11:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            attendees: None,
            creator: None,
            organizer: None,
            conference_data: None,
            recurring_event_id: None,
            ical_uid: None,
            calendar_id: Some("cal-1".to_string()),
            status: None,
            html_link: None,
            extra: Default::default(),
        }];

        upsert_calendar_events(&conn, &events).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM events WHERE id = 'e-1'", [], |r| r.get(0))
            .unwrap();
        assert!(raw_json.is_some());

        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["id"], "e-1");
        assert_eq!(parsed["summary"], "Team Meeting");
    }

    #[test]
    fn test_upsert_calendar_events_updates_raw_json() {
        let conn = build_test_db(&empty_state());

        let events = vec![CalendarEvent {
            id: Some("e-1".to_string()),
            summary: Some("Original Meeting".to_string()),
            start: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T10:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            end: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T11:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            calendar_id: Some("cal-1".to_string()),
            ..Default::default()
        }];
        upsert_calendar_events(&conn, &events).unwrap();

        // Update the event (upsert_calendar_events uses existence check, not timestamp comparison)
        let updated_events = vec![CalendarEvent {
            id: Some("e-1".to_string()),
            summary: Some("Updated Meeting".to_string()),
            start: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T10:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            end: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T11:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            calendar_id: Some("cal-1".to_string()),
            ..Default::default()
        }];
        upsert_calendar_events(&conn, &updated_events).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM events WHERE id = 'e-1'", [], |r| r.get(0))
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["summary"], "Updated Meeting");
    }

    #[test]
    fn test_upsert_recipes_stores_raw_json() {
        let conn = build_test_db(&empty_state());

        let response = GetRecipesResponse {
            default_recipes: vec![],
            public_recipes: vec![Recipe {
                id: Some("r-1".to_string()),
                slug: Some("test-recipe".to_string()),
                user_id: None,
                workspace_id: None,
                config: None,
                created_at: Some("2026-01-20T10:00:00Z".to_string()),
                updated_at: Some("2026-01-20T10:00:00Z".to_string()),
                deleted_at: None,
                visibility: Some("public".to_string()),
                creation_context: None,
                source_recipe_id: None,
                publisher_slug: Some("granola".to_string()),
                creator_name: Some("Test User".to_string()),
                creator_avatar: None,
                creator_info: None,
                shared_with: None,
                extra: Default::default(),
            }],
            user_recipes: vec![],
            shared_recipes: vec![],
            unlisted_recipes: vec![],
        };

        upsert_recipes(&conn, &response).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM recipes WHERE id = 'r-1'", [], |r| r.get(0))
            .unwrap();
        assert!(raw_json.is_some());

        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["id"], "r-1");
        assert_eq!(parsed["slug"], "test-recipe");
    }

    #[test]
    fn test_upsert_recipes_updates_raw_json() {
        let conn = build_test_db(&empty_state());

        let response = GetRecipesResponse {
            default_recipes: vec![],
            public_recipes: vec![Recipe {
                id: Some("r-1".to_string()),
                slug: Some("original-recipe".to_string()),
                created_at: Some("2026-01-20T10:00:00Z".to_string()),
                updated_at: Some("2026-01-20T10:00:00Z".to_string()),
                visibility: Some("public".to_string()),
                ..Default::default()
            }],
            user_recipes: vec![],
            shared_recipes: vec![],
            unlisted_recipes: vec![],
        };
        upsert_recipes(&conn, &response).unwrap();

        // Update with newer timestamp
        let updated_response = GetRecipesResponse {
            default_recipes: vec![],
            public_recipes: vec![Recipe {
                id: Some("r-1".to_string()),
                slug: Some("updated-recipe".to_string()),
                created_at: Some("2026-01-20T10:00:00Z".to_string()),
                updated_at: Some("2026-01-20T11:00:00Z".to_string()),
                visibility: Some("public".to_string()),
                ..Default::default()
            }],
            user_recipes: vec![],
            shared_recipes: vec![],
            unlisted_recipes: vec![],
        };
        upsert_recipes(&conn, &updated_response).unwrap();

        let raw_json: Option<String> = conn
            .query_row("SELECT raw_json FROM recipes WHERE id = 'r-1'", [], |r| r.get(0))
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw_json.unwrap()).unwrap();
        assert_eq!(parsed["slug"], "updated-recipe");
    }

    #[test]
    fn test_upsert_recipes_extra_json() {
        let conn = build_test_db(&empty_state());

        let mut extra = std::collections::HashMap::new();
        extra.insert("custom_field".to_string(), json!("custom_value"));

        let response = GetRecipesResponse {
            default_recipes: vec![],
            public_recipes: vec![Recipe {
                id: Some("r-1".to_string()),
                slug: Some("test-recipe".to_string()),
                user_id: None,
                workspace_id: None,
                config: None,
                created_at: Some("2026-01-20T10:00:00Z".to_string()),
                updated_at: Some("2026-01-20T10:00:00Z".to_string()),
                deleted_at: None,
                visibility: Some("public".to_string()),
                creation_context: None,
                source_recipe_id: None,
                publisher_slug: Some("granola".to_string()),
                creator_name: Some("Test User".to_string()),
                creator_avatar: None,
                creator_info: None,
                shared_with: None,
                extra,
            }],
            user_recipes: vec![],
            shared_recipes: vec![],
            unlisted_recipes: vec![],
        };

        let stats = upsert_recipes(&conn, &response).unwrap();
        assert_eq!(stats.inserted, 1);

        // Verify extra_json was persisted
        let extra_json: Option<String> = conn
            .query_row("SELECT extra_json FROM recipes WHERE id = 'r-1'", [], |r| r.get(0))
            .unwrap();
        assert!(extra_json.is_some());
        let parsed: serde_json::Value = serde_json::from_str(&extra_json.unwrap()).unwrap();
        assert_eq!(parsed["custom_field"], "custom_value");
    }

    #[test]
    fn test_serialize_document_json_with_data() {
        let mut extra = std::collections::HashMap::new();
        extra.insert("custom".to_string(), json!("value"));

        let doc = Document {
            id: Some("doc-1".to_string()),
            title: Some("Test".to_string()),
            people: Some(crate::models::DocumentPeople {
                creator: Some(crate::models::DocumentCreator {
                    name: Some("Alice".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            google_calendar_event: Some(json!({"id": "event-1"})),
            extra,
            ..Default::default()
        };

        let json = serialize_document_json(&doc);
        assert!(json.people_json.is_some());
        assert!(json.people_json.unwrap().contains("Alice"));
        assert!(json.event_json.is_some());
        assert!(json.event_json.unwrap().contains("event-1"));
        assert!(json.extra_json.is_some());
        assert!(json.extra_json.unwrap().contains("custom"));
        assert!(json.raw_json.is_some());
    }

    #[test]
    fn test_serialize_document_json_empty_extra() {
        let doc = Document {
            id: Some("doc-1".to_string()),
            ..Default::default()
        };
        let json = serialize_document_json(&doc);
        assert!(json.people_json.is_none());
        assert!(json.event_json.is_none());
        assert!(json.extra_json.is_none());
        assert!(json.raw_json.is_some());
    }

    #[test]
    fn test_serialize_event_json_with_data() {
        let event = CalendarEvent {
            id: Some("e-1".to_string()),
            start: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T10:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            end: Some(crate::models::EventDateTime {
                date_time: Some("2026-01-20T11:00:00Z".to_string()),
                time_zone: None,
                extra: Default::default(),
            }),
            attendees: Some(vec![crate::models::EventAttendee {
                email: Some("alice@example.com".to_string()),
                ..Default::default()
            }]),
            conference_data: Some(json!({"uri": "https://meet.google.com/abc"})),
            ..Default::default()
        };

        let json = serialize_event_json(&event);
        assert_eq!(json.start_time.as_deref(), Some("2026-01-20T10:00:00Z"));
        assert_eq!(json.end_time.as_deref(), Some("2026-01-20T11:00:00Z"));
        assert!(json.attendees_json.unwrap().contains("alice@example.com"));
        assert!(json.conference_data_json.unwrap().contains("meet.google.com"));
        assert!(json.extra_json.is_none());
    }

    #[test]
    fn test_serialize_event_json_empty() {
        let event = CalendarEvent::default();
        let json = serialize_event_json(&event);
        assert!(json.start_time.is_none());
        assert!(json.end_time.is_none());
        assert!(json.attendees_json.is_none());
        assert!(json.conference_data_json.is_none());
        assert!(json.extra_json.is_none());
    }

    #[test]
    fn test_serialize_template_json_with_data() {
        let template = PanelTemplate {
            id: Some("t-1".to_string()),
            sections: Some(vec![crate::models::TemplateSection {
                heading: Some("Notes".to_string()),
                ..Default::default()
            }]),
            chat_suggestions: Some(vec![crate::models::ChatSuggestion {
                label: Some("Summarize".to_string()),
                message: Some("Please summarize".to_string()),
            }]),
            ..Default::default()
        };

        let json = serialize_template_json(&template);
        assert!(json.sections_json.unwrap().contains("Notes"));
        assert!(json.chat_suggestions_json.unwrap().contains("Summarize"));
        assert!(json.extra_json.is_none());
    }

    #[test]
    fn test_serialize_recipe_json_with_data() {
        let recipe = Recipe {
            id: Some("r-1".to_string()),
            config: Some(crate::models::RecipeConfig {
                name: Some("Test Recipe".to_string()),
                model: Some("gpt-4".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let json = serialize_recipe_json(&recipe);
        assert!(json.config_json.unwrap().contains("gpt-4"));
        assert!(json.extra_json.is_none());
    }

    #[test]
    fn test_serialize_recipe_json_empty() {
        let recipe = Recipe::default();
        let json = serialize_recipe_json(&recipe);
        assert!(json.config_json.is_none());
        assert!(json.extra_json.is_none());
    }

    // Regression tests: upsert with NULL updated_at must not fail on re-sync.
    // Previously, the existence check used `row.get::<_, String>(0)` which
    // returned Err on NULL, making .ok() return None ("not found"), causing
    // a duplicate INSERT and UNIQUE constraint failure.

    #[test]
    fn test_upsert_templates_null_updated_at_resync() {
        let conn = build_test_db(&empty_state());

        let templates = vec![PanelTemplate {
            id: Some("t-null".to_string()),
            title: Some("Summary".to_string()),
            updated_at: None,
            ..Default::default()
        }];

        let stats = upsert_templates(&conn, &templates).unwrap();
        assert_eq!(stats.inserted, 1);

        // Re-sync the same template — this must not fail
        let stats = upsert_templates(&conn, &templates).unwrap();
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.unchanged, 1);
    }

    #[test]
    fn test_upsert_documents_null_updated_at_resync() {
        let conn = build_test_db(&empty_state());

        let docs = vec![Document {
            id: Some("doc-null".to_string()),
            title: Some("No Timestamp".to_string()),
            updated_at: None,
            ..Default::default()
        }];

        let stats = upsert_documents(&conn, &docs).unwrap();
        assert_eq!(stats.inserted, 1);

        let stats = upsert_documents(&conn, &docs).unwrap();
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.unchanged, 1);
    }

    #[test]
    fn test_upsert_recipes_null_updated_at_resync() {
        let conn = build_test_db(&empty_state());

        let response = GetRecipesResponse {
            default_recipes: vec![],
            public_recipes: vec![Recipe {
                id: Some("r-null".to_string()),
                slug: Some("no-timestamp".to_string()),
                updated_at: None,
                ..Default::default()
            }],
            user_recipes: vec![],
            shared_recipes: vec![],
            unlisted_recipes: vec![],
        };

        let stats = upsert_recipes(&conn, &response).unwrap();
        assert_eq!(stats.inserted, 1);

        let stats = upsert_recipes(&conn, &response).unwrap();
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.unchanged, 1);
    }
}
