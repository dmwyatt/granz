//! Shared test fixtures for db module tests
//!
//! This module provides reusable test state builders and database setup helpers
//! to reduce duplication across db module tests.

#[cfg(test)]
use rusqlite::Connection;
#[cfg(test)]
use serde_json::{json, Value};

/// Creates a comprehensive test state with data for all collections
#[cfg(test)]
#[allow(dead_code)]
pub fn full_state() -> Value {
    json!({
        "documents": {
            "doc-1": {
                "id": "doc-1",
                "title": "Test Meeting",
                "created_at": "2026-01-20T10:00:00Z",
                "updated_at": "2026-01-20T11:00:00Z",
                "type": "meeting"
            }
        },
        "transcripts": {
            "doc-1": [
                {"id": "utt-1", "document_id": "doc-1", "text": "Hello everyone"}
            ]
        },
        "people": [
            {"id": "p-1", "name": "Alice Smith", "email": "alice@example.com"}
        ],
        "calendars": [
            {"id": "cal-1", "provider": "google", "primary": true, "summary": "test"}
        ]
    })
}

/// Creates a test state focused on meetings and transcripts
#[cfg(test)]
pub fn meetings_state() -> Value {
    json!({
        "documents": {
            "doc-1": {
                "id": "doc-1",
                "title": "AI Strategy Meeting",
                "created_at": "2026-01-20T10:00:00Z",
                "notes_plain": "Machine learning discussion notes",
                "people": {
                    "creator": {"name": "Alice", "email": "alice@example.com"},
                    "attendees": [
                        {"email": "bob@example.com", "details": {"person": {"name": {"fullName": "Bob Jones"}}}}
                    ]
                }
            },
            "doc-2": {
                "id": "doc-2",
                "title": "Weekly Standup",
                "created_at": "2026-01-22T09:00:00Z",
                "people": {
                    "attendees": [{"email": "charlie@example.com"}]
                }
            }
        },
        "transcripts": {
            "doc-1": [
                {"id": "u1", "document_id": "doc-1", "text": "Hello everyone", "source": "microphone"},
                {"id": "u2", "document_id": "doc-1", "text": "Let's talk about neural networks today", "source": "system"}
            ]
        }
    })
}

/// Creates a test state focused on transcripts
#[cfg(test)]
pub fn transcripts_state() -> Value {
    json!({
        "documents": {
            "doc-1": {"id": "doc-1", "title": "AI Meeting", "created_at": "2026-01-20T10:00:00Z"},
            "doc-2": {"id": "doc-2", "title": "Other Meeting", "created_at": "2026-01-21T10:00:00Z"}
        },
        "transcripts": {
            "doc-1": [
                {"id": "u1", "document_id": "doc-1", "text": "Hello everyone", "source": "microphone"},
                {"id": "u2", "document_id": "doc-1", "text": "Let's talk about neural networks today", "source": "system"},
                {"id": "u3", "document_id": "doc-1", "text": "Great idea", "source": "microphone"}
            ],
            "doc-2": [
                {"id": "u4", "document_id": "doc-2", "text": "Something about neural architectures", "source": "system"}
            ]
        }
    })
}

/// Creates a test state focused on people
#[cfg(test)]
pub fn people_state() -> Value {
    json!({
        "documents": {
            "doc-1": {
                "id": "doc-1",
                "title": "AI Meeting",
                "created_at": "2026-01-20T10:00:00Z",
                "people": {
                    "creator": {"name": "Alice Smith", "email": "alice@example.com"},
                    "attendees": [
                        {"email": "bob@example.com", "details": {"person": {"name": {"fullName": "Bob Jones"}}}}
                    ]
                }
            },
            "doc-2": {
                "id": "doc-2",
                "title": "Standup",
                "created_at": "2026-01-21T09:00:00Z",
                "people": {
                    "attendees": [
                        {"email": "charlie@example.com", "details": {"person": {"name": {"fullName": "Charlie Brown"}}}}
                    ]
                }
            }
        },
        "people": [
            {"id": "p-1", "name": "Alice Smith", "email": "alice@example.com", "company_name": "Acme Corp"},
            {"id": "p-2", "name": "Bob Jones", "email": "bob@example.com", "company_name": "Widgets Inc"}
        ]
    })
}

