use chrono::FixedOffset;
use colored::Colorize;
use rusqlite::Connection;

use crate::embed::search::SemanticSearchResult;
use crate::models::{
    Calendar, CalendarEvent, Document, PanelTemplate, Person, Recipe, TranscriptUtterance,
};
use crate::query::search::ContextWindow;

/// Format a meeting list entry for TTY display.
pub fn format_meeting_row(doc: &Document, tz: &FixedOffset) -> String {
    let title = doc
        .title
        .as_deref()
        .unwrap_or("(untitled)")
        .bold()
        .to_string();
    let date = doc
        .created_at
        .as_deref()
        .map(|d| format_date_short(d, tz))
        .unwrap_or_default()
        .dimmed()
        .to_string();
    let id = doc
        .id
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(8)
        .collect::<String>()
        .dimmed()
        .to_string();

    format!("{} {} {}", id, date, title)
}

/// Format a meeting detail view for TTY.
pub fn format_meeting_detail(doc: &Document, tz: &FixedOffset) -> String {
    let mut lines = Vec::new();

    let title = doc.title.as_deref().unwrap_or("(untitled)");
    lines.push(title.bold().to_string());
    lines.push("─".repeat(title.len()));

    if let Some(date) = &doc.created_at {
        lines.push(format!("{} {}", "Date:".dimmed(), format_date_short(date, tz)));
    }
    if let Some(id) = &doc.id {
        lines.push(format!("{}   {}", "ID:".dimmed(), id));
    }

    if let Some(people) = &doc.people {
        let attendees = people.attendees.as_deref().unwrap_or(&[]);
        if !attendees.is_empty() {
            lines.push(String::new());
            lines.push("Attendees:".dimmed().to_string());
            for a in attendees {
                let name = a.full_name().unwrap_or("(unknown)");
                let email = a
                    .email
                    .as_deref()
                    .map(|e| format!(" <{}>", e))
                    .unwrap_or_default();
                lines.push(format!("  {} {}{}", "•".dimmed(), name, email.dimmed()));
            }
        }
    }

    if let Some(summary) = &doc.summary {
        lines.push(String::new());
        lines.push("Summary:".dimmed().to_string());
        lines.push(format!("  {}", summary));
    }

    if let Some(notes) = &doc.notes_plain {
        if !notes.is_empty() {
            lines.push(String::new());
            lines.push("Notes:".dimmed().to_string());
            for line in notes.lines().take(20) {
                lines.push(format!("  {}", line));
            }
        }
    }

    lines.join("\n")
}

/// Format a transcript context window for TTY.
pub fn format_context_window(window: &ContextWindow, doc_title: Option<&str>, tz: &FixedOffset) -> String {
    let mut lines = Vec::new();

    if let Some(title) = doc_title {
        lines.push(format!("{} {}", "Meeting:".dimmed(), title.bold()));
    }

    for utt in &window.before {
        lines.push(format_utterance(utt, false, tz));
    }
    lines.push(format_utterance(&window.matched, true, tz));
    for utt in &window.after {
        lines.push(format_utterance(utt, false, tz));
    }

    lines.join("\n")
}

pub fn format_utterance(utt: &TranscriptUtterance, highlight: bool, tz: &FixedOffset) -> String {
    let timestamp = utt
        .start_timestamp
        .as_deref()
        .map(|s| format_time_only(s, tz))
        .unwrap_or_default();
    let text = utt.text.as_deref().unwrap_or("");

    let speaker_prefix = match utt.source.as_deref() {
        Some("microphone") => format!("{} ", "You:".cyan()),
        Some("system") => format!("{} ", "Other:".dimmed()),
        _ => String::new(),
    };

    if highlight {
        format!("  {} {} {}{}", "▶".green(), timestamp.dimmed(), speaker_prefix, text.bold())
    } else {
        let styled_text = match utt.source.as_deref() {
            Some("microphone") => text.cyan().to_string(),
            _ => text.to_string(),
        };
        format!("  {} {} {}{}", " ", timestamp.dimmed(), speaker_prefix, styled_text)
    }
}

/// Format a person for TTY list display.
pub fn format_person_row(person: &Person) -> String {
    let name = person
        .name
        .as_deref()
        .unwrap_or("(unknown)")
        .bold()
        .to_string();
    let email = person
        .email
        .as_deref()
        .unwrap_or("")
        .dimmed()
        .to_string();
    let company = person
        .company_name
        .as_deref()
        .map(|c| format!(" ({})", c))
        .unwrap_or_default()
        .dimmed()
        .to_string();

    format!("{} {}{}", name, email, company)
}

