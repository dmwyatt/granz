use chrono::FixedOffset;
use colored::Colorize;

use crate::models::{
    Calendar, CalendarEvent, Document, PanelTemplate, Person, Recipe, TranscriptUtterance,
};

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
    let id = person
        .id
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(8)
        .collect::<String>()
        .dimmed()
        .to_string();
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

    format!("{} {} {}{}", id, name, email, company)
}

/// Format a calendar for TTY list display.
pub fn format_calendar_row(cal: &Calendar) -> String {
    let id = cal
        .id
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(8)
        .collect::<String>()
        .dimmed()
        .to_string();
    let summary = cal.summary.as_deref().unwrap_or("(unnamed)");
    let primary = if cal.primary == Some(true) {
        " ★".yellow().to_string()
    } else {
        String::new()
    };
    format!("{} {}{}", id, summary, primary)
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
    let id = tmpl
        .id
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(8)
        .collect::<String>()
        .dimmed()
        .to_string();
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

    format!("{} {} {}{}", id, symbol, title, category)
}

/// Format a recipe for TTY list display.
pub fn format_recipe_row(recipe: &Recipe) -> String {
    let id = recipe
        .id
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(8)
        .collect::<String>()
        .dimmed()
        .to_string();
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

    format!("{} {}{}", id, name, visibility)
}

pub(super) fn format_date_short(s: &str, tz: &FixedOffset) -> String {
    // Try to parse and format nicely, fallback to raw string
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        dt.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string()
    } else if s.len() >= 10 {
        s[..10].to_string()
    } else {
        s.to_string()
    }
}

pub(super) fn format_time_only(s: &str, tz: &FixedOffset) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        dt.with_timezone(tz).format("%H:%M:%S").to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn strip(s: &str) -> String {
        strip_ansi_escapes::strip_str(s)
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
