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
