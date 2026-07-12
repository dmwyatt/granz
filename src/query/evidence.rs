//! Lexical match evidence for one document: where in its panels, notes, and
//! transcript the query terms actually occur.
//!
//! Match semantics mirror the `--context` path exactly: a site matches when
//! it contains every query token (`matches_all_tokens`), panels split into
//! sections on the most frequent heading level, notes split into paragraphs.
//! Sites are collected in display priority order (AI-notes panels first as
//! the most distilled source, then the user's notes, then the transcript)
//! and only the first `max_matches` are excerpted.

use anyhow::Result;
use rusqlite::Connection;

use crate::models::{Document, SpeakerFilter};
use crate::query::fts::{matches_all_tokens, FtsToken};
use crate::query::hybrid::BestChunk;
use crate::query::shape::{
    excerpt_around_match, title_matches, EvidenceSource, MatchEvidence, ShapedMeeting, Signals,
};
use crate::query::text::{split_into_paragraphs, split_markdown_sections, strip_panel_footer};

/// How evidence is selected and excerpted per document.
#[derive(Debug, Clone)]
pub struct EvidenceOptions {
    /// How many match sites to excerpt (the rest are only counted).
    pub max_matches: usize,
    /// Rough excerpt width in characters.
    pub max_chars: usize,
    /// When set, only transcript utterances by this speaker count as
    /// evidence; panel and notes sites have no speaker to attribute and are
    /// excluded, and the semantic and title fallback tiers are disabled.
    pub speaker: Option<SpeakerFilter>,
}

impl Default for EvidenceOptions {
    fn default() -> Self {
        Self { max_matches: 1, max_chars: 160, speaker: None }
    }
}

/// All lexical match evidence for one document.
#[derive(Debug)]
pub struct DocumentEvidence {
    /// Excerpted evidence, at most `max_matches` entries.
    pub matches: Vec<MatchEvidence>,
    /// Total match sites in the document, shown or not.
    pub total: usize,
    /// Sources of the sites beyond `matches`, deduped in display order.
    pub remaining_sources: Vec<EvidenceSource>,
}

/// A match site before excerpting: the source, the matched text, and its
/// display metadata.
struct Site {
    source: EvidenceSource,
    text: String,
    speaker: Option<String>,
    timestamp: Option<String>,
    section: Option<String>,
}

/// Collect every lexical match site for `doc` and excerpt the first
/// `opts.max_matches` of them.
pub fn collect_document_evidence(
    conn: &Connection,
    doc: &Document,
    tokens: &[FtsToken],
    opts: &EvidenceOptions,
) -> Result<DocumentEvidence> {
    // An empty query matches nothing, mirroring the FTS empty-phrase
    // behavior; without this, matches_all_tokens is vacuously true and
    // every site in the document would count as a match.
    if tokens.is_empty() {
        return Ok(DocumentEvidence {
            matches: Vec::new(),
            total: 0,
            remaining_sources: Vec::new(),
        });
    }

    let mut sites = Vec::new();

    // A speaker filter restricts evidence to attributable transcript sites.
    if opts.speaker.is_none() {
        if let Some(doc_id) = doc.id.as_deref() {
            collect_panel_sites(conn, doc_id, tokens, &mut sites)?;
        }
        collect_notes_sites(doc.notes_plain.as_deref(), tokens, &mut sites);
    }
    if let Some(doc_id) = doc.id.as_deref() {
        collect_transcript_sites(conn, doc_id, tokens, opts.speaker.as_ref(), &mut sites)?;
    }

    let total = sites.len();
    let mut matches = Vec::new();
    let mut remaining_sources = Vec::new();
    for (i, site) in sites.into_iter().enumerate() {
        if i < opts.max_matches {
            matches.push(MatchEvidence {
                source: site.source,
                excerpt: excerpt_around_match(&site.text, tokens, opts.max_chars),
                speaker: site.speaker,
                timestamp: site.timestamp,
                section: site.section,
            });
        } else if !remaining_sources.contains(&site.source) {
            remaining_sources.push(site.source);
        }
    }

    Ok(DocumentEvidence { matches, total, remaining_sources })
}

/// Ranking facts about one document from the hybrid pipeline.
pub struct RankingFacts<'a> {
    /// The FTS retriever surfaced this document.
    pub keyword: bool,
    /// The document's best semantic chunk, when one exists.
    pub best_chunk: Option<&'a BestChunk>,
    /// Cross-encoder relevance when the rerank stage ran.
    pub score: Option<f32>,
}

