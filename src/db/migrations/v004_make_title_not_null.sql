-- v004_make_title_not_null.sql
-- Make documents.title NOT NULL
--
-- SQLite doesn't support ALTER TABLE ... SET NOT NULL, so we use the
-- column recreation pattern: add new column, copy data, drop old, rename.
-- Any NULL titles become empty strings.

-- Add new NOT NULL column with default
ALTER TABLE documents ADD COLUMN title_new TEXT NOT NULL DEFAULT '';

-- Copy existing data (NULL becomes empty string)
UPDATE documents SET title_new = COALESCE(title, '');

-- Drop old column and rename
ALTER TABLE documents DROP COLUMN title;
ALTER TABLE documents RENAME COLUMN title_new TO title;
