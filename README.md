# grans

A fast CLI tool for searching, filtering, and querying your [Granola](https://granola.ai) meeting notes. Data is synced from the Granola API and stored in a local SQLite database.

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
# Binary at target/release/grans

# With GPU acceleration for semantic search:
cargo build --release --features directml # Windows (any GPU, recommended)
cargo build --release --features cuda     # NVIDIA (requires CUDA toolkit + cuDNN)
cargo build --release --features coreml   # macOS Apple Silicon
```

## Quick Start

```bash
# Sync your data from Granola (requires being logged into Granola app)
grans sync

# Now you can query your meetings
grans list
grans search "project kickoff"
```

## Usage

```
grans [OPTIONS] <COMMAND>
```

### Global Options

| Flag | Description |
|------|-------------|
| `--db <path>` | Use a specific database file instead of the default |
| `--token <token>` | Use a specific API token instead of reading from Granola's config |
| `--json` | Output as JSON |
| `--no-color` | Disable colored output (human-readable format without ANSI codes) |
| `--utc` | Display timestamps in UTC instead of local time |
| `--verbose` / `-v` | Enable verbose debug output (written to stderr) |

### Data Storage

grans stores your meeting data in a local SQLite database at:
- macOS: `~/Library/Application Support/grans/grans.db`
- Linux: `~/.local/share/grans/grans.db`
- Windows: `%APPDATA%/grans/grans.db`

Data is fetched from the Granola API using `grans sync` and accumulates over time. Unlike Granola's local cache (which only holds recent meetings), grans preserves all your synced data indefinitely.

## Commands

grans uses a task-centric CLI design. Common tasks are promoted to top-level commands, while entity exploration and administrative tasks are grouped under `browse` and `admin` respectively.

### Quick Reference

**Daily Use Commands** (top-level):
- `sync` - Sync data from Granola API
- `list` (`ls`) - List meetings
- `show` - Show meeting details
- `search` (`s`) - Search meetings, transcripts, notes, and panels
- `with` (`w`) - Show meetings with a person
- `recent` - Show this week's meetings
- `today` - Show today's meetings
- `embed` - Build embeddings for semantic search
- `dropbox` - Dropbox sync (init, push, pull, status, logout)
- `info` - Show database statistics

**Browse Commands** (entity exploration):
- `browse people` - List/show people and their meetings
- `browse calendars` - List calendars and events
- `browse templates` - List/show panel templates
- `browse recipes` - List/show recipes

**Admin Commands** (maintenance):
- `admin db` - Database management (clear, info, list)
- `admin transcripts` - Transcript management (fetch, status)
- `admin token` - Print the current Granola API token
- `benchmark quality` - Measure semantic search quality against a test suite

### Sync

Sync your data from the Granola API to your local database.

```bash
# Full sync (all data types)
grans sync

# Sync specific data types
grans sync documents              # Just documents
grans sync transcripts            # Just transcripts (one API call per document)
grans sync panels                 # Just AI-generated panels (one API call per document)
grans sync people                 # Just people
grans sync calendars              # Just calendar events
grans sync templates              # Just templates
grans sync recipes                # Just recipes

# Options
grans sync --dry-run              # Preview what would sync
grans sync transcripts --embed    # Build embeddings after syncing transcripts
grans sync documents --limit 50   # Limit to 50 documents
grans sync documents --since 7d   # Only docs updated in last 7 days
grans sync transcripts --delay-ms 500  # Rate limiting for transcripts
grans sync transcripts --retry         # Retry previously failed documents
grans sync panels --limit 10          # Fetch panels for up to 10 documents
grans sync panels --retry             # Retry previously failed panel fetches
```

**Note:** Sync requires a Granola auth token. By default, it reads the token from Granola's local config file. You can also provide a token explicitly with `--token`:

```bash
grans --token <TOKEN> sync
```

To get the token from a machine with Granola installed:

```bash
grans admin token             # Print to stdout
grans admin token --clipboard # Copy to clipboard without printing
```

### Search

Search across meeting titles, transcripts, notes, and AI-generated panels.

```bash
# Search everything (titles, transcripts, notes, panels)
grans search "standup"
grans s "standup"    # short alias

# Search specific targets
grans search "AI" --in titles
grans search "budget" --in titles,notes
grans search "action items" --in panels
grans search "demo" --in transcripts --date this-week

# Semantic search (vector similarity via local embeddings)
# Searches transcripts, AI-generated panel sections, and your notes
grans search "deployment strategy" --semantic
grans search "what was decided about the API" --semantic

# Filter semantic search by source type with --in
grans search "budget" --semantic --in panels          # Only AI notes
grans search "budget" --semantic --in transcripts     # Only transcripts
grans search "budget" --semantic --in notes,panels    # Notes + AI notes

# Limit results (default 10, use 0 for no limit)
# Works with keyword, context window, and semantic search
grans search "budget" --limit 5
grans search "budget" --context 2 --limit 3
grans search "budget" --semantic --limit 5
grans search "budget" --limit 0  # No limit

# Search with context (utterances for transcripts, sections for panels, paragraphs for notes)
grans search "action items" --context 3
grans search "action items" --context 2 --in panels
grans search "budget discussion" --semantic --context 2

# Limit to a specific meeting
grans search "budget" --meeting "Weekly Standup"

# Filter transcript matches by speaker
grans search "action items" --context 3 --speaker me      # only your matches
grans search "deadline" --context 3 --speaker other        # only others' matches

# Include soft-deleted meetings in search results
grans search "budget" --include-deleted
grans search "old project" --semantic --include-deleted
```

Semantic search uses a local embedding model (`nomic-embed-text-v1.5`) to find meetings by meaning rather than exact keywords. On first use, the model is downloaded automatically (~270MB). Embeddings are built from transcripts, AI-generated panel sections, and your notes, and are stored in the main database. Use `--in` to restrict which sources are searched (e.g. `--in panels` to only search AI notes).

Transcript chunks include speaker labels (`[You]` / `[Other]`) when speaker data is available, improving search relevance for queries like "what did I say about..." vs "what did they say about...".

If many chunks need embedding, semantic search will prompt for confirmation. Use `--yes` (`-y`) to skip the prompt:

```bash
grans search "deployment" --semantic --yes
```

### Embed

Build embeddings for semantic search. Use this to control when embedding happens instead of waiting for the first semantic search.

```bash
# Build embeddings for new/changed chunks (prompts for confirmation)
grans embed

# Skip confirmation prompt
grans embed --yes
grans embed -y

# Show embedding status with per-type breakdown (transcripts/panels/notes)
grans embed status

# Clear all embeddings (for dev/testing)
grans embed clear

# Clear N most recent embeddings
grans embed clear --count 10

# Force re-embed everything: clear then embed
grans embed clear --yes && grans embed --yes
```

Embeddings are built automatically during `grans sync --embed` or on the first semantic search if not already present. The `embed` command gives you explicit control over when this happens, which is useful when you have a lot of new content and don't want the first search to block.

### List Meetings

```bash
# List all meetings
grans list
grans ls    # short alias

# Filter by date
grans list --date today
grans list --date this-week
grans list --date last-month
grans list --from 2026-01-01 --to 2026-01-15

# Filter by person
grans list --person "lisa"

# Include soft-deleted meetings
grans list --include-deleted
grans list --date this-week --include-deleted

# This week's meetings (shortcut)
grans recent

# Today's meetings (shortcut)
grans today
```

### Show Meeting Details

```bash
# Show meeting by title or ID
grans show "Claude Code"
grans show 3219f4e3   # by ID prefix

# Export transcript or notes
grans show "Weekly Standup" --transcript > transcript.txt
grans show "Weekly Standup" --notes > notes.md

# Both together (notes first, then transcript)
grans show "Weekly Standup" --notes --transcript

# Filter transcript by speaker
grans show "Weekly Standup" --transcript --speaker me      # only your utterances
grans show "Weekly Standup" --transcript --speaker other   # only others' utterances

# AI-generated panels are shown automatically under "AI Notes"
# when present for a meeting

# JSON format (includes source field for speaker identification)
grans show "Weekly Standup" --transcript --json
grans show "Weekly Standup" --notes --json
```

### Meetings with a Person

```bash
# Show all meetings with a person
grans with "todd"
grans w "todd"    # short alias

# Filter by date
grans with "alice" --date this-week
grans with "bob" --from 2026-01-01

# Include soft-deleted meetings
grans with "todd" --include-deleted
```

### People

```bash
# List all people
grans browse people list

# Filter by company
grans browse people list --company "Acme"

# Show person details
grans browse people show "lisa"
```

### Calendars

```bash
# List calendars
grans browse calendars list

# Show events
grans browse calendars events
grans browse calendars events --calendar "user@example.com" --date this-week
```

### Templates

```bash
# List panel templates
grans browse templates list
grans browse templates list --category "Team"

# Show template details
grans browse templates show "Stand-Up"
```

### Recipes

```bash
# List recipes
grans browse recipes list
grans browse recipes list --visibility public

# Show recipe details
grans browse recipes show "meeting-summary"
```

### Info

Show statistics about your local database.

```bash
# Show database statistics
grans info

# JSON output for scripting
grans info --json
```

Displays content counts (documents, transcripts, panels, people, embeddings, etc.), date range of documents, embedding model, and database information (path, size, schema version).

### Database Management

Manage the local SQLite database.

```bash
# Clear database (will require re-sync)
grans admin db clear

# Clear all database files
grans admin db clear --all

# Show database location and size
grans admin db info

# List all database files
grans admin db list
```

### Transcript Management

Fetch transcripts for specific documents from Granola's API.

```bash
# Fetch transcript for a specific document
grans admin transcripts fetch <document-id>
grans admin transcripts fetch <document-id> --dry-run

# Show transcript status
grans admin transcripts status
```

For bulk transcript syncing, use `grans sync transcripts` instead.

**Note:** Requires a Granola auth token. Reads from Granola's local config file by default, or use `--token` to provide one explicitly.

### Dropbox Sync

Share your grans database across multiple machines via Dropbox.

**Why use this?** Two operations in grans are slow:

1. **Transcript sync** (`grans sync transcripts`) - Fetches transcripts from Granola's API with rate limiting (~1.5s per document). For 200 meetings, that's ~5 minutes.

2. **Embedding generation** - First semantic search builds vector embeddings for transcripts, panel sections, and notes, which takes time on CPU.

Once you've done this work on one machine, Dropbox sync lets you share the results instead of repeating it everywhere.

**Initial setup (on your primary machine):**

```bash
# 1. Sync all data including transcripts from Granola API
grans sync

# 2. Build embeddings by running a semantic search (slow first time)
grans search "anything" --semantic

# 3. Connect to Dropbox (one-time OAuth)
grans dropbox init

# 4. Upload your database
grans dropbox push
```

**On other machines:**

```bash
# 1. Connect to Dropbox
grans dropbox init

# 2. Download the databases
grans dropbox pull

# 3. Queries now work instantly - no need to re-sync or rebuild
grans search "deployment" --semantic
```

**Keeping machines in sync:**

```bash
# After syncing new data on your primary machine
grans sync
grans dropbox push

# On other machines
grans dropbox pull
```

**Commands:**

| Command | Description |
|---------|-------------|
| `grans dropbox init` | One-time Dropbox authentication |
| `grans dropbox push` | Upload database to Dropbox |
| `grans dropbox pull` | Download database from Dropbox |
| `grans dropbox status` | Show sync status with local vs remote comparison |
| `grans dropbox logout` | Remove Dropbox authentication |

**Sync status** shows a side-by-side comparison of local and remote database:

```
Sync Status
───────────
Authentication: Connected
Last push: 2025-01-27 15:30:00 UTC
Last pull: Never

                             Local              Remote
                             ─────              ──────
Documents:                     423                 418
With transcripts:              389                 385
Utterances:                  52.8K               51.2K
Date range:           2023-06 → 2025-01   2023-06 → 2025-01
Schema version:                  3                   3
Database size:             45.0 MB             44.8 MB
Embeddings:                 52.8K               51.2K
```

This helps you see at a glance whether your local database is ahead of or behind the remote copy, without downloading the full database.

**Conflict handling:** Sync refuses to overwrite newer files by default. Use `--force` to override:

```bash
grans dropbox push --force   # Overwrite remote even if it's newer
grans dropbox pull --force   # Overwrite local even if it's newer
```

**What gets synced:**
- Database (meeting data, transcripts, FTS indices, vector embeddings for semantic search)

The sync uses a sandboxed Dropbox app folder (`Apps/grans/`), so it only accesses its own files, not your full Dropbox.

## Output Modes

- **TTY** (default): Human-readable formatted output with colors in terminals, automatically stripped when piped. Timestamps are shown in your local timezone.
- **JSON** (`--json`): Structured JSON output for scripting. Timestamps remain as raw ISO 8601 UTC strings.

```bash
# Pipe output (colors automatically stripped)
grans list | head -5

# JSON for scripting
grans list --json | jq '.[].title'

# Force no color in terminal
grans list --no-color

# Display timestamps in UTC instead of local time
grans list --utc
```

## Debugging

Use `--verbose` (or `-v`) to enable debug logging on stderr. This shows API requests/responses, timing, auth resolution, and sync details without affecting stdout output.

```bash
# Debug a sync operation
grans -v sync

# Debug with JSON output (debug on stderr, JSON on stdout)
grans --json -v info

# Fine-grained control via GRANS_LOG env var
GRANS_LOG=grans::api=debug grans sync
GRANS_LOG=grans::api=trace,grans::db=debug grans sync
```

The `GRANS_LOG` environment variable uses [env_logger filter syntax](https://docs.rs/env_logger/latest/env_logger/#enabling-logging) and takes precedence over `--verbose` when set.

### Update

Update grans to the latest version from GitHub.

```bash
# Check for updates without installing
grans update --check

# Download and install the latest version
grans update

# Show current version
grans --version
```

The update command downloads the appropriate binary for your platform from GitHub releases, verifies its SHA256 checksum, and replaces the current binary.

**Build Waiting**: If a release build is in progress on GitHub Actions, grans will detect it and offer to wait:

```bash
# Interactive: prompts to wait if a build is in progress
grans update

# Auto-wait for builds (for scripts/CI)
grans update --wait

# Set a custom timeout (default: 600 seconds)
grans update --wait --timeout 300
```

**Private Repositories**: For private repositories, grans will prompt to use your `gh` CLI credentials if available. For non-interactive/scripted usage:

```bash
# Use gh CLI auth automatically (no prompt)
grans update --check --use-gh-auth

# Or set an environment variable
export GH_TOKEN=$(gh auth token)
grans update --check
```

### Benchmark Quality

Measure semantic search quality against a test suite of queries with known expected results.

```bash
# Run benchmark with default settings (precision@10)
grans benchmark quality --file tests/semantic_search_benchmark.json

# Check top 5 results (precision@5)
grans benchmark quality --file tests/semantic_search_benchmark.json --k 5

# Show detailed results for each query
grans benchmark quality --file tests/semantic_search_benchmark.json --detail
```

The benchmark reports:
- **Precision@k**: Percentage of queries where the expected meeting appears in the top k results
- **Mean Reciprocal Rank (MRR)**: Average of 1/rank for found matches (measures how high expected results appear)

This is useful for:
- Testing chunking strategy changes
- Evaluating embedding model updates
- Comparing semantic search performance across database versions

Use the `--db` flag to benchmark against different database files without affecting your main database:

```bash
grans --db /path/to/test.db benchmark quality --file tests/semantic_search_benchmark.json
```

## Date Filters

Relative terms: `today`, `yesterday`, `this-week`, `last-week`, `this-month`, `last-month`

Duration shorthands: `3d` (3 days ago), `2w` (2 weeks ago), `1m` (1 month ago):

```bash
grans list --from 2w             # meetings from the last 2 weeks
grans list --from 4w --to 2w     # meetings between 4 and 2 weeks ago
grans sync transcripts --since 7d
```

Absolute ranges with `--from` and `--to` (ISO 8601 dates):

```bash
grans list --from 2026-01-01 --to 2026-01-31
grans list --from 2026-01-15  # open-ended
```

