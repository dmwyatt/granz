//! Shaping, paging, and rendering shared by the search-family commands.

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::context::RunContext;
use crate::models::Document;

/// Truncate a vec to `limit` items. A limit of 0 means no limit.
pub fn apply_limit<T>(mut items: Vec<T>, limit: usize) -> Vec<T> {
    if limit > 0 && items.len() > limit {
        items.truncate(limit);
    }
    items
}

/// Shape ranked documents into meeting cards, in the order given, and cut
/// the display page. Returns the page and the total result count.
///
/// Without a speaker filter the page is cut first and only displayed cards
/// are shaped. With one, every candidate is shaped so that meetings whose
/// evidence the filter eliminates drop out of the count entirely, and the
/// page is cut from the survivors.
pub fn shape_and_page<'a>(
    conn: &Connection,
    docs: Vec<(Document, Option<f32>)>,
    facts_for: impl Fn(&Document, Option<f32>) -> crate::query::evidence::RankingFacts<'a>,
    tokens: &[crate::query::fts::FtsToken],
    opts: &crate::query::evidence::EvidenceOptions,
    limit: usize,
) -> Result<(Vec<crate::query::shape::ShapedMeeting>, usize)> {
    let shape = |docs: &[(Document, Option<f32>)]| {
        docs.iter()
            .map(|(doc, score)| {
                let facts = facts_for(doc, *score);
                crate::query::evidence::shape_meeting(conn, doc, tokens, &facts, opts)
            })
            .collect::<Result<Vec<_>>>()
    };

    if opts.speaker.is_none() {
        let total = docs.len();
        let page = apply_limit(docs, limit);
        Ok((shape(&page)?, total))
    } else {
        let survivors: Vec<_> =
            shape(&docs)?.into_iter().filter(|m| m.total_matches > 0).collect();
        let total = survivors.len();
        Ok((apply_limit(survivors, limit), total))
    }
}

