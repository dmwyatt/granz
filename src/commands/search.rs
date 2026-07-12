use std::io::{self, Write};

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::models::{Document, SpeakerFilter};
use crate::output::format::OutputMode;
use crate::query::dates::DateRange;
use crate::query::filter::SearchTarget;

/// Threshold for prompting before embedding during semantic search.
const EMBED_WARN_THRESHOLD: usize = 200;

/// Typed search mode, replacing the old 11-parameter search function.
/// Each variant carries only the parameters it actually uses.
pub enum SearchMode {
    Keyword {
        targets: Vec<SearchTarget>,
        meeting_filter: Option<String>,
        limit: usize,
    },
    ContextWindow {
        targets: Vec<SearchTarget>,
        context_size: usize,
        meeting_filter: Option<String>,
        speaker_filter: Option<SpeakerFilter>,
        limit: usize,
    },
    Semantic {
        targets: Vec<SearchTarget>,
        context_size: usize,
        speaker_filter: Option<SpeakerFilter>,
        yes: bool,
        limit: usize,
    },
    Hybrid {
        targets: Vec<SearchTarget>,
        meeting_filter: Option<String>,
        rerank: bool,
        min_score: Option<f32>,
        yes: bool,
        limit: usize,
        /// Match snippets shown per meeting card.
        matches: usize,
    },
}

impl SearchMode {
    /// Construct a SearchMode from CLI arguments.
    ///
    /// A bare search runs hybrid. --semantic, --context, and --keyword force
    /// their modes, and --speaker also routes to the keyword path because
    /// hybrid does not use it. --hybrid needs no parameter here: it is the
    /// default, and clap conflicts keep it from combining with forcing flags.
    #[allow(clippy::too_many_arguments)]
    pub fn from_cli_args(
        fast: bool,
        min_score: Option<f32>,
        keyword: bool,
        semantic: bool,
        context: usize,
        in_targets: &str,
        meeting_filter: Option<&str>,
        speaker: Option<&SpeakerFilter>,
        yes: bool,
        limit: usize,
        matches: usize,
    ) -> Self {
        if semantic {
            SearchMode::Semantic {
                targets: SearchTarget::parse_list(in_targets),
                context_size: context,
                speaker_filter: speaker.cloned(),
                yes,
                limit,
            }
        } else if context > 0 {
            SearchMode::ContextWindow {
                targets: SearchTarget::parse_list(in_targets),
                context_size: context,
                meeting_filter: meeting_filter.map(String::from),
                speaker_filter: speaker.cloned(),
                limit,
            }
        } else if keyword || speaker.is_some() {
            SearchMode::Keyword {
                targets: SearchTarget::parse_list(in_targets),
                meeting_filter: meeting_filter.map(String::from),
                limit,
            }
        } else {
            SearchMode::Hybrid {
                targets: SearchTarget::parse_list(in_targets),
                meeting_filter: meeting_filter.map(String::from),
                rerank: !fast,
                min_score,
                yes,
                limit,
                matches,
            }
        }
    }
}

/// Unified search entry point. Dispatches to the appropriate handler
/// based on the SearchMode variant.
pub fn search(
    conn: &Connection,
    query: &str,
    mode: SearchMode,
    date_range: Option<DateRange>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    match mode {
        SearchMode::Semantic {
            targets,
            context_size,
            speaker_filter,
            yes,
            limit,
        } => semantic_search(conn, query, &targets, context_size, date_range, speaker_filter.as_ref(), yes, limit, include_deleted, ctx),
        SearchMode::ContextWindow {
            targets,
            context_size,
            meeting_filter,
            speaker_filter,
            limit,
        } => context_window_search(
            conn,
            query,
            &targets,
            meeting_filter.as_deref(),
            context_size,
            limit,
            date_range,
            speaker_filter.as_ref(),
            include_deleted,
            ctx,
        ),
        SearchMode::Keyword {
            targets,
            meeting_filter,
            limit,
        } => keyword_search(
            conn,
            query,
            &targets,
            meeting_filter.as_deref(),
            limit,
            date_range,
            include_deleted,
            ctx,
        ),
        SearchMode::Hybrid {
            targets,
            meeting_filter,
            rerank,
            min_score,
            yes,
            limit,
            matches,
        } => hybrid_search(
            conn,
            query,
            &targets,
            meeting_filter.as_deref(),
            rerank,
            min_score,
            yes,
            limit,
            matches,
            date_range,
            include_deleted,
            ctx,
        ),
    }
}

