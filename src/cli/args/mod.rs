use clap::{Parser, Subcommand, ValueEnum};

use crate::cli::help;
use crate::query::filter::SearchTarget;
use crate::query::speaker::SpeakerSelector;

fn parse_speaker_selector(s: &str) -> Result<SpeakerSelector, String> {
    SpeakerSelector::parse(s).ok_or_else(|| {
        "empty speaker filter: expected 'me', 'other', or a speaker's name".to_string()
    })
}

/// Reranker scores are sigmoid probabilities, so a threshold outside
/// [0, 1] (or a NaN, which no comparison admits) can only reject every
/// candidate. Refusing it here beats an empty result set that reads like
/// an empty corpus.
fn parse_min_score(s: &str) -> Result<f32, String> {
    let score: f32 = s.parse().map_err(|_| "not a number".to_string())?;
    if !(0.0..=1.0).contains(&score) {
        return Err("must be between 0.0 and 1.0 (reranker scores are probabilities)".to_string());
    }
    Ok(score)
}

#[derive(Parser, Debug)]
#[command(
    name = "grans",
    version = env!("GRANS_VERSION"),
    about = "Query your Granola meeting notes",
    after_long_help = help::overview()
)]
pub struct Cli {
    /// Output as JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Disable colored output (uses human-readable format without ANSI codes)
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Display timestamps in UTC instead of local time
    #[arg(long, global = true)]
    pub utc: bool,

    /// Use a specific database file instead of the default
    #[arg(long, global = true)]
    pub db: Option<std::path::PathBuf>,

    /// Use a specific API token [env: GRANS_TOKEN]
    ///
    /// Without this, grans uses its own stored credentials (see
    /// `grans auth login`), falling back to the token Granola's desktop app
    /// stored locally.
    #[arg(long, global = true)]
    pub token: Option<String>,

    /// Enable verbose output for debugging API calls, sync operations, and errors
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    // === Daily Use Commands ===
    /// Find the meetings most relevant to a topic (ranked, by meaning and by words)
    ///
    /// Fuses keyword and semantic rankings, then reranks the top candidates
    /// with a cross-encoder (--fast skips the rerank stage). Results are the
    /// best few meetings for the query, not a complete list; when you need
    /// every meeting containing exact words, or what one person said, use
    /// `grans grep`. The first search downloads the embedding and reranker
    /// models.
    #[command(visible_alias = "s", after_long_help = help::for_command("search"))]
    Search {
        /// Search query; words match in any order, "quoted phrases" must match exactly
        query: String,

        /// Where to search (comma-separated)
        #[arg(long, value_delimiter = ',', default_value = crate::query::filter::DEFAULT_SEARCH_TARGETS)]
        r#in: Vec<SearchTarget>,

        /// Skip the cross-encoder rerank stage
        /// (fusion order only; faster, but no relevance scores)
        #[arg(long)]
        fast: bool,

        /// Minimum reranker relevance score (0-1)
        #[arg(long, conflicts_with = "fast", value_parser = parse_min_score)]
        min_score: Option<f32>,

        /// Maximum match snippets shown per meeting in search results
        /// (0 = headers only)
        #[arg(long, default_value = "1")]
        matches: usize,

        /// Context shown around each match snippet: utterances for transcripts, sections for AI notes, paragraphs for notes (0 = disabled)
        #[arg(long, default_value = "0")]
        context: usize,

        /// Limit to a specific meeting (ID or title substring)
        #[arg(long)]
        meeting: Option<String>,

        /// Filter from date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        from: Option<String>,

        /// Filter to date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        to: Option<String>,

        /// Relative date filter, overrides --from/--to [today, yesterday, this-week, last-week, this-month, last-month]
        #[arg(long)]
        date: Option<String>,

        /// Maximum number of results to return (0 = no limit)
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Include soft-deleted meetings in results
        #[arg(long)]
        include_deleted: bool,
    },