/// Print numbered meeting cards to stdout.
pub fn print_shaped_cards(shaped: &[crate::query::shape::ShapedMeeting], ctx: &RunContext) {
    for (i, meeting) in shaped.iter().enumerate() {
        println!(
            "{}\n",
            crate::output::card::format_shaped_meeting(meeting, i + 1, &ctx.tz)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SpeakerFilter;

    /// Documents in a fixed order with scores, as a ranked pipeline would
    /// hand them to shaping.
    fn ranked_docs(conn: &Connection) -> Vec<(Document, Option<f32>)> {
        let docs = crate::db::meetings::get_meetings_by_ids(
            &conn,
            &["doc-c".to_string(), "doc-a".to_string(), "doc-b".to_string()],
        )
        .unwrap();
        let mut by_id: std::collections::HashMap<String, Document> =
            docs.into_iter().map(|d| (d.id.clone().unwrap(), d)).collect();
        vec![
            (by_id.remove("doc-c").unwrap(), Some(0.5)),
            (by_id.remove("doc-a").unwrap(), Some(0.9)),
            (by_id.remove("doc-b").unwrap(), None),
        ]
    }

    fn plain_facts<'a>(_: &Document, score: Option<f32>) -> crate::query::evidence::RankingFacts<'a> {
        crate::query::evidence::RankingFacts { keyword: true, best_chunk: None, score }
    }

    #[test]
    fn shape_and_page_preserves_ranking_order_and_scores() {
        // Shaping is presentation-only: the cards must come back in exactly
        // the order the ranked list was handed in, whatever the db returns.
        use crate::db::test_fixtures::build_test_db;
        use serde_json::json;

        let conn = build_test_db(&json!({
            "documents": {
                "doc-a": {"id": "doc-a", "title": "Alpha", "created_at": "2026-01-01T10:00:00Z"},
                "doc-b": {"id": "doc-b", "title": "Beta", "created_at": "2026-01-02T10:00:00Z"},
                "doc-c": {"id": "doc-c", "title": "Gamma", "created_at": "2026-01-03T10:00:00Z"}
            }
        }));

        let (shaped, total) = shape_and_page(
            &conn,
            ranked_docs(&conn),
            plain_facts,
            &crate::query::fts::parse_query("alpha"),
            &crate::query::evidence::EvidenceOptions::default(),
            0,
        )
        .unwrap();

        assert_eq!(total, 3);
        let ids: Vec<&str> = shaped.iter().map(|m| m.document_id.as_str()).collect();
        assert_eq!(ids, vec!["doc-c", "doc-a", "doc-b"]);
        assert_eq!(shaped[0].score, Some(0.5));
        assert_eq!(shaped[1].score, Some(0.9));
        assert_eq!(shaped[2].score, None);
    }

    #[test]
    fn shape_and_page_speaker_filter_drops_meetings_and_recounts() {
        // With a speaker filter, meetings without a matching utterance by
        // that speaker vanish from both the page and the total, in order.
        use crate::db::test_fixtures::build_test_db;
        use serde_json::json;

        let conn = build_test_db(&json!({
            "documents": {
                "doc-a": {"id": "doc-a", "title": "Alpha", "created_at": "2026-01-01T10:00:00Z",
                          "notes_plain": "kumquat in notes only"},
                "doc-b": {"id": "doc-b", "title": "Beta", "created_at": "2026-01-02T10:00:00Z"},
                "doc-c": {"id": "doc-c", "title": "Gamma", "created_at": "2026-01-03T10:00:00Z"}
            },
            "transcripts": {
                "doc-b": [
                    {"id": "u1", "document_id": "doc-b", "text": "the kumquat by me",
                     "start_timestamp": "2026-01-02T10:01:00Z", "end_timestamp": "2026-01-02T10:01:05Z",
                     "source": "microphone", "is_final": true}
                ],
                "doc-c": [
                    {"id": "u2", "document_id": "doc-c", "text": "the kumquat by them",
                     "start_timestamp": "2026-01-03T10:01:00Z", "end_timestamp": "2026-01-03T10:01:05Z",
                     "source": "system", "is_final": true}
                ]
            }
        }));
        let opts = crate::query::evidence::EvidenceOptions {
            speaker: Some(SpeakerFilter::Me),
            ..Default::default()
        };

        let (shaped, total) = shape_and_page(
            &conn,
            ranked_docs(&conn),
            plain_facts,
            &crate::query::fts::parse_query("kumquat"),
            &opts,
            0,
        )
        .unwrap();

        // doc-c's match is by the other speaker; doc-a's is unattributable
        // notes evidence. Only doc-b survives.
        assert_eq!(total, 1);
        assert_eq!(shaped.len(), 1);
        assert_eq!(shaped[0].document_id, "doc-b");
        assert_eq!(shaped[0].score, None);
        assert_eq!(shaped[0].matches[0].speaker.as_deref(), Some("microphone"));
    }

    #[test]
    fn shape_and_page_speaker_filter_pages_survivors_in_ranked_order() {
        // With a speaker filter and a truncating limit, the total counts
        // every survivor while the page holds only the leading ones, in
        // the ranked order they arrived.
        use crate::db::test_fixtures::build_test_db;
        use serde_json::json;

        let conn = build_test_db(&json!({
            "documents": {
                "doc-a": {"id": "doc-a", "title": "Alpha", "created_at": "2026-01-01T10:00:00Z",
                          "notes_plain": "kumquat in notes only"},
                "doc-b": {"id": "doc-b", "title": "Beta", "created_at": "2026-01-02T10:00:00Z"},
                "doc-c": {"id": "doc-c", "title": "Gamma", "created_at": "2026-01-03T10:00:00Z"}
            },
            "transcripts": {
                "doc-b": [
                    {"id": "u1", "document_id": "doc-b", "text": "the kumquat by me",
                     "start_timestamp": "2026-01-02T10:01:00Z", "end_timestamp": "2026-01-02T10:01:05Z",
                     "source": "microphone", "is_final": true}
                ],
                "doc-c": [
                    {"id": "u2", "document_id": "doc-c", "text": "another kumquat by me",
                     "start_timestamp": "2026-01-03T10:01:00Z", "end_timestamp": "2026-01-03T10:01:05Z",
                     "source": "microphone", "is_final": true}
                ]
            }
        }));
        let opts = crate::query::evidence::EvidenceOptions {
            speaker: Some(SpeakerFilter::Me),
            ..Default::default()
        };

        // Ranked order is [doc-c, doc-a, doc-b]; doc-a has no attributable
        // evidence, so the survivors are [doc-c, doc-b].
        let (shaped, total) = shape_and_page(
            &conn,
            ranked_docs(&conn),
            plain_facts,
            &crate::query::fts::parse_query("kumquat"),
            &opts,
            1,
        )
        .unwrap();

        assert_eq!(total, 2);
        assert_eq!(shaped.len(), 1);
        assert_eq!(shaped[0].document_id, "doc-c");
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
}