/// Truncate a vec to `limit` items. A limit of 0 means no limit.
fn apply_limit<T>(mut items: Vec<T>, limit: usize) -> Vec<T> {
    if limit > 0 && items.len() > limit {
        items.truncate(limit);
    }
    items
}

/// Truncate two vecs to a combined `limit`, taking from the first vec first.
/// A limit of 0 means no limit.
fn apply_limit_mixed<A, B>(mut a: Vec<A>, mut b: Vec<B>, limit: usize) -> (Vec<A>, Vec<B>) {
    if limit == 0 {
        return (a, b);
    }
    let total = a.len() + b.len();
    if total <= limit {
        return (a, b);
    }
    if a.len() >= limit {
        a.truncate(limit);
        b.clear();
    } else {
        b.truncate(limit - a.len());
    }
    (a, b)
}

fn keyword_search(
    conn: &Connection,
    query: &str,
    targets: &[SearchTarget],
    meeting_filter: Option<&str>,
    limit: usize,
    date_range: Option<DateRange>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    let search_titles = targets.contains(&SearchTarget::Titles);
    let search_transcripts = targets.contains(&SearchTarget::Transcripts);
    let search_notes = targets.contains(&SearchTarget::Notes);
    let search_panels = targets.contains(&SearchTarget::Panels);

    let results = crate::db::meetings::search_meetings(
        conn,
        query,
        search_titles,
        search_transcripts,
        search_notes,
        search_panels,
        date_range.as_ref(),
        include_deleted,
    )?;

    let results = filter_by_meeting(results, meeting_filter);
    render_meeting_list(results, query, limit, ctx);

    Ok(())
}

/// Run keyword and semantic retrieval, fuse the rankings, rerank the top
/// candidates (unless skipped via --fast), and display the resulting
/// meetings as shaped cards with match evidence.
#[allow(clippy::too_many_arguments)]
fn hybrid_search(
    conn: &Connection,
    query: &str,
    targets: &[SearchTarget],
    meeting_filter: Option<&str>,
    rerank: bool,
    min_score: Option<f32>,
    yes: bool,
    limit: usize,
    matches: usize,
    date_range: Option<DateRange>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    if !confirm_embedding_work(conn, yes, ctx)? {
        return Ok(());
    }

    let embedder = crate::embed::model::FastEmbedModel::new()?;
    let spec = crate::embed::config::EmbedSpec::resolve_stored(conn, crate::embed::MODEL_MAX_TOKENS);
    let index =
        crate::embed::ensure_embeddings(conn, &embedder, crate::embed::DEFAULT_BATCH_SIZE, &spec)?;

    let ranking = crate::query::hybrid::hybrid_ranked(
        conn,
        &embedder,
        &index,
        query,
        targets,
        date_range.as_ref(),
        include_deleted,
    )?;

    // Ranked (id, score) pairs; ordering is the pipeline's and is not
    // touched again below.
    let ordered: Vec<(String, Option<f32>)> = if rerank {
        let reranker =
            crate::embed::rerank::FastEmbedReranker::new(crate::embed::rerank::DEFAULT_RERANK_MODEL)?;
        let ranking_ctx = crate::query::adjust::RankingContext::load(conn)?;
        let cfg = crate::query::adjust::RankingConfig::default();
        let mut reranked = crate::query::rerank::rerank_hybrid(
            conn,
            &reranker,
            query,
            &ranking,
            &ranking_ctx,
            &cfg,
        )?;
        if let Some(min) = min_score {
            reranked.retain(|d| d.score >= min);
        }
        reranked.into_iter().map(|d| (d.document_id, Some(d.score))).collect()
    } else {
        ranking.fused.iter().map(|d| (d.document_id.clone(), None)).collect()
    };

    let ids: Vec<String> = ordered.iter().map(|(id, _)| id.clone()).collect();
    let docs = crate::db::meetings::get_meetings_by_ids(conn, &ids)?;
    let mut doc_by_id: std::collections::HashMap<String, Document> =
        docs.into_iter().filter_map(|d| d.id.clone().map(|id| (id, d))).collect();
    let ordered_docs: Vec<(Document, Option<f32>)> = ordered
        .into_iter()
        .filter_map(|(id, score)| doc_by_id.remove(&id).map(|doc| (doc, score)))
        .collect();

    let filter_lower = meeting_filter.map(str::to_lowercase);
    let filtered: Vec<(Document, Option<f32>)> = ordered_docs
        .into_iter()
        .filter(|(doc, _)| {
            filter_lower.as_deref().is_none_or(|f| matches_meeting_filter(doc, f))
        })
        .collect();

    let total = filtered.len();
    let page = apply_limit(filtered, limit);

    let tokens = crate::query::fts::parse_query(query);
    let limits = crate::query::evidence::EvidenceLimits {
        max_matches: matches,
        ..Default::default()
    };
    let shaped = shape_page(conn, &page, &ranking, &tokens, &limits)?;

    render_shaped_meeting_list(&shaped, query, total, limit, ctx);
    Ok(())
}

