-- Maintain the FTS5 indexes with triggers, and rebuild what has already drifted.
--
-- transcript_fts and panels_fts were maintained by hand: db/transcripts.rs and
-- db/panels.rs deleted the index rows, deleted the source rows, inserted the new
-- source rows, then indexed them. Four statements in autocommit with no
-- transaction around them, and the index write came last. Anything that stopped
-- the process in between -- Ctrl-C during a sync, a crash, a dropped connection
-- mid-loop -- committed source rows that nothing ever indexed. On a 449 MB
-- database that left roughly one utterance in 1500 unfindable with zero orphaned
-- index entries, which is the signature an interruption leaves and not the one a
-- stale-index bug would (#97).
--
-- A trigger writes the index entry in the same statement as the row itself, so
-- there is no window to be interrupted in and no code path left that could
-- forget. This is SQLite's documented pattern for external-content FTS5 tables.
--
-- notes_fts is deliberately untouched. Nothing has ever populated it in
-- production, so it needs a write path before triggers or a rebuild would mean
-- anything, and turning it on changes what search returns. That is #85.

-- Heal indexes that drifted before the triggers existed. 'rebuild' discards the
-- index and re-derives it from the content table, so this is a no-op in effect
-- on a database that was already consistent, and the only repair available to
-- one that was not.
INSERT INTO transcript_fts(transcript_fts) VALUES('rebuild');
INSERT INTO panels_fts(panels_fts) VALUES('rebuild');

-- 'delete' takes the column values as they were indexed, which is why the
-- delete and update triggers pass `old`: re-reading the content table (what a
-- bare DELETE FROM transcript_fts does) reads whatever is there now, which is
-- not necessarily what the index holds.
DROP TRIGGER IF EXISTS transcript_utterances_ai;
CREATE TRIGGER transcript_utterances_ai AFTER INSERT ON transcript_utterances BEGIN
    INSERT INTO transcript_fts(rowid, text) VALUES (new.rowid, new.text);
END;

DROP TRIGGER IF EXISTS transcript_utterances_ad;
CREATE TRIGGER transcript_utterances_ad AFTER DELETE ON transcript_utterances BEGIN
    INSERT INTO transcript_fts(transcript_fts, rowid, text) VALUES('delete', old.rowid, old.text);
END;

DROP TRIGGER IF EXISTS transcript_utterances_au;
CREATE TRIGGER transcript_utterances_au AFTER UPDATE ON transcript_utterances BEGIN
    INSERT INTO transcript_fts(transcript_fts, rowid, text) VALUES('delete', old.rowid, old.text);
    INSERT INTO transcript_fts(rowid, text) VALUES (new.rowid, new.text);
END;

DROP TRIGGER IF EXISTS panels_ai;
CREATE TRIGGER panels_ai AFTER INSERT ON panels BEGIN
    INSERT INTO panels_fts(rowid, content_markdown) VALUES (new.rowid, new.content_markdown);
END;

DROP TRIGGER IF EXISTS panels_ad;
CREATE TRIGGER panels_ad AFTER DELETE ON panels BEGIN
    INSERT INTO panels_fts(panels_fts, rowid, content_markdown)
        VALUES('delete', old.rowid, old.content_markdown);
END;

DROP TRIGGER IF EXISTS panels_au;
CREATE TRIGGER panels_au AFTER UPDATE ON panels BEGIN
    INSERT INTO panels_fts(panels_fts, rowid, content_markdown)
        VALUES('delete', old.rowid, old.content_markdown);
    INSERT INTO panels_fts(rowid, content_markdown) VALUES (new.rowid, new.content_markdown);
END;
