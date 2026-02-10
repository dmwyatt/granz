-- Migration v002: Capture missing API fields
-- Adds high-priority queryable columns and extra_json columns on all entity tables

-- High-priority columns for calendar events
ALTER TABLE events ADD COLUMN attendees_json TEXT;
ALTER TABLE events ADD COLUMN conference_data_json TEXT;
ALTER TABLE events ADD COLUMN description TEXT;

-- High-priority column for templates
ALTER TABLE templates ADD COLUMN chat_suggestions_json TEXT;

-- extra_json columns on all entity tables
ALTER TABLE documents ADD COLUMN extra_json TEXT;
ALTER TABLE events ADD COLUMN extra_json TEXT;
ALTER TABLE templates ADD COLUMN extra_json TEXT;
ALTER TABLE recipes ADD COLUMN extra_json TEXT;
ALTER TABLE people ADD COLUMN extra_json TEXT;
ALTER TABLE calendars ADD COLUMN extra_json TEXT;
