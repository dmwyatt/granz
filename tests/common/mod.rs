#![allow(dead_code)]

use std::path::PathBuf;

use assert_cmd::Command;
use rusqlite::Connection;
use tempfile::TempDir;

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
        let conn = Connection::open(&db_path).unwrap();
        create_test_tables(&conn);
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

fn create_test_tables(conn: &Connection) {
    conn.execute_batch(
        r#"
        CREATE TABLE documents (
            id TEXT PRIMARY KEY,
            title TEXT,
            created_at TEXT,
            updated_at TEXT,
            deleted_at TEXT,
            doc_type TEXT,
            notes_plain TEXT,
            notes_markdown TEXT,
            summary TEXT,
            people_json TEXT,
            google_calendar_event_json TEXT,
            extra_json TEXT,
            raw_json TEXT
        );

        CREATE TABLE transcript_utterances (
            id TEXT PRIMARY KEY,
            document_id TEXT NOT NULL,
            start_timestamp TEXT,
            end_timestamp TEXT,
            text TEXT,
            transcript_source TEXT NOT NULL DEFAULT 'cache',
            source TEXT,
            is_final INTEGER,
            api_snapshot TEXT,
            FOREIGN KEY (document_id) REFERENCES documents(id)
        );

        CREATE TABLE people (
            id TEXT PRIMARY KEY,
            name TEXT,
            email TEXT,
            company_name TEXT,
            job_title TEXT,
            extra_json TEXT
        );

        CREATE TABLE events (
            id TEXT PRIMARY KEY,
            summary TEXT,
            start_time TEXT,
            end_time TEXT,
            calendar_id TEXT,
            attendees_json TEXT,
            conference_data_json TEXT,
            description TEXT,
            extra_json TEXT,
            raw_json TEXT
        );

        CREATE TABLE calendars (
            id TEXT PRIMARY KEY,
            provider TEXT,
            "primary" INTEGER,
            access_role TEXT,
            summary TEXT,
            background_color TEXT,
            extra_json TEXT
        );

        CREATE TABLE templates (
            id TEXT PRIMARY KEY,
            title TEXT,
            category TEXT,
            symbol TEXT,
            color TEXT,
            description TEXT,
            is_granola INTEGER,
            owner_id TEXT,
            sections_json TEXT,
            created_at TEXT,
            updated_at TEXT,
            deleted_at TEXT,
            chat_suggestions_json TEXT,
            extra_json TEXT,
            raw_json TEXT
        );

        CREATE TABLE recipes (
            id TEXT PRIMARY KEY,
            slug TEXT,
            visibility TEXT,
            publisher_slug TEXT,
            creator_name TEXT,
            config_json TEXT,
            created_at TEXT,
            updated_at TEXT,
            deleted_at TEXT,
            user_id TEXT,
            workspace_id TEXT,
            extra_json TEXT,
            raw_json TEXT
        );

        CREATE TABLE document_people (
            document_id TEXT NOT NULL,
            email TEXT,
            full_name TEXT,
            role TEXT NOT NULL,
            source TEXT NOT NULL,
            FOREIGN KEY (document_id) REFERENCES documents(id)
        );

        CREATE TABLE metadata (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        -- Embedding tables (merged from embeddings.db)
        CREATE TABLE embedding_metadata (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE TABLE chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_type TEXT NOT NULL,
            source_id TEXT NOT NULL,
            document_id TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            text TEXT NOT NULL,
            metadata_json TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(source_type, source_id)
        );

        CREATE TABLE embeddings (
            chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
            vector BLOB NOT NULL
        );

        CREATE TABLE transcript_sync_log (
            document_id TEXT PRIMARY KEY REFERENCES documents(id),
            status TEXT NOT NULL,
            last_attempted_at TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE panels (
            id TEXT PRIMARY KEY,
            document_id TEXT NOT NULL REFERENCES documents(id),
            title TEXT,
            content_json TEXT,
            content_markdown TEXT,
            original_content_json TEXT,
            template_slug TEXT,
            created_at TEXT,
            updated_at TEXT,
            deleted_at TEXT,
            extra_json TEXT,
            chat_url TEXT,
            api_snapshot TEXT
        );

        CREATE INDEX idx_transcript_utterances_document ON transcript_utterances(document_id);

        CREATE INDEX idx_panels_document_id ON panels(document_id);

        CREATE TABLE panel_sync_log (
            document_id TEXT PRIMARY KEY REFERENCES documents(id),
            status TEXT NOT NULL,
            last_attempted_at TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 1
        );

        CREATE INDEX idx_chunks_document ON chunks(document_id);
        CREATE INDEX idx_chunks_source_type ON chunks(source_type);

        CREATE VIRTUAL TABLE transcript_fts USING fts5(
            text,
            content='transcript_utterances',
            content_rowid='rowid'
        );

        CREATE VIRTUAL TABLE notes_fts USING fts5(
            notes_plain,
            notes_markdown,
            content='documents',
            content_rowid='rowid'
        );

        CREATE VIRTUAL TABLE panels_fts USING fts5(
            content_markdown,
            content='panels',
            content_rowid='rowid'
        );

        -- Set schema version via user_version pragma (used by rusqlite_migration)
        PRAGMA user_version = 13;
        "#,
    )
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
            ).unwrap();
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
            ).unwrap();
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

    // Populate FTS indexes
    conn.execute("INSERT INTO transcript_fts(transcript_fts) VALUES('rebuild')", []).unwrap();
    conn.execute("INSERT INTO notes_fts(notes_fts) VALUES('rebuild')", []).unwrap();
    conn.execute("INSERT INTO panels_fts(panels_fts) VALUES('rebuild')", []).unwrap();
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
                    "is_final": true
                },
                {
                    "id": "utt-b2",
                    "document_id": "doc-beta",
                    "start_timestamp": "2025-07-20T14:01:30.000Z",
                    "end_timestamp": "2025-07-20T14:02:00.000Z",
                    "text": "The latency improved by forty percent after optimization.",
                    "source": "system",
                    "is_final": true
                },
                {
                    "id": "utt-b3",
                    "document_id": "doc-beta",
                    "start_timestamp": "2025-07-20T14:02:00.000Z",
                    "end_timestamp": "2025-07-20T14:02:30.000Z",
                    "text": "We should deploy the prototype to staging next sprint.",
                    "source": "system",
                    "is_final": true
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
