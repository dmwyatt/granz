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
| `--token <token>` | Use a specific API token (also read from `GRANS_TOKEN`) |
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
- `search` (`s`) - Ranked search across meetings, transcripts, notes, and panels
- `grep` (`g`) - List every meeting containing given words
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

**Auth Commands** (Granola sign-in):
- `auth login` - Sign in to Granola and store credentials for grans
- `auth status` - Show whether grans has its own session, and its expiry
- `auth logout` - Remove the stored credentials

**Admin Commands** (maintenance):
- `admin db` - Database management (clear, info, list, rebuild-fts, rebind)
- `admin token` - Print the current Granola API token
- `benchmark quality` - Measure search quality (FTS or semantic) against a labeled test suite

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

# Fetch one document's transcript (full ID or unique prefix), replacing any existing transcript
grans sync transcripts 504fe9f6
grans sync transcripts 504fe9f6 --dry-run
grans sync transcripts 504fe9f6 --embed   # Rebuild embeddings afterward

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

**Note:** Sync requires a Granola auth token. grans looks for one in this order:

1. `--token <TOKEN>`, or the `GRANS_TOKEN` environment variable
2. grans's own stored credentials, from `grans auth login`
3. The token Granola's desktop app stored locally (not on macOS, see below)

**Account binding:** The first sync binds the database to the Granola account
the token belongs to, and each document records which account the database was
bound to when that document first entered it. From then on, syncing with a
token that belongs to a different account is an error naming both accounts.
This prevents one account's data from being silently upserted over another's
(for example, after signing in to the wrong account). When the switch is
intentional, run `grans admin db rebind` first.

The mismatch is a hard error wherever a token is resolved, including under
`--dry-run` for `sync` and the `documents`, `people`, `calendars`,
`templates`, and `recipes` subcommands. Bulk `sync transcripts --dry-run` and
`sync panels --dry-run` are local-database previews that never resolve a
token, so for those two the check first applies on the real run.

### Signing in

```bash
grans auth login              # Sign in and store credentials for grans
grans auth login --provider microsoft
grans auth status             # Show whether grans has a session, and its expiry
grans auth logout             # Remove the stored credentials
```

`grans auth login` opens your browser to Granola's login. When it finishes, you
land on a `granola.ai` page that offers to open the Granola app. **Cancel that
dialog**, then copy the URL from your address bar and paste it back into grans:

```
https://www.granola.ai/app-redirect?code=...
```

The authorization code is tied to grans's login attempt, so letting Granola
open the link hands the code to an app that cannot complete the sign-in.

This creates a session on your Granola account, separate from the desktop
app's. It appears in Granola's session list and you can revoke it there.
`grans auth logout` only removes the local copy.

Once signed in, grans refreshes its own access token and does not need Granola
running, or installed.

### Where credentials are stored

grans stores its session in the platform keychain: Windows Credential Manager,
the macOS Keychain, or the Secret Service on Linux. `grans auth status` names
the one in use.

The refresh token is the part worth protecting. It mints new access tokens
until the session is revoked, so unlike the six-hour access token it stays
valuable.

Where no keychain is reachable (a headless Linux box, some WSL setups), grans
falls back to `data_dir()/auth.toml`, written `0600`, and warns you at login
and in `grans auth status`. That keeps other local users out, but the token is
unencrypted: anyone with a copy of that file, from a backup or a disk image,
holds a working session until you revoke it. If a keychain later becomes
available, the next grans command moves the credentials into it and deletes
the file.

On macOS, grans marks its keychain item as readable by any application, the
same thing `security add-generic-password -A` does. Without that, macOS ties
the item to the code signature of the binary that created it. grans is not
signed with a Developer ID, so on Apple Silicon it carries only the ad-hoc
signature the linker applies, which is a hash of the binary itself: every
rebuild or self-update is a different application as far as the keychain is
concerned, and you would be asked for your login password on every read, with
"Always Allow" lasting only until the next build.

The cost is that any program running as you can read the refresh token without
being challenged, the same posture `gh` and `aws` take. What the keychain still
gives you over the `auth.toml` fallback is encryption at rest and a token that
stays unreadable in backups and disk images.

