-- Add api_snapshot column to panels and transcript_utterances.
-- Stores a redacted copy of the API JSON response (bulk content fields replaced
-- with "[stored]" sentinel) so we preserve metadata fields that aren't explicitly
-- modeled without duplicating the large content already stored in dedicated columns.
ALTER TABLE panels ADD COLUMN api_snapshot TEXT;
ALTER TABLE transcript_utterances ADD COLUMN api_snapshot TEXT;
