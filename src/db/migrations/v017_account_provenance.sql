-- Account log and per-row sync provenance (#132).
--
-- accounts is an append-only log of every Granola account this database has
-- ever synced from: the stable account id, the email captured when the
-- account was first seen, and when that was. Nothing enforces single-account
-- use; data from multiple accounts coexisting is supported by design.
--
-- source_account_id on each account-tied table records the account a row
-- first arrived under. It is stamped on insert only; updates to an existing
-- row never touch it. Tables that hang off documents (transcript_utterances,
-- panels, document_people, chunks) derive their provenance through
-- document_id and carry no column of their own.

CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL UNIQUE,
    granola_user_id TEXT,
    email TEXT NOT NULL,
    first_seen_at TEXT NOT NULL
);

ALTER TABLE documents ADD COLUMN source_account_id TEXT;
ALTER TABLE people ADD COLUMN source_account_id TEXT;
ALTER TABLE calendars ADD COLUMN source_account_id TEXT;
ALTER TABLE events ADD COLUMN source_account_id TEXT;
ALTER TABLE templates ADD COLUMN source_account_id TEXT;
ALTER TABLE recipes ADD COLUMN source_account_id TEXT;