    /// List every meeting containing the given words
    ///
    /// Complete lexical lookup over the local full-text index: the reported
    /// count is a fact about your synced meetings, and --limit only trims
    /// how many are shown. Words match in any order; "quoted phrases" must
    /// match exactly. Use --speaker to require the match in what one
    /// speaker said. To find meetings by meaning, use `grans search`.
    #[command(visible_alias = "g", after_long_help = help::for_command("grep"))]
    Grep {
        /// Words to look up; words match in any order, "quoted phrases" must match exactly
        query: String,

        /// Where to search (comma-separated)
        #[arg(long, value_delimiter = ',', default_value = crate::query::filter::DEFAULT_SEARCH_TARGETS)]
        r#in: Vec<SearchTarget>,

        /// Maximum match snippets shown per meeting (0 = headers only)
        #[arg(long, default_value = "1")]
        matches: usize,

        /// Context shown around each match snippet: utterances for transcripts, sections for AI notes, paragraphs for notes (0 = disabled)
        #[arg(long, default_value = "0")]
        context: usize,

        /// Limit to a specific meeting (ID or title substring)
        #[arg(long)]
        meeting: Option<String>,

        /// Filter from date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        from: Option<String>,

        /// Filter to date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        to: Option<String>,

        /// Relative date filter, overrides --from/--to [today, yesterday, this-week, last-week, this-month, last-month]
        #[arg(long)]
        date: Option<String>,

        /// Filter matches by speaker: "me" (your utterances), "other" (everyone else), or a name to match Granola's detected speaker (partial names are fine); only meetings where that speaker's utterances match survive. Requires transcripts in --in
        #[arg(long, value_parser = parse_speaker_selector)]
        speaker: Option<SpeakerSelector>,

        /// Maximum number of meetings to show (0 = no limit)
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Include soft-deleted meetings in results
        #[arg(long)]
        include_deleted: bool,
    },

    /// List meetings
    #[command(visible_alias = "ls", after_long_help = help::for_command("list"))]
    List {
        /// Filter by person name or email
        #[arg(long)]
        person: Option<String>,

        /// Filter from date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        from: Option<String>,

        /// Filter to date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        to: Option<String>,

        /// Relative date filter, overrides --from/--to [today, yesterday, this-week, last-week, this-month, last-month]
        #[arg(long)]
        date: Option<String>,

        /// Include soft-deleted meetings in results
        #[arg(long)]
        include_deleted: bool,
    },

    /// Show one meeting: its details, transcript, your notes, and AI notes
    #[command(after_long_help = help::for_command("show"))]
    Show {
        /// Meeting ID or title substring
        meeting: String,

        /// Output only the transcript
        #[arg(long)]
        transcript: bool,

        /// Output only the notes you wrote
        #[arg(long)]
        notes: bool,

        /// Filter transcript by speaker: "me" (your utterances), "other" (everyone else), or a name to match Granola's detected speaker (partial names are fine)
        #[arg(long, value_parser = parse_speaker_selector)]
        speaker: Option<SpeakerSelector>,
    },

    /// Show meetings with a person
    #[command(visible_alias = "w", after_long_help = help::for_command("with"))]
    With {
        /// Person name or email fragment
        person: String,

        /// Filter from date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        from: Option<String>,

        /// Filter to date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        to: Option<String>,

        /// Relative date filter, overrides --from/--to [today, yesterday, this-week, last-week, this-month, last-month]
        #[arg(long)]
        date: Option<String>,

        /// Include soft-deleted meetings in results
        #[arg(long)]
        include_deleted: bool,
    },

    /// Show this week's meetings
    #[command(after_long_help = help::for_command("recent"))]
    Recent,

    /// Show today's meetings
    #[command(after_long_help = help::for_command("today"))]
    Today,

    /// Show what the local database holds
    #[command(after_long_help = help::for_command("info"))]
    Info,

