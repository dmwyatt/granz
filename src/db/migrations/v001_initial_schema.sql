-- v001_initial_schema.sql
-- Initial schema combining main tables and embeddings tables
--
-- Note: Uses IF NOT EXISTS to handle upgrading existing databases where
-- tables may already exist from the old schema system.

-- Main content tables
CREATE TABLE IF NOT EXISTS documents (
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
    google_calendar_event_json TEXT
);

CREATE TABLE IF NOT EXISTS transcript_utterances (
    id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL,
    start_timestamp TEXT,
    end_timestamp TEXT,
    text TEXT,
    transcript_source TEXT NOT NULL DEFAULT 'cache',
    FOREIGN KEY (document_id) REFERENCES documents(id)
);

CREATE TABLE IF NOT EXISTS people (
    id TEXT PRIMARY KEY,
    name TEXT,
    email TEXT,
    company_name TEXT,
    job_title TEXT
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    summary TEXT,
    start_time TEXT,
    end_time TEXT,
    calendar_id TEXT
);

CREATE TABLE IF NOT EXISTS calendars (
    id TEXT PRIMARY KEY,
    provider TEXT,
    is_primary INTEGER,
    access_role TEXT,
    summary TEXT,
    background_color TEXT
);

CREATE TABLE IF NOT EXISTS templates (
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
    deleted_at TEXT
);

CREATE TABLE IF NOT EXISTS recipes (
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
    workspace_id TEXT
);

CREATE TABLE IF NOT EXISTS document_people (
    document_id TEXT NOT NULL,
    email TEXT,
    full_name TEXT,
    role TEXT NOT NULL,
    source TEXT NOT NULL,
    FOREIGN KEY (document_id) REFERENCES documents(id)
);

-- FTS5 virtual tables for full-text search
-- Note: DROP + CREATE because FTS5 doesn't support IF NOT EXISTS.
-- These are external content tables (content=) so no data is lost -
-- the index is rebuilt from the source tables on next use.
DROP TABLE IF EXISTS transcript_fts;
CREATE VIRTUAL TABLE transcript_fts USING fts5(
    text,
    content='transcript_utterances',
    content_rowid='rowid'
);

DROP TABLE IF EXISTS notes_fts;
CREATE VIRTUAL TABLE notes_fts USING fts5(
    notes_plain,
    notes_markdown,
    content='documents',
    content_rowid='rowid'
);

-- Embedding tables (merged from embeddings.db)
CREATE TABLE IF NOT EXISTS embedding_metadata (
    key TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE IF NOT EXISTS chunks (
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

CREATE TABLE IF NOT EXISTS embeddings (
    chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
    vector BLOB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_chunks_document ON chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_chunks_source_type ON chunks(source_type);

-- Metadata table for sync timestamps and other key-value storage
CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT
);
