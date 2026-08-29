# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Workflow

The `main` branch is protected. All changes must go through a pull request with passing CI tests. Do not push directly to main.

Before starting work, ensure you have the latest code: `git fetch origin` and check if your branch is behind. When starting new work from main, pull first.

### What CI runs

`ci.yml` gates the PR. It runs on three platforms, but not the same tests on
each: ubuntu runs the full suite, while macOS and Windows run `--bins` only.
That is deliberate. Every platform-gated line in this repo is in `src/`, so all
of its tests are unit tests in the bin target, and nothing in `tests/*.rs` is
platform-gated. Each of those integration tests spawns `grans.exe`, which
costs ~2s per spawn on the Windows runner against 0.037s on ubuntu.

The consequence for writing tests: **a platform-specific test belongs in `src/`,
under `#[cfg(...)]`.** Put one in `tests/*.rs` and no PR will ever run it on the
platform it is about.

`platform-tests.yml` runs the full suite on all three platforms after merge to
main, plus weekly, and files an issue labelled `ci: platform-tests` when it
fails. So a macOS- or Windows-only break in `tests/*.rs` surfaces shortly after
merge rather than on the PR.

The required status check is `CI Status`, not the individual test legs. It
aggregates them and its name survives changes to the matrix.

## Coding Standards

- Write clean, maintainable code
- No bandaids - fix problems from first principles, not symptoms
- Write idiomatic Rust - follow conventions and leverage the type system
- Leave code better than you found it

## Build & Test Commands

```bash
cargo check                    # Type-check without building (fast iteration)
cargo build                    # Debug build
cargo build --release          # Release build (add --features directml on Windows for GPU embedding)
cargo test --no-fail-fast      # Run all tests -- see below, plain `cargo test` does not
cargo test <module>::tests     # Run tests for a specific module, e.g. cargo test db::meetings::tests
cargo test <test_name>         # Run a single test by name
cargo install --path .         # Install locally
```

`cargo test` runs several binaries here: the unit tests in the bin target, then
the integration binaries in `tests/`. It stops after the first binary that
fails, and the bin target goes first, so one broken unit test means the
integration tests never launch and the run reports nothing about them. Use
`--no-fail-fast` and do not read a run that died in the bin target as full
coverage.

On Windows, run tests from PowerShell rather than Git Bash. Git Bash prepends
its coreutils to PATH, so PATH-sensitive tests pass there and fail on a GitHub
Actions windows runner.

The embedder is platform-selected in `src/embed/model/mod.rs`: macOS builds
`llama.rs` (llama.cpp on Metal, compiled from source by `llama-cpp-sys-2`, so a
macOS build needs `cmake`), every other platform builds `onnx.rs` (fastembed).
Neither is a cargo feature; a macOS build cannot be checked from Windows and
vice versa, so a change to either file is verified only on that platform's CI
leg or a real machine of that platform.

A pre-commit hook (local, in the shared `.git/hooks`) rejects commits whose
staged Rust files are unformatted; run `cargo fmt` before committing.

## Sanity Check

After making changes, sync data and run queries to verify things work end-to-end:

```bash
cargo run -- sync                                 # Fetch latest data from Granola API
cargo run -- list
cargo run -- grep "test" --in titles              # FTS path, no models needed
cargo run -- search "test" --fast                 # hybrid fusion path; a bare search adds the rerank stage, which is slow in debug builds
cargo run -- browse people list
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
  - `auth.rs`: Token resolution order (`--token`/`GRANS_TOKEN`, stored credentials, local store)
  - `credentials.rs`: The `GranolaCredentials` type (refresh token, access token, expiry)
  - `credential_store.rs`: Where they live: platform keychain, falling back to a `0600` `auth.toml`
  - `granola_auth.rs`: Granola PKCE login and token refresh
  - `local_store.rs`: Reads the token Granola's desktop app stored (legacy fallback)
  - `identity.rs`: Client version/platform reported to Granola (`GRANS_GRANOLA_VERSION`)
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

1. Create `src/db/migrations/v00X_description.sql` (next sequential number).
   Before choosing the number, check open PRs and other local branches for
   migrations claiming it (`gh pr list`, `git branch -a`); concurrent sessions
   have collided here. A migration number already applied to a real database
   is immovable; renumber the unmerged side.
2. Use `ALTER TABLE ADD COLUMN` for new columns, `CREATE TABLE IF NOT EXISTS` for new tables
3. Register in `migrations()` in `src/db/migrations/mod.rs`:
   ```rust
   M::up(include_str!("v00X_description.sql")),
   ```
4. Update `let total = N;` count in `open_and_migrate()`
5. Add the migration's `include_str!` entry to `MIGRATION_SQL` in
   `tests/common/mod.rs` (the integration tests replay the real migration
   files; a drift guard fails every `TestEnv` if the entry is missing)
6. Add migration tests in `src/db/migrations/mod.rs`

Schema version is tracked via SQLite's `PRAGMA user_version`. The system auto-backs up the database before applying migrations.

## Tracker Hygiene

Multiple agent sessions work this repo concurrently and dispatch off issue labels, so stale tracker state is a dispatch bug, not a cosmetic one.

- Closing a tracking issue whose content has been delivered is part of delivering it; close with evidence in the same session.
- `status: ready` is a claim other agents act on. If your findings invalidate an issue's premise, re-triage it in the same session; don't leave stale `ready` labels.
- `status:` labels are single-valued; replace, don't stack.
- Efforts that file 3+ related issues get a milestone when filed; milestone names carry state (e.g. "(tabled)").
- Blocked issues carry `status: blocked` and name their blocker in the body's first line. When closing an issue, check `gh issue list --label "status: blocked"` for issues it unblocks and re-triage them.

## Granola API Explorer

`scripts/granola-api.py` is a standalone script for querying the Granola API directly. Use it to explore endpoints, inspect response shapes, and investigate API behavior during development.

```bash
uv run scripts/granola-api.py v2/get-documents            # Query an endpoint
uv run scripts/granola-api.py v1/get-document-panels \
  --body '{"document_id": "abc"}'                         # With a request body
```

Output is raw JSON; pipe through `jq` for filtering.
