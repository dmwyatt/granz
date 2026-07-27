-- Migration v014: Surface Granola's per-utterance speaker attribution.
--
-- Granola began returning a detected speaker per utterance on 2026-07-21.
-- Those fields were never modeled, so they landed in the flattened `extra`
-- map and rode into `api_snapshot` (added in v013) unchanged. The data is
-- already on disk; this lifts the name into a column we can query.
--
-- A dedicated column rather than reading `api_snapshot` at query time:
-- json_extract cannot use an index, and the filter and the DISTINCT lookup
-- that resolves `--speaker <name>` both run over every utterance.
--
-- The column is nullable and mostly NULL. Only the system channel is ever
-- attributed (the microphone channel is the local user), and only meetings
-- recorded after the cutover carry attribution at all.

ALTER TABLE transcript_utterances ADD COLUMN speaker_name TEXT;

UPDATE transcript_utterances
   SET speaker_name = json_extract(api_snapshot, '$.detected_speaker_name')
 WHERE api_snapshot IS NOT NULL;

CREATE INDEX idx_transcript_utterances_speaker_name
    ON transcript_utterances(speaker_name);