    /// Fetch your meetings from Granola into the local database
    #[command(
        args_conflicts_with_subcommands = true,
        after_long_help = help::for_command("sync")
    )]
    Sync {
        #[command(subcommand)]
        action: Option<SyncAction>,

        /// Everything: the meeting list, then transcripts, then AI notes, then embeddings
        #[arg(long)]
        all: bool,

        /// Also retry meetings whose transcript or AI notes failed to fetch before
        #[arg(long, requires = "all")]
        retry: bool,

        /// Show what would be done without making changes
        #[arg(long, global = true)]
        dry_run: bool,
    },

    /// Share the database between machines through Dropbox
    #[command(after_long_help = help::for_command("dropbox"))]
    Dropbox {
        #[command(subcommand)]
        action: DropboxAction,
    },

    /// Manage grans's Granola sign-in (login, status, logout)
    #[command(after_long_help = help::for_command("auth"))]
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },

    // === Grouped Commands ===
    /// Browse people, calendars, templates, and recipes
    #[command(after_long_help = help::for_command("browse"))]
    Browse {
        #[command(subcommand)]
        action: BrowseAction,
    },

    /// Administrative commands (db, token)
    #[command(after_long_help = help::for_command("admin"))]
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },

    /// Update grans to the latest version
    #[command(after_long_help = help::for_command("update"))]
    Update {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,

        /// Use gh CLI authentication without prompting (for scripts)
        #[arg(long)]
        use_gh_auth: bool,

        /// Install without confirming, waiting first if a release build is in progress (for scripts). Stops with an error if the build status cannot be checked or the build's release does not appear.
        #[arg(long)]
        wait: bool,

        /// Maximum time to wait for a build (seconds)
        #[arg(long, default_value = "600")]
        timeout: u64,
    },

    /// Build the embeddings that let search match by meaning
    #[command(after_long_help = help::for_command("embed"))]
    Embed {
        #[command(subcommand)]
        action: Option<EmbedAction>,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,

        /// Number of chunks to embed per batch (higher values use more memory but may be faster on GPU)
        #[arg(long, default_value = "16")]
        batch_size: usize,

        /// Experiment knob: target tokens per chunk (overrides the stored scheme)
        #[arg(long, hide = true, value_name = "N")]
        chunk_target_tokens: Option<usize>,

        /// Experiment knob: overlap tokens between chunks (overrides the stored scheme)
        #[arg(long, hide = true, value_name = "N")]
        chunk_overlap_tokens: Option<usize>,

        /// Experiment knob: how consecutive transcript chunks overlap
        #[arg(long, hide = true, value_parser = ["chars", "utterances"])]
        overlap_mode: Option<String>,

        /// Experiment knob: prepend meeting title/date/attendees to the embed input
        #[arg(long, hide = true, num_args = 0..=1, default_missing_value = "true")]
        contextual_headers: Option<bool>,
    },

    /// Benchmarking commands
    #[command(after_long_help = help::for_command("benchmark"))]
    Benchmark {
        #[command(subcommand)]
        action: BenchmarkAction,
    },

    /// Print a skill that points AI agents at this help
    ///
    /// Writes an agent skill (a SKILL.md) to stdout, for you to redirect to
    /// wherever your agent tool reads skills.
    #[command(after_long_help = help::for_command("skill"))]
    Skill,
}

// === Benchmark Subcommands ===

/// Search mode measured by the quality benchmark. New modes appear here as
/// the hybrid pipeline phases land.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityMode {
    /// FTS5 keyword search (the grep verb's retrieval)
    Fts,
    /// Semantic search over embeddings
    Semantic,
    /// RRF fusion of FTS and semantic rankings (what `search --fast` shows)
    Hybrid,
    /// Fusion + jina-reranker-v1-turbo-en cross-encoder blended with the
    /// fusion prior (the full search pipeline)
    RerankJina,
    /// Fusion + bge-reranker-base cross-encoder
    RerankBge,
}

impl QualityMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            QualityMode::Fts => "fts",
            QualityMode::Semantic => "semantic",
            QualityMode::Hybrid => "hybrid",
            QualityMode::RerankJina => "rerank-jina",
            QualityMode::RerankBge => "rerank-bge",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum BenchmarkAction {
    /// Benchmark semantic search performance
    #[command(after_long_help = help::for_command("benchmark semantic-search"))]
    SemanticSearch {
        /// Number of search queries to run
        #[arg(long, default_value = "100")]
        queries: usize,

        /// Use synthetic vectors instead of real data
        #[arg(long)]
        synthetic: bool,

        /// Number of vectors to generate in synthetic mode
        #[arg(long, default_value = "10000")]
        vectors: usize,

        /// Number of warmup queries before measuring
        #[arg(long, default_value = "5")]
        warmup: usize,

        /// Minimum similarity score threshold
        #[arg(long, default_value = "0.0")]
        min_score: f32,
    },

    /// Run search quality benchmark against a labeled golden set
    #[command(after_long_help = help::for_command("benchmark quality"))]
    Quality {
        /// Path to benchmark JSON file
        #[arg(long)]
        file: std::path::PathBuf,

        /// Number of top results scored (hit-rate@k, recall@k, MRR@k)
        #[arg(long, default_value = "10")]
        k: usize,

        /// Search mode to benchmark
        #[arg(
            long,
            value_enum,
            default_value = "semantic",
            conflicts_with = "compare"
        )]
        mode: QualityMode,

        /// Compare modes, e.g. fts,semantic: per-query rank table plus
        /// win/loss/tie summary
        #[arg(long, value_enum, value_delimiter = ',')]
        compare: Vec<QualityMode>,

        /// Show detailed results for each query
        #[arg(long)]
        detail: bool,

        /// Append results to the ledger and save per-query output under runs/
        /// (both in the benchmarks directory next to the golden set)
        #[arg(long)]
        record: bool,

        /// Note stored with the ledger entry
        #[arg(long, requires = "record")]
        note: Option<String>,

        /// Write each query's reranked candidates (fused rank, RRF score,
        /// passage, rerank score) as JSONL, for offline ranking experiments
        /// (rerank modes only)
        #[arg(long, value_name = "PATH", conflicts_with = "compare")]
        dump_candidates: Option<std::path::PathBuf>,

        /// Experiment knob: title-match boost weight for rerank modes,
        /// overriding the adopted default (0 disables the boost)
        #[arg(long, hide = true, value_name = "W")]
        title_boost_weight: Option<f32>,
    },
}