/// Shape one display page of ranked documents into meeting cards, in the
/// order given.
fn shape_page(
    conn: &Connection,
    page: &[(Document, Option<f32>)],
    ranking: &crate::query::hybrid::HybridRanking,
    tokens: &[crate::query::fts::FtsToken],
    limits: &crate::query::evidence::EvidenceLimits,
) -> Result<Vec<crate::query::shape::ShapedMeeting>> {
    page.iter()
        .map(|(doc, score)| {
            let doc_id = doc.id.as_deref().unwrap_or_default();
            let facts = crate::query::evidence::RankingFacts {
                keyword: ranking.keyword_ids.contains(doc_id),
                best_chunk: ranking.best_chunks.get(doc_id),
                score: *score,
            };
            crate::query::evidence::shape_meeting(conn, doc, tokens, &facts, limits)
        })
        .collect()
}

/// Print shaped meeting cards, honoring the output mode.
fn render_shaped_meeting_list(
    shaped: &[crate::query::shape::ShapedMeeting],
    query: &str,
    total: usize,
    limit: usize,
    ctx: &RunContext,
) {
    match ctx.output_mode {
        OutputMode::Json => {
            println!(
                "{}",
                crate::output::json::format_shaped_meetings(shaped, query, total, limit)
            );
        }
        OutputMode::Tty => {
            if shaped.is_empty() {
                println!("No meetings found matching \"{}\".", query);
                return;
            }
            if total > shaped.len() {
                println!(
                    "Found {} meeting(s) matching \"{}\" (showing {}):\n",
                    total,
                    query,
                    shaped.len()
                );
            } else {
                println!("Found {} meeting(s) matching \"{}\":\n", shaped.len(), query);
            }
            for (i, meeting) in shaped.iter().enumerate() {
                println!(
                    "{}\n",
                    crate::output::card::format_shaped_meeting(meeting, i + 1, &ctx.tz)
                );
            }
            if total > shaped.len() {
                println!("Use --limit 0 to show all {} results.", total);
            }
        }
    }
}

/// True when the document's title or id contains the lowercased filter.
fn matches_meeting_filter(doc: &Document, filter_lower: &str) -> bool {
    doc.title
        .as_ref()
        .map(|t| t.to_lowercase().contains(filter_lower))
        .unwrap_or(false)
        || doc
            .id
            .as_ref()
            .map(|id| id.to_lowercase().contains(filter_lower))
            .unwrap_or(false)
}

/// Keep only documents whose title or id contains `filter` (case-insensitive).
/// No filter keeps everything.
fn filter_by_meeting(results: Vec<Document>, meeting_filter: Option<&str>) -> Vec<Document> {
    let Some(filter) = meeting_filter else {
        return results;
    };
    let filter_lower = filter.to_lowercase();
    results.into_iter().filter(|doc| matches_meeting_filter(doc, &filter_lower)).collect()
}

/// Print a ranked meeting list, honoring the output mode and result limit.
fn render_meeting_list(results: Vec<Document>, query: &str, limit: usize, ctx: &RunContext) {
    let total = results.len();
    let results = apply_limit(results, limit);

    let refs: Vec<_> = results.iter().collect();

    match ctx.output_mode {
        OutputMode::Json => {
            println!("{}", crate::output::json::format_meetings(&refs));
        }
        OutputMode::Tty => {
            if results.is_empty() {
                println!("No meetings found matching \"{}\".", query);
                return;
            }
            if total > results.len() {
                println!(
                    "Found {} meeting(s) matching \"{}\" (showing {}):\n",
                    total,
                    query,
                    results.len()
                );
            } else {
                println!("Found {} meeting(s) matching \"{}\":\n", results.len(), query);
            }
            for doc in &results {
                println!("{}", crate::output::table::format_meeting_row(doc, &ctx.tz));
            }
            if total > results.len() {
                println!("\nUse --limit 0 to show all {} results.", total);
            }
        }
    }
}

