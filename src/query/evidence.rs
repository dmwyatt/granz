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

use crate::models::Document;
use crate::query::fts::{matches_all_tokens, FtsToken};
use crate::query::shape::{excerpt_around_match, EvidenceSource, MatchEvidence};
use crate::query::text::{split_into_paragraphs, split_markdown_sections, strip_panel_footer};

/// Caps on how much evidence is excerpted per document.
#[derive(Debug, Clone)]
pub struct EvidenceLimits {
    /// How many match sites to excerpt (the rest are only counted).
    pub max_matches: usize,
    /// Rough excerpt width in characters.
    pub max_chars: usize,
}

impl Default for EvidenceLimits {
    fn default() -> Self {
        Self { max_matches: 1, max_chars: 160 }
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
/// `limits.max_matches` of them.
pub fn collect_document_evidence(
    conn: &Connection,
    doc: &Document,
    tokens: &[FtsToken],
    limits: &EvidenceLimits,
) -> Result<DocumentEvidence> {
    let mut sites = Vec::new();

    if let Some(doc_id) = doc.id.as_deref() {
        collect_panel_sites(conn, doc_id, tokens, &mut sites)?;
    }
    collect_notes_sites(doc.notes_plain.as_deref(), tokens, &mut sites);
    if let Some(doc_id) = doc.id.as_deref() {
        collect_transcript_sites(conn, doc_id, tokens, &mut sites)?;
    }

    let total = sites.len();
    let mut matches = Vec::new();
    let mut remaining_sources = Vec::new();
    for (i, site) in sites.into_iter().enumerate() {
        if i < limits.max_matches {
            matches.push(MatchEvidence {
                source: site.source,
                excerpt: excerpt_around_match(&site.text, tokens, limits.max_chars),
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

/// Transcript utterances that contain every token, in time order.
fn collect_transcript_sites(
    conn: &Connection,
    doc_id: &str,
    tokens: &[FtsToken],
    sites: &mut Vec<Site>,
) -> Result<()> {
    for utt in crate::db::transcripts::load_transcript(conn, doc_id)? {
        let Some(text) = utt.text.filter(|t| !t.is_empty()) else {
            continue;
        };
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
        let limits = EvidenceLimits { max_matches, max_chars: 160 };
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
}