/// Format a calendar for TTY list display.
pub fn format_calendar_row(cal: &Calendar) -> String {
    let summary = cal.summary.as_deref().unwrap_or("(unnamed)");
    let primary = if cal.primary == Some(true) {
        " ★".yellow().to_string()
    } else {
        String::new()
    };
    format!("{}{}", summary, primary)
}

/// Format an event for TTY display.
pub fn format_event_row(event: &CalendarEvent, tz: &FixedOffset) -> String {
    let summary = event
        .summary
        .as_deref()
        .unwrap_or("(no title)")
        .bold()
        .to_string();
    let start = event
        .start
        .as_ref()
        .and_then(|s| s.date_time.as_deref())
        .map(|d| format_date_short(d, tz))
        .unwrap_or_default()
        .dimmed()
        .to_string();

    format!("{} {}", start, summary)
}

/// Format a template for TTY list display.
pub fn format_template_row(tmpl: &PanelTemplate) -> String {
    let title = tmpl
        .title
        .as_deref()
        .unwrap_or("(untitled)")
        .bold()
        .to_string();
    let category = tmpl
        .category
        .as_deref()
        .map(|c| format!(" [{}]", c))
        .unwrap_or_default()
        .dimmed()
        .to_string();
    let symbol = tmpl.symbol.as_deref().unwrap_or("");

    format!("{} {}{}", symbol, title, category)
}

/// Format a recipe for TTY list display.
pub fn format_recipe_row(recipe: &Recipe) -> String {
    let name = recipe
        .config
        .as_ref()
        .and_then(|c| c.description.as_deref())
        .or(recipe.slug.as_deref())
        .unwrap_or("(unnamed)")
        .bold()
        .to_string();
    let visibility = recipe
        .visibility
        .as_deref()
        .map(|v| format!(" [{}]", v))
        .unwrap_or_default()
        .dimmed()
        .to_string();

    format!("{}{}", name, visibility)
}

/// Fixed content width for search separator dash-fill calculation.
const SEPARATOR_WIDTH: usize = 60;

/// Format a search result separator with index, total, and title.
///
/// Produces output like: `── [1/3] Team Standup ──────────────────────`
/// When score is provided: `── [1/3] Team Standup (0.85) ───────────────`
pub fn format_search_separator(index: usize, total: usize, title: &str, score: Option<f32>) -> String {
    let counter = format!("[{}/{}]", index, total);
    let score_part = match score {
        Some(s) => format!(" ({:.2})", s),
        None => String::new(),
    };

    // "── " is 3 display columns (each ─ is 1 column), then counter, space, title, score, space
    let prefix_cols = 3; // "── " = 2 dashes + 1 space
    let content_cols = prefix_cols + counter.len() + 1 + title.len() + score_part.len() + 1;
    let trail_dashes = if content_cols < SEPARATOR_WIDTH {
        SEPARATOR_WIDTH - content_cols
    } else {
        3
    };

    format!(
        "{}{} {}{} {}",
        "── ".cyan(),
        counter.cyan().bold(),
        title.bold(),
        score_part,
        "─".repeat(trail_dashes).cyan()
    )
}

/// Format a semantic search result for TTY display.
pub fn format_semantic_result(result: &SemanticSearchResult, conn: &Connection, tz: &FixedOffset) -> String {
    let score_str = format!("{:.2}", result.score);
    let score_colored = if result.score > 0.8 {
        score_str.green().bold().to_string()
    } else if result.score > 0.6 {
        score_str.yellow().to_string()
    } else {
        score_str.dimmed().to_string()
    };

    let (title, date) = lookup_document_meta(conn, &result.document_id, tz);
    let title_str = title.bold().to_string();
    let date_str = date.dimmed().to_string();
    let id_prefix_len = 8.min(result.document_id.len());
    let id_str = format!("[{}]", &result.document_id[..id_prefix_len]).dimmed().to_string();

    let snippet = truncate_text(&result.matched_text, 80);
    let snippet_line = format!("      {} \"{}\"", "\u{21b3}".dimmed(), snippet.dimmed());

    let context_line = result.match_context.as_ref().map(|ctx| {
        format!("      {} {}", "*".dimmed(), ctx.dimmed())
    });

    match context_line {
        Some(ctx) => format!("{} {} {} {}\n{}\n{}", score_colored, id_str, date_str, title_str, snippet_line, ctx),
        None => format!("{} {} {} {}\n{}", score_colored, id_str, date_str, title_str, snippet_line),
    }
}