fn context_window_search(
    conn: &Connection,
    query: &str,
    targets: &[SearchTarget],
    meeting_filter: Option<&str>,
    context_size: usize,
    limit: usize,
    date_range: Option<DateRange>,
    speaker_filter: Option<&SpeakerFilter>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    let search_transcripts = targets.contains(&SearchTarget::Transcripts);
    let search_panels = targets.contains(&SearchTarget::Panels);
    let search_notes = targets.contains(&SearchTarget::Notes);
    let titles_only =
        targets.contains(&SearchTarget::Titles) && !search_transcripts && !search_panels && !search_notes;

    if titles_only {
        if ctx.output_mode == OutputMode::Tty {
            eprintln!("Context search does not support titles. Try: --in transcripts,panels,notes");
        }
        return Ok(());
    }

    // Collect transcript context windows
    let mut tty_blocks: Vec<(String, String)> = Vec::new(); // (title, formatted_body)
    let mut json_windows: Vec<crate::output::json::ContextWindowJson> = Vec::new();
    let mut text_json_windows: Vec<crate::output::json::TextContextWindowJson> = Vec::new();

    if search_transcripts {
        let results = crate::db::transcripts::search_transcripts(
            conn,
            query,
            meeting_filter,
            context_size,
            date_range.as_ref(),
            include_deleted,
        )?;
        for (doc_title, windows) in &results {
            for w in windows {
                // Apply speaker filter: skip windows where the matched utterance doesn't pass
                if let Some(filter) = speaker_filter {
                    if !filter.matches(w.matched.source.as_deref()) {
                        continue;
                    }
                }
                match ctx.output_mode {
                    OutputMode::Json => {
                        json_windows.push(crate::output::json::ContextWindowJson::from_window(
                            w, doc_title,
                        ));
                    }
                    OutputMode::Tty => {
                        tty_blocks.push((
                            doc_title.clone(),
                            crate::output::table::format_context_window(w, None, &ctx.tz),
                        ));
                    }
                }
            }
        }
    }

    // Collect panel context windows
    if search_panels {
        let results = crate::db::panels::search_panels_with_context(
            conn,
            query,
            meeting_filter,
            context_size,
            date_range.as_ref(),
            include_deleted,
        )?;
        for (doc_id, doc_title, windows) in &results {
            for w in windows {
                match ctx.output_mode {
                    OutputMode::Json => {
                        text_json_windows.push(
                            crate::output::json::TextContextWindowJson::from_window(
                                w, doc_id, doc_title, "panel",
                            ),
                        );
                    }
                    OutputMode::Tty => {
                        tty_blocks.push((
                            doc_title.clone(),
                            crate::output::table::format_text_context_window(w, None),
                        ));
                    }
                }
            }
        }
    }

    // Collect notes context windows
    if search_notes {
        let results = crate::db::notes::search_notes_with_context(
            conn,
            query,
            meeting_filter,
            context_size,
            date_range.as_ref(),
            include_deleted,
        )?;
        for (doc_id, doc_title, windows) in &results {
            for w in windows {
                match ctx.output_mode {
                    OutputMode::Json => {
                        text_json_windows.push(
                            crate::output::json::TextContextWindowJson::from_window(
                                w, doc_id, doc_title, "notes",
                            ),
                        );
                    }
                    OutputMode::Tty => {
                        tty_blocks.push((
                            doc_title.clone(),
                            crate::output::table::format_text_context_window(w, None),
                        ));
                    }
                }
            }
        }
    }

    match ctx.output_mode {
        OutputMode::Json => {
            let (json_windows, text_json_windows) =
                apply_limit_mixed(json_windows, text_json_windows, limit);
            println!(
                "{}",
                crate::output::json::format_mixed_context_windows(&json_windows, &text_json_windows)
            );
        }
        OutputMode::Tty => {
            use colored::Colorize;

            if tty_blocks.is_empty() {
                println!("No context matches found for \"{}\".", query);
                return Ok(());
            }
            let total = tty_blocks.len();
            let tty_blocks = apply_limit(tty_blocks, limit);
            let shown = tty_blocks.len();
            if total > shown {
                println!(
                    "Found {} match(es) for \"{}\" (showing {}):\n",
                    total.to_string().cyan().bold(),
                    query,
                    shown.to_string().cyan().bold()
                );
            } else {
                println!(
                    "Found {} match(es) for \"{}\":\n",
                    shown.to_string().cyan().bold(),
                    query
                );
            }
            for (i, (title, body)) in tty_blocks.iter().enumerate() {
                println!(
                    "{}",
                    crate::output::table::format_search_separator(i + 1, shown, title, None)
                );
                println!("{}", body);
                println!();
            }
            if total > shown {
                println!("Use --limit 0 to show all {} results.", total);
            }
        }
    }

    Ok(())
}

