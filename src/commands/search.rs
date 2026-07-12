//! `grans search`: ranked discovery.
//!
//! Hybrid retrieval (FTS and semantic rankings fused with RRF) plus a
//! cross-encoder rerank (skipped by `--fast`). Results are the best few
//! meetings for the query, cut from bounded candidate pools, so the output
//! never claims a corpus total; it cross-links `grans grep` with the
//! uncapped FTS match count instead.

use std::io::{self, Write};

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::commands::search_common::{print_shaped_cards, shape_and_page};
use crate::models::Document;
use crate::output::format::OutputMode;
use crate::query::dates::DateRange;
use crate::query::filter::SearchTarget;

/// Threshold for prompting before embedding during a search.
const EMBED_WARN_THRESHOLD: usize = 200;

/// Options for a ranked search.
pub struct SearchOptions {
    pub targets: Vec<SearchTarget>,
    pub meeting_filter: Option<String>,
    pub rerank: bool,
    pub min_score: Option<f32>,
    pub yes: bool,
    pub limit: usize,
    /// Match snippets shown per meeting card.
    pub matches: usize,
    /// Neighboring units rendered around each shown match.
    pub context: usize,
}

impl SearchOptions {
    /// Construct SearchOptions from CLI arguments. A bare search reranks;
    /// --fast keeps fusion order.
    #[allow(clippy::too_many_arguments)]
    pub fn from_cli_args(
        fast: bool,
        min_score: Option<f32>,
        context: usize,
        in_targets: &str,
        meeting_filter: Option<&str>,
        yes: bool,
        limit: usize,
        matches: usize,
    ) -> Self {
        SearchOptions {
            targets: SearchTarget::parse_list(in_targets),
            meeting_filter: meeting_filter.map(String::from),
            rerank: !fast,
            min_score,
            yes,
            limit,
            matches,
            context,
        }
    }
}

/// Run keyword and semantic retrieval, fuse the rankings, rerank the top
/// candidates (unless skipped via --fast), and display the resulting
/// meetings as shaped cards with match evidence.
pub fn search(
    conn: &Connection,
    query: &str,
    opts: SearchOptions,
    date_range: Option<DateRange>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    if !confirm_embedding_work(conn, opts.yes, ctx)? {
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
        &opts.targets,
        opts.meeting_filter.as_deref(),
        date_range.as_ref(),
        include_deleted,
    )?;

    // Ranked (id, score) pairs; ordering is the pipeline's and is not
    // touched again below.
    let ordered: Vec<(String, Option<f32>)> = if opts.rerank {
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
        if let Some(min) = opts.min_score {
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

    let tokens = crate::query::fts::parse_query(query);
    let evidence_opts = crate::query::evidence::EvidenceOptions {
        max_matches: opts.matches,
        context: opts.context,
        ..Default::default()
    };
    let (shaped, _) = shape_and_page(
        conn,
        ordered_docs,
        |doc, score| {
            let doc_id = doc.id.as_deref().unwrap_or_default();
            crate::query::evidence::RankingFacts {
                keyword: ranking.keyword_ids.contains(doc_id),
                best_chunk: ranking.best_chunks.get(doc_id),
                score,
            }
        },
        &tokens,
        &evidence_opts,
        opts.limit,
    )?;

    render_ranked_meeting_list(&shaped, query, ranking.keyword_total, opts.limit, ctx);
    Ok(())
}

/// Header for ranked results: claims only what is shown, never a total.
fn ranked_header(shown: usize, query: &str) -> String {
    format!("Top {} match(es) for \"{}\":", shown, query)
}

/// Cross-link to the complete lookup, backed by the uncapped FTS count.
fn grep_cross_link(keyword_total: usize, query: &str) -> String {
    format!(
        "{} meeting(s) contain these words; grans grep \"{}\" lists them all.",
        keyword_total, query
    )
}

/// Print ranked results, honoring the output mode. The list is a pooled
/// best-k, so no corpus total is claimed; when the query's words appear
/// anywhere, the output points at `grans grep` for the complete list.
fn render_ranked_meeting_list(
    shaped: &[crate::query::shape::ShapedMeeting],
    query: &str,
    keyword_total: usize,
    limit: usize,
    ctx: &RunContext,
) {
    match ctx.output_mode {
        OutputMode::Json => {
            println!(
                "{}",
                crate::output::json::format_search_meetings(shaped, query, keyword_total, limit)
            );
        }
        OutputMode::Tty => {
            if shaped.is_empty() {
                println!("No matches for \"{}\".", query);
            } else {
                println!("{}\n", ranked_header(shaped.len(), query));
                print_shaped_cards(shaped, ctx);
            }
            if keyword_total > 0 {
                println!("{}", grep_cross_link(keyword_total, query));
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_cli_args_defaults_rerank_on() {
        let opts = SearchOptions::from_cli_args(
            false, None, 0, "titles,notes", None, false, 10, 1,
        );
        assert_eq!(opts.targets.len(), 2);
        assert!(opts.targets.contains(&SearchTarget::Titles));
        assert!(opts.targets.contains(&SearchTarget::Notes));
        assert!(opts.meeting_filter.is_none());
        assert!(opts.rerank);
        assert_eq!(opts.min_score, None);
        assert!(!opts.yes);
        assert_eq!(opts.limit, 10);
        assert_eq!(opts.matches, 1);
        assert_eq!(opts.context, 0);
    }

    #[test]
    fn from_cli_args_fast_skips_rerank() {
        let opts = SearchOptions::from_cli_args(true, None, 0, "titles", None, false, 10, 1);
        assert!(!opts.rerank);
    }

    #[test]
    fn from_cli_args_min_score_threads() {
        let opts =
            SearchOptions::from_cli_args(false, Some(0.4), 0, "titles", None, false, 10, 1);
        assert_eq!(opts.min_score, Some(0.4));
    }

    #[test]
    fn from_cli_args_meeting_filter_threads() {
        let opts = SearchOptions::from_cli_args(
            false, None, 0, "transcripts", Some("daily"), false, 10, 1,
        );
        assert_eq!(opts.meeting_filter.as_deref(), Some("daily"));
    }

    #[test]
    fn from_cli_args_matches_and_context_thread() {
        let opts = SearchOptions::from_cli_args(false, None, 3, "titles", None, false, 10, 4);
        assert_eq!(opts.context, 3);
        assert_eq!(opts.matches, 4);
    }

    #[test]
    fn ranked_header_claims_only_the_shown_count() {
        assert_eq!(ranked_header(7, "budget"), "Top 7 match(es) for \"budget\":");
    }

    #[test]
    fn grep_cross_link_names_the_complete_verb() {
        assert_eq!(
            grep_cross_link(312, "budget"),
            "312 meeting(s) contain these words; grans grep \"budget\" lists them all."
        );
    }
}
