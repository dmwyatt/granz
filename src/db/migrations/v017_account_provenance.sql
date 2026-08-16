-- Account binding history and per-document sync provenance (#132).
--
-- accounts records which Granola account this database is bound to. The
-- active binding is the row with the highest id; rebinding appends a row
-- rather than overwriting, so the history of bindings is preserved.
--
-- documents.source_account_id records the account the database was bound to
-- when the row first entered the database. It is stamped on insert only;
-- updates to an existing row never touch it.

CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL,
    granola_user_id TEXT,
    email TEXT,
    bound_at TEXT NOT NULL
);

ALTER TABLE documents ADD COLUMN source_account_id TEXT;
