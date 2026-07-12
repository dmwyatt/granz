//! TTY rendering for shaped search results: one card per meeting with the
//! match evidence underneath.
//!
//! Color degrades automatically: `--no-color` disables ANSI globally via
//! `colored::control::set_override`, so the same renderer serves colored
//! and plain output.

use chrono::FixedOffset;
use colored::Colorize;

use crate::query::shape::{ContextUnit, EvidenceSource, Excerpt, MatchEvidence, ShapedMeeting};

/// Card body indent, sized to clear the rank gutter.
const INDENT: &str = "    ";
/// Context-line indent, one step deeper than the snippet it surrounds.
const CONTEXT_INDENT: &str = "      ";
/// Snippet wrap width in characters, excluding the indent.
const SNIPPET_WRAP: usize = 72;

/// Human label for an evidence source.
fn source_label(source: EvidenceSource) -> &'static str {
    match source {
        EvidenceSource::Transcript => "transcript",
        EvidenceSource::Panel => "AI notes",
        EvidenceSource::Notes => "your notes",
    }
}

/// Render one shaped meeting as a card: header line, evidence blocks, and
/// the collapse line when more matches exist than are shown.
pub fn format_shaped_meeting(m: &ShapedMeeting, rank: usize, tz: &FixedOffset) -> String {
    let mut lines = vec![header_line(m, rank, tz)];

    for evidence in &m.matches {
        lines.push(format!("{INDENT}{}", source_line(evidence, tz)));
        for unit in &evidence.context_before {
            lines.push(context_block(unit, tz));
        }
        lines.push(snippet_block(&evidence.excerpt));
        for unit in &evidence.context_after {
            lines.push(context_block(unit, tz));
        }
    }
    if m.matches.is_empty() && m.signals.title {
        lines.push(format!("{INDENT}{}", "title match".dimmed().italic()));
    }
    if let Some(line) = collapse_line(m) {
        lines.push(format!("{INDENT}{}", line.dimmed()));
    }

    lines.join("\n")
}

/// `NN. <id> <date> <title>` — the same column order and styling as the
/// unshaped meeting rows, with a rank gutter in front.
fn header_line(m: &ShapedMeeting, rank: usize, tz: &FixedOffset) -> String {
    let id: String = m.document_id.chars().take(8).collect();
    let date = m
        .created_at
        .as_deref()
        .map(|d| super::table::format_date_short(d, tz))
        .unwrap_or_default();
    let title = m.title.as_deref().unwrap_or("(untitled)");
    format!(
        "{} {} {} {}",
        format!("{rank:>2}.").dimmed(),
        id.dimmed(),
        date.dimmed(),
        title.bold()
    )
}

/// `transcript › 10:14:07 You`, `AI notes › Migration Plan`, `your notes`.
fn source_line(evidence: &MatchEvidence, tz: &FixedOffset) -> String {
    let label = source_label(evidence.source).dimmed();
    let mut details = Vec::new();
    if let Some(ts) = evidence.timestamp.as_deref() {
        details.push(super::table::format_time_only(ts, tz).dimmed().to_string());
    }
    match evidence.speaker.as_deref() {
        Some("microphone") => details.push("You".cyan().to_string()),
        Some("system") => details.push("Other".dimmed().to_string()),
        _ => {}
    }
    if let Some(section) = evidence.section.as_deref() {
        details.push(section.dimmed().to_string());
    }

    if details.is_empty() {
        label.to_string()
    } else {
        format!("{} {} {}", label, "›".dimmed(), details.join(" "))
    }
}

