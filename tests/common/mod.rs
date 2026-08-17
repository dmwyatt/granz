#![allow(dead_code)]

use std::path::PathBuf;

use assert_cmd::Command;
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};
use tempfile::TempDir;

/// The real migration files, verbatim. There is no lib target, so tests
/// cannot call `db::migrations`; replaying the same SQL through the same
/// crate keeps the test schema definitionally identical to production,
/// including the `user_version` pragma rusqlite_migration maintains.
const MIGRATION_SQL: &[&str] = &[
    include_str!("../../src/db/migrations/v001_initial_schema.sql"),
    include_str!("../../src/db/migrations/v002_capture_missing_fields.sql"),
    include_str!("../../src/db/migrations/v003_utterance_metadata.sql"),
    include_str!("../../src/db/migrations/v004_make_title_not_null.sql"),
    include_str!("../../src/db/migrations/v005_transcript_sync_log.sql"),
    include_str!("../../src/db/migrations/v006_panels.sql"),
    include_str!("../../src/db/migrations/v007_transcript_utterance_index.sql"),
    include_str!("../../src/db/migrations/v008_panel_chat_url.sql"),
    include_str!("../../src/db/migrations/v009_document_raw_json.sql"),
    include_str!("../../src/db/migrations/v010_rename_audio_source_to_source.sql"),
    include_str!("../../src/db/migrations/v011_raw_json_templates_recipes_events.sql"),
    include_str!("../../src/db/migrations/v012_rename_is_primary_to_primary.sql"),
    include_str!("../../src/db/migrations/v013_api_snapshot.sql"),
    include_str!("../../src/db/migrations/v014_utterance_speaker_name.sql"),
    include_str!("../../src/db/migrations/v015_fts_triggers.sql"),
    include_str!("../../src/db/migrations/v016_titles_fts.sql"),
    include_str!("../../src/db/migrations/v017_account_provenance.sql"),
];

/// A self-contained test environment with a test database and isolated data directory.
pub struct TestEnv {
    pub dir: TempDir,
    pub db_path: PathBuf,
}

impl TestEnv {
    /// Create a test environment with the given JSON state content.
    pub fn with_state(state_json: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data").join("grans");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db_path = data_dir.join("grans.db");

        // Parse the state JSON and insert into database
        let state: serde_json::Value = serde_json::from_str(state_json).unwrap();
        let mut conn = Connection::open(&db_path).unwrap();
        apply_real_migrations(&mut conn);
        insert_test_data(&conn, &state);

        TestEnv { dir, db_path }
    }

    /// Create a test environment with a rich fixture containing known data.
    pub fn with_fixture() -> Self {
        Self::with_state(&fixture_state())
    }

    /// Get a Command configured to run grans with this environment.
    pub fn cmd(&self) -> Command {
        let mut cmd = assert_cmd::cargo_bin_cmd!("grans");
        cmd.env("XDG_DATA_HOME", self.dir.path().join("data"));
        // Ensure no color codes pollute test output
        cmd.env("NO_COLOR", "1");
        cmd
    }

    /// Get a Command with --json flag.
    pub fn cmd_json(&self) -> Command {
        let mut cmd = self.cmd();
        cmd.arg("--json");
        cmd
    }
}

/// Build the schema by replaying the real migrations, exactly as
/// `db::migrations::open_and_migrate` would on a fresh database.
fn apply_real_migrations(conn: &mut Connection) {
    // include_str! makes a deleted or renamed migration a compile error, but
    // a newly added file is invisible until listed above. Count the files on
    // disk so that gap fails loudly instead of drifting.
    let on_disk = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src/db/migrations"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .and_then(|e| e.to_str())
                == Some("sql")
        })
        .count();
    assert_eq!(
        MIGRATION_SQL.len(),
        on_disk,
        "MIGRATION_SQL is out of sync with src/db/migrations; add the new include_str! entry"
    );

    Migrations::new(MIGRATION_SQL.iter().copied().map(M::up).collect())
        .to_latest(conn)
        .unwrap();
}