grans creates the item already carrying that marking rather than applying it
afterwards, because changing an existing item's access control is itself gated
on the signature that keeps changing. Upgrading an entry left by an older grans
therefore replaces it rather than amending it, and costs no prompt either.

### Reading Granola's local token

On Windows and Linux, and without `grans auth login`, grans falls back to the
token Granola's desktop app stored, decrypting the `supabase.json.enc` store
that recent versions use (falling back to the legacy plaintext
`supabase.json`).

**There is no such fallback on macOS.** Granola keeps its data-encryption key
in the macOS data-protection keychain, gated on Granola's own code signature,
so no other program can read it. grans does not try: with no session of its
own it tells you to run `grans auth login`, which is the only thing that
works there.

To print whichever token grans resolves:

```bash
grans admin token             # Print to stdout
grans admin token --clipboard # Copy to clipboard without printing
```

### If login reports an out-of-date client

grans identifies itself to Granola with a desktop client version, which Granola
rejects if it falls below their minimum. Override it without waiting for a
grans release:

```bash
export GRANS_GRANOLA_VERSION=7.441.6   # your installed Granola version
```

### Search and Grep

Two verbs query meeting content, with two different promises:

- `grans search` (alias `s`) is ranked discovery: the best few meetings for a query, matched by meaning as well as by words. Keyword (FTS5) and semantic rankings are fused with reciprocal rank fusion, then the top candidates are reranked by a cross-encoder. Results come from bounded candidate pools, so search shows a `Top N match(es)` list and never reports a corpus total.
- `grans grep` (alias `g`) is complete lexical lookup: every meeting where the words literally appear. Its `Found N meeting(s)` count is a fact about your synced data, and `--limit` only trims how many are shown. Grep never loads models and never prompts.

When search finds any meetings containing the query's literal words, it says so in a footer and points at grep, e.g. `312 meeting(s) contain these words; grans grep "budget" lists them all.` The suggested command echoes any search filters that affect the count (`--in`, `--meeting`, date flags, `--include-deleted`), so running it reports the number the footer claims.

Migrating from the old flags: `search --keyword` is now `grep`, `search --speaker me` is now `grep --speaker me`, and `--hybrid` is gone because hybrid retrieval is simply what `search` does.

```bash
# Ranked discovery: fuses keyword + semantic rankings, then reranks
grans search "standup"
grans s "standup"    # short alias
grans search "quarterly budget review" --min-score 0.5   # drop low-relevance results

# Skip the rerank stage for a faster fusion-only search (no relevance scores)
grans search "quarterly budget review" --fast

# Complete lookup: every meeting containing these words
grans grep "budget"
grans g "budget"     # short alias

# Complete and speaker-attributed: only that speaker's utterances count
grans grep "action items" --speaker me            # things you said
grans grep "deadline" --speaker other             # things anyone else said
grans grep "deadline" --speaker "Jane Doe"        # things Jane said
grans grep "deadline" --speaker jane              # partial names work

# Search specific targets (both verbs)
grans search "AI" --in titles
grans grep "budget" --in titles,notes
grans search "action items" --in panels
grans grep "demo" --in transcripts --date this-week

# Limit results (default 10, use 0 for no limit)
grans search "budget" --limit 5
grans grep "budget" --limit 0   # list every match

# Show more match snippets per meeting (default 1)
grans search "budget" --matches 3

# Show context around each match (utterances for transcripts, sections for
# AI notes, paragraphs for notes); both verbs
grans search "action items" --context 3
grans grep "action items" --context 2

# Limit to a specific meeting (ID or title substring); both verbs
grans search "budget" --meeting "Weekly Standup"
grans grep "budget" --meeting "Weekly Standup"

# Include soft-deleted meetings in results (both verbs)
grans search "budget" --include-deleted
```

