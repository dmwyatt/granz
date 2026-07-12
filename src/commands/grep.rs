//! `grans grep`: complete lexical lookup.
//!
//! FTS-only retrieval with an honest total: every meeting containing the
//! words is counted, and `--limit` only trims how many are shown. This path
//! never loads an embedding model and never prompts.

use anyhow::{bail, Result};
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::commands::search_common::{fts_meetings, render_shaped_meeting_list, shape_and_page};
use crate::models::{Document, SpeakerFilter};
use crate::query::dates::DateRange;
use crate::query::filter::{filter_by_meeting, SearchTarget};

/// Options for a grep lookup.
pub struct GrepOptions {
    pub targets: Vec<SearchTarget>,
    pub meeting_filter: Option<String>,
    pub limit: usize,
    /// Match snippets shown per meeting card.
    pub matches: usize,
    /// Only meetings with matching utterances by this speaker survive.
    pub speaker: Option<SpeakerFilter>,
    /// Neighboring units rendered around each shown match.
    pub context: usize,
}

/// Run a complete FTS lookup and display every matching meeting as shaped
/// cards. Grep results carry no rerank score and no semantic chunk; content
/// matches show lexical evidence and title-only matches show a bare title
/// card.
pub fn grep(
    conn: &Connection,
    query: &str,
    opts: GrepOptions,
    date_range: Option<DateRange>,
    include_deleted: bool,
    ctx: &RunContext,
) -> Result<()> {
    check_speaker_targets(opts.speaker.is_some(), &opts.targets)?;

    let results = fts_meetings(conn, query, &opts.targets, date_range.as_ref(), include_deleted)?;
    let results = filter_by_meeting(results, opts.meeting_filter.as_deref());
    let docs: Vec<(Document, Option<f32>)> =
        results.into_iter().map(|doc| (doc, None)).collect();

    let tokens = crate::query::fts::parse_query(query);
    let evidence_opts = crate::query::evidence::EvidenceOptions {
        max_matches: opts.matches,
        speaker: opts.speaker,
        context: opts.context,
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
        &evidence_opts,
        opts.limit,
    )?;

    render_shaped_meeting_list(&shaped, query, total, opts.limit, ctx);
    Ok(())
}

/// `--speaker` restricts match evidence to transcript utterances, so the
/// target list must include transcripts for the filter to have anything to
/// match against.
fn check_speaker_targets(speaker: bool, targets: &[SearchTarget]) -> Result<()> {
    if speaker && !targets.contains(&SearchTarget::Transcripts) {
        bail!(
            "--speaker matches transcript utterances, but --in excludes transcripts; \
             add transcripts to --in"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_with_targets_excluding_transcripts_is_an_error() {
        let targets = SearchTarget::parse_list("titles,notes");
        let err = check_speaker_targets(true, &targets).unwrap_err();
        assert!(err.to_string().contains("transcripts"));
    }

    #[test]
    fn speaker_with_transcripts_included_is_accepted() {
        let targets = SearchTarget::parse_list("notes,transcripts");
        assert!(check_speaker_targets(true, &targets).is_ok());
    }

    #[test]
    fn no_speaker_accepts_any_targets() {
        let targets = SearchTarget::parse_list("notes");
        assert!(check_speaker_targets(false, &targets).is_ok());
    }
}