/// Creates a test state focused on calendars and events
#[cfg(test)]
pub fn calendars_state() -> Value {
    json!({
        "calendars": [
            {"id": "cal-1", "provider": "google", "primary": true, "summary": "primary@example.com"},
            {"id": "cal-2", "provider": "google", "primary": false, "summary": "other@example.com"}
        ],
        "events": [
            {"id": "ev-1", "summary": "Morning Standup", "start_time": "2026-01-20T09:00:00Z", "calendar_id": "cal-1"},
            {"id": "ev-2", "summary": "Afternoon Meeting", "start_time": "2026-01-21T14:00:00Z", "calendar_id": "cal-2"}
        ]
    })
}

/// Creates a test state focused on panels (AI-generated notes)
#[cfg(test)]
pub fn panels_state() -> Value {
    json!({
        "documents": {
            "doc-1": {"id": "doc-1", "title": "AI Meeting", "created_at": "2026-01-20T10:00:00Z"},
            "doc-2": {"id": "doc-2", "title": "Other Meeting", "created_at": "2026-01-21T10:00:00Z"}
        },
        "panels": {
            "doc-1": [
                {
                    "id": "panel-1",
                    "document_id": "doc-1",
                    "title": "Summary",
                    "content_json": "{}",
                    "content_markdown": "### Key Decisions\n\nDiscussed Q1 roadmap and priorities.\n\n### Action Items\n\n- Review the deployment plan\n- Schedule follow-up meeting\n\n### Next Steps\n\nPrepare quarterly report by Friday.",
                    "template_slug": "meeting-notes",
                    "created_at": "2026-01-20T11:00:00Z"
                }
            ]
        }
    })
}

/// Creates a test state focused on panel templates
#[cfg(test)]
pub fn templates_state() -> Value {
    json!({
        "panelTemplates": [
            {"id": "tmpl-1", "title": "Meeting Notes", "category": "general", "is_granola": true, "sections": [{"id": "s1", "title": "Action Items"}]},
            {"id": "tmpl-2", "title": "Standup", "category": "agile", "is_granola": false},
            {"id": "tmpl-deleted", "title": "Deleted", "deleted_at": "2026-01-15T00:00:00Z"}
        ]
    })
}

/// Creates a test state focused on recipes
#[cfg(test)]
pub fn recipes_state() -> Value {
    json!({
        "publicRecipes": [
            {
                "id": "r-1",
                "slug": "summary-recipe",
                "visibility": "public",
                "creator_name": "Admin",
                "config": {"name": "Summary", "description": "Summarize meetings", "instructions": "Summarize the meeting", "model": "gpt-4"}
            }
        ],
        "userRecipes": [
            {"id": "r-2", "slug": "my-recipe", "visibility": "user", "creator_name": "Me"}
        ]
    })
}