Ranked search runs keyword and semantic retrieval together and fuses the two rankings with reciprocal rank fusion, so a meeting ranked well by either retriever surfaces, and one ranked well by both rises to the top. The top 50 fused candidates are then scored by a cross-encoder reranker (`jina-reranker-v1-turbo-en`) for how well each meeting actually answers the query, and the final order blends that judgment with the fusion ranking and a small boost for meetings whose title matches the query (damped when many meetings share the title, as recurring series do). Reranking takes roughly 2.2 seconds per query on CPU, most of it model inference; `--fast` skips the stage and returns fusion-order results (no relevance scores) in about 75 milliseconds.

Grep matches every word in the query, in any order, in the title as well as the body (`grans grep "budget review"` finds a meeting titled "Budget review" and one whose transcript mentions both words; quote a phrase inside the query, e.g. `grans grep '"budget review"'`, to require it verbatim). Matching is word-based everywhere, so the query words match whole tokens, not substrings inside a longer word (`art` does not match a title reading "Quarterly planning"). Results are ranked by relevance: titles, notes, transcripts, and AI notes are all scored by BM25, each meeting is ranked by its strongest match, and newer meetings break ties. Use grep when completeness is the point, e.g. auditing every mention of a term, or when you need matches attributed to a speaker: `--speaker` keeps only meetings where that speaker's transcript utterances match the query, and the cards show exactly those utterances. Notes and AI notes carry no speaker, so combining `--speaker` with an `--in` list that excludes transcripts is an error. Speaker filtering is grep-only because semantic retrieval has no per-utterance attribution, so search could not honor the filter without capping the answer.

`--speaker` takes `me`, `other`, or a speaker's name. `me` and `other` split on the audio channel and work on every meeting: `me` is your microphone, `other` is everyone else. A name matches Granola's own per-utterance attribution, which it began providing on 2026-07-21 and only on the remote side of the call, so meetings recorded before then have no names to match. Names are matched case-insensitively as substrings, so `--speaker jane` finds Jane Doe; quoting the full name (`--speaker "Jane Doe"`) pins it exactly when several names share a fragment. A name that matches several speakers searches all of them and says which on stderr; one that matches nobody is an error listing the speakers you do have, so a typo never looks like a genuine absence of results. In `--json`, each match carries `speaker` (the channel, `me` or `other`) and, when attributed, `speaker_name`.

Both verbs render the same cards. Each card shows why the meeting matched: the source of the best match (`AI notes` with its section heading, `your notes`, or `transcript` with time and speaker, named when Granola attributed the utterance and `You`/`Other` otherwise), a snippet with the query terms highlighted, and a `+N more matches` line when the meeting matched in more places. `--matches N` shows up to N snippets per meeting (default 1), and `--context N` renders N neighboring units around each shown match inside the card (the utterances around a transcript hit, the sections around an AI-notes hit, the paragraphs around a notes hit), with the matched unit shown whole. In search results, a meeting that matched semantically but contains none of the query's literal words shows its best-matching passage without highlights, and a meeting that matched only by its title says `title match`. The relevance score is not shown in the card view; `--json` carries it (`score`), along with which retrievers surfaced each meeting (`signals`), the full match list, and snippet highlight offsets. `--min-score` drops search results below a relevance threshold; it conflicts with `--fast`, since only the rerank stage produces that score. Both verbs support `--in`, `--meeting`, date filters, and `--limit` (which counts meetings everywhere).

