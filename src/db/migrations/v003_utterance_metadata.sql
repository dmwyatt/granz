-- Migration v003: Add utterance metadata columns
-- Adds audio_source and is_final columns to transcript_utterances table
-- to store speaker identification (microphone vs system) and finalization status.
--
-- audio_source values:
--   - "microphone": utterance from local user (your microphone)
--   - "system": utterance from remote participants (system audio)
--
-- is_final: whether the transcription is finalized (1) or provisional (0)
--
-- IMPORTANT: Existing utterances will have NULL for these columns.
-- The Granola API returns these fields, but we weren't storing them before
-- this migration. New transcripts fetched via `grans sync` will have values,
-- but historical data remains NULL unless manually re-synced.
--
-- To backfill existing transcripts, users would need to re-fetch from the API.
-- This is safe for embeddings (chunks are keyed by content_hash, not utterance rows).
-- A future `grans transcripts backfill` command could automate this.

ALTER TABLE transcript_utterances ADD COLUMN audio_source TEXT;
ALTER TABLE transcript_utterances ADD COLUMN is_final INTEGER;
