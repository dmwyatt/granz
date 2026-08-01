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

-- Keep the index in step with triggers, mirroring v015_fts_triggers.sql:
-- the index entry is part of the same statement as the row write, so no code
-- path can forget it and no interruption can separate them. The update and
-- delete triggers pass `old` because 'delete' needs the values as indexed.
DROP TRIGGER IF EXISTS documents_ai;
CREATE TRIGGER documents_ai AFTER INSERT ON documents BEGIN
    INSERT INTO titles_fts(rowid, title) VALUES (new.rowid, new.title);
END;

DROP TRIGGER IF EXISTS documents_ad;
CREATE TRIGGER documents_ad AFTER DELETE ON documents BEGIN
    INSERT INTO titles_fts(titles_fts, rowid, title) VALUES('delete', old.rowid, old.title);
END;

DROP TRIGGER IF EXISTS documents_au;
CREATE TRIGGER documents_au AFTER UPDATE ON documents BEGIN
    INSERT INTO titles_fts(titles_fts, rowid, title) VALUES('delete', old.rowid, old.title);
    INSERT INTO titles_fts(rowid, title) VALUES (new.rowid, new.title);
END;