// === Embed Subcommands ===

#[derive(Subcommand, Debug)]
pub enum EmbedAction {
    /// Show how much content is embedded and how much is waiting
    #[command(after_long_help = help::for_command("embed status"))]
    Status,
    /// Clear embeddings (for dev/testing)
    #[command(after_long_help = help::for_command("embed clear"))]
    Clear {
        /// Number of most recent embeddings to clear (clears all if not specified)
        #[arg(long)]
        count: Option<usize>,

        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

// === Sync Subcommands ===

/// Default delay between per-document API requests, shared by the transcript
/// and panel sync legs and the `sync --all` pipeline.
pub const DEFAULT_SYNC_DELAY_MS: u64 = 1500;

#[derive(Subcommand, Debug, Clone)]
pub enum SyncAction {
    /// Sync the meeting list (Granola calls meetings documents)
    #[command(after_long_help = help::for_command("sync documents"))]
    Documents,

    /// Sync transcripts for meetings that have none
    #[command(after_long_help = help::for_command("sync transcripts"))]
    Transcripts {
        /// Fetch the transcript for a single meeting (full ID or unique prefix), replacing any existing transcript
        #[arg(value_name = "DOCUMENT_ID", conflicts_with_all = ["limit", "since", "delay_ms", "retry"])]
        document_id: Option<String>,

        /// Maximum number of meetings to fetch transcripts for
        #[arg(long)]
        limit: Option<usize>,

        /// Only sync transcripts for meetings created on or after this date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        since: Option<String>,

        /// Delay between API requests in milliseconds
        #[arg(long, default_value_t = DEFAULT_SYNC_DELAY_MS)]
        delay_ms: u64,

        /// Retry meetings that previously failed or had no transcript
        #[arg(long)]
        retry: bool,

        /// Build embeddings after sync completes
        #[arg(long)]
        embed: bool,
    },

    /// Sync people (contacts) from Granola API
    #[command(after_long_help = help::for_command("sync people"))]
    People,

    /// Sync calendar events from Granola API
    #[command(after_long_help = help::for_command("sync calendars"))]
    Calendars,

    /// Sync the templates Granola generates AI notes from
    #[command(after_long_help = help::for_command("sync templates"))]
    Templates,

    /// Sync recipes (Granola's saved prompts)
    #[command(after_long_help = help::for_command("sync recipes"))]
    Recipes,

    /// Sync AI notes: the summaries Granola generates (it calls them panels)
    #[command(after_long_help = help::for_command("sync panels"))]
    Panels {
        /// Maximum number of meetings to fetch AI notes for
        #[arg(long)]
        limit: Option<usize>,

        /// Only sync AI notes for meetings created on or after this date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        since: Option<String>,

        /// Delay between API requests in milliseconds
        #[arg(long, default_value_t = DEFAULT_SYNC_DELAY_MS)]
        delay_ms: u64,

        /// Retry meetings that previously failed or had no AI notes
        #[arg(long)]
        retry: bool,
    },
}

// === Browse Subcommands ===

#[derive(Subcommand, Debug)]
pub enum BrowseAction {
    /// The people Granola has on record
    #[command(after_long_help = help::for_command("browse people"))]
    People {
        #[command(subcommand)]
        action: PeopleAction,
    },
    /// Your calendars and their events
    #[command(after_long_help = help::for_command("browse calendars"))]
    Calendars {
        #[command(subcommand)]
        action: CalendarsAction,
    },
    /// The templates Granola generates AI notes from
    #[command(after_long_help = help::for_command("browse templates"))]
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
    /// Granola's recipes (saved prompts)
    #[command(after_long_help = help::for_command("browse recipes"))]
    Recipes {
        #[command(subcommand)]
        action: RecipesAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum PeopleAction {
    /// List people
    #[command(after_long_help = help::for_command("browse people list"))]
    List {
        /// Filter by company name
        #[arg(long)]
        company: Option<String>,
    },
    /// Show person details
    #[command(after_long_help = help::for_command("browse people show"))]
    Show {
        /// Person ID, name, or email fragment
        query: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum CalendarsAction {
    /// List calendars
    #[command(after_long_help = help::for_command("browse calendars list"))]
    List,
    /// Show calendar events
    #[command(after_long_help = help::for_command("browse calendars events"))]
    Events {
        /// Filter by calendar ID (any part of it)
        #[arg(long)]
        calendar: Option<String>,

        /// Filter from date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        from: Option<String>,

        /// Filter to date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        to: Option<String>,

        /// Relative date filter, overrides --from/--to [today, yesterday, this-week, last-week, this-month, last-month]
        #[arg(long)]
        date: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TemplatesAction {
    /// List templates
    #[command(after_long_help = help::for_command("browse templates list"))]
    List {
        /// Filter by category
        #[arg(long)]
        category: Option<String>,
    },
    /// Show template details
    #[command(after_long_help = help::for_command("browse templates show"))]
    Show {
        /// Template ID or title substring
        query: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RecipesAction {
    /// List recipes
    #[command(after_long_help = help::for_command("browse recipes list"))]
    List {
        /// Filter by visibility (public, shared, user, unlisted)
        #[arg(long)]
        visibility: Option<String>,
    },
    /// Show recipe details
    #[command(after_long_help = help::for_command("browse recipes show"))]
    Show {
        /// Recipe ID or name substring
        query: String,
    },
}

// === Admin Subcommands ===

#[derive(Subcommand, Debug)]
pub enum AdminAction {
    /// Database management
    #[command(after_long_help = help::for_command("admin db"))]
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Print the current Granola API token
    #[command(after_long_help = help::for_command("admin token"))]
    Token {
        /// Copy to clipboard instead of printing
        #[arg(long, short = 'c')]
        clipboard: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DbAction {
    /// Delete the database (run `grans sync --all` to download it again)
    #[command(after_long_help = help::for_command("admin db clear"))]
    Clear {
        /// Remove all database files in the data directory
        #[arg(long)]
        all: bool,
    },
    /// Show database location, size and search index health
    #[command(after_long_help = help::for_command("admin db info"))]
    Info,
    /// List all database files
    #[command(after_long_help = help::for_command("admin db list"))]
    List,
    /// Rebuild the full-text search indexes from the tables they index
    ///
    /// The repair for the drift that `grans admin db info` reports. Re-derives
    /// each index from its source, so nothing is lost and no re-sync is needed.
    #[command(after_long_help = help::for_command("admin db rebuild-fts"))]
    RebuildFts,
}

// === Auth Subcommands ===

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Sign in to Granola and store credentials for grans
    ///
    /// Opens a browser to Granola's login. It ends on a granola.ai page that
    /// tries to hand off to the Granola app rather than to grans, so cancel
    /// that dialog and paste the address bar URL back here instead.
    #[command(after_long_help = help::for_command("auth login"))]
    Login {
        /// Identity provider to sign in with
        #[arg(long, value_enum, default_value_t = AuthProvider::Google)]
        provider: AuthProvider,

        /// Read an existing refresh token from stdin instead of signing in
        ///
        /// Keeps the token out of shell history and the process list.
        #[arg(long)]
        refresh_token_stdin: bool,
    },
    /// Show whether grans has its own Granola session, and which account it is
    ///
    /// The account email comes from the local accounts log when this database
    /// has synced from the account before; otherwise one get-user-info API
    /// call fetches it.
    #[command(after_long_help = help::for_command("auth status"))]
    Status,
    /// Remove grans's stored credentials
    #[command(after_long_help = help::for_command("auth logout"))]
    Logout,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum AuthProvider {
    Google,
    Microsoft,
}

#[derive(Subcommand, Debug)]
pub enum DropboxAction {
    /// Set up Dropbox authentication (one-time setup)
    #[command(after_long_help = help::for_command("dropbox init"))]
    Init,
    /// Upload database to Dropbox
    #[command(after_long_help = help::for_command("dropbox push"))]
    Push {
        /// Overwrite even if remote is newer
        #[arg(long)]
        force: bool,
    },
    /// Download database from Dropbox
    #[command(after_long_help = help::for_command("dropbox pull"))]
    Pull {
        /// Overwrite even if local is newer
        #[arg(long)]
        force: bool,
    },
    /// Show sync status
    #[command(after_long_help = help::for_command("dropbox status"))]
    Status,
    /// Remove Dropbox authentication
    #[command(after_long_help = help::for_command("dropbox logout"))]
    Logout,
}

#[cfg(test)]
mod tests;
