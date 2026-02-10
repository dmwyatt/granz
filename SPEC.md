# grans - Granola Cache Search CLI

## Overview

`grans` is a Rust CLI tool for searching, filtering, and querying the local Granola meeting notes cache file (`cache-v3.json`). It provides structured access to meetings, transcripts, people, calendars, templates, recipes, and other data stored in Granola's local cache.

The tool is designed to be resilient to schema changes in the cache file (which we don't control), providing warnings and structural diffs when the schema evolves while continuing to function on known fields.

### Problem

Granola stores rich meeting data locally in a large (~13MB) JSON cache file but provides no CLI-based way to query it. Users need to search transcripts, find meetings by person/date/topic, look up people, and generally explore this data from the terminal.

### Goals

- Fast, structured querying of Granola cache data
- Resilience to schema changes with clear reporting
- Cross-platform (Linux, macOS, Windows)
- Unix-friendly output (TTY-aware formatting, JSON option)
- SQLite-backed queries with FTS5 full-text search

## Requirements

### Functional

1. **Meeting queries**: List, search, and filter meetings by title, person, date range, topic
2. **Transcript search**: Full-text search through meeting transcripts with configurable context (N utterances before/after)
3. **People lookup**: Find people by name, email, company; show their meeting history
4. **Calendar queries**: List calendars, show events by date/calendar
5. **Template browsing**: List and inspect panel templates
6. **Recipe browsing**: List and inspect recipes (public, shared, user)
7. **Schema validation**: Detect and report schema changes with structural diffs
8. **Date filtering**: Support both relative (`today`, `yesterday`, `last-week`, `this-month`) and absolute (`--from 2026-01-01 --to 2026-01-15`) date ranges
9. **Output formats**: Human-readable (pretty) by default in TTY, plain when piped, JSON via `--json` flag

### Non-Functional

1. **Performance**: Auto-builds SQLite index from cache file; sub-100ms queries on subsequent runs. Rebuilds only when cache file is newer than the index or schema version changes.
2. **Cross-platform**: Build and run on Linux, macOS, and Windows
3. **Resilience**: Never crash on unexpected schema; degrade gracefully with warnings
4. **Composability**: Output is pipe-friendly; TTY detection adjusts formatting

## Non-Goals

- Modifying the cache file
- Syncing with Granola's servers
- Real-time watching/tailing of the file
- Providing a TUI or interactive mode (just a CLI)
- Replacing Granola's own UI

## Technical Design

### Architecture

SQLite is the sole runtime data layer. The parser is used only during ETL (rebuilding the index from the cache file). All commands query the database directly.

```
┌─────────────────────────────────────────────┐
│                  CLI Layer                    │
│  (clap subcommands, output formatting)       │
├─────────────────────────────────────────────┤
│              Database Layer                   │
│  (SQLite queries, FTS5 full-text search,     │
│   auto-rebuild on stale/missing index)       │
├─────────────────────────────────────────────┤
│              Schema Layer                    │
│  (validation, diff, unknown field tracking)  │
├─────────────────────────────────────────────┤
│           Parser / ETL Layer                  │
│  (double-encoding unwrap, typed structs,     │
│   JSON → SQLite index build)                 │
└─────────────────────────────────────────────┘
```

On each invocation:
1. `ensure_fresh_db()` checks if the SQLite index exists, has the correct schema version, and is newer than the cache file
2. If stale or missing, the cache file is parsed and the index is rebuilt (with a stderr message)
3. Commands execute SQL queries against the fresh index

### Subcommand Structure

Hybrid resource + action style:

```
grans meetings list [--person <name>] [--from <date>] [--to <date>] [--last-week] [--today]
grans meetings show <id-or-title>
grans meetings search <query> [--in titles,transcripts,notes] [--from <date>] [--to <date>]

grans transcripts search <query> [--meeting <id>] [--context <N>] [--from <date>] [--to <date>]

grans people list [--company <name>]
grans people show <name-or-email>
grans people meetings <name-or-email>

grans calendars list
grans calendars events [--calendar <id>] [--from <date>] [--to <date>]

grans templates list [--category <cat>]
grans templates show <id-or-title>

grans recipes list [--visibility public|shared|user]
grans recipes show <id-or-slug>

grans schema check        # Report current schema status
grans schema diff         # Show structural diff from known schema
grans schema log          # Show history of detected changes
```

### Global Flags

```
--file <path>       # Override cache file location
--json              # Output as JSON
--no-color          # Disable colored output
--strict            # Error on schema changes instead of warning
```

### File Location Auto-Detection

Search order:
1. `--file` flag if provided
2. `GRANS_CACHE_FILE` environment variable
3. Platform-specific Granola paths:
   - macOS: `~/Library/Application Support/Granola/cache-v3.json`
   - Linux/WSL: Check common paths, `~/.config/Granola/`, etc.
   - Windows: `%APPDATA%/Granola/cache-v3.json`

### Parser Design

The cache file has a double-encoded structure:
```json
{"cache": "<stringified JSON containing the actual state>"}
```

The parser:
1. Validates the outer wrapper structure (warn if it changes)
2. Extracts and parses the inner stringified JSON
3. Deserializes known fields into typed structs
4. Preserves unknown fields in a `HashMap<String, serde_json::Value>` catch-all
5. Logs a warning on first encounter of new unknown fields

### Core Data Models

These are the serde-deserialized types. All use `#[serde(default)]` and `Option<T>` liberally for resilience.

#### CacheWrapper
- `cache: String` — the stringified inner JSON

#### CacheState
- `state: State`
- `version: u64`

#### State
Contains all known top-level keys. Unknown keys captured via `#[serde(flatten)] extra: HashMap<String, Value>`.

#### Event
- `id`, `summary`, `start` (DateTimeWithZone), `end` (DateTimeWithZone), `attendees` (Vec<Attendee>), `creator`, `organizer`, `conferenceData`, `recurringEventId`, `iCalUID`, `calendarId`, `status`, `htmlLink`

#### Document (Meeting)
- `id`, `created_at`, `title`, `user_id`, `type`, `notes` (ProseMirrorDoc), `notes_plain`, `notes_markdown`, `google_calendar_event` (Event), `updated_at`, `deleted_at`, `people`, `meeting_end_count`, `valid_meeting`, `summary`, `workspace_id`, `visibility`, `creation_source`, `privacy_mode_enabled`, etc.

#### TranscriptUtterance
- `document_id`, `start_timestamp`, `end_timestamp`, `text`, `source`, `id`, `is_final`

#### Person
- `id`, `user_id`, `name`, `job_title`, `company_name`, `company_description`, `email`, `avatar`, `user_type`, `subscription_name`

#### DocumentPanel
- `document_id`, `created_at`, `title`, `content` (ProseMirrorDoc), `template_slug`, `id`, `updated_at`

#### PanelTemplate
- `id`, `is_granola`, `owner_id`, `category`, `title`, `sections` (Vec<TemplateSection>), `color`, `symbol`, `description`

#### Recipe
- `id`, `slug`, `config` (RecipeConfig), `visibility`, `publisher_slug`, `creator_name`

### Schema Change Detection

The schema layer maintains a "known schema" definition — a structural fingerprint covering all fields that `grans` functionality depends on. This means fingerprinting is not limited to top-level keys; it tracks the full path of any field the tool reads (e.g. `state.documents.*.google_calendar_event.attendees[].email`). This ensures we detect both obvious breakage (removed keys) and subtle changes (type changes in nested structures like transcript utterances or attendee objects).

On each run:

1. Compute the structural fingerprint of the parsed data for all paths the tool depends on
2. Compare against the stored known schema
3. If differences found:
   - In default mode: print a warning to stderr with a summary of changes
   - In `--strict` mode: exit with error
   - Log the full structural diff (keys added/removed/type-changed) with timestamp to the schema log file

The schema log is stored at `<data_dir>/schema-changes.log` (where `<data_dir>` is the XDG data directory, e.g. `~/.local/share/grans/`).

The known schema fingerprint is stored at `<data_dir>/known-schema.json` and can be updated via `grans schema check --update` to acknowledge a new schema version.

### SQLite Database

Stored at `<data_dir>/index.db`. Automatically built and refreshed on every invocation when stale or missing. Contains:

- `documents` table: id, title, created_at, updated_at, deleted_at, type, notes_plain, notes_markdown, summary, people_json, google_calendar_event_json
- `transcript_utterances` table: id, document_id, start_timestamp, end_timestamp, text
- `people` table: id, name, email, company_name, job_title
- `events` table: id, summary, start_time, end_time, calendar_id
- `calendars` table: id, provider, primary, access_role, summary, background_color
- `templates` table: id, title, category, symbol, color, description, is_granola, owner_id, sections_json, created_at, updated_at, deleted_at
- `recipes` table: id, slug, visibility, publisher_slug, creator_name, config_json, created_at, updated_at, deleted_at, user_id, workspace_id
- `document_people` junction table: document_id, email, full_name, role (creator/attendee)
- `metadata` table: cache_path, last_modified, schema_version
- FTS5 virtual tables: `transcript_fts` (text), `notes_fts` (notes_plain)

Freshness check on each run:
1. If DB file doesn't exist → rebuild
2. If stored `schema_version` != code's `SCHEMA_VERSION` → rebuild
3. If cache file mtime > stored mtime → rebuild
4. Otherwise, use existing DB

### Output Formatting

- **TTY mode**: Colored, formatted tables/sections. Meeting titles bold, dates dimmed, etc.
- **Piped mode**: Plain text, one record per line, tab-separated fields where applicable
- **JSON mode** (`--json`): Structured JSON output, one object per record or a JSON array

### Date Parsing

Relative terms supported:
- `today`, `yesterday`, `this-week`, `last-week`, `this-month`, `last-month`

Absolute: ISO 8601 dates (`2026-01-20`) or date-times. `--from` and `--to` define a range; either can be omitted for open-ended ranges.

## Module Structure

```
src/
├── main.rs              # Entry point: parse args, ensure DB, dispatch commands
├── cli/
│   ├── args.rs          # Clap derive definitions
│   └── context.rs       # RunContext (cache_path, output_mode)
├── commands/            # Subcommand handlers (delegate to db::*)
│   ├── meetings.rs
│   ├── transcripts.rs
│   ├── people.rs
│   ├── calendars.rs
│   ├── templates.rs
│   ├── recipes.rs
│   └── schema.rs
├── db/                  # SQLite: schema, ETL, queries
│   ├── build.rs         # Schema DDL + ETL from State → tables
│   ├── connection.rs    # ensure_fresh_db(), staleness checks
│   ├── meetings.rs      # list/show/search meetings
│   ├── transcripts.rs   # FTS5 transcript search + context windows
│   ├── people.rs        # list/find people, meetings-by-person
│   ├── calendars.rs     # list calendars/events
│   ├── templates.rs     # list/show templates
│   └── recipes.rs       # list/show recipes
├── output/              # TTY/plain/JSON formatting
├── parser/              # Serde models (used by ETL + schema commands)
├── platform/            # Cross-platform cache file resolution
├── query/               # Date range parsing, search utilities
│   ├── dates.rs         # DateRange, relative/absolute parsing
│   ├── filter.rs        # SearchTarget enum
│   └── search.rs        # ContextWindow type, contains_ignore_case
└── schema/              # Schema validation and diffing
```

## Dependencies

- `serde` / `serde_json` — JSON parsing and deserialization
- `clap` (derive) — CLI argument parsing
- `chrono` — Date/time handling and parsing
- `directories` — Cross-platform XDG/app data paths
- `rusqlite` (bundled) — SQLite database (sole runtime data layer)
- `colored` — Terminal colors (TTY-aware)
- `anyhow` / `thiserror` — Error handling

## Resolved Design Decisions

1. **Meeting search scope**: `grans meetings search` supports an `--in` flag that accepts a comma-separated list of targets: `titles`, `transcripts`, `notes`. Defaults to `titles`. Users can combine: `--in titles,transcripts,notes`.
2. **People matching**: Case-insensitive substring matching. "Jane" matches "Jane Doe", "jane.doe@...".
3. **Schema fingerprint depth**: Fingerprints all paths the tool's functionality depends on, recursively. Not just top-level — if we read `attendees[].email`, we fingerprint that path.
4. **File not found behavior**: Error with the list of paths checked, suggest using `--file <path>` or setting `GRANS_CACHE_FILE` environment variable.
