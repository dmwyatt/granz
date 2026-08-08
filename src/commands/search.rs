//! `grans search`: ranked discovery.
//!
//! Hybrid retrieval (FTS and semantic rankings fused with RRF) plus a
//! cross-encoder rerank (skipped by `--fast`). Results are the best few
//! meetings for the query, cut from bounded candidate pools, so the output
//! never claims a corpus total; it cross-links `grans grep` with the
//! uncapped FTS match count instead.
//!
//! Search is read-only over the embedding store: it searches what is
//! embedded and warns about what is not. `grans embed` is the only
//! command that creates or repairs embeddings (#126).

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::commands::search_common::{print_shaped_cards, shape_and_page};
use crate::embed::freshness::IndexFreshness;
use crate::models::Document;
use crate::output::format::OutputMode;
use crate::query::dates::DateRange;
use crate::query::filter::{DEFAULT_SEARCH_TARGETS, SearchTarget, targets_to_flag_value};

/// Filter values that affect the match count, kept so the grep cross-link
/// can reproduce that count. Dates and the meeting filter are echoed as the
/// user typed them; `in_targets` is the parsed target list, re-joined into
/// the flag when the suggested command is built. Only filters that change
/// the match count belong here.
pub struct FilterEcho {
    /// Parsed `--in` targets.
    pub in_targets: Vec<SearchTarget>,
    pub meeting: Option<String>,
    pub date: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub include_deleted: bool,
}

impl Default for FilterEcho {
    fn default() -> Self {
        FilterEcho {
            in_targets: SearchTarget::all(),
            meeting: None,
            date: None,
            from: None,
            to: None,
            include_deleted: false,
        }
    }
}

/// Options for a ranked search.
pub struct SearchOptions {
    pub targets: Vec<SearchTarget>,
    pub meeting_filter: Option<String>,
    pub rerank: bool,
    pub min_score: Option<f32>,
    pub limit: usize,
    /// Match snippets shown per meeting card.
    pub matches: usize,
    /// Neighboring units rendered around each shown match.
    pub context: usize,
    /// Raw filter values, echoed into the grep cross-link.
    pub echo: FilterEcho,
}