/// Assemble the shaped card for one meeting.
///
/// Evidence degrades in tiers: lexical match sites first; when no query
/// term appears literally in the content, the semantic best chunk stands in
/// (no highlights); when there is no content evidence at all, the card is a
/// bare title match.
pub fn shape_meeting(
    conn: &Connection,
    doc: &Document,
    tokens: &[FtsToken],
    facts: &RankingFacts,
    opts: &EvidenceOptions,
) -> Result<ShapedMeeting> {
    let evidence = collect_document_evidence(conn, doc, tokens, opts)?;
    let signals = Signals {
        keyword: facts.keyword,
        semantic: facts.best_chunk.is_some(),
        title: doc.title.as_deref().map(|t| title_matches(t, tokens)).unwrap_or(false),
    };

    // Tier on whether lexical evidence exists, not on whether any was
    // excerpted: --matches 0 keeps matches empty while total still counts.
    // A speaker filter disables the semantic fallback: an unattributable
    // chunk cannot stand in for evidence the filter asks to attribute.
    let (matches, total, remaining_sources) = if evidence.total > 0 {
        (evidence.matches, evidence.total, evidence.remaining_sources)
    } else if let Some(m) = facts
        .best_chunk
        .filter(|_| opts.speaker.is_none())
        .and_then(|c| chunk_evidence(c, tokens, opts))
    {
        (vec![m], 1, Vec::new())
    } else {
        (Vec::new(), 0, Vec::new())
    };

    Ok(ShapedMeeting {
        document_id: doc.id.clone().unwrap_or_default(),
        title: doc.title.clone(),
        created_at: doc.created_at.clone(),
        score: facts.score,
        signals,
        total_matches: total,
        matches,
        remaining_sources,
    })
}

/// Tier-2 evidence: the semantic best chunk, excerpted like any other site.
fn chunk_evidence(
    chunk: &BestChunk,
    tokens: &[FtsToken],
    opts: &EvidenceOptions,
) -> Option<MatchEvidence> {
    let source = match chunk.source_type.as_str() {
        "transcript_window" => EvidenceSource::Transcript,
        "panel_section" => EvidenceSource::Panel,
        "notes_paragraph" => EvidenceSource::Notes,
        _ => return None,
    };
    Some(MatchEvidence {
        source,
        excerpt: excerpt_around_match(&chunk.text, tokens, opts.max_chars),
        speaker: None,
        timestamp: None,
        section: chunk.section_heading.clone(),
    })
}

/// Panel sections whose body contains every token, in panel order.
fn collect_panel_sites(
    conn: &Connection,
    doc_id: &str,
    tokens: &[FtsToken],
    sites: &mut Vec<Site>,
) -> Result<()> {
    for panel in crate::db::panels::load_panels(conn, doc_id)? {
        let Some(markdown) = panel.content_markdown.as_deref().filter(|m| !m.is_empty()) else {
            continue;
        };
        let cleaned = strip_panel_footer(markdown);
        for (heading, body) in split_markdown_sections(cleaned) {
            if matches_all_tokens(body, tokens) {
                sites.push(Site {
                    source: EvidenceSource::Panel,
                    text: body.to_string(),
                    speaker: None,
                    timestamp: None,
                    section: heading.map(String::from),
                });
            }
        }
    }
    Ok(())
}

/// Notes paragraphs that contain every token, in document order.
fn collect_notes_sites(notes_plain: Option<&str>, tokens: &[FtsToken], sites: &mut Vec<Site>) {
    let Some(notes) = notes_plain.filter(|n| !n.trim().is_empty()) else {
        return;
    };
    for para in split_into_paragraphs(notes) {
        if matches_all_tokens(para, tokens) {
            sites.push(Site {
                source: EvidenceSource::Notes,
                text: para.to_string(),
                speaker: None,
                timestamp: None,
                section: None,
            });
        }
    }
}

