use rusqlite::Connection;
use serde::Serialize;

use crate::embed::search::SemanticSearchResult;
use crate::models::{
    Calendar, CalendarEvent, Document, PanelTemplate, Person, Recipe, TranscriptUtterance,
};
use crate::query::search::ContextWindow;

/// Serialize any serializable value to pretty JSON string.
pub fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}

/// Format a list of documents as JSON.
pub fn format_meetings(docs: &[&Document]) -> String {
    to_json(&docs)
}

/// Format a single document as JSON.
pub fn format_meeting_detail(doc: &Document) -> String {
    to_json(doc)
}

/// JSON-serializable context window.
#[derive(Debug, Serialize)]
pub struct ContextWindowJson {
    pub document_id: String,
    pub document_title: String,
    pub before: Vec<UtteranceJson>,
    pub matched: UtteranceJson,
    pub after: Vec<UtteranceJson>,
}

#[derive(Debug, Serialize)]
pub struct UtteranceJson {
    pub id: String,
    pub text: String,
    pub start_timestamp: String,
    pub end_timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ContextWindowJson {
    pub fn from_window(window: &ContextWindow, doc_title: &str) -> Self {
        ContextWindowJson {
            document_id: window
                .matched
                .document_id
                .clone()
                .unwrap_or_default(),
            document_title: doc_title.to_string(),
            before: window.before.iter().map(|u| UtteranceJson::from_utt(u)).collect(),
            matched: UtteranceJson::from_utt(&window.matched),
            after: window.after.iter().map(|u| UtteranceJson::from_utt(u)).collect(),
        }
    }
}

impl UtteranceJson {
    fn from_utt(utt: &TranscriptUtterance) -> Self {
        UtteranceJson {
            id: utt.id.clone().unwrap_or_default(),
            text: utt.text.clone().unwrap_or_default(),
            start_timestamp: utt.start_timestamp.clone().unwrap_or_default(),
            end_timestamp: utt.end_timestamp.clone().unwrap_or_default(),
            source: utt.source.clone(),
        }
    }
}

/// JSON-serializable text context window (panels/notes).
#[derive(Debug, Serialize)]
pub struct TextContextWindowJson {
    pub document_id: String,
    pub document_title: String,
    pub source_type: String,
    pub before: Vec<TextSegmentJson>,
    pub matched: TextSegmentJson,
    pub after: Vec<TextSegmentJson>,
}

#[derive(Debug, Serialize)]
pub struct TextSegmentJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub text: String,
}

impl TextContextWindowJson {
    pub fn from_window(
        window: &crate::query::search::TextContextWindow,
        doc_id: &str,
        doc_title: &str,
        source_type: &str,
    ) -> Self {
        TextContextWindowJson {
            document_id: doc_id.to_string(),
            document_title: doc_title.to_string(),
            source_type: source_type.to_string(),
            before: window.before.iter().map(TextSegmentJson::from_segment).collect(),
            matched: TextSegmentJson::from_segment(&window.matched),
            after: window.after.iter().map(TextSegmentJson::from_segment).collect(),
        }
    }
}

impl TextSegmentJson {
    fn from_segment(seg: &crate::query::search::TextSegment) -> Self {
        TextSegmentJson {
            label: seg.label.clone(),
            text: seg.text.clone(),
        }
    }
}

/// Format mixed context windows (transcript + text) as JSON.
pub fn format_mixed_context_windows(
    transcript_windows: &[ContextWindowJson],
    text_windows: &[TextContextWindowJson],
) -> String {
    #[derive(Serialize)]
    struct MixedResponse<'a> {
        #[serde(skip_serializing_if = "<[ContextWindowJson]>::is_empty")]
        transcript_results: &'a [ContextWindowJson],
        #[serde(skip_serializing_if = "<[TextContextWindowJson]>::is_empty")]
        text_results: &'a [TextContextWindowJson],
    }
    to_json(&MixedResponse {
        transcript_results: transcript_windows,
        text_results: text_windows,
    })
}

/// JSON-serializable semantic search result.
#[derive(Debug, Serialize)]
pub struct SemanticResultJson {
    pub document_id: String,
    pub score: f32,
    pub title: String,
    pub created_at: String,
    pub source_type: String,
    pub matched_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_context: Option<String>,
}