/// Check embedding status and, on a TTY, prompt before a large or full
/// re-embedding run. Returns false when the user declines (the search
/// should stop). `yes` skips the prompt.
fn confirm_embedding_work(conn: &Connection, yes: bool, ctx: &RunContext) -> Result<bool> {
    // Resolve via the model constant: this path must not load the ONNX
    // model just to count pending chunks.
    let spec = crate::embed::config::EmbedSpec::resolve_stored(conn, crate::embed::MODEL_MAX_TOKENS);
    let status = crate::embed::get_embedding_status(conn, crate::embed::model::MODEL_NAME, &spec)?;
    let needs_full_reembed = status.orphaned_chunks > 0
        || status.legacy_max_length_warning
        || status.model_changed_warning;

    if (status.pending_chunks > EMBED_WARN_THRESHOLD || needs_full_reembed) && !yes {
        if ctx.output_mode == OutputMode::Tty {
            if needs_full_reembed {
                eprintln!("\nWarning: Embeddings need to be rebuilt.");
                if status.orphaned_chunks > 0 {
                    eprintln!(
                        "  - {} existing chunks are orphaned and will be deleted",
                        status.orphaned_chunks
                    );
                }
                if status.legacy_max_length_warning {
                    eprintln!("  - Existing embeddings use an outdated chunking strategy");
                }
                if status.model_changed_warning {
                    eprintln!("  - Existing embeddings were created by a different embedding model");
                }
                eprintln!(
                    "  - {} new chunks need embedding",
                    status.pending_chunks
                );
                eprintln!();
                eprintln!("Existing embeddings cannot be used. Run `grans embed` to rebuild.");
            } else {
                eprintln!(
                    "\nWarning: {} chunks need embedding.",
                    status.pending_chunks
                );
                eprintln!("This may take a while. Run `grans embed` separately to control when this happens.");
            }
            eprint!("Proceed anyway? [y/N] ");
            io::stderr().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Search cancelled. Run `grans embed` to build embeddings.");
                return Ok(false);
            }
        }
    }

    Ok(true)
}

fn semantic_search(
    conn: &Connection,
    query: &str,
    targets: &[SearchTarget],
    context_size: usize,
    date_range: Option<DateRange>,
    speaker_filter: Option<&SpeakerFilter>,
    yes: bool,
    limit: usize,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    use crate::query::filter::semantic_source_filter;

    // Parse --in targets and compute source type filter for semantic search
    let filter = semantic_source_filter(targets);

    // If only non-embeddable targets selected (e.g. --in titles), inform user
    if let Some(ref f) = filter {
        if f.is_empty() {
            if ctx.output_mode == OutputMode::Tty {
                eprintln!("Semantic search does not support searching in titles only.");
                eprintln!("Try: grans search \"{}\" --semantic --in transcripts,panels,notes", query);
            }
            return Ok(());
        }
    }

    if !confirm_embedding_work(conn, yes, ctx)? {
        return Ok(());
    }

    let filter_strs = filter.as_deref();
    let (results, total_count) =
        crate::embed::semantic_search(conn, query, date_range.as_ref(), limit, filter_strs, include_deleted)?;

    if results.is_empty() {
        if ctx.output_mode == OutputMode::Tty {
            println!("No semantic matches found for \"{}\".", query);
        }
        return Ok(());
    }

    // If context is requested, build context windows for each result
    if context_size > 0 {
        return display_semantic_results_with_context(
            conn,
            &results,
            total_count,
            context_size,
            query,
            limit,
            speaker_filter,
            ctx,
        );
    }

    // Original behavior: display without context
    match ctx.output_mode {
        OutputMode::Json => {
            println!(
                "{}",
                crate::output::json::format_semantic_results(&results, query, total_count, limit, conn)
            );
        }
        OutputMode::Tty => {
            if total_count > results.len() {
                println!(
                    "Found {} semantic match(es) for \"{}\" (showing top {}):\n",
                    total_count,
                    query,
                    results.len()
                );
            } else {
                println!(
                    "Found {} semantic match(es) for \"{}\":\n",
                    results.len(),
                    query
                );
            }
            for result in &results {
                println!(
                    "{}",
                    crate::output::table::format_semantic_result(result, conn, &ctx.tz)
                );
            }
            if total_count > results.len() {
                println!("\nUse --limit 0 to show all {} results.", total_count);
            }
        }
    }

    Ok(())
}

