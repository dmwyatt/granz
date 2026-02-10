# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Workflow

The `main` branch is protected. All changes must go through a pull request with passing CI tests. Do not push directly to main.

Before starting work, ensure you have the latest code: `git fetch origin` and check if your branch is behind. When starting new work from main, pull first.

## Coding Standards

- Write clean, maintainable code
- No bandaids - fix problems from first principles, not symptoms
- Write idiomatic Rust - follow conventions and leverage the type system
- Leave code better than you found it

## Build & Test Commands

```bash
cargo check                    # Type-check without building (fast iteration)
cargo build                    # Debug build
cargo build --release          # Release build
cargo test                     # Run all tests (inline in modules)
cargo test <module>::tests     # Run tests for a specific module, e.g. cargo test db::meetings::tests
cargo test <test_name>         # Run a single test by name
cargo install --path .         # Install locally
```

## Sanity Check

After making changes, sync data and run queries to verify things work end-to-end:

```bash
cargo run -- sync              # Fetch latest data from Granola API
cargo run -- meetings list
cargo run -- meetings search "test" --in titles
cargo run -- people list
```

## Documentation

When changes affect user-facing behavior (new commands, changed flags, modified output, new features), update `README.md` to reflect those changes. Keep the README in sync with the actual CLI interface. Internal refactors that don't change the CLI surface do not require README updates.

## Architecture

**grans** is a Rust CLI tool that queries Granola meeting data. It fetches data from the Granola API via `grans sync` and stores it in a local SQLite database for fast querying.

### Layered Design

```
CLI (main.rs, cli/) → Commands (commands/) → DB queries (db/) → SQLite
                                    ↓
                            API (api/) — for sync
```

- **cli/**: Clap derive definitions and `RunContext` (output mode)
- **commands/**: Dispatch to db/ queries or api/ calls, select output formatter
- **api/**: Granola API client and authentication
  - `auth.rs`: Reads auth token from Granola's `supabase.json` config
  - `client.rs`: HTTP client for Granola API endpoints
  - `types.rs`: API request/response wrappers (domain types live in `models.rs`)
- **db/**: SQLite queries, FTS5 search, upsert logic
  - `connection.rs`: Database connection and schema version management
  - `schema.rs`: Schema DDL (table and FTS5 index definitions)
  - `sync.rs`: Upsert functions for syncing API data to SQLite
  - `test_fixtures.rs`: Test helper functions (under `#[cfg(test)]`)
  - Resource modules: `meetings.rs`, `transcripts.rs`, `people.rs`, `calendars.rs`, `templates.rs`, `recipes.rs`, `panels.rs`
- **output/**: Tri-modal formatting (TTY colored tables / plain tab-separated / JSON)
- **query/**: Date range parsing (relative + absolute), search utilities
- **embed/**: Semantic embeddings for similarity search
- **sync/**: Dropbox OAuth and sync functionality for sharing databases across machines
- **update/**: Self-update functionality
- **platform.rs**: Cross-platform path resolution for database and config files

### Key Design Patterns

- **API-first data**: All data comes from the Granola API; no local cache files
- **Tri-modal output**: Auto-detected via `isatty()`, overridable with `--json` or `--no-color`
- **FTS5 search**: Transcript and notes search with configurable context windows
- **Incremental sync**: Tracks last sync time per entity type

### Database Migrations

**Always use the migration system for schema changes.** Never modify schema directly.

Migrations live in `src/db/migrations/` using `rusqlite_migration`. To add a schema change:

1. Create `src/db/migrations/v00X_description.sql` (next sequential number)
2. Use `ALTER TABLE ADD COLUMN` for new columns, `CREATE TABLE IF NOT EXISTS` for new tables
3. Register in `migrations()` in `src/db/migrations/mod.rs`:
   ```rust
   M::up(include_str!("v00X_description.sql")),
   ```
4. Update `let total = N;` count in `open_and_migrate()`
5. Update `tests/common/mod.rs` `create_test_tables()` to match new schema
6. Add migration tests in `src/db/migrations/mod.rs`

Schema version is tracked via SQLite's `PRAGMA user_version`. The system auto-backs up the database before applying migrations.

## Granola API Explorer

`scripts/granola-api.py` is a standalone script for querying the Granola API directly. Use it to explore endpoints, inspect response shapes, and investigate API behavior during development.

```bash
uv run scripts/granola-api.py v2/get-documents            # Query an endpoint
uv run scripts/granola-api.py v1/get-document-panels \
  --body '{"document_id": "abc"}'                         # With a request body
```

Output is raw JSON; pipe through `jq` for filtering.