The JSON envelopes differ where the contracts do: grep JSON reports `total_meetings` (the complete count), while search JSON reports `keyword_total` (the uncapped count of meetings containing the query's words, backing the footer) and no total, because its meeting list is a pooled best-k.

The semantic half of search uses a local embedding model (`nomic-embed-text-v1.5`) to match by meaning rather than exact keywords. Embeddings are built from transcripts, AI-generated panel sections, and your notes, and are stored in the main database.

A `grans search` reads local models and embeddings; `grans grep` never does. The first search downloads the embedding model (~270MB) and, when reranking, the reranker model too (~150MB); both are one-time downloads, and search skips the embedding model entirely when there are no embeddings to search. Search never builds or repairs embeddings: it searches what `grans embed` has built and warns on stderr when the index cannot cover everything, i.e. when data has synced since the last embed (recent meetings may be missing from results), when no embeddings exist yet, when the existing embeddings were built by a different model (both of the latter fall back to keyword-only results), or when they were built with an outdated chunking strategy. Each warning says to run `grans embed`; searching itself never blocks.

When an upgrade changes the embedding model, the next `grans embed` detects the stale embeddings and rebuilds them all. This full rebuild is a one-time cost and can take a while on large databases.

Transcript chunks include speaker labels (`[You]` / `[Other]`) when speaker data is available, improving search relevance for queries like "what did I say about..." vs "what did they say about...".

### Embed

Build embeddings for hybrid search. This is the only command that creates or updates them; search reads them as-is.

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

Embeddings are built by this command or during `grans sync --embed`; search only reads them. Run one of the two after syncing new content to make it searchable semantically.

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
grans show "Weekly Standup" --transcript --speaker me           # only your utterances
grans show "Weekly Standup" --transcript --speaker other        # everyone else
grans show "Weekly Standup" --transcript --speaker "Jane Doe"   # only Jane's

# AI-generated panels are shown automatically under "AI Notes"
# when present for a meeting

# JSON format (includes source and detected_speaker_name per utterance)
grans show "Weekly Standup" --transcript --json
grans show "Weekly Standup" --notes --json
```

Transcript lines are labelled with the speaker: `You` for your microphone, the speaker's name when Granola attributed the utterance, and `Other` when it did not. `--speaker` accepts the same values here as on `grep`, described above.

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

# Show database location, size and search index health
grans admin db info

# List all database files
grans admin db list

# Rebuild the full-text search indexes from the tables they index
grans admin db rebuild-fts

# Bind the database to the current token's Granola account
grans admin db rebind
```

`admin db info` ends with a line per full-text index saying whether it still
agrees with the table it indexes. An index that has drifted makes `grep` and
`search` quietly return too few results while the database otherwise looks
healthy, and `admin db rebuild-fts` repairs it by re-deriving each index from
its source. Nothing is lost and no re-sync is needed.

`admin db rebind` is for switching the database to a different Granola account
on purpose (for example, after Granola's account-to-account note import). Sync
refuses to run while the token's account differs from the one the database is
bound to; rebinding appends a new binding, keeping the old one as history, and
does not change the account recorded on existing documents.

### Dropbox Sync

Share your grans database across multiple machines via Dropbox.

**Why use this?** Two operations in grans are slow:

1. **Transcript sync** (`grans sync transcripts`) - Fetches transcripts from Granola's API with rate limiting (~1.5s per document). For 200 meetings, that's ~5 minutes.

2. **Embedding generation** - `grans embed` builds vector embeddings for transcripts, panel sections, and notes, which takes time on CPU.

Once you've done this work on one machine, Dropbox sync lets you share the results instead of repeating it everywhere.

**Initial setup (on your primary machine):**

```bash
# 1. Sync all data including transcripts from Granola API
grans sync

# 2. Build embeddings (slow first time)
grans embed -y

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
grans search "deployment"
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

**Transfer progress:** `push` and `pull` both stream the database and show a progress bar with throughput and an ETA. Databases with embeddings run to hundreds of megabytes, so either can take a while on a slow connection:

```
Downloading database (424.3 MB)...
[grans] 114.55 MiB/424.25 MiB [========>                     ] 27% 4.21 MiB/s, ETA 1m
```

```
Uploading database (428.9 MB)...
[grans] 96.00 MiB/428.90 MiB [======>                       ] 22% 3.80 MiB/s, ETA 1m
```

The bar is written to stderr and is skipped when output is redirected. Uploads above 150 MB are sent as chunked sessions, so the bar advances 8 MB at a time; smaller ones stream in a single request and advance smoothly.

**Conflict handling:** Sync compares content, not timestamps. It records the content hash both copies held at the last successful sync, and uses that to tell which side has moved since:

| Situation | What happens |
|-----------|--------------|
| Both copies identical | Nothing transfers, either direction |
| Only your side changed | Transfers normally |
| The other side changed too | Refuses, naming which copy would be lost |
| Copies differ and grans has never synced them | Refuses, since neither can be shown to supersede the other |

Use `--force` to overwrite regardless:

```bash
grans dropbox push --force   # Replace the Dropbox copy with yours
grans dropbox pull --force   # Replace your copy with Dropbox's
```

Because identical copies transfer nothing, pushing or pulling twice in a row is cheap and safe: the second run compares hashes and stops.

**Verification:** a pull downloads to a temporary file and has to clear three checks before anything replaces your database:

| Check | Catches |
|-------|---------|
| Byte count against the size Dropbox reported | An interrupted or truncated transfer |
| Dropbox `content_hash`, computed while streaming | Content that differs from the stored file |
| `PRAGMA quick_check` plus a schema probe | A corrupt file, or one that is not a grans database |

If any check fails the temp file is discarded and your existing database is left untouched. The download is also flushed to the device before the rename, so a crash cannot leave a database whose contents were never written.

On a 424 MB database this costs about 1.3s; hashing runs alongside the transfer and adds no measurable time.

**Troubleshooting a failed sync:** run the command again with `--verbose` to log each request, its HTTP status, and the throughput achieved:

```bash
grans --verbose dropbox pull
```

```
[DEBUG grans::sync::dropbox] POST https://api.dropboxapi.com/2/files/get_metadata (metadata for /grans.db)
[DEBUG grans::sync::dropbox]   response: 200 OK in 270.7261ms
[DEBUG grans::sync::dropbox] POST https://content.dropboxapi.com/2/files/download (download /grans.db)
[DEBUG grans::sync::dropbox]   response: 200 OK in 712.5304ms
[DEBUG grans::sync::dropbox]   body complete: 424.28 MB in 6.7065285s (63.26 MB/s)
```

A download that fails reports how far it got, which separates a connection that dropped mid-transfer from one that never delivered anything.

Use `GRANS_LOG` for finer control, e.g. `GRANS_LOG=grans::sync=debug`.

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

Measure search quality against a suite of queries with known expected results. The suite is a JSON file you author against your own database:

```json
{
  "description": "My search quality suite",
  "queries": [
    {
      "query": "what did we decide about the API redesign",
      "query_type": "paraphrase",
      "relevant_meetings": ["Architecture Sync", "API v2 Planning"],
      "relevant_meeting_ids": ["abc123", "def456"],
      "rationale": "Both meetings covered the decision"
    }
  ]
}
```

Results are matched to labels by document ID when `relevant_meeting_ids` is present, otherwise by exact title against `relevant_meetings` (ID matching is preferred; recurring meetings often share one title, which over-credits title matching). `query_type` is an optional stratum label (for example `exact-term`, `paraphrase`, `mixed`); when present, the report includes a per-stratum breakdown. `rationale` is a free-text note for your own reference.

```bash
# Score semantic search (the default mode) at k=10
grans benchmark quality --file my-benchmark.json

# Score keyword (FTS) search instead
grans benchmark quality --file my-benchmark.json --mode fts

# Compare modes: per-query rank table plus win/loss/tie summary
grans benchmark quality --file my-benchmark.json --compare fts,semantic

# Check top 5 results
grans benchmark quality --file my-benchmark.json --k 5

# Show detailed results for each query
grans benchmark quality --file my-benchmark.json --detail

# Append the run to the results ledger (ledger.jsonl in the benchmarks
# directory, with full per-query output under runs/)
grans benchmark quality --file my-benchmark.json --record --note "baseline"
```

The benchmark reports:
- **hit-rate@k**: Percentage of queries where an expected meeting appears in the top k results
- **recall@k**: Fraction of each query's expected meetings found in the top k, averaged over queries
- **MRR@k**: Average of 1/rank of the first relevant result (0 when it falls outside the top k)
- **Latency**: Average and median per-query search time for the mode

This is useful for:
- Comparing keyword and semantic retrieval on the same suite
- Testing chunking strategy changes
- Evaluating embedding model updates
- Comparing search performance across database versions

Use the `--db` flag to benchmark against a different database file without affecting your main database:

```bash
grans --db /path/to/test.db benchmark quality --file my-benchmark.json
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