fn insert_test_data(conn: &Connection, state: &serde_json::Value) {
    // Insert documents and populate document_people
    if let Some(docs) = state.get("documents").and_then(|d| d.as_object()) {
        for (_, doc) in docs {
            let doc_id = doc.get("id").and_then(|v| v.as_str());
            let people_json = doc.get("people").map(|p| p.to_string());
            let event_json = doc.get("google_calendar_event").map(|e| e.to_string());
            conn.execute(
                "INSERT INTO documents (id, title, created_at, updated_at, deleted_at, doc_type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                        "INSERT INTO transcript_utterances (id, document_id, start_timestamp, end_timestamp, text, source, is_final, speaker_name)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            utt.get("id").and_then(|v| v.as_str()),
                            utt.get("document_id").and_then(|v| v.as_str()),
                            utt.get("start_timestamp").and_then(|v| v.as_str()),
                            utt.get("end_timestamp").and_then(|v| v.as_str()),
                            utt.get("text").and_then(|v| v.as_str()),
                            utt.get("source").and_then(|v| v.as_str()),
                            utt.get("is_final").and_then(|v| v.as_bool()),
                            utt.get("speaker_name").and_then(|v| v.as_str()),
                        ],
                    ).unwrap();
                }
            }
        }
    }

    // Insert people
    if let Some(people) = state.get("people").and_then(|p| p.as_array()) {
        for person in people {
            conn.execute(
                "INSERT INTO people (id, name, email, company_name, job_title)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    person.get("id").and_then(|v| v.as_str()),
                    person.get("name").and_then(|v| v.as_str()),
                    person.get("email").and_then(|v| v.as_str()),
                    person.get("company_name").and_then(|v| v.as_str()),
                    person.get("job_title").and_then(|v| v.as_str()),
                ],
            )
            .unwrap();
        }
    }

    // Insert calendars
    if let Some(calendars) = state.get("calendars").and_then(|c| c.as_array()) {
        for cal in calendars {
            conn.execute(
                "INSERT INTO calendars (id, provider, \"primary\", access_role, summary, background_color)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    cal.get("id").and_then(|v| v.as_str()),
                    cal.get("provider").and_then(|v| v.as_str()),
                    cal.get("primary").and_then(|v| v.as_bool()),
                    cal.get("accessRole").and_then(|v| v.as_str()),
                    cal.get("summary").and_then(|v| v.as_str()),
                    cal.get("backgroundColor").and_then(|v| v.as_str()),
                ],
            ).unwrap();
        }
    }

    // Insert events
    if let Some(events) = state.get("events").and_then(|e| e.as_array()) {
        for event in events {
            // Extract start_time from nested structure
            let start_time = event
                .get("start")
                .and_then(|s| s.get("dateTime"))
                .and_then(|d| d.as_str());
            let end_time = event
                .get("end")
                .and_then(|e| e.get("dateTime"))
                .and_then(|d| d.as_str());
            conn.execute(
                "INSERT INTO events (id, summary, start_time, end_time, calendar_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    event.get("id").and_then(|v| v.as_str()),
                    event.get("summary").and_then(|v| v.as_str()),
                    start_time,
                    end_time,
                    event.get("calendarId").and_then(|v| v.as_str()),
                ],
            )
            .unwrap();
        }
    }

    // Insert templates
    if let Some(templates) = state.get("panelTemplates").and_then(|t| t.as_array()) {
        for tmpl in templates {
            let sections_json = tmpl.get("sections").map(|s| s.to_string());
            conn.execute(
                "INSERT INTO templates (id, title, category, symbol, color, description, is_granola, owner_id, sections_json, created_at, updated_at, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                ],
            ).unwrap();
        }
    }

    // Insert recipes (both public and user)
    for key in ["publicRecipes", "userRecipes"] {
        if let Some(recipes) = state.get(key).and_then(|r| r.as_array()) {
            for recipe in recipes {
                let config_json = recipe.get("config").map(|c| c.to_string());
                conn.execute(
                    "INSERT INTO recipes (id, slug, visibility, publisher_slug, creator_name, config_json, created_at, updated_at, deleted_at, user_id, workspace_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                            panel.get("extra_json").and_then(|v| v.as_str()),
                            panel.get("chat_url").and_then(|v| v.as_str()),
                        ],
                    ).unwrap();
                }
            }
        }
    }

    // transcript_fts and panels_fts are already populated: the triggers
    // installed by v015 indexed each row as it was inserted, which is the same
    // path production takes. Rebuilding them here would paper over a broken
    // trigger, which is the mistake #85 documents.
    //
    // notes_fts still needs the explicit rebuild, because nothing populates it
    // anywhere -- that is the bug in #85, and until it is fixed this is what
    // makes `--in notes` return anything in tests.
    conn.execute("INSERT INTO notes_fts(notes_fts) VALUES('rebuild')", [])
        .unwrap();
}