/// Format a context window with a score header for semantic search results.
pub fn format_context_window_with_score(
    window: &ContextWindow,
    doc_title: Option<&str>,
    score: f32,
    tz: &FixedOffset,
) -> String {
    let mut lines = Vec::new();

    // Header with meeting title and score (skipped when title is None,
    // since the caller uses format_search_separator instead)
    if let Some(title) = doc_title {
        let score_str = format!("({:.2})", score);
        let score_colored = if score > 0.8 {
            score_str.green().bold().to_string()
        } else if score > 0.6 {
            score_str.yellow().to_string()
        } else {
            score_str.dimmed().to_string()
        };
        lines.push(format!(
            "{} {}  {}",
            "Meeting:".dimmed(),
            title.bold(),
            score_colored
        ));
    }

    // Context utterances
    for utt in &window.before {
        lines.push(format_utterance(utt, false, tz));
    }
    lines.push(format_utterance(&window.matched, true, tz));
    for utt in &window.after {
        lines.push(format_utterance(utt, false, tz));
    }

    lines.join("\n")
}

/// Format a text-based context window (panels/notes) for TTY.
pub fn format_text_context_window(
    window: &crate::query::search::TextContextWindow,
    doc_title: Option<&str>,
) -> String {
    let mut lines = Vec::new();

    if let Some(title) = doc_title {
        lines.push(format!("{} {}", "Meeting:".dimmed(), title.bold()));
    }

    for seg in &window.before {
        lines.push(format_text_segment(seg, false));
    }
    lines.push(format_text_segment(&window.matched, true));
    for seg in &window.after {
        lines.push(format_text_segment(seg, false));
    }

    lines.join("\n")
}

fn format_text_segment(seg: &crate::query::search::TextSegment, highlight: bool) -> String {
    let mut lines = Vec::new();

    // Section title: ▶ goes here for highlighted segments
    if let Some(label) = seg.label.as_deref() {
        let bracket_label = format!("[{}]", label);
        if highlight {
            lines.push(format!("  {} {}", "▶".green(), bracket_label.bold()));
        } else {
            lines.push(format!("    {}", bracket_label.dimmed()));
        }
    }

    // Content lines: uniform indent, ▶ on first line only when no label
    let mut first = true;
    for line in seg.text.lines() {
        if first && highlight && seg.label.is_none() {
            lines.push(format!("  {} {}", "▶".green(), line.bold()));
        } else {
            lines.push(format!("    {}", line));
        }
        first = false;
    }

    lines.join("\n")
}

fn lookup_document_meta(conn: &Connection, doc_id: &str, tz: &FixedOffset) -> (String, String) {
    let result: Option<(String, String)> = conn
        .query_row(
            "SELECT COALESCE(title, '(untitled)'), COALESCE(created_at, '') FROM documents WHERE id = ?1",
            [doc_id],
            |row| {
                let title: String = row.get(0)?;
                let date: String = row.get(1)?;
                Ok((title, format_date_short(&date, tz)))
            },
        )
        .ok();

    result.unwrap_or_else(|| ("(unknown)".to_string(), String::new()))
}