/// JSON-serializable semantic search result with context window.
#[derive(Debug, Serialize)]
pub struct SemanticResultWithContextJson {
    pub document_id: String,
    pub score: f32,
    pub title: String,
    pub created_at: String,
    pub source_type: String,
    pub matched_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_context: Option<String>,
    pub context: ContextWindowJson,
}

/// Wrapper for semantic search response with metadata.
#[derive(Debug, Serialize)]
pub struct SemanticSearchResponse<T> {
    pub query: String,
    pub total_matches: usize,
    pub limit: usize,
    pub returned: usize,
    pub results: Vec<T>,
}

/// One match's evidence in shaped search JSON.
#[derive(Debug, Serialize)]
pub struct ShapedMatchJson {
    /// `transcript`, `panel`, or `notes`.
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub snippet: String,
    /// Char ranges `[start, end)` into `snippet` where query terms occur.
    pub highlights: Vec<(usize, usize)>,
}

/// One meeting in shaped search JSON.
#[derive(Debug, Serialize)]
pub struct ShapedMeetingJson {
    pub id: String,
    pub title: Option<String>,
    pub created_at: Option<String>,
    /// Cross-encoder relevance; absent under `--fast`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// Which retrievers surfaced the meeting: `keyword`, `semantic`, `title`.
    pub signals: Vec<&'static str>,
    pub total_matches: usize,
    pub matches: Vec<ShapedMatchJson>,
}

/// Response envelope for shaped search results.
#[derive(Debug, Serialize)]
pub struct ShapedSearchResponse {
    pub query: String,
    pub total_meetings: usize,
    pub limit: usize,
    pub returned: usize,
    pub meetings: Vec<ShapedMeetingJson>,
}

fn shaped_source_label(source: crate::query::shape::EvidenceSource) -> &'static str {
    use crate::query::shape::EvidenceSource;
    match source {
        EvidenceSource::Transcript => "transcript",
        EvidenceSource::Panel => "panel",
        EvidenceSource::Notes => "notes",
    }
}

impl ShapedMeetingJson {
    fn from_shaped(m: &crate::query::shape::ShapedMeeting) -> Self {
        let mut signals = Vec::new();
        if m.signals.keyword {
            signals.push("keyword");
        }
        if m.signals.semantic {
            signals.push("semantic");
        }
        if m.signals.title {
            signals.push("title");
        }
        ShapedMeetingJson {
            id: m.document_id.clone(),
            title: m.title.clone(),
            created_at: m.created_at.clone(),
            score: m.score,
            signals,
            total_matches: m.total_matches,
            matches: m
                .matches
                .iter()
                .map(|ev| ShapedMatchJson {
                    source: shaped_source_label(ev.source),
                    speaker: ev.speaker.as_deref().map(|s| match s {
                        "microphone" => "me".to_string(),
                        "system" => "other".to_string(),
                        other => other.to_string(),
                    }),
                    timestamp: ev.timestamp.clone(),
                    section: ev.section.clone(),
                    snippet: ev.excerpt.text.clone(),
                    highlights: ev.excerpt.highlights.clone(),
                })
                .collect(),
        }
    }
}

/// Format shaped search results as JSON with metadata.
pub fn format_shaped_meetings(
    results: &[crate::query::shape::ShapedMeeting],
    query: &str,
    total_meetings: usize,
    limit: usize,
) -> String {
    let meetings: Vec<ShapedMeetingJson> =
        results.iter().map(ShapedMeetingJson::from_shaped).collect();
    let response = ShapedSearchResponse {
        query: query.to_string(),
        total_meetings,
        limit,
        returned: meetings.len(),
        meetings,
    };
    to_json(&response)
}

/// Format semantic search results as JSON with metadata.
pub fn format_semantic_results(
    results: &[SemanticSearchResult],
    query: &str,
    total_matches: usize,
    limit: usize,
    conn: &Connection,
) -> String {
    let json_results: Vec<SemanticResultJson> = results
        .iter()
        .map(|r| {
            let (title, created_at) = lookup_document_meta(conn, &r.document_id);
            SemanticResultJson {
                document_id: r.document_id.clone(),
                score: r.score,
                title,
                created_at,
                source_type: r.source_type.clone(),
                matched_text: r.matched_text.clone(),
                match_context: r.match_context.clone(),
            }
        })
        .collect();

    let response = SemanticSearchResponse {
        query: query.to_string(),
        total_matches,
        limit,
        returned: json_results.len(),
        results: json_results,
    };

    to_json(&response)
}