/// Creates an in-memory SQLite connection with the provided test state
///
/// This creates tables and inserts data directly from the JSON state.
#[cfg(test)]
pub fn build_test_db(state: &Value) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::create_tables(&conn).unwrap();

    // Insert documents and populate document_people
    if let Some(docs) = state.get("documents").and_then(|d| d.as_object()) {
        for (_, doc) in docs {
            let doc_id = doc.get("id").and_then(|v| v.as_str());
            let people_json = doc.get("people").map(|p| p.to_string());
            let event_json = doc.get("google_calendar_event").map(|e| e.to_string());
            let extra_json = doc.get("extra").map(|e| e.to_string());
            conn.execute(
                "INSERT INTO documents (id, title, created_at, updated_at, deleted_at, doc_type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json, extra_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    doc_id,
                    doc.get("title").and_then(|v| v.as_str()),
                    doc.get("created_at").and_then(|v| v.as_str()),
                    doc.get("updated_at").and_then(|v| v.as_str()),
                    doc.get("deleted_at").and_then(|v| v.as_str()),
                    doc.get("type").and_then(|v| v.as_str()),
                    doc.get("notes_plain").and_then(|v| v.as_str()),
                    doc.get("notes_markdown").and_then(|v| v.as_str()),
                    doc.get("summary").and_then(|v| v.as_str()),
                    people_json,
                    event_json,
                    extra_json,
                ],
            ).unwrap();

            // Extract people from document and insert into document_people
            if let Some(doc_id) = doc_id {
                if let Some(people) = doc.get("people") {
                    // Insert creator
                    if let Some(creator) = people.get("creator") {
                        let email = creator.get("email").and_then(|v| v.as_str());
                        let name = creator.get("name").and_then(|v| v.as_str());
                        conn.execute(
                            "INSERT INTO document_people (document_id, email, full_name, role, source) VALUES (?1, ?2, ?3, 'creator', 'document')",
                            rusqlite::params![doc_id, email, name],
                        ).unwrap();
                    }

                    // Insert attendees
                    if let Some(attendees) = people.get("attendees").and_then(|a| a.as_array()) {
                        for attendee in attendees {
                            let email = attendee.get("email").and_then(|v| v.as_str());
                            // Extract full name from nested structure: details.person.name.fullName
                            let full_name = attendee
                                .get("details")
                                .and_then(|d| d.get("person"))
                                .and_then(|p| p.get("name"))
                                .and_then(|n| n.get("fullName"))
                                .and_then(|f| f.as_str());
                            conn.execute(
                                "INSERT INTO document_people (document_id, email, full_name, role, source) VALUES (?1, ?2, ?3, 'attendee', 'document')",
                                rusqlite::params![doc_id, email, full_name],
                            ).unwrap();
                        }
                    }
                }
            }
        }
    }

    // Insert transcripts
    if let Some(transcripts) = state.get("transcripts").and_then(|t| t.as_object()) {
        for (_, utts) in transcripts {
            if let Some(arr) = utts.as_array() {
                for utt in arr {
                    conn.execute(
                        "INSERT INTO transcript_utterances (id, document_id, start_timestamp, end_timestamp, text, source, is_final)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            utt.get("id").and_then(|v| v.as_str()),
                            utt.get("document_id").and_then(|v| v.as_str()),
                            utt.get("start_timestamp").and_then(|v| v.as_str()),
                            utt.get("end_timestamp").and_then(|v| v.as_str()),
                            utt.get("text").and_then(|v| v.as_str()),
                            utt.get("source").and_then(|v| v.as_str()),
                            utt.get("is_final").and_then(|v| v.as_bool()),
                        ],
                    ).unwrap();
                }
            }
        }
    }

    // Insert people
    if let Some(people) = state.get("people").and_then(|p| p.as_array()) {
        for person in people {
            let extra_json = person.get("extra").map(|e| e.to_string());
            conn.execute(
                "INSERT INTO people (id, name, email, company_name, job_title, extra_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    person.get("id").and_then(|v| v.as_str()),
                    person.get("name").and_then(|v| v.as_str()),
                    person.get("email").and_then(|v| v.as_str()),
                    person.get("company_name").and_then(|v| v.as_str()),
                    person.get("job_title").and_then(|v| v.as_str()),
                    extra_json,
                ],
            ).unwrap();
        }
    }

    // Insert calendars
    if let Some(calendars) = state.get("calendars").and_then(|c| c.as_array()) {
        for cal in calendars {
            let extra_json = cal.get("extra").map(|e| e.to_string());
            conn.execute(
                "INSERT INTO calendars (id, provider, \"primary\", access_role, summary, background_color, extra_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    cal.get("id").and_then(|v| v.as_str()),
                    cal.get("provider").and_then(|v| v.as_str()),
                    cal.get("primary").and_then(|v| v.as_bool()),
                    cal.get("accessRole").and_then(|v| v.as_str()),
                    cal.get("summary").and_then(|v| v.as_str()),
                    cal.get("backgroundColor").and_then(|v| v.as_str()),
                    extra_json,
                ],
            ).unwrap();
        }
    }

    // Insert events
    if let Some(events) = state.get("events").and_then(|e| e.as_array()) {
        for event in events {
            let attendees_json = event.get("attendees").map(|a| a.to_string());
            let conference_data_json = event.get("conference_data").map(|c| c.to_string());
            let extra_json = event.get("extra").map(|e| e.to_string());
            conn.execute(
                "INSERT INTO events (id, summary, start_time, end_time, calendar_id, attendees_json, conference_data_json, description, extra_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    event.get("id").and_then(|v| v.as_str()),
                    event.get("summary").and_then(|v| v.as_str()),
                    event.get("start_time").and_then(|v| v.as_str()),
                    event.get("end_time").and_then(|v| v.as_str()),
                    event.get("calendar_id").and_then(|v| v.as_str()),
                    attendees_json,
                    conference_data_json,
                    event.get("description").and_then(|v| v.as_str()),
                    extra_json,
                ],
            ).unwrap();
        }
    }

    // Insert templates
    if let Some(templates) = state.get("panelTemplates").and_then(|t| t.as_array()) {
        for tmpl in templates {
            let sections_json = tmpl.get("sections").map(|s| s.to_string());
            let chat_suggestions_json = tmpl.get("chat_suggestions").map(|c| c.to_string());
            let extra_json = tmpl.get("extra").map(|e| e.to_string());
            conn.execute(
                "INSERT INTO templates (id, title, category, symbol, color, description, is_granola, owner_id, sections_json, created_at, updated_at, deleted_at, chat_suggestions_json, extra_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                rusqlite::params![
                    tmpl.get("id").and_then(|v| v.as_str()),
                    tmpl.get("title").and_then(|v| v.as_str()),
                    tmpl.get("category").and_then(|v| v.as_str()),
                    tmpl.get("symbol").and_then(|v| v.as_str()),
                    tmpl.get("color").and_then(|v| v.as_str()),
                    tmpl.get("description").and_then(|v| v.as_str()),
                    tmpl.get("is_granola").and_then(|v| v.as_bool()),
                    tmpl.get("owner_id").and_then(|v| v.as_str()),
                    sections_json,
                    tmpl.get("created_at").and_then(|v| v.as_str()),
                    tmpl.get("updated_at").and_then(|v| v.as_str()),
                    tmpl.get("deleted_at").and_then(|v| v.as_str()),
                    chat_suggestions_json,
                    extra_json,
                ],
            ).unwrap();
        }
    }

    // Insert recipes (both public and user)
    for key in ["publicRecipes", "userRecipes"] {
        if let Some(recipes) = state.get(key).and_then(|r| r.as_array()) {
            for recipe in recipes {
                let config_json = recipe.get("config").map(|c| c.to_string());
                let extra_json = recipe.get("extra").map(|e| e.to_string());
                conn.execute(
                    "INSERT INTO recipes (id, slug, visibility, publisher_slug, creator_name, config_json, created_at, updated_at, deleted_at, user_id, workspace_id, extra_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    rusqlite::params![
                        recipe.get("id").and_then(|v| v.as_str()),
                        recipe.get("slug").and_then(|v| v.as_str()),
                        recipe.get("visibility").and_then(|v| v.as_str()),
                        recipe.get("publisher_slug").and_then(|v| v.as_str()),
                        recipe.get("creator_name").and_then(|v| v.as_str()),
                        config_json,
                        recipe.get("created_at").and_then(|v| v.as_str()),
                        recipe.get("updated_at").and_then(|v| v.as_str()),
                        recipe.get("deleted_at").and_then(|v| v.as_str()),
                        recipe.get("user_id").and_then(|v| v.as_str()),
                        recipe.get("workspace_id").and_then(|v| v.as_str()),
                        extra_json,
                    ],
                ).unwrap();
            }
        }
    }

    // Insert panels
    if let Some(panels_by_doc) = state.get("panels").and_then(|p| p.as_object()) {
        for (_, panels) in panels_by_doc {
            if let Some(arr) = panels.as_array() {
                for panel in arr {
                    let extra_json = panel.get("extra").map(|e| e.to_string());
                    conn.execute(
                        "INSERT INTO panels (id, document_id, title, content_json, content_markdown, original_content_json, template_slug, created_at, updated_at, deleted_at, extra_json, chat_url)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                        rusqlite::params![
                            panel.get("id").and_then(|v| v.as_str()),
                            panel.get("document_id").and_then(|v| v.as_str()),
                            panel.get("title").and_then(|v| v.as_str()),
                            panel.get("content_json").and_then(|v| v.as_str()),
                            panel.get("content_markdown").and_then(|v| v.as_str()),
                            panel.get("original_content_json").and_then(|v| v.as_str()),
                            panel.get("template_slug").and_then(|v| v.as_str()),
                            panel.get("created_at").and_then(|v| v.as_str()),
                            panel.get("updated_at").and_then(|v| v.as_str()),
                            panel.get("deleted_at").and_then(|v| v.as_str()),
                            extra_json,
                            panel.get("chat_url").and_then(|v| v.as_str()),
                        ],
                    ).unwrap();
                }
            }
        }
    }

    // Populate FTS indexes
    conn.execute("INSERT INTO transcript_fts(transcript_fts) VALUES('rebuild')", []).unwrap();
    conn.execute("INSERT INTO notes_fts(notes_fts) VALUES('rebuild')", []).unwrap();
    conn.execute("INSERT INTO panels_fts(panels_fts) VALUES('rebuild')", []).unwrap();

    conn
}