fn truncate_text(text: &str, max_len: usize) -> String {
    // Collapse newlines and whitespace
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_len {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

fn format_date_short(s: &str, tz: &FixedOffset) -> String {
    // Try to parse and format nicely, fallback to raw string
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        dt.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string()
    } else if s.len() >= 10 {
        s[..10].to_string()
    } else {
        s.to_string()
    }
}

fn format_time_only(s: &str, tz: &FixedOffset) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        dt.with_timezone(tz).format("%H:%M:%S").to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{build_test_db, meetings_state};
    use crate::embed::search::SemanticSearchResult;
    use crate::query::search::TextSegment;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn format_search_separator_contains_counter_and_title() {
        let output = format_search_separator(1, 3, "Team Standup", None);
        let plain = strip_ansi_escapes::strip_str(&output);
        assert!(plain.contains("[1/3]"), "Expected [1/3] counter, got: {}", plain);
        assert!(plain.contains("Team Standup"), "Expected title, got: {}", plain);
    }

    #[test]
    fn format_search_separator_with_score_includes_score() {
        let output = format_search_separator(2, 5, "Project Kickoff", Some(0.85));
        let plain = strip_ansi_escapes::strip_str(&output);
        assert!(plain.contains("[2/5]"), "Expected [2/5] counter, got: {}", plain);
        assert!(plain.contains("Project Kickoff"), "Expected title, got: {}", plain);
        assert!(plain.contains("(0.85)"), "Expected score, got: {}", plain);
    }

    #[test]
    fn format_search_separator_pads_with_trailing_dashes() {
        let output = format_search_separator(1, 1, "Short", None);
        let plain = strip_ansi_escapes::strip_str(&output);
        // Should have trailing ─ chars to fill to SEPARATOR_WIDTH
        let dash_count = plain.chars().filter(|c| *c == '─').count();
        assert!(dash_count > 5, "Expected trailing dashes, got {} dashes in: {}", dash_count, plain);
    }

    #[test]
    fn format_search_separator_handles_long_title() {
        let long_title = "A".repeat(100);
        let output = format_search_separator(1, 1, &long_title, None);
        let plain = strip_ansi_escapes::strip_str(&output);
        // Should still have at least 3 trailing dashes
        assert!(plain.contains(&long_title), "Expected long title, got: {}", plain);
        // Count trailing dashes (not counting the leading ──)
        let trailing = plain.rsplit("── ").next().unwrap_or("");
        let trailing_dashes: String = trailing.chars().filter(|c| *c == '─').collect();
        assert!(trailing_dashes.len() >= 3, "Expected at least 3 trailing dashes for long title");
    }

    #[test]
    fn format_semantic_result_includes_document_id_prefix() {
        let conn = build_test_db(&meetings_state());
        let result = SemanticSearchResult {
            document_id: "doc-1".to_string(),
            score: 0.85,
            source_type: "transcript".to_string(),
            matched_text: "Hello everyone".to_string(),
            window_start_idx: None,
            window_end_idx: None,
            match_context: None,
        };

        let output = format_semantic_result(&result, &conn, &utc());
        // Strip ANSI codes for assertion
        let plain = strip_ansi_escapes::strip_str(&output);
        assert!(
            plain.contains("[doc-1]"),
            "Expected output to contain document ID prefix [doc-1], got: {}",
            plain
        );
    }

    #[test]
    fn format_semantic_result_truncates_long_id() {
        let conn = build_test_db(&meetings_state());
        let long_id = "abcdef01-2345-6789-abcd-ef0123456789";
        let result = SemanticSearchResult {
            document_id: long_id.to_string(),
            score: 0.72,
            source_type: "transcript".to_string(),
            matched_text: "Some text".to_string(),
            window_start_idx: None,
            window_end_idx: None,
            match_context: None,
        };

        let output = format_semantic_result(&result, &conn, &utc());
        let plain = strip_ansi_escapes::strip_str(&output);
        assert!(
            plain.contains("[abcdef01]"),
            "Expected output to contain truncated ID prefix [abcdef01], got: {}",
            plain
        );
        assert!(
            !plain.contains(long_id),
            "Full ID should not appear in output"
        );
    }

    fn strip(s: &str) -> String {
        strip_ansi_escapes::strip_str(s)
    }

    #[test]
    fn format_text_segment_highlighted_with_label_puts_marker_on_title() {
        let seg = TextSegment {
            label: Some("Sprint Updates".to_string()),
            text: "- Todd: working on feature\n- Jane: fixing bug".to_string(),
        };
        let output = strip(&format_text_segment(&seg, true));
        let lines: Vec<&str> = output.lines().collect();

        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].contains("▶") && lines[0].contains("[Sprint Updates]"),
            "▶ should be on the section title line, got: {:?}",
            lines[0]
        );
        assert!(
            !lines[1].contains("▶"),
            "Content lines should not have ▶, got: {:?}",
            lines[1]
        );
    }

    #[test]
    fn format_text_segment_highlighted_without_label_puts_marker_on_first_content() {
        let seg = TextSegment {
            label: None,
            text: "- Todd: working on feature\n- Jane: fixing bug".to_string(),
        };
        let output = strip(&format_text_segment(&seg, true));
        let lines: Vec<&str> = output.lines().collect();

        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("▶") && lines[0].contains("Todd"),
            "▶ should be on first content line, got: {:?}",
            lines[0]
        );
        assert!(
            !lines[1].contains("▶"),
            "Second line should not have ▶, got: {:?}",
            lines[1]
        );
    }

    #[test]
    fn format_text_segment_non_highlighted_with_label_no_marker() {
        let seg = TextSegment {
            label: Some("Context Section".to_string()),
            text: "- Some context content".to_string(),
        };
        let output = strip(&format_text_segment(&seg, false));

        assert!(!output.contains("▶"), "Non-highlighted should not have ▶");
        assert!(output.contains("[Context Section]"));
    }

    #[test]
    fn format_text_segment_title_less_indented_than_content() {
        let seg = TextSegment {
            label: Some("Section Title".to_string()),
            text: "- First line\n- Second line".to_string(),
        };
        let output = strip(&format_text_segment(&seg, false));
        let lines: Vec<&str> = output.lines().collect();

        let title_indent = lines[0].len() - lines[0].trim_start().len();
        let content_indent = lines[1].len() - lines[1].trim_start().len();
        assert!(
            title_indent <= content_indent,
            "Title indent ({}) should be <= content indent ({})\nOutput:\n{}",
            title_indent,
            content_indent,
            output
        );
    }

    #[test]
    fn format_utterance_microphone_shows_you_label() {
        let utt = TranscriptUtterance {
            source: Some("microphone".to_string()),
            text: Some("Hello from me".to_string()),
            ..Default::default()
        };
        let output = strip(&format_utterance(&utt, false, &utc()));
        assert!(output.contains("You:"), "Expected 'You:' label, got: {}", output);
        assert!(output.contains("Hello from me"), "Expected text, got: {}", output);
    }

    #[test]
    fn format_utterance_system_shows_other_label() {
        let utt = TranscriptUtterance {
            source: Some("system".to_string()),
            text: Some("Hello from them".to_string()),
            ..Default::default()
        };
        let output = strip(&format_utterance(&utt, false, &utc()));
        assert!(output.contains("Other:"), "Expected 'Other:' label, got: {}", output);
    }

    #[test]
    fn format_utterance_no_source_no_label() {
        let utt = TranscriptUtterance {
            text: Some("No source".to_string()),
            ..Default::default()
        };
        let output = strip(&format_utterance(&utt, false, &utc()));
        assert!(!output.contains("You:"), "Should not have 'You:' label");
        assert!(!output.contains("Other:"), "Should not have 'Other:' label");
    }

    #[test]
    fn format_utterance_highlighted_includes_speaker() {
        let utt = TranscriptUtterance {
            source: Some("microphone".to_string()),
            text: Some("Highlighted text".to_string()),
            ..Default::default()
        };
        let output = strip(&format_utterance(&utt, true, &utc()));
        assert!(output.contains("▶"), "Expected highlight marker");
        assert!(output.contains("You:"), "Expected 'You:' label in highlighted");
    }

    #[test]
    fn format_text_segment_multiline_content_uniformly_indented() {
        let seg = TextSegment {
            label: Some("Section".to_string()),
            text: "- Line one\n- Line two\n- Line three".to_string(),
        };
        let output = strip(&format_text_segment(&seg, true));
        let lines: Vec<&str> = output.lines().collect();

        // All content lines (skip title at index 0) should have same indent
        let indents: Vec<usize> = lines[1..]
            .iter()
            .map(|l| l.len() - l.trim_start().len())
            .collect();
        assert!(
            indents.windows(2).all(|w| w[0] == w[1]),
            "Content lines should have uniform indent, got {:?}\nOutput:\n{}",
            indents,
            output
        );
    }

    // === Timezone-aware display tests ===

    #[test]
    fn format_date_short_converts_to_local_timezone() {
        // UTC midnight → should show as previous day 19:00 in UTC-5
        let utc_minus_5 = FixedOffset::west_opt(5 * 3600).unwrap();
        let result = format_date_short("2026-01-22T00:00:00Z", &utc_minus_5);
        assert_eq!(result, "2026-01-21 19:00");
    }

    #[test]
    fn format_date_short_utc_preserves_original() {
        let result = format_date_short("2026-01-22T14:30:00Z", &utc());
        assert_eq!(result, "2026-01-22 14:30");
    }

    #[test]
    fn format_time_only_converts_to_local_timezone() {
        // UTC 02:00 → should show as 11:00 in UTC+9
        let utc_plus_9 = FixedOffset::east_opt(9 * 3600).unwrap();
        let result = format_time_only("2026-01-22T02:00:00Z", &utc_plus_9);
        assert_eq!(result, "11:00:00");
    }

    #[test]
    fn format_time_only_utc_preserves_original() {
        let result = format_time_only("2026-01-22T14:30:45Z", &utc());
        assert_eq!(result, "14:30:45");
    }

    #[test]
    fn format_date_short_with_positive_offset() {
        // UTC 23:00 → should show as next day 08:00 in UTC+9
        let utc_plus_9 = FixedOffset::east_opt(9 * 3600).unwrap();
        let result = format_date_short("2026-01-22T23:00:00Z", &utc_plus_9);
        assert_eq!(result, "2026-01-23 08:00");
    }
}