/// Build a fixture state with known, deterministic data.
pub fn fixture_state() -> String {
    serde_json::json!({
        "documents": {
            "doc-alpha": {
                "id": "doc-alpha",
                "title": "Project Alpha Kickoff",
                "created_at": "2025-06-15T10:00:00.000Z",
                "updated_at": "2025-06-15T11:00:00.000Z",
                "type": "meeting",

                "notes_plain": "Discussed the project timeline and milestones for Q3 delivery.",
                "notes_markdown": "# Project Alpha\n\nDiscussed the **project timeline** and milestones for Q3 delivery.",
                "summary": "Kickoff meeting for Project Alpha covering timeline and team assignments.",
                "people": {
                    "title": "Project Alpha Kickoff",
                    "creator": {
                        "name": "Alice Johnson",
                        "email": "alice@example.com"
                    },
                    "attendees": [
                        {
                            "email": "alice@example.com",
                            "details": {
                                "person": {
                                    "name": {"fullName": "Alice Johnson"}
                                }
                            }
                        },
                        {
                            "email": "bob@example.com",
                            "details": {
                                "person": {
                                    "name": {"fullName": "Bob Smith"}
                                }
                            }
                        }
                    ]
                },
                "google_calendar_event": {
                    "id": "evt-alpha",
                    "summary": "Project Alpha Kickoff",
                    "start": {"dateTime": "2025-06-15T10:00:00-05:00", "timeZone": "America/Chicago"},
                    "end": {"dateTime": "2025-06-15T11:00:00-05:00", "timeZone": "America/Chicago"},
                    "attendees": [
                        {"email": "alice@example.com", "responseStatus": "accepted"},
                        {"email": "bob@example.com", "responseStatus": "accepted"}
                    ],
                    "calendarId": "alice@example.com",
                    "status": "confirmed"
                }
            },
            "doc-beta": {
                "id": "doc-beta",
                "title": "Beta Feature Review",
                "created_at": "2025-07-20T14:00:00.000Z",
                "updated_at": "2025-07-20T15:00:00.000Z",
                "type": "meeting",

                "notes_plain": "Reviewed the beta feature progress. Performance benchmarks look promising.",
                "notes_markdown": "# Beta Review\n\nReviewed the beta feature progress. **Performance benchmarks** look promising.",
                "summary": "Beta feature review with performance benchmarks.",
                "people": {
                    "title": "Beta Feature Review",
                    "creator": {
                        "name": "Bob Smith",
                        "email": "bob@example.com"
                    },
                    "attendees": [
                        {
                            "email": "bob@example.com",
                            "details": {
                                "person": {
                                    "name": {"fullName": "Bob Smith"}
                                }
                            }
                        },
                        {
                            "email": "carol@widgets.io",
                            "details": {
                                "person": {
                                    "name": {"fullName": "Carol Williams"}
                                }
                            }
                        }
                    ]
                },
                "google_calendar_event": {
                    "id": "evt-beta",
                    "summary": "Beta Feature Review",
                    "start": {"dateTime": "2025-07-20T14:00:00-05:00", "timeZone": "America/Chicago"},
                    "end": {"dateTime": "2025-07-20T15:00:00-05:00", "timeZone": "America/Chicago"},
                    "attendees": [
                        {"email": "bob@example.com", "responseStatus": "accepted"},
                        {"email": "carol@widgets.io", "responseStatus": "tentative"}
                    ],
                    "calendarId": "alice@example.com",
                    "status": "confirmed"
                }
            },
            "doc-gamma": {
                "id": "doc-gamma",
                "title": "Gamma Sprint Planning",
                "created_at": "2025-08-10T09:00:00.000Z",
                "updated_at": "2025-08-10T10:00:00.000Z",
                "type": "meeting",

                "notes_plain": "Sprint planning for the gamma release. Prioritized bug fixes over new features.",
                "notes_markdown": "# Gamma Sprint\n\nSprint planning for the gamma release. Prioritized **bug fixes** over new features.",
                "summary": "Sprint planning session prioritizing bug fixes for gamma release.",
                "people": {
                    "title": "Gamma Sprint Planning",
                    "creator": {
                        "name": "Alice Johnson",
                        "email": "alice@example.com"
                    },
                    "attendees": [
                        {
                            "email": "alice@example.com",
                            "details": {
                                "person": {
                                    "name": {"fullName": "Alice Johnson"}
                                }
                            }
                        },
                        {
                            "email": "carol@widgets.io",
                            "details": {
                                "person": {
                                    "name": {"fullName": "Carol Williams"}
                                }
                            }
                        }
                    ]
                },
                "google_calendar_event": {
                    "id": "evt-gamma",
                    "summary": "Gamma Sprint Planning",
                    "start": {"dateTime": "2025-08-10T09:00:00-05:00", "timeZone": "America/Chicago"},
                    "end": {"dateTime": "2025-08-10T10:00:00-05:00", "timeZone": "America/Chicago"},
                    "attendees": [
                        {"email": "alice@example.com", "responseStatus": "accepted"},
                        {"email": "carol@widgets.io", "responseStatus": "accepted"}
                    ],
                    "calendarId": "alice@example.com",
                    "status": "confirmed"
                }
            }
        },
        "transcripts": {
            "doc-alpha": [
                {
                    "id": "utt-a1",
                    "document_id": "doc-alpha",
                    "start_timestamp": "2025-06-15T10:01:00.000Z",
                    "end_timestamp": "2025-06-15T10:01:30.000Z",
                    "text": "Welcome everyone to the kickoff meeting.",
                    "source": "system",
                    "is_final": true
                },
                {
                    "id": "utt-a2",
                    "document_id": "doc-alpha",
                    "start_timestamp": "2025-06-15T10:01:30.000Z",
                    "end_timestamp": "2025-06-15T10:02:00.000Z",
                    "text": "Today we will discuss the project timeline.",
                    "source": "system",
                    "is_final": true
                },
                {
                    "id": "utt-a3",
                    "document_id": "doc-alpha",
                    "start_timestamp": "2025-06-15T10:02:00.000Z",
                    "end_timestamp": "2025-06-15T10:02:30.000Z",
                    "text": "The deadline for the prototype is September fifteenth.",
                    "source": "system",
                    "is_final": true
                },
                {
                    "id": "utt-a4",
                    "document_id": "doc-alpha",
                    "start_timestamp": "2025-06-15T10:02:30.000Z",
                    "end_timestamp": "2025-06-15T10:03:00.000Z",
                    "text": "We need to finalize resource allocation by next week.",
                    "source": "system",
                    "is_final": true
                },
                {
                    "id": "utt-a5",
                    "document_id": "doc-alpha",
                    "start_timestamp": "2025-06-15T10:03:00.000Z",
                    "end_timestamp": "2025-06-15T10:03:30.000Z",
                    "text": "Any questions before we wrap up?",
                    "source": "system",
                    "is_final": true
                }
            ],
            "doc-beta": [
                {
                    "id": "utt-b1",
                    "document_id": "doc-beta",
                    "start_timestamp": "2025-07-20T14:01:00.000Z",
                    "end_timestamp": "2025-07-20T14:01:30.000Z",
                    "text": "Let us review the performance benchmarks.",
                    "source": "system",
                    "is_final": true,
                    "speaker_name": "Priya Raman"
                },
                {
                    "id": "utt-b2",
                    "document_id": "doc-beta",
                    "start_timestamp": "2025-07-20T14:01:30.000Z",
                    "end_timestamp": "2025-07-20T14:02:00.000Z",
                    "text": "The latency improved by forty percent after optimization.",
                    "source": "system",
                    "is_final": true,
                    "speaker_name": "Priya Nair"
                },
                {
                    "id": "utt-b3",
                    "document_id": "doc-beta",
                    "start_timestamp": "2025-07-20T14:02:00.000Z",
                    "end_timestamp": "2025-07-20T14:02:30.000Z",
                    "text": "We should deploy the prototype to staging next sprint.",
                    "source": "system",
                    "is_final": true,
                    "speaker_name": "Marcus Webb"
                }
            ]
        },
        "people": [
            {
                "id": "person-alice",
                "name": "Alice Johnson",
                "email": "alice@example.com",
                "company_name": "Acme Corp",
                "job_title": "Engineering Manager"
            },
            {
                "id": "person-bob",
                "name": "Bob Smith",
                "email": "bob@example.com",
                "company_name": "Acme Corp",
                "job_title": "Senior Engineer"
            },
            {
                "id": "person-carol",
                "name": "Carol Williams",
                "email": "carol@widgets.io",
                "company_name": "Widgets Inc",
                "job_title": "Product Manager"
            }
        ],
        "calendars": [
            {
                "id": "cal-primary",
                "provider": "google",
                "primary": true,
                "accessRole": "owner",
                "summary": "alice@example.com",
                "backgroundColor": "#4285f4"
            },
            {
                "id": "cal-secondary",
                "provider": "google",
                "primary": false,
                "accessRole": "reader",
                "summary": "Team Calendar",
                "backgroundColor": "#33b679"
            }
        ],
        "events": [
            {
                "id": "evt-alpha",
                "summary": "Project Alpha Kickoff",
                "start": {"dateTime": "2025-06-15T10:00:00-05:00", "timeZone": "America/Chicago"},
                "end": {"dateTime": "2025-06-15T11:00:00-05:00", "timeZone": "America/Chicago"},
                "calendarId": "cal-primary",
                "status": "confirmed",
                "attendees": [
                    {"email": "alice@example.com", "responseStatus": "accepted"},
                    {"email": "bob@example.com", "responseStatus": "accepted"}
                ]
            },
            {
                "id": "evt-beta",
                "summary": "Beta Feature Review",
                "start": {"dateTime": "2025-07-20T14:00:00-05:00", "timeZone": "America/Chicago"},
                "end": {"dateTime": "2025-07-20T15:00:00-05:00", "timeZone": "America/Chicago"},
                "calendarId": "cal-primary",
                "status": "confirmed",
                "attendees": [
                    {"email": "bob@example.com", "responseStatus": "accepted"},
                    {"email": "carol@widgets.io", "responseStatus": "tentative"}
                ]
            },
            {
                "id": "evt-gamma",
                "summary": "Gamma Sprint Planning",
                "start": {"dateTime": "2025-08-10T09:00:00-05:00", "timeZone": "America/Chicago"},
                "end": {"dateTime": "2025-08-10T10:00:00-05:00", "timeZone": "America/Chicago"},
                "calendarId": "cal-primary",
                "status": "confirmed",
                "attendees": [
                    {"email": "alice@example.com", "responseStatus": "accepted"},
                    {"email": "carol@widgets.io", "responseStatus": "accepted"}
                ]
            }
        ],
        "panelTemplates": [
            {
                "id": "tmpl-meeting",
                "title": "Meeting Notes",
                "category": "meetings",
                "symbol": "M",
                "color": "#4285f4",
                "description": "Standard meeting notes template",
                "is_granola": true,
                "sections": [
                    {"title": "Summary", "content": ""},
                    {"title": "Action Items", "content": ""}
                ]
            },
            {
                "id": "tmpl-standup",
                "title": "Daily Standup",
                "category": "agile",
                "symbol": "S",
                "color": "#ea4335",
                "description": "Daily standup template with yesterday/today/blockers",
                "is_granola": false,
                "sections": [
                    {"title": "Yesterday", "content": ""},
                    {"title": "Today", "content": ""},
                    {"title": "Blockers", "content": ""}
                ]
            }
        ],
        "publicRecipes": [
            {
                "id": "recipe-summarize",
                "slug": "meeting-summarizer",
                "visibility": "public",
                "publisher_slug": "granola",
                "creator_name": "Granola Team",
                "config": {
                    "model": "gpt-4",
                    "description": "Summarize meeting notes",
                    "instructions": "Create a concise summary of the meeting."
                }
            }
        ],
        "userRecipes": [
            {
                "id": "recipe-custom",
                "slug": "my-action-items",
                "visibility": "user",
                "publisher_slug": "user123",
                "creator_name": "Alice Johnson",
                "config": {
                    "model": "gpt-4",
                    "description": "Extract action items",
                    "instructions": "List all action items from the meeting."
                }
            }
        ],
        "panels": {
            "doc-alpha": [
                {
                    "id": "panel-alpha-1",
                    "document_id": "doc-alpha",
                    "title": "Summary",
                    "content_markdown": "Discussed project timeline and milestones.",
                    "template_slug": "meeting-notes",
                    "created_at": "2025-06-15T11:00:00.000Z",
                    "chat_url": "https://notes.granola.ai/t/alpha-meeting-123"
                }
            ],
            "doc-beta": [
                {
                    "id": "panel-beta-1",
                    "document_id": "doc-beta",
                    "title": "Notes",
                    "content_markdown": "Performance benchmarks reviewed.",
                    "template_slug": "meeting-notes",
                    "created_at": "2025-07-20T15:00:00.000Z"
                }
            ]
        },
        "sharedRecipes": [],
        "unlistedRecipes": []
    })
    .to_string()
}
