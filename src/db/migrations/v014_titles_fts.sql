-- Full-text search over document titles.
--
-- External-content FTS5 over `documents`, mirroring notes_fts, so title
-- matching is word-based (implicit AND, word-boundary tokens) and bm25-scored,
-- competing with the other sources on relevance instead of pre-empting them.
--
-- DROP + CREATE because FTS5 has no IF NOT EXISTS; the index is rebuilt from
-- `documents` below, so re-running loses nothing.
DROP TABLE IF EXISTS titles_fts;
CREATE VIRTUAL TABLE titles_fts USING fts5(
    title,
    content='documents',
    content_rowid='rowid'
);

-- Backfill from the documents already in the table so databases synced before
-- this migration work without a re-sync.
INSERT INTO titles_fts(titles_fts) VALUES('rebuild');
