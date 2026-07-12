use std::io::{self, Write};

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::commands::search_common::{fts_meetings, render_shaped_meeting_list, shape_and_page};
use crate::models::{Document, SpeakerFilter};
use crate::output::format::OutputMode;
use crate::query::dates::DateRange;
use crate::query::filter::{filter_by_meeting, matches_meeting_filter, SearchTarget};

/// Threshold for prompting before embedding during hybrid search.
const EMBED_WARN_THRESHOLD: usize = 200;

/// Typed search mode, replacing the old 11-parameter search function.
/// Each variant carries only the parameters it actually uses.
pub enum SearchMode {
    Keyword {
        targets: Vec<SearchTarget>,
        meeting_filter: Option<String>,
        limit: usize,
        /// Match snippets shown per meeting card.
        matches: usize,
        /// Only meetings with matching utterances by this speaker survive.
        speaker: Option<SpeakerFilter>,
        /// Neighboring units rendered around each shown match.
        context: usize,
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
        /// Only meetings with matching utterances by this speaker survive.
        speaker: Option<SpeakerFilter>,
        /// Neighboring units rendered around each shown match.
        context: usize,
    },
}

impl SearchMode {
    /// Construct a SearchMode from CLI arguments.
    ///
    /// A bare search runs hybrid and --keyword forces plain FTS; --speaker
    /// (an evidence filter) and --context (card expansion) compose with
    /// either retrieval mode. --hybrid needs no parameter here: it is the
    /// default, and clap conflicts keep it from combining with --keyword.
    #[allow(clippy::too_many_arguments)]
    pub fn from_cli_args(
        fast: bool,
        min_score: Option<f32>,
        keyword: bool,
        context: usize,
        in_targets: &str,
        meeting_filter: Option<&str>,
        speaker: Option<&SpeakerFilter>,
        yes: bool,
        limit: usize,
        matches: usize,
    ) -> Self {
        if keyword {
            SearchMode::Keyword {
                targets: SearchTarget::parse_list(in_targets),
                meeting_filter: meeting_filter.map(String::from),
                limit,
                matches,
                speaker: speaker.cloned(),
                context,
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
                speaker: speaker.cloned(),
                context,
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
        SearchMode::Keyword {
            targets,
            meeting_filter,
            limit,
            matches,
            speaker,
            context,
        } => keyword_search(
            conn,
            query,
            &targets,
            meeting_filter.as_deref(),
            limit,
            matches,
            speaker,
            context,
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
            speaker,
            context,
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
            speaker,
            context,
            date_range,
            include_deleted,
            ctx,
        ),
    }
}

/// Run plain FTS retrieval and display the resulting meetings as shaped
/// cards. Keyword results carry no rerank score and no semantic chunk;
/// content matches show lexical evidence and title-only matches show a
/// bare title card.
#[allow(clippy::too_many_arguments)]
fn keyword_search(
    conn: &Connection,
    query: &str,
    targets: &[SearchTarget],
    meeting_filter: Option<&str>,
    limit: usize,
    matches: usize,
    speaker: Option<SpeakerFilter>,
    context: usize,
    date_range: Option<DateRange>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    let results = fts_meetings(conn, query, targets, date_range.as_ref(), include_deleted)?;

    let results = filter_by_meeting(results, meeting_filter);
    let docs: Vec<(Document, Option<f32>)> =
        results.into_iter().map(|doc| (doc, None)).collect();

    let tokens = crate::query::fts::parse_query(query);
    let opts = crate::query::evidence::EvidenceOptions {
        max_matches: matches,
        speaker,
        context,
        ..Default::default()
    };
    let (shaped, total) = shape_and_page(
        conn,
        docs,
        |_, _| crate::query::evidence::RankingFacts {
            keyword: true,
            best_chunk: None,
            score: None,
        },
        &tokens,
        &opts,
        limit,
    )?;

    render_shaped_meeting_list(&shaped, query, total, limit, ctx);
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
    speaker: Option<SpeakerFilter>,
    context: usize,
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

    let tokens = crate::query::fts::parse_query(query);
    let opts = crate::query::evidence::EvidenceOptions {
        max_matches: matches,
        speaker,
        context,
        ..Default::default()
    };
    let (shaped, total) = shape_and_page(
        conn,
        filtered,
        |doc, score| {
            let doc_id = doc.id.as_deref().unwrap_or_default();
            crate::query::evidence::RankingFacts {
                keyword: ranking.keyword_ids.contains(doc_id),
                best_chunk: ranking.best_chunks.get(doc_id),
                score,
            }
        },
        &tokens,
        &opts,
        limit,
    )?;

    render_shaped_meeting_list(&shaped, query, total, limit, ctx);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_cli_args_defaults_to_hybrid() {
        let mode = SearchMode::from_cli_args(
            false, None, false, 0, "titles,notes", None, None, false, 10, 1,
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
                speaker,
                context,
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
                assert_eq!(speaker, None);
                assert_eq!(context, 0);
            }
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_matches_threads_to_hybrid() {
        let mode = SearchMode::from_cli_args(
            false, None, false, 0, "titles", None, None, false, 10, 3,
        );
        match mode {
            SearchMode::Hybrid { matches, .. } => assert_eq!(matches, 3),
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_keyword_flag_forces_keyword() {
        let mode = SearchMode::from_cli_args(
            false, None, true, 0, "titles,notes", Some("standup"), None, false, 10, 1,
        );
        match mode {
            SearchMode::Keyword {
                targets,
                meeting_filter,
                limit,
                matches,
                speaker,
                context,
            } => {
                assert_eq!(targets.len(), 2);
                assert!(targets.contains(&SearchTarget::Titles));
                assert!(targets.contains(&SearchTarget::Notes));
                assert_eq!(meeting_filter.as_deref(), Some("standup"));
                assert_eq!(limit, 10);
                assert_eq!(matches, 1);
                assert_eq!(speaker, None);
                assert_eq!(context, 0);
            }
            _ => panic!("Expected Keyword variant"),
        }
    }

    #[test]
    fn from_cli_args_context_threads_to_both_modes() {
        // --context is a presentation option, not a mode: it no longer
        // forces the keyword path.
        let mode = SearchMode::from_cli_args(
            false, None, false, 3, "titles", None, None, false, 10, 1,
        );
        match mode {
            SearchMode::Hybrid { context, .. } => assert_eq!(context, 3),
            _ => panic!("Expected Hybrid variant"),
        }
        let mode = SearchMode::from_cli_args(
            false, None, true, 2, "titles", None, None, false, 10, 1,
        );
        match mode {
            SearchMode::Keyword { context, .. } => assert_eq!(context, 2),
            _ => panic!("Expected Keyword variant"),
        }
    }

    #[test]
    fn from_cli_args_matches_threads_to_keyword() {
        let mode = SearchMode::from_cli_args(
            false, None, true, 0, "titles", None, None, false, 10, 4,
        );
        match mode {
            SearchMode::Keyword { matches, .. } => assert_eq!(matches, 4),
            _ => panic!("Expected Keyword variant"),
        }
    }

    #[test]
    fn from_cli_args_fast_skips_rerank() {
        let mode = SearchMode::from_cli_args(
            true, None, false, 0, "titles", None, None, false, 10, 1,
        );
        match mode {
            SearchMode::Hybrid { rerank, .. } => assert!(!rerank),
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_min_score_threads_to_hybrid() {
        let mode = SearchMode::from_cli_args(
            false, Some(0.4), false, 0, "titles", None, None, false, 10, 1,
        );
        match mode {
            SearchMode::Hybrid { min_score, .. } => assert_eq!(min_score, Some(0.4)),
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_speaker_stays_on_the_hybrid_default() {
        // #60: a bare --speaker used to silently route to the keyword path
        // (which then dropped the filter); it now composes with hybrid.
        let speaker = SpeakerFilter::Me;
        let mode = SearchMode::from_cli_args(
            false, None, false, 0, "transcripts", None, Some(&speaker), false, 10, 1,
        );
        match mode {
            SearchMode::Hybrid { speaker, .. } => assert_eq!(speaker, Some(SpeakerFilter::Me)),
            _ => panic!("Expected Hybrid variant"),
        }
    }

    #[test]
    fn from_cli_args_speaker_threads_to_keyword() {
        let speaker = SpeakerFilter::Other;
        let mode = SearchMode::from_cli_args(
            false, None, true, 0, "transcripts", None, Some(&speaker), false, 10, 1,
        );
        match mode {
            SearchMode::Keyword { speaker, .. } => {
                assert_eq!(speaker, Some(SpeakerFilter::Other));
            }
            _ => panic!("Expected Keyword variant"),
        }
    }

    #[test]
    fn from_cli_args_meeting_filter_threads_to_hybrid() {
        let mode =
            SearchMode::from_cli_args(false, None, false, 0, "transcripts", Some("daily"), None, false, 10, 1);
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
            SearchMode::from_cli_args(false, None, true, 0, "transcripts", Some("daily"), None, false, 10, 1);
        match mode {
            SearchMode::Keyword {
                meeting_filter, ..
            } => {
                assert_eq!(meeting_filter.as_deref(), Some("daily"));
            }
            _ => panic!("Expected Keyword variant"),
        }
    }
}
