-- Track transcript sync failures to prevent head-of-line blocking.
-- Only failures are logged; successful fetches don't need entries because
-- those documents already have transcript_utterances rows.
CREATE TABLE IF NOT EXISTS transcript_sync_log (
    document_id TEXT PRIMARY KEY REFERENCES documents(id),
    status TEXT NOT NULL,
    last_attempted_at TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 1
);