/// The quoted snippet, word-wrapped with a hanging indent, query terms
/// emphasized.
fn snippet_block(excerpt: &Excerpt) -> String {
    let ranges = wrap_ranges(&excerpt.text, SNIPPET_WRAP);
    let last = ranges.len().saturating_sub(1);
    ranges
        .iter()
        .enumerate()
        .map(|(i, &(start, end))| {
            let body = render_span(excerpt, start, end);
            let open = if i == 0 { "\"" } else { " " };
            let close = if i == last { "\"" } else { "" };
            format!("{INDENT}{open}{body}{close}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A dimmed neighboring unit around a match under --context: transcript
/// neighbors carry their time and speaker, panel neighbors their section
/// heading, wrapped like the snippet one indent step deeper.
fn context_block(unit: &ContextUnit, tz: &FixedOffset) -> String {
    let mut head = String::new();
    if let Some(ts) = unit.timestamp.as_deref() {
        head.push_str(&super::table::format_time_only(ts, tz));
        head.push(' ');
    }
    match unit.speaker.as_deref() {
        Some("microphone") => head.push_str("You  "),
        Some("system") => head.push_str("Other  "),
        _ => {}
    }
    if let Some(section) = unit.section.as_deref() {
        head.push_str(section);
        head.push_str(" › ");
    }
    let full = format!("{head}{}", unit.text);
    let chars: Vec<char> = full.chars().collect();
    wrap_ranges(&full, SNIPPET_WRAP)
        .iter()
        .map(|&(start, end)| {
            let line: String = chars[start..end].iter().collect();
            format!("{CONTEXT_INDENT}{}", line.dimmed())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render `excerpt.text[start..end]` (char indices) with the highlight
/// spans that fall inside it emphasized.
fn render_span(excerpt: &Excerpt, start: usize, end: usize) -> String {
    let chars: Vec<char> = excerpt.text.chars().collect();
    let mut out = String::new();
    let mut pos = start;
    for &(hs, he) in &excerpt.highlights {
        let (hs, he) = (hs.max(start), he.min(end));
        if hs >= he || hs < pos {
            continue;
        }
        out.extend(&chars[pos..hs]);
        let term: String = chars[hs..he].iter().collect();
        out.push_str(&term.yellow().bold().to_string());
        pos = he;
    }
    out.extend(&chars[pos..end]);
    out
}

/// Word-wrap `text` at `width` characters, returning char ranges per line.
/// Breaks at spaces (the space is consumed); a single overlong word breaks
/// hard at the width.
fn wrap_ranges(text: &str, width: usize) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut ranges = Vec::new();
    let mut start = 0;
    while chars.len() - start > width {
        let window_end = start + width;
        let break_at = (start..window_end).rev().find(|&i| chars[i] == ' ');
        match break_at {
            Some(space) if space > start => {
                ranges.push((start, space));
                start = space + 1;
            }
            _ => {
                ranges.push((start, window_end));
                start = window_end;
            }
        }
    }
    ranges.push((start, chars.len()));
    ranges
}

/// `+N more match(es) in <sources>` when matches were collapsed.
fn collapse_line(m: &ShapedMeeting) -> Option<String> {
    let hidden = m.total_matches.saturating_sub(m.matches.len());
    if hidden == 0 {
        return None;
    }
    let noun = if hidden == 1 { "match" } else { "matches" };
    let sources = match m
        .remaining_sources
        .iter()
        .map(|&s| source_label(s))
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => String::new(),
        [one] => format!(" in {one}"),
        [a, b] => format!(" in {a} and {b}"),
        [head @ .., last] => format!(" in {}, and {last}", head.join(", ")),
    };
    Some(format!("+{hidden} more {noun}{sources}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::shape::Signals;
    use chrono::FixedOffset;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn strip(s: &str) -> String {
        strip_ansi_escapes::strip_str(s)
    }

    fn base_meeting() -> ShapedMeeting {
        ShapedMeeting {
            document_id: "abcdef01-2345-6789".to_string(),
            title: Some("Weekly Infra Sync".to_string()),
            created_at: Some("2026-05-12T14:30:00Z".to_string()),
            score: Some(0.63),
            signals: Signals { keyword: true, semantic: true, title: false },
            total_matches: 1,
            matches: vec![MatchEvidence {
                source: EvidenceSource::Panel,
                excerpt: Excerpt {
                    text: "created database snapshots before the migration".to_string(),
                    highlights: vec![(8, 16), (38, 47)],
                },
                speaker: None,
                timestamp: None,
                section: Some("Migration Plan".to_string()),
                context_before: Vec::new(),
                context_after: Vec::new(),
            }],
            remaining_sources: Vec::new(),
        }
    }

    #[test]
    fn card_header_carries_rank_id_date_and_title() {
        let out = strip(&format_shaped_meeting(&base_meeting(), 3, &utc()));
        let header = out.lines().next().unwrap();
        assert_eq!(header, " 3. abcdef01 2026-05-12 14:30 Weekly Infra Sync");
    }

    #[test]
    fn card_shows_no_numeric_score() {
        let out = strip(&format_shaped_meeting(&base_meeting(), 1, &utc()));
        assert!(!out.contains("0.63"), "score leaked into card:\n{out}");
    }

    #[test]
    fn panel_evidence_shows_source_and_section() {
        let out = strip(&format_shaped_meeting(&base_meeting(), 1, &utc()));
        assert!(out.contains("    AI notes › Migration Plan"), "got:\n{out}");
    }

    #[test]
    fn snippet_renders_quoted() {
        let out = strip(&format_shaped_meeting(&base_meeting(), 1, &utc()));
        assert!(
            out.contains("    \"created database snapshots before the migration\""),
            "got:\n{out}"
        );
    }

    #[test]
    fn highlights_are_emphasized_in_color_mode() {
        colored::control::set_override(true);
        let m = base_meeting();
        let out = format_shaped_meeting(&m, 1, &utc());
        colored::control::unset_override();
        // The highlighted term carries ANSI styling; its neighbors do not.
        assert!(out.contains("\u{1b}["), "no ANSI emitted:\n{out:?}");
        assert!(strip(&out).contains("database"));
    }

    #[test]
    fn transcript_evidence_shows_time_and_speaker() {
        let mut m = base_meeting();
        m.matches = vec![MatchEvidence {
            source: EvidenceSource::Transcript,
            excerpt: Excerpt { text: "say the migration runs".to_string(), highlights: vec![] },
            speaker: Some("microphone".to_string()),
            timestamp: Some("2026-05-12T14:31:07Z".to_string()),
            section: None,
            context_before: Vec::new(),
            context_after: Vec::new(),
        }];
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(out.contains("    transcript › 14:31:07 You"), "got:\n{out}");
    }

    #[test]
    fn notes_evidence_has_bare_source_line() {
        let mut m = base_meeting();
        m.matches[0].source = EvidenceSource::Notes;
        m.matches[0].section = None;
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(out.contains("    your notes\n"), "got:\n{out}");
    }

    #[test]
    fn title_only_card_says_so() {
        let mut m = base_meeting();
        m.matches.clear();
        m.total_matches = 0;
        m.signals.title = true;
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(out.contains("    title match"), "got:\n{out}");
    }

    #[test]
    fn collapse_line_reports_hidden_matches_and_sources() {
        let mut m = base_meeting();
        m.total_matches = 4;
        m.remaining_sources = vec![EvidenceSource::Notes, EvidenceSource::Transcript];
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(
            out.contains("    +3 more matches in your notes and transcript"),
            "got:\n{out}"
        );
    }

    #[test]
    fn collapse_line_lists_three_sources_with_commas() {
        // --matches 0 collapses everything, so all three sources can appear.
        let mut m = base_meeting();
        m.matches.clear();
        m.total_matches = 5;
        m.remaining_sources = vec![
            EvidenceSource::Panel,
            EvidenceSource::Notes,
            EvidenceSource::Transcript,
        ];
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(
            out.contains("    +5 more matches in AI notes, your notes, and transcript"),
            "got:\n{out}"
        );
    }

    #[test]
    fn collapse_line_singular_without_sources() {
        let mut m = base_meeting();
        m.total_matches = 2;
        m.remaining_sources = vec![EvidenceSource::Panel];
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(out.contains("    +1 more match in AI notes"), "got:\n{out}");
    }

    #[test]
    fn context_units_render_around_the_snippet() {
        let mut m = base_meeting();
        m.matches = vec![MatchEvidence {
            source: EvidenceSource::Transcript,
            excerpt: Excerpt {
                text: "run the migration tonight".to_string(),
                highlights: vec![(8, 17)],
            },
            speaker: Some("microphone".to_string()),
            timestamp: Some("2026-05-12T14:31:07Z".to_string()),
            section: None,
            context_before: vec![ContextUnit {
                text: "are we ready for it".to_string(),
                speaker: Some("system".to_string()),
                timestamp: Some("2026-05-12T14:30:50Z".to_string()),
                section: None,
            }],
            context_after: vec![ContextUnit {
                text: "ok, scheduling it now".to_string(),
                speaker: Some("system".to_string()),
                timestamp: Some("2026-05-12T14:31:20Z".to_string()),
                section: None,
            }],
        }];
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[1], "    transcript › 14:31:07 You");
        assert_eq!(lines[2], "      14:30:50 Other  are we ready for it");
        assert_eq!(lines[3], "    \"run the migration tonight\"");
        assert_eq!(lines[4], "      14:31:20 Other  ok, scheduling it now");
    }

    #[test]
    fn panel_context_units_carry_their_section_heading() {
        let mut m = base_meeting();
        m.matches[0].context_after = vec![ContextUnit {
            text: "follow up next week".to_string(),
            speaker: None,
            timestamp: None,
            section: Some("Action Items".to_string()),
        }];
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(
            out.contains("      Action Items › follow up next week"),
            "got:\n{out}"
        );
    }

    #[test]
    fn long_snippets_wrap_with_hanging_indent() {
        let mut m = base_meeting();
        m.matches[0].excerpt = Excerpt {
            text: "alpha beta gamma delta ".repeat(8).trim_end().to_string(),
            highlights: vec![],
        };
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        let snippet_lines: Vec<&str> =
            out.lines().filter(|l| l.contains("alpha")).collect();
        assert!(snippet_lines.len() > 1, "expected wrapping:\n{out}");
        assert!(snippet_lines[0].starts_with("    \""));
        assert!(snippet_lines[1].starts_with("     "));
        assert!(out.trim_end().ends_with('"'));
        for line in &snippet_lines {
            assert!(line.chars().count() <= 4 + 1 + SNIPPET_WRAP + 1, "overlong: {line}");
        }
    }

    #[test]
    fn highlight_split_across_wrap_boundary_survives() {
        // A highlight that would straddle a wrap point still renders all
        // its characters (styling aside).
        let word = "migration";
        let text = format!("{} {}", "x".repeat(SNIPPET_WRAP - 4), word);
        let start = text.chars().count() - word.len();
        let mut m = base_meeting();
        m.matches[0].excerpt =
            Excerpt { text: text.clone(), highlights: vec![(start, start + word.len())] };
        let out = strip(&format_shaped_meeting(&m, 1, &utc()));
        assert!(out.contains(word), "highlighted word lost:\n{out}");
    }
}
