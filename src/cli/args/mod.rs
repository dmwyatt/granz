use clap::{Parser, Subcommand, ValueEnum};

use crate::models::SpeakerFilter;
use crate::query::filter::SearchTarget;

fn parse_speaker_filter(s: &str) -> Result<SpeakerFilter, String> {
    SpeakerFilter::parse(s).ok_or_else(|| format!("invalid speaker filter '{}': expected 'me' or 'other'", s))
}

#[derive(Parser, Debug)]
#[command(name = "grans", version = env!("GRANS_VERSION"), about = "Query your Granola meeting notes")]
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

    /// Use a specific API token instead of reading from Granola's config
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
    /// Search meetings, transcripts, and notes (ranked discovery)
    ///
    /// Fuses keyword and semantic rankings, then reranks the top candidates
    /// with a cross-encoder (--fast skips the rerank stage). Results are the
    /// best few meetings for the query, not a complete list; when you need
    /// every meeting containing exact words, or matches attributed to a
    /// speaker, use `grans grep`. The first search downloads the embedding
    /// and reranker models, and a search may prompt before embedding new
    /// content (--yes accepts).
    #[command(visible_alias = "s")]
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
        #[arg(long, conflicts_with = "fast")]
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

        /// Skip embedding confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,

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
    /// match exactly. Never loads models and never prompts. Use
    /// --speaker to require the match in a specific speaker's utterances.
    /// For ranked discovery by meaning, use `grans search`.
    #[command(visible_alias = "g")]
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

        /// Filter matches by speaker: "me" (your utterances) or "other" (others' utterances); only meetings where that speaker's utterances match survive. Requires transcripts in --in
        #[arg(long, value_parser = parse_speaker_filter)]
        speaker: Option<SpeakerFilter>,

        /// Maximum number of meetings to show (0 = no limit)
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Include soft-deleted meetings in results
        #[arg(long)]
        include_deleted: bool,
    },

    /// List meetings
    #[command(visible_alias = "ls")]
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

    /// Show meeting details
    Show {
        /// Meeting ID or title substring
        meeting: String,

        /// Output only the transcript
        #[arg(long)]
        transcript: bool,

        /// Output only the notes
        #[arg(long)]
        notes: bool,

        /// Filter transcript by speaker: "me" (your utterances) or "other" (others' utterances)
        #[arg(long, value_parser = parse_speaker_filter)]
        speaker: Option<SpeakerFilter>,
    },

    /// Show meetings with a person
    #[command(visible_alias = "w")]
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
    Recent,

    /// Show today's meetings
    Today,

    /// Show database statistics
    Info,

    /// Sync data from Granola API
    Sync {
        #[command(subcommand)]
        action: Option<SyncAction>,

        /// Show what would be done without making changes
        #[arg(long, global = true)]
        dry_run: bool,
    },

    /// Dropbox sync (init, push, pull, status, logout)
    Dropbox {
        #[command(subcommand)]
        action: DropboxAction,
    },

    // === Grouped Commands ===
    /// Browse entities (people, calendars, templates, recipes)
    Browse {
        #[command(subcommand)]
        action: BrowseAction,
    },

    /// Administrative commands (db, token)
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },

    /// Update grans to the latest version
    Update {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,

        /// Use gh CLI authentication without prompting (for scripts)
        #[arg(long)]
        use_gh_auth: bool,

        /// Wait for in-progress builds to complete before updating (for scripts)
        #[arg(long)]
        wait: bool,

        /// Maximum time to wait for a build (seconds)
        #[arg(long, default_value = "600")]
        timeout: u64,
    },

    /// Build embeddings for hybrid search
    Embed {
        #[command(subcommand)]
        action: Option<EmbedAction>,

        /// Skip confirmation prompt
        #[arg(long, short = 'y', global = true)]
        yes: bool,

        /// Number of chunks to embed per batch (higher values use more memory but may be faster on GPU)
        #[arg(long, default_value = "16", global = true)]
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
    Benchmark {
        #[command(subcommand)]
        action: BenchmarkAction,
    },
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
    Quality {
        /// Path to benchmark JSON file
        #[arg(long)]
        file: std::path::PathBuf,

        /// Number of top results scored (hit-rate@k, recall@k, MRR@k)
        #[arg(long, default_value = "10")]
        k: usize,

        /// Search mode to benchmark
        #[arg(long, value_enum, default_value = "semantic", conflicts_with = "compare")]
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
    /// Show embedding status
    Status,
    /// Clear embeddings (for dev/testing)
    Clear {
        /// Number of most recent embeddings to clear (clears all if not specified)
        #[arg(long)]
        count: Option<usize>,
    },
}

// === Sync Subcommands ===

#[derive(Subcommand, Debug, Clone)]
pub enum SyncAction {
    /// Sync documents (meetings) from Granola API
    Documents,

    /// Sync transcripts for documents
    Transcripts {
        /// Fetch the transcript for a single document (full ID or unique prefix), replacing any existing transcript
        #[arg(value_name = "DOCUMENT_ID", conflicts_with_all = ["limit", "since", "delay_ms", "retry"])]
        document_id: Option<String>,

        /// Maximum number of documents to fetch transcripts for
        #[arg(long)]
        limit: Option<usize>,

        /// Only sync transcripts for documents created on or after this date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        since: Option<String>,

        /// Delay between API requests in milliseconds
        #[arg(long, default_value = "1500")]
        delay_ms: u64,

        /// Retry documents that previously failed or had no transcript
        #[arg(long)]
        retry: bool,

        /// Build embeddings after sync completes
        #[arg(long)]
        embed: bool,
    },

    /// Sync people (contacts) from Granola API
    People,

    /// Sync calendar events from Granola API
    Calendars,

    /// Sync panel templates from Granola API
    Templates,

    /// Sync recipes from Granola API
    Recipes,

    /// Sync AI-generated panels for documents
    Panels {
        /// Maximum number of documents to fetch panels for
        #[arg(long)]
        limit: Option<usize>,

        /// Only sync panels for documents created on or after this date [e.g., 2024-01-15, 2024-01-15T10:30:00Z, or duration: 3d, 2w, 1m]
        #[arg(long)]
        since: Option<String>,

        /// Delay between API requests in milliseconds
        #[arg(long, default_value = "1500")]
        delay_ms: u64,

        /// Retry documents that previously failed or had no panels
        #[arg(long)]
        retry: bool,
    },
}

// === Browse Subcommands ===

#[derive(Subcommand, Debug)]
pub enum BrowseAction {
    /// Query people
    People {
        #[command(subcommand)]
        action: PeopleAction,
    },
    /// Query calendars
    Calendars {
        #[command(subcommand)]
        action: CalendarsAction,
    },
    /// Browse templates
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
    /// Browse recipes
    Recipes {
        #[command(subcommand)]
        action: RecipesAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum PeopleAction {
    /// List people
    List {
        /// Filter by company name
        #[arg(long)]
        company: Option<String>,
    },
    /// Show person details
    Show {
        /// Person ID, name, or email fragment
        query: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum CalendarsAction {
    /// List calendars
    List,
    /// Show calendar events
    Events {
        /// Filter by calendar ID
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
    List {
        /// Filter by category
        #[arg(long)]
        category: Option<String>,
    },
    /// Show template details
    Show {
        /// Template ID or title substring
        query: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RecipesAction {
    /// List recipes
    List {
        /// Filter by visibility (public, shared, user, unlisted)
        #[arg(long)]
        visibility: Option<String>,
    },
    /// Show recipe details
    Show {
        /// Recipe ID or name substring
        query: String,
    },
}

// === Admin Subcommands ===

#[derive(Subcommand, Debug)]
pub enum AdminAction {
    /// Database management
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Print the current Granola API token
    Token {
        /// Copy to clipboard instead of printing
        #[arg(long, short = 'c')]
        clipboard: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DbAction {
    /// Clear the database (run 'grans sync' to repopulate)
    Clear {
        /// Remove all database files in the data directory
        #[arg(long)]
        all: bool,
    },
    /// Show database location and size
    Info,
    /// List all database files
    List,
}

#[derive(Subcommand, Debug)]
pub enum DropboxAction {
    /// Set up Dropbox authentication (one-time setup)
    Init,
    /// Upload database to Dropbox
    Push {
        /// Overwrite even if remote is newer
        #[arg(long)]
        force: bool,
    },
    /// Download database from Dropbox
    Pull {
        /// Overwrite even if local is newer
        #[arg(long)]
        force: bool,
    },
    /// Show sync status
    Status,
    /// Remove Dropbox authentication
    Logout,
}

#[cfg(test)]
mod tests;