fn display_semantic_results_with_context(
    conn: &Connection,
    results: &[crate::embed::search::SemanticSearchResult],
    total_count: usize,
    context_size: usize,
    query: &str,
    limit: usize,
    speaker_filter: Option<&SpeakerFilter>,
    ctx: &RunContext,
) -> Result<()> {
    use crate::db::transcripts::{build_context_window_from_indices, load_transcript};
    use crate::query::search::ContextWindow;

    // Build context windows for each result
    let mut results_with_context: Vec<(crate::embed::search::SemanticSearchResult, ContextWindow)> =
        Vec::new();

    for result in results {
        // Skip if no window indices
        let (start_idx, end_idx) = match (result.window_start_idx, result.window_end_idx) {
            (Some(s), Some(e)) => (s, e),
            _ => continue,
        };

        // Load the transcript for this document
        let utterances = load_transcript(conn, &result.document_id)?;
        if utterances.is_empty() {
            continue;
        }

        // Build context window
        if let Some(window) =
            build_context_window_from_indices(&utterances, start_idx, end_idx, context_size)
        {
            // Apply speaker filter to matched utterance
            if let Some(filter) = speaker_filter {
                if !filter.matches(window.matched.source.as_deref()) {
                    continue;
                }
            }
            results_with_context.push((result.clone(), window));
        }
    }

    if results_with_context.is_empty() {
        if ctx.output_mode == OutputMode::Tty {
            println!("No semantic matches with context found for \"{}\".", query);
        }
        return Ok(());
    }

    match ctx.output_mode {
        OutputMode::Json => {
            println!(
                "{}",
                crate::output::json::format_semantic_results_with_context(
                    &results_with_context,
                    query,
                    total_count,
                    limit,
                    conn
                )
            );
        }
        OutputMode::Tty => {
            use colored::Colorize;

            let shown = results_with_context.len();
            if total_count > shown {
                println!(
                    "Found {} semantic match(es) for \"{}\" (showing top {}):\n",
                    total_count.to_string().cyan().bold(),
                    query,
                    shown.to_string().cyan().bold()
                );
            } else {
                println!(
                    "Found {} semantic match(es) for \"{}\":\n",
                    shown.to_string().cyan().bold(),
                    query
                );
            }
            for (i, (result, window)) in results_with_context.iter().enumerate() {
                let (title, _) = lookup_document_meta(conn, &result.document_id);
                println!(
                    "{}",
                    crate::output::table::format_search_separator(
                        i + 1,
                        shown,
                        &title,
                        Some(result.score)
                    )
                );
                println!(
                    "{}",
                    crate::output::table::format_context_window_with_score(
                        window,
                        None,
                        result.score,
                        &ctx.tz,
                    )
                );
                println!();
            }
            if total_count > shown {
                println!("Use --limit 0 to show all {} results.", total_count);
            }
        }
    }

    Ok(())
}