/// Format semantic search results with context windows as JSON with metadata.
pub fn format_semantic_results_with_context(
    results: &[(SemanticSearchResult, ContextWindow)],
    query: &str,
    total_matches: usize,
    limit: usize,
    conn: &Connection,
) -> String {
    let json_results: Vec<SemanticResultWithContextJson> = results
        .iter()
        .map(|(r, window)| {
            let (title, created_at) = lookup_document_meta(conn, &r.document_id);
            SemanticResultWithContextJson {
                document_id: r.document_id.clone(),
                score: r.score,
                title: title.clone(),
                created_at,
                source_type: r.source_type.clone(),
                matched_text: r.matched_text.clone(),
                match_context: r.match_context.clone(),
                context: ContextWindowJson::from_window(window, &title),
            }
        })
        .collect();

    let response = SemanticSearchResponse {
        query: query.to_string(),
        total_matches,
        limit,
        returned: json_results.len(),
        results: json_results,
    };

    to_json(&response)
}

fn lookup_document_meta(conn: &Connection, doc_id: &str) -> (String, String) {
    let result: Option<(String, String)> = conn
        .query_row(
            "SELECT COALESCE(title, ''), COALESCE(created_at, '') FROM documents WHERE id = ?1",
            [doc_id],
            |row| {
                let title: String = row.get(0)?;
                let created_at: String = row.get(1)?;
                Ok((title, created_at))
            },
        )
        .ok();

    result.unwrap_or_else(|| (String::new(), String::new()))
}

/// Format a list of people as JSON.
pub fn format_people(people: &[&Person]) -> String {
    to_json(&people)
}

/// Format a list of calendars as JSON.
pub fn format_calendars(calendars: &[&Calendar]) -> String {
    to_json(&calendars)
}

/// Format a list of events as JSON.
pub fn format_events(events: &[&CalendarEvent]) -> String {
    to_json(&events)
}

/// Format a list of templates as JSON.
pub fn format_templates(templates: &[&PanelTemplate]) -> String {
    to_json(&templates)
}

/// Format a list of recipes as JSON.
pub fn format_recipes(recipes: &[&Recipe]) -> String {
    to_json(&recipes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::shape::{
        EvidenceSource, Excerpt, MatchEvidence, ShapedMeeting, Signals,
    };

    fn shaped() -> ShapedMeeting {
        ShapedMeeting {
            document_id: "doc-1".to_string(),
            title: Some("Infra Sync".to_string()),
            created_at: Some("2026-05-12T14:30:00Z".to_string()),
            score: Some(0.63),
            signals: Signals { keyword: true, semantic: false, title: true },
            total_matches: 3,
            matches: vec![MatchEvidence {
                source: EvidenceSource::Transcript,
                excerpt: Excerpt {
                    text: "run the migration tonight".to_string(),
                    highlights: vec![(8, 17)],
                },
                speaker: Some("microphone".to_string()),
                timestamp: Some("2026-05-12T14:31:07Z".to_string()),
                section: None,
            }],
            remaining_sources: vec![EvidenceSource::Notes],
        }
    }

    #[test]
    fn shaped_json_shape_and_signals() {
        let out = format_shaped_meetings(&[shaped()], "migration", 12, 10);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["query"], "migration");
        assert_eq!(v["total_meetings"], 12);
        assert_eq!(v["returned"], 1);
        let m = &v["meetings"][0];
        assert_eq!(m["id"], "doc-1");
        assert_eq!(m["signals"], serde_json::json!(["keyword", "title"]));
        assert_eq!(m["total_matches"], 3);
        let mt = &m["matches"][0];
        assert_eq!(mt["source"], "transcript");
        assert_eq!(mt["speaker"], "me");
        assert_eq!(mt["snippet"], "run the migration tonight");
        assert_eq!(mt["highlights"], serde_json::json!([[8, 17]]));
        assert!(mt.get("section").is_none());
    }

    #[test]
    fn shaped_json_omits_score_when_rerank_skipped() {
        let mut m = shaped();
        m.score = None;
        let out = format_shaped_meetings(&[m], "q", 1, 0);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["meetings"][0].get("score").is_none());
    }
}