/// Transcript utterances that contain every token, in time order. With a
/// speaker filter, only that speaker's utterances count.
fn collect_transcript_sites(
    conn: &Connection,
    doc_id: &str,
    tokens: &[FtsToken],
    speaker: Option<&SpeakerFilter>,
    sites: &mut Vec<Site>,
) -> Result<()> {
    for utt in crate::db::transcripts::load_transcript(conn, doc_id)? {
        let Some(text) = utt.text.filter(|t| !t.is_empty()) else {
            continue;
        };
        if let Some(filter) = speaker {
            if !filter.matches(utt.source.as_deref()) {
                continue;
            }
        }
        if matches_all_tokens(&text, tokens) {
            sites.push(Site {
                source: EvidenceSource::Transcript,
                text,
                speaker: utt.source,
                timestamp: utt.start_timestamp,
                section: None,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use crate::query::fts::parse_query;
    use serde_json::json;

    fn evidence_state() -> serde_json::Value {
        json!({
            "documents": {
                "doc-1": {
                    "id": "doc-1",
                    "title": "Infra Sync",
                    "created_at": "2026-01-20T10:00:00Z",
                    "notes_plain": "Reviewed the kumquat rollout.\n\nUnrelated paragraph.\n\nSecond kumquat mention in notes."
                }
            },
            "transcripts": {
                "doc-1": [
                    {"id": "u1", "document_id": "doc-1", "text": "Let's discuss the kumquat rollout.",
                     "start_timestamp": "2026-01-20T10:01:00Z", "end_timestamp": "2026-01-20T10:01:05Z",
                     "source": "microphone", "is_final": true},
                    {"id": "u2", "document_id": "doc-1", "text": "Nothing relevant here.",
                     "start_timestamp": "2026-01-20T10:02:00Z", "end_timestamp": "2026-01-20T10:02:05Z",
                     "source": "system", "is_final": true},
                    {"id": "u3", "document_id": "doc-1", "text": "Back to the kumquat question.",
                     "start_timestamp": "2026-01-20T10:03:00Z", "end_timestamp": "2026-01-20T10:03:05Z",
                     "source": "system", "is_final": true}
                ]
            },
            "panels": {
                "doc-1": [
                    {"id": "panel-1", "document_id": "doc-1", "title": "Summary", "content_json": "{}",
                     "content_markdown": "### Decisions\n\nShip the kumquat rollout next week.\n\n### Action Items\n\n- Unrelated item",
                     "template_slug": "meeting-notes", "created_at": "2026-01-20T11:00:00Z"}
                ]
            }
        })
    }

    fn load_doc(conn: &Connection, id: &str) -> Document {
        crate::db::meetings::get_meetings_by_ids(conn, &[id.to_string()])
            .unwrap()
            .remove(0)
    }

    fn collect(
        conn: &Connection,
        doc: &Document,
        query: &str,
        max_matches: usize,
    ) -> DocumentEvidence {
        let limits = EvidenceOptions { max_matches, max_chars: 160, speaker: None };
        collect_document_evidence(conn, doc, &parse_query(query), &limits).unwrap()
    }

    #[test]
    fn counts_every_match_site_across_sources() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let ev = collect(&conn, &doc, "kumquat", 1);
        // 1 panel section + 2 notes paragraphs + 2 utterances.
        assert_eq!(ev.total, 5);
        assert_eq!(ev.matches.len(), 1);
    }

    #[test]
    fn panel_evidence_ranks_first_and_carries_its_section() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let ev = collect(&conn, &doc, "kumquat", 1);
        let m = &ev.matches[0];
        assert_eq!(m.source, EvidenceSource::Panel);
        assert_eq!(m.section.as_deref(), Some("Decisions"));
        assert!(m.excerpt.text.contains("kumquat rollout next week"));
        assert!(!m.excerpt.highlights.is_empty());
    }

    #[test]
    fn evidence_orders_panels_then_notes_then_transcript() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let ev = collect(&conn, &doc, "kumquat", 5);
        let sources: Vec<EvidenceSource> = ev.matches.iter().map(|m| m.source).collect();
        assert_eq!(
            sources,
            vec![
                EvidenceSource::Panel,
                EvidenceSource::Notes,
                EvidenceSource::Notes,
                EvidenceSource::Transcript,
                EvidenceSource::Transcript,
            ]
        );
        assert!(ev.remaining_sources.is_empty());
    }

    #[test]
    fn transcript_evidence_carries_speaker_and_timestamp() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let ev = collect(&conn, &doc, "kumquat", 5);
        let transcript: Vec<_> = ev
            .matches
            .iter()
            .filter(|m| m.source == EvidenceSource::Transcript)
            .collect();
        assert_eq!(transcript[0].speaker.as_deref(), Some("microphone"));
        assert_eq!(transcript[0].timestamp.as_deref(), Some("2026-01-20T10:01:00Z"));
        assert_eq!(transcript[1].speaker.as_deref(), Some("system"));
    }

    #[test]
    fn remaining_sources_dedupe_in_display_order() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let ev = collect(&conn, &doc, "kumquat", 1);
        assert_eq!(
            ev.remaining_sources,
            vec![EvidenceSource::Notes, EvidenceSource::Transcript]
        );
    }

    #[test]
    fn no_match_yields_empty_evidence() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let ev = collect(&conn, &doc, "zyzzyva", 3);
        assert_eq!(ev.total, 0);
        assert!(ev.matches.is_empty());
        assert!(ev.remaining_sources.is_empty());
    }

    #[test]
    fn multi_token_query_requires_all_tokens_in_one_site() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        // "kumquat" and "question" only co-occur in utterance u3.
        let ev = collect(&conn, &doc, "kumquat question", 5);
        assert_eq!(ev.total, 1);
        assert_eq!(ev.matches[0].source, EvidenceSource::Transcript);
        assert_eq!(ev.matches[0].timestamp.as_deref(), Some("2026-01-20T10:03:00Z"));
    }

    #[test]
    fn document_without_notes_or_content_matches_nothing() {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-2": {"id": "doc-2", "title": "Empty", "created_at": "2026-01-01T00:00:00Z"}
            }
        }));
        let doc = load_doc(&conn, "doc-2");
        let ev = collect(&conn, &doc, "kumquat", 3);
        assert_eq!(ev.total, 0);
    }

    // --- shape_meeting ---

    fn best_chunk(text: &str, source_type: &str, section: Option<&str>) -> BestChunk {
        BestChunk {
            text: text.to_string(),
            source_type: source_type.to_string(),
            section_heading: section.map(String::from),
        }
    }

    fn shape(
        conn: &Connection,
        doc: &Document,
        query: &str,
        facts: &RankingFacts,
    ) -> ShapedMeeting {
        shape_meeting(conn, doc, &parse_query(query), facts, &EvidenceOptions::default()).unwrap()
    }

    #[test]
    fn lexical_evidence_outranks_the_semantic_chunk() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let chunk = best_chunk("some semantic chunk", "transcript_window", None);
        let facts =
            RankingFacts { keyword: true, best_chunk: Some(&chunk), score: Some(0.9) };

        let shaped = shape(&conn, &doc, "kumquat", &facts);

        assert_eq!(shaped.matches.len(), 1);
        assert_eq!(shaped.matches[0].source, EvidenceSource::Panel);
        assert_eq!(shaped.total_matches, 5);
        assert!(shaped.signals.keyword && shaped.signals.semantic);
        assert!(!shaped.signals.title);
        assert_eq!(shaped.score, Some(0.9));
    }

    #[test]
    fn semantic_only_match_falls_back_to_the_best_chunk() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        // No content in doc-1 contains "zyzzyva"; the chunk stands in.
        let chunk = best_chunk(
            "discussion of adjacent topics",
            "panel_section",
            Some("Decisions"),
        );
        let facts = RankingFacts { keyword: false, best_chunk: Some(&chunk), score: Some(0.4) };

        let shaped = shape(&conn, &doc, "zyzzyva", &facts);

        assert_eq!(shaped.matches.len(), 1);
        assert_eq!(shaped.matches[0].source, EvidenceSource::Panel);
        assert_eq!(shaped.matches[0].section.as_deref(), Some("Decisions"));
        assert_eq!(shaped.matches[0].excerpt.text, "discussion of adjacent topics");
        assert!(shaped.matches[0].excerpt.highlights.is_empty());
        assert_eq!(shaped.total_matches, 1);
        assert!(!shaped.signals.keyword);
        assert!(shaped.signals.semantic);
    }

    #[test]
    fn title_only_match_yields_a_bare_card() {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-t": {"id": "doc-t", "title": "Kumquat Planning", "created_at": "2026-01-05T00:00:00Z"}
            }
        }));
        let doc = load_doc(&conn, "doc-t");
        let facts = RankingFacts { keyword: true, best_chunk: None, score: None };

        let shaped = shape(&conn, &doc, "kumquat", &facts);

        assert!(shaped.matches.is_empty());
        assert_eq!(shaped.total_matches, 0);
        assert!(shaped.signals.title);
        assert!(shaped.signals.keyword);
        assert!(!shaped.signals.semantic);
        assert_eq!(shaped.score, None);
    }

    #[test]
    fn matches_zero_keeps_lexical_count_and_never_falls_back_to_the_chunk() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let chunk = best_chunk("some semantic chunk", "transcript_window", None);
        let facts = RankingFacts { keyword: true, best_chunk: Some(&chunk), score: None };
        let limits = EvidenceOptions { max_matches: 0, max_chars: 160, speaker: None };

        let shaped =
            shape_meeting(&conn, &doc, &parse_query("kumquat"), &facts, &limits).unwrap();

        // Headers-only: no snippets, but the real lexical count and its
        // sources survive for the collapse line, and the semantic chunk
        // does not stand in.
        assert!(shaped.matches.is_empty());
        assert_eq!(shaped.total_matches, 5);
        assert_eq!(
            shaped.remaining_sources,
            vec![
                EvidenceSource::Panel,
                EvidenceSource::Notes,
                EvidenceSource::Transcript
            ]
        );
    }

    #[test]
    fn empty_query_yields_no_lexical_evidence() {
        // An empty query matches nothing lexically (mirroring the FTS
        // empty-phrase behavior), rather than vacuously matching every
        // site.
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let ev = collect(&conn, &doc, "", 3);
        assert_eq!(ev.total, 0);
        assert!(ev.matches.is_empty());
    }

    // --- speaker filter ---

    #[test]
    fn speaker_filter_counts_only_matching_utterances() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        // "kumquat" sites: 1 panel, 2 notes, u1 (microphone), u3 (system).
        // With a speaker filter only transcript sites by that speaker count.
        let opts = EvidenceOptions {
            speaker: Some(crate::models::SpeakerFilter::Me),
            ..Default::default()
        };
        let ev =
            collect_document_evidence(&conn, &doc, &parse_query("kumquat"), &opts).unwrap();
        assert_eq!(ev.total, 1);
        assert_eq!(ev.matches[0].source, EvidenceSource::Transcript);
        assert_eq!(ev.matches[0].speaker.as_deref(), Some("microphone"));
    }

    #[test]
    fn speaker_filter_excludes_unattributable_sources() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let opts = EvidenceOptions {
            speaker: Some(crate::models::SpeakerFilter::Other),
            ..Default::default()
        };
        let ev =
            collect_document_evidence(&conn, &doc, &parse_query("kumquat"), &opts).unwrap();
        // Only u3 ("Back to the kumquat question.", system); the panel and
        // notes sites have no speaker to attribute and do not count.
        assert_eq!(ev.total, 1);
        assert_eq!(ev.matches[0].speaker.as_deref(), Some("system"));
        assert!(ev.matches[0].excerpt.text.contains("question"));
    }

    #[test]
    fn speaker_filter_disables_semantic_and_title_fallbacks() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        // "rollout" appears only in u1 (microphone). Filtering to Other
        // leaves no attributable evidence, and neither the semantic chunk
        // nor the title may stand in.
        let chunk = best_chunk("some semantic chunk", "transcript_window", None);
        let facts = RankingFacts { keyword: true, best_chunk: Some(&chunk), score: None };
        let opts = EvidenceOptions {
            speaker: Some(crate::models::SpeakerFilter::Other),
            ..Default::default()
        };

        let shaped =
            shape_meeting(&conn, &doc, &parse_query("rollout"), &facts, &opts).unwrap();

        assert_eq!(shaped.total_matches, 0);
        assert!(shaped.matches.is_empty());
    }

    #[test]
    fn shaped_meeting_carries_identity_fields() {
        let conn = build_test_db(&evidence_state());
        let doc = load_doc(&conn, "doc-1");
        let facts = RankingFacts { keyword: true, best_chunk: None, score: Some(0.7) };

        let shaped = shape(&conn, &doc, "kumquat", &facts);

        assert_eq!(shaped.document_id, "doc-1");
        assert_eq!(shaped.title.as_deref(), Some("Infra Sync"));
        assert_eq!(shaped.created_at.as_deref(), Some("2026-01-20T10:00:00Z"));
        assert_eq!(
            shaped.remaining_sources,
            vec![EvidenceSource::Notes, EvidenceSource::Transcript]
        );
    }
}