fn lookup_document_meta(conn: &Connection, doc_id: &str) -> (String, String) {
    let result: Option<(String, String)> = conn
        .query_row(
            "SELECT COALESCE(title, '(untitled)'), COALESCE(created_at, '') FROM documents WHERE id = ?1",
            [doc_id],
            |row| {
                let title: String = row.get(0)?;
                let date: String = row.get(1)?;
                Ok((title, date))
            },
        )
        .ok();

    result.unwrap_or_else(|| ("(unknown)".to_string(), String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_cli_args_defaults_to_hybrid() {
        let mode = SearchMode::from_cli_args(
            false, None, false, false, 0, "titles,notes", None, None, false, 10, 1,
        );
        match mode {
            SearchMode::Hybrid {
                targets,
                meeting_filter,
                rerank,
                min_score,
                yes,
                limit,
                matches,
            } => {
                assert_eq!(targets.len(), 2);
                assert!(targets.contains(&SearchTarget::Titles));
                assert!(targets.contains(&SearchTarget::Notes));
                assert!(meeting_filter.is_none());
                assert!(rerank);
                assert_eq!(min_score, None);
                assert!(!yes);
                assert_eq!(limit, 10);
                assert_eq!(matches, 1);
            }
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn shape_page_preserves_ranking_order_and_scores() {
        // Shaping is presentation-only: the cards must come back in exactly
        // the order the ranked page was handed in, whatever the db returns.
        use crate::db::test_fixtures::build_test_db;
        use serde_json::json;

        let conn = build_test_db(&json!({
            "documents": {
                "doc-a": {"id": "doc-a", "title": "Alpha", "created_at": "2026-01-01T10:00:00Z"},
                "doc-b": {"id": "doc-b", "title": "Beta", "created_at": "2026-01-02T10:00:00Z"},
                "doc-c": {"id": "doc-c", "title": "Gamma", "created_at": "2026-01-03T10:00:00Z"}
            }
        }));
        let docs = crate::db::meetings::get_meetings_by_ids(
            &conn,
            &["doc-c".to_string(), "doc-a".to_string(), "doc-b".to_string()],
        )
        .unwrap();
        let mut by_id: std::collections::HashMap<String, Document> =
            docs.into_iter().map(|d| (d.id.clone().unwrap(), d)).collect();
        let page: Vec<(Document, Option<f32>)> = vec![
            (by_id.remove("doc-c").unwrap(), Some(0.5)),
            (by_id.remove("doc-a").unwrap(), Some(0.9)),
            (by_id.remove("doc-b").unwrap(), None),
        ];
        let ranking = crate::query::hybrid::HybridRanking {
            fused: Vec::new(),
            best_chunks: std::collections::HashMap::new(),
            keyword_ids: std::collections::HashSet::new(),
        };

        let shaped = shape_page(
            &conn,
            &page,
            &ranking,
            &crate::query::fts::parse_query("alpha"),
            &crate::query::evidence::EvidenceLimits::default(),
        )
        .unwrap();

        let ids: Vec<&str> = shaped.iter().map(|m| m.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-c", "doc-a", "doc-b"]);
        assert_eq!(shaped[0].score, Some(0.5));
        assert_eq!(shaped[1].score, Some(0.9));
        assert_eq!(shaped[2].score, None);
    }

    #[test]
    fn from_cli_args_matches_threads_to_hybrid() {
        let mode = SearchMode::from_cli_args(
            false, None, false, false, 0, "titles", None, None, false, 10, 3,
        );
        match mode {
            SearchMode::Hybrid { matches, .. } => assert_eq!(matches, 3),
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_keyword_flag_forces_keyword() {
        let mode = SearchMode::from_cli_args(
            false, None, true, false, 0, "titles,notes", Some("standup"), None, false, 10, 1,
        );
        match mode {
            SearchMode::Keyword {
                targets,
                meeting_filter,
                limit,
            } => {
                assert_eq!(targets.len(), 2);
                assert!(targets.contains(&SearchTarget::Titles));
                assert!(targets.contains(&SearchTarget::Notes));
                assert_eq!(meeting_filter.as_deref(), Some("standup"));
                assert_eq!(limit, 10);
            }
            _ => panic!("Expected Keyword variant"),
        }
    }

    #[test]
    fn from_cli_args_fast_skips_rerank() {
        let mode = SearchMode::from_cli_args(
            true, None, false, false, 0, "titles", None, None, false, 10, 1,
        );
        match mode {
            SearchMode::Hybrid { rerank, .. } => assert!(!rerank),
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_min_score_threads_to_hybrid() {
        let mode = SearchMode::from_cli_args(
            false, Some(0.4), false, false, 0, "titles", None, None, false, 10, 1,
        );
        match mode {
            SearchMode::Hybrid { min_score, .. } => assert_eq!(min_score, Some(0.4)),
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_semantic_takes_priority() {
        let mode = SearchMode::from_cli_args(false, None, false, true, 5, "titles", Some("standup"), None, true, 10, 1);
        match mode {
            SearchMode::Semantic {
                targets,
                context_size,
                yes,
                limit,
                ..
            } => {
                assert_eq!(targets, vec![SearchTarget::Titles]);
                assert_eq!(context_size, 5);
                assert!(yes);
                assert_eq!(limit, 10);
            }
            _ => panic!("Expected Semantic variant"),
        }
    }

    #[test]
    fn from_cli_args_context_forces_context_window() {
        let mode = SearchMode::from_cli_args(false, None, false, false, 3, "titles,notes", Some("standup"), None, false, 10, 1);
        match mode {
            SearchMode::ContextWindow {
                targets,
                context_size,
                meeting_filter,
                limit,
                ..
            } => {
                assert_eq!(context_size, 3);
                assert_eq!(meeting_filter.as_deref(), Some("standup"));
                assert_eq!(targets.len(), 2);
                assert!(targets.contains(&SearchTarget::Titles));
                assert!(targets.contains(&SearchTarget::Notes));
                assert_eq!(limit, 10);
            }
            _ => panic!("Expected ContextWindow variant"),
        }
    }

    #[test]
    fn from_cli_args_speaker_routes_bare_search_to_keyword() {
        let speaker = SpeakerFilter::Me;
        let mode = SearchMode::from_cli_args(
            false, None, false, false, 0, "transcripts", None, Some(&speaker), false, 10, 1,
        );
        match mode {
            SearchMode::Keyword { .. } => {}
            _ => panic!("Expected Keyword variant"),
        }
    }

    #[test]
    fn from_cli_args_meeting_filter_threads_to_hybrid() {
        let mode =
            SearchMode::from_cli_args(false, None, false, false, 0, "transcripts", Some("daily"), None, false, 10, 1);
        match mode {
            SearchMode::Hybrid {
                meeting_filter, ..
            } => {
                assert_eq!(meeting_filter.as_deref(), Some("daily"));
            }
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_meeting_filter_threads_to_keyword() {
        let mode =
            SearchMode::from_cli_args(false, None, true, false, 0, "transcripts", Some("daily"), None, false, 10, 1);
        match mode {
            SearchMode::Keyword {
                meeting_filter, ..
            } => {
                assert_eq!(meeting_filter.as_deref(), Some("daily"));
            }
            _ => panic!("Expected Keyword variant"),
        }
    }

    #[test]
    fn from_cli_args_meeting_filter_threads_to_context_window() {
        let mode =
            SearchMode::from_cli_args(false, None, false, false, 5, "titles", Some("retro"), None, false, 10, 1);
        match mode {
            SearchMode::ContextWindow {
                meeting_filter, ..
            } => {
                assert_eq!(meeting_filter.as_deref(), Some("retro"));
            }
            _ => panic!("Expected ContextWindow variant"),
        }
    }

    #[test]
    fn from_cli_args_speaker_filter_threads_to_context_window() {
        let speaker = SpeakerFilter::Me;
        let mode = SearchMode::from_cli_args(false, None, false, false, 3, "transcripts", None, Some(&speaker), false, 10, 1);
        match mode {
            SearchMode::ContextWindow {
                speaker_filter, ..
            } => {
                assert_eq!(speaker_filter, Some(SpeakerFilter::Me));
            }
            _ => panic!("Expected ContextWindow variant"),
        }
    }

    #[test]
    fn from_cli_args_speaker_filter_threads_to_semantic() {
        let speaker = SpeakerFilter::Other;
        let mode = SearchMode::from_cli_args(false, None, false, true, 0, "transcripts", None, Some(&speaker), false, 10, 1);
        match mode {
            SearchMode::Semantic {
                speaker_filter, ..
            } => {
                assert_eq!(speaker_filter, Some(SpeakerFilter::Other));
            }
            _ => panic!("Expected Semantic variant"),
        }
    }

    #[test]
    fn apply_limit_zero_means_no_limit() {
        let items = vec![1, 2, 3, 4, 5];
        assert_eq!(apply_limit(items, 0), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn apply_limit_truncates() {
        let items = vec![1, 2, 3, 4, 5];
        assert_eq!(apply_limit(items, 3), vec![1, 2, 3]);
    }

    #[test]
    fn apply_limit_noop_when_under() {
        let items = vec![1, 2, 3];
        assert_eq!(apply_limit(items, 10), vec![1, 2, 3]);
    }

    #[test]
    fn apply_limit_mixed_zero_means_no_limit() {
        let a = vec![1, 2, 3];
        let b = vec![4, 5, 6];
        let (ra, rb) = apply_limit_mixed(a, b, 0);
        assert_eq!(ra, vec![1, 2, 3]);
        assert_eq!(rb, vec![4, 5, 6]);
    }

    #[test]
    fn apply_limit_mixed_truncates_second_first() {
        let a = vec![1, 2, 3];
        let b = vec![4, 5, 6];
        let (ra, rb) = apply_limit_mixed(a, b, 4);
        assert_eq!(ra, vec![1, 2, 3]);
        assert_eq!(rb, vec![4]);
    }

    #[test]
    fn apply_limit_mixed_truncates_both() {
        let a = vec![1, 2, 3];
        let b = vec![4, 5, 6];
        let (ra, rb) = apply_limit_mixed(a, b, 2);
        assert_eq!(ra, vec![1, 2]);
        assert!(rb.is_empty());
    }

    #[test]
    fn apply_limit_mixed_noop_when_under() {
        let a = vec![1, 2];
        let b = vec![3];
        let (ra, rb) = apply_limit_mixed(a, b, 10);
        assert_eq!(ra, vec![1, 2]);
        assert_eq!(rb, vec![3]);
    }
}
