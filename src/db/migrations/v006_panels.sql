-- Panels: AI-generated meeting notes from the get-document-panels API

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
    extra_json TEXT
);

CREATE INDEX idx_panels_document_id ON panels(document_id);

-- Tracks per-document panel sync attempts/failures (same pattern as transcript_sync_log)
CREATE TABLE panel_sync_log (
    document_id TEXT PRIMARY KEY REFERENCES documents(id),
    status TEXT NOT NULL,
    last_attempted_at TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 1
);

-- Full-text search on panel markdown content
CREATE VIRTUAL TABLE panels_fts USING fts5(
    content_markdown,
    content='panels',
    content_rowid='rowid'
);