impl SearchOptions {
    /// Construct SearchOptions from CLI arguments. A bare search reranks;
    /// --fast keeps fusion order. Targets and the meeting filter derive
    /// from the raw values in `echo`.
    pub fn from_cli_args(
        fast: bool,
        min_score: Option<f32>,
        context: usize,
        limit: usize,
        matches: usize,
        echo: FilterEcho,
    ) -> Self {
        SearchOptions {
            targets: echo.in_targets.clone(),
            meeting_filter: echo.meeting.clone(),
            rerank: !fast,
            min_score,
            limit,
            matches,
            context,
            echo,
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
    let embedder = crate::embed::model::FastEmbedModel::new()?;

    // The reranker's model load needs nothing retrieval produces, so start
    // it here and let embedding and retrieval run in front of it. Spawning
    // at the top of the command hides the load just as completely but races
    // the embedder for the one-time ONNX runtime init, slowing embedder
    // init by ~70ms (placement A/B in #82's PR). Only statement order and
    // this comment keep the spawn after the embedder; moving it up still
    // compiles and passes, it just pays that contention again.
    let pending_reranker = opts
        .rerank
        .then(|| {
            crate::embed::rerank::PendingReranker::spawn(crate::embed::rerank::DEFAULT_RERANK_MODEL)
        })
        .transpose()?;

    let (index, freshness) =
        crate::embed::freshness::load_search_index(conn, crate::embed::model::MODEL_NAME)?;
    if let Some(warning) = freshness_warning(&freshness) {
        eprintln!("[grans] {}", warning);
    }

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

    let reranker = pending_reranker
        .map(crate::embed::rerank::PendingReranker::join)
        .transpose()?;
    let reranker = reranker
        .as_ref()
        .map(|r| r as &dyn crate::embed::rerank::Reranker);
    // `ordered` is the pipeline's final order; nothing below re-sorts it.
    let ordered =
        crate::query::rerank::order_candidates(conn, query, &ranking, reranker, opts.min_score)?;

    let ids: Vec<String> = ordered.iter().map(|(id, _)| id.clone()).collect();
    let docs = crate::db::meetings::get_meetings_by_ids(conn, &ids)?;
    let mut doc_by_id: std::collections::HashMap<String, Document> = docs
        .into_iter()
        .filter_map(|d| d.id.clone().map(|id| (id, d)))
        .collect();
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

    render_ranked_meeting_list(&shaped, query, ranking.keyword_total, &opts, ctx);
    Ok(())
}

/// Header for ranked results: claims only what is shown, never a total.
fn ranked_header(shown: usize, query: &str) -> String {
    format!("Top {} match(es) for \"{}\":", shown, query)
}

/// The grep command that reproduces `keyword_total`: the query plus every
/// search filter that affects the match count, echoed as given. Flags that
/// only shape presentation or the ranked pipeline (--limit, --matches,
/// --context, --fast, --min-score) are omitted because they do not
/// change what counts as a match.
fn grep_command_echo(query: &str, filters: &FilterEcho) -> String {
    let mut cmd = format!("grans grep \"{}\"", query);
    let in_flag = targets_to_flag_value(&filters.in_targets);
    if in_flag != DEFAULT_SEARCH_TARGETS {
        cmd.push_str(&format!(" --in {}", in_flag));
    }
    if let Some(meeting) = &filters.meeting {
        cmd.push_str(&format!(" --meeting \"{}\"", meeting));
    }
    if let Some(date) = &filters.date {
        cmd.push_str(&format!(" --date {}", date));
    }
    if let Some(from) = &filters.from {
        cmd.push_str(&format!(" --from {}", from));
    }
    if let Some(to) = &filters.to {
        cmd.push_str(&format!(" --to {}", to));
    }
    if filters.include_deleted {
        cmd.push_str(" --include-deleted");
    }
    cmd
}

/// Footer cross-linking the complete lookup, backed by the uncapped FTS
/// count. None when no meeting contains the query's words, in which case
/// no footer is printed.
fn grep_cross_link(keyword_total: usize, query: &str, filters: &FilterEcho) -> Option<String> {
    if keyword_total == 0 {
        return None;
    }
    Some(format!(
        "{} meeting(s) contain these words; {} lists them all.",
        keyword_total,
        grep_command_echo(query, filters)
    ))
}

/// Print ranked results, honoring the output mode. The list is a pooled
/// best-k, so no corpus total is claimed; when the query's words appear
/// anywhere, the output points at `grans grep` for the complete list.
fn render_ranked_meeting_list(
    shaped: &[crate::query::shape::ShapedMeeting],
    query: &str,
    keyword_total: usize,
    opts: &SearchOptions,
    ctx: &RunContext,
) {
    match ctx.output_mode {
        OutputMode::Json => {
            println!(
                "{}",
                crate::output::json::format_search_meetings(
                    shaped,
                    query,
                    keyword_total,
                    opts.limit
                )
            );
        }
        OutputMode::Tty => {
            if shaped.is_empty() {
                println!("No matches for \"{}\".", query);
            } else {
                println!("{}\n", ranked_header(shaped.len(), query));
                print_shaped_cards(shaped, ctx);
            }
            if let Some(footer) = grep_cross_link(keyword_total, query, &opts.echo) {
                println!("{}", footer);
            }
        }
    }
}

/// The stderr warning for an index that cannot cover everything, or None
/// when it is fresh. Printed in every output mode; stderr keeps JSON
/// stdout clean.
fn freshness_warning(freshness: &IndexFreshness) -> Option<String> {
    match freshness {
        IndexFreshness::Fresh => None,
        IndexFreshness::Stale => Some(
            "Meeting data has synced since the last embed; recent content may be \
             missing from these results. Run `grans embed` to include it."
                .to_string(),
        ),
        IndexFreshness::Empty => Some(
            "No embeddings exist; showing keyword-only results. Run `grans embed` \
             to enable semantic search."
                .to_string(),
        ),
        IndexFreshness::ModelMismatch { stored_model } => Some(format!(
            "Existing embeddings were created by a different model ({}); showing \
             keyword-only results. Run `grans embed` to rebuild them.",
            stored_model
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_with(f: impl FnOnce(&mut FilterEcho)) -> FilterEcho {
        let mut echo = FilterEcho::default();
        f(&mut echo);
        echo
    }

    #[test]
    fn from_cli_args_defaults_rerank_on() {
        let opts = SearchOptions::from_cli_args(
            false,
            None,
            0,
            10,
            1,
            echo_with(|e| e.in_targets = vec![SearchTarget::Titles, SearchTarget::Notes]),
        );
        assert_eq!(opts.targets.len(), 2);
        assert!(opts.targets.contains(&SearchTarget::Titles));
        assert!(opts.targets.contains(&SearchTarget::Notes));
        assert!(opts.meeting_filter.is_none());
        assert!(opts.rerank);
        assert_eq!(opts.min_score, None);
        assert_eq!(opts.limit, 10);
        assert_eq!(opts.matches, 1);
        assert_eq!(opts.context, 0);
    }

    #[test]
    fn from_cli_args_fast_skips_rerank() {
        let opts = SearchOptions::from_cli_args(true, None, 0, 10, 1, FilterEcho::default());
        assert!(!opts.rerank);
    }

    #[test]
    fn from_cli_args_min_score_threads() {
        let opts = SearchOptions::from_cli_args(false, Some(0.4), 0, 10, 1, FilterEcho::default());
        assert_eq!(opts.min_score, Some(0.4));
    }

    #[test]
    fn from_cli_args_meeting_filter_threads() {
        let opts = SearchOptions::from_cli_args(
            false,
            None,
            0,
            10,
            1,
            echo_with(|e| e.meeting = Some("daily".to_string())),
        );
        assert_eq!(opts.meeting_filter.as_deref(), Some("daily"));
    }

    #[test]
    fn from_cli_args_matches_and_context_thread() {
        let opts = SearchOptions::from_cli_args(false, None, 3, 10, 4, FilterEcho::default());
        assert_eq!(opts.context, 3);
        assert_eq!(opts.matches, 4);
    }

    #[test]
    fn freshness_warning_silent_when_fresh() {
        assert_eq!(freshness_warning(&IndexFreshness::Fresh), None);
    }

    #[test]
    fn freshness_warning_stale_points_at_embed() {
        let warning = freshness_warning(&IndexFreshness::Stale).unwrap();
        assert!(warning.contains("synced since the last embed"));
        assert!(warning.contains("grans embed"));
    }

    #[test]
    fn freshness_warning_empty_says_keyword_only() {
        let warning = freshness_warning(&IndexFreshness::Empty).unwrap();
        assert!(warning.contains("keyword-only"));
        assert!(warning.contains("grans embed"));
    }

    #[test]
    fn freshness_warning_model_mismatch_names_the_model() {
        let warning = freshness_warning(&IndexFreshness::ModelMismatch {
            stored_model: "old-model".to_string(),
        })
        .unwrap();
        assert!(warning.contains("old-model"));
        assert!(warning.contains("grans embed"));
    }

    #[test]
    fn ranked_header_claims_only_the_shown_count() {
        assert_eq!(
            ranked_header(7, "budget"),
            "Top 7 match(es) for \"budget\":"
        );
    }

    #[test]
    fn grep_cross_link_suppressed_when_nothing_matches_the_words() {
        assert_eq!(grep_cross_link(0, "budget", &FilterEcho::default()), None);
    }

    #[test]
    fn grep_cross_link_bare_default_echoes_plain_grep() {
        assert_eq!(
            grep_cross_link(312, "budget", &FilterEcho::default()).as_deref(),
            Some("312 meeting(s) contain these words; grans grep \"budget\" lists them all.")
        );
    }

    #[test]
    fn grep_command_echo_omits_the_default_target_list() {
        assert_eq!(
            grep_command_echo("budget", &FilterEcho::default()),
            "grans grep \"budget\""
        );
    }

    #[test]
    fn grep_command_echo_echoes_a_non_default_in_list() {
        let echo = echo_with(|e| e.in_targets = vec![SearchTarget::Titles, SearchTarget::Notes]);
        assert_eq!(
            grep_command_echo("budget", &echo),
            "grans grep \"budget\" --in titles,notes"
        );
    }

    #[test]
    fn grep_command_echo_quotes_the_meeting_filter() {
        let echo = echo_with(|e| e.meeting = Some("Weekly Standup".to_string()));
        assert_eq!(
            grep_command_echo("budget", &echo),
            "grans grep \"budget\" --meeting \"Weekly Standup\""
        );
    }

    #[test]
    fn grep_command_echo_echoes_the_raw_date_flag() {
        let echo = echo_with(|e| e.date = Some("last-week".to_string()));
        assert_eq!(
            grep_command_echo("budget", &echo),
            "grans grep \"budget\" --date last-week"
        );
    }

    #[test]
    fn grep_command_echo_echoes_raw_from_and_to() {
        let echo = echo_with(|e| {
            e.from = Some("2026-01-01".to_string());
            e.to = Some("3d".to_string());
        });
        assert_eq!(
            grep_command_echo("budget", &echo),
            "grans grep \"budget\" --from 2026-01-01 --to 3d"
        );
    }

    #[test]
    fn grep_command_echo_echoes_include_deleted() {
        let echo = echo_with(|e| e.include_deleted = true);
        assert_eq!(
            grep_command_echo("budget", &echo),
            "grans grep \"budget\" --include-deleted"
        );
    }

    #[test]
    fn grep_command_echo_combines_every_count_affecting_filter() {
        let echo = FilterEcho {
            in_targets: vec![SearchTarget::Transcripts],
            meeting: Some("Weekly Standup".to_string()),
            date: Some("last-week".to_string()),
            from: None,
            to: None,
            include_deleted: true,
        };
        assert_eq!(
            grep_cross_link(41, "budget", &echo).as_deref(),
            Some(
                "41 meeting(s) contain these words; grans grep \"budget\" --in transcripts \
                 --meeting \"Weekly Standup\" --date last-week --include-deleted lists them all."
            )
        );
    }
}
