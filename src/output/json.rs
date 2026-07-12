use serde::Serialize;

use crate::models::{
    Calendar, CalendarEvent, Document, PanelTemplate, Person, Recipe,
};

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
    /// Neighboring units before the match, oldest first (`--context`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context_before: Vec<ContextUnitJson>,
    /// Neighboring units after the match (`--context`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context_after: Vec<ContextUnitJson>,
}

/// A neighboring content unit around a match in shaped search JSON.
#[derive(Debug, Serialize)]
pub struct ContextUnitJson {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
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

/// Response envelope for grep results. `total_meetings` is the complete
/// count of matching meetings, a fact about the corpus; `returned` is the
/// page cut by `limit`.
#[derive(Debug, Serialize)]
pub struct GrepResponse {
    pub query: String,
    pub total_meetings: usize,
    pub limit: usize,
    pub returned: usize,
    pub meetings: Vec<ShapedMeetingJson>,
}

/// Response envelope for ranked search results. The meeting list is a
/// pooled best-k, so no total is claimed; `keyword_total` is the uncapped
/// count of meetings containing the query words (what `grans grep` would
/// report).
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub keyword_total: usize,
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

/// Map a raw utterance source to the JSON speaker label.
fn shaped_speaker_label(source: Option<&str>) -> Option<String> {
    source.map(|s| match s {
        "microphone" => "me".to_string(),
        "system" => "other".to_string(),
        other => other.to_string(),
    })
}

impl ContextUnitJson {
    fn from_unit(unit: &crate::query::shape::ContextUnit) -> Self {
        ContextUnitJson {
            text: unit.text.clone(),
            speaker: shaped_speaker_label(unit.speaker.as_deref()),
            timestamp: unit.timestamp.clone(),
            section: unit.section.clone(),
        }
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
                    speaker: shaped_speaker_label(ev.speaker.as_deref()),
                    timestamp: ev.timestamp.clone(),
                    section: ev.section.clone(),
                    snippet: ev.excerpt.text.clone(),
                    highlights: ev.excerpt.highlights.clone(),
                    context_before: ev.context_before.iter().map(ContextUnitJson::from_unit).collect(),
                    context_after: ev.context_after.iter().map(ContextUnitJson::from_unit).collect(),
                })
                .collect(),
        }
    }
}

/// Format grep results as JSON with the complete match count.
pub fn format_grep_meetings(
    results: &[crate::query::shape::ShapedMeeting],
    query: &str,
    total_meetings: usize,
    limit: usize,
) -> String {
    let meetings: Vec<ShapedMeetingJson> =
        results.iter().map(ShapedMeetingJson::from_shaped).collect();
    let response = GrepResponse {
        query: query.to_string(),
        total_meetings,
        limit,
        returned: meetings.len(),
        meetings,
    };
    to_json(&response)
}

/// Format ranked search results as JSON with the uncapped FTS count.
pub fn format_search_meetings(
    results: &[crate::query::shape::ShapedMeeting],
    query: &str,
    keyword_total: usize,
    limit: usize,
) -> String {
    let meetings: Vec<ShapedMeetingJson> =
        results.iter().map(ShapedMeetingJson::from_shaped).collect();
    let response = SearchResponse {
        query: query.to_string(),
        keyword_total,
        limit,
        returned: meetings.len(),
        meetings,
    };
    to_json(&response)
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
                context_before: Vec::new(),
                context_after: Vec::new(),
            }],
            remaining_sources: vec![EvidenceSource::Notes],
        }
    }

    #[test]
    fn grep_json_shape_and_signals() {
        let out = format_grep_meetings(&[shaped()], "migration", 12, 10);
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
    fn grep_json_carries_no_keyword_total() {
        let out = format_grep_meetings(&[shaped()], "migration", 12, 10);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("keyword_total").is_none());
    }

    #[test]
    fn search_json_carries_keyword_total_not_total_meetings() {
        // The ranked list is a pooled best-k, so no total is claimed; the
        // uncapped FTS count backs the grep cross-link instead.
        let out = format_search_meetings(&[shaped()], "migration", 312, 10);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["query"], "migration");
        assert_eq!(v["keyword_total"], 312);
        assert_eq!(v["returned"], 1);
        assert_eq!(v["limit"], 10);
        assert!(v.get("total_meetings").is_none());
        assert_eq!(v["meetings"][0]["id"], "doc-1");
    }

    #[test]
    fn search_json_omits_score_when_rerank_skipped() {
        let mut m = shaped();
        m.score = None;
        let out = format_search_meetings(&[m], "q", 1, 0);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["meetings"][0].get("score").is_none());
    }
}
