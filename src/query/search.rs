use crate::models::TranscriptUtterance;

// Re-export from text module for backwards compatibility
pub use crate::query::text::contains_ignore_case;

/// A matched utterance with its surrounding context.
#[derive(Debug, Clone)]
pub struct ContextWindow {
    pub before: Vec<TranscriptUtterance>,
    pub matched: TranscriptUtterance,
    pub after: Vec<TranscriptUtterance>,
}

/// A labeled segment of text (e.g., a markdown section or paragraph).
#[derive(Debug, Clone)]
pub struct TextSegment {
    /// Optional label (e.g., section heading). `None` for paragraphs or preamble.
    pub label: Option<String>,
    /// The text content of the segment.
    pub text: String,
}

/// A matched text segment with its surrounding context segments.
#[derive(Debug, Clone)]
pub struct TextContextWindow {
    pub before: Vec<TextSegment>,
    pub matched: TextSegment,
    pub after: Vec<TextSegment>,
}

/// Build context windows for text segments that match the query.
///
/// For each segment whose text contains the query (case-insensitive),
/// includes `context_size` segments before and after as context.
pub fn build_text_context_windows(
    segments: &[TextSegment],
    query: &str,
    context_size: usize,
) -> Vec<TextContextWindow> {
    let mut results = Vec::new();

    for (i, seg) in segments.iter().enumerate() {
        if !contains_ignore_case(&seg.text, query) {
            continue;
        }

        let before_start = i.saturating_sub(context_size);
        let after_end = (i + 1 + context_size).min(segments.len());

        let before: Vec<TextSegment> = segments[before_start..i].to_vec();
        let matched = seg.clone();
        let after: Vec<TextSegment> = segments[i + 1..after_end].to_vec();

        results.push(TextContextWindow {
            before,
            matched,
            after,
        });
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_segment(label: Option<&str>, text: &str) -> TextSegment {
        TextSegment {
            label: label.map(String::from),
            text: text.to_string(),
        }
    }

    #[test]
    fn build_text_context_windows_basic_match() {
        let segments = vec![
            make_segment(Some("Intro"), "Welcome to the meeting"),
            make_segment(Some("Action Items"), "Review the roadmap"),
            make_segment(Some("Next Steps"), "Schedule follow-up"),
        ];
        let windows = build_text_context_windows(&segments, "roadmap", 1);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].matched.text, "Review the roadmap");
        assert_eq!(windows[0].before.len(), 1);
        assert_eq!(windows[0].after.len(), 1);
    }

    #[test]
    fn build_text_context_windows_no_match() {
        let segments = vec![
            make_segment(None, "First paragraph"),
            make_segment(None, "Second paragraph"),
        ];
        let windows = build_text_context_windows(&segments, "nonexistent", 1);
        assert!(windows.is_empty());
    }

    #[test]
    fn build_text_context_windows_match_at_start() {
        let segments = vec![
            make_segment(None, "Target text here"),
            make_segment(None, "Second"),
            make_segment(None, "Third"),
        ];
        let windows = build_text_context_windows(&segments, "target", 2);
        assert_eq!(windows.len(), 1);
        assert!(windows[0].before.is_empty());
        assert_eq!(windows[0].after.len(), 2);
    }

    #[test]
    fn build_text_context_windows_match_at_end() {
        let segments = vec![
            make_segment(None, "First"),
            make_segment(None, "Second"),
            make_segment(None, "Target text here"),
        ];
        let windows = build_text_context_windows(&segments, "target", 2);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].before.len(), 2);
        assert!(windows[0].after.is_empty());
    }

    #[test]
    fn build_text_context_windows_case_insensitive() {
        let segments = vec![make_segment(None, "UPPERCASE match")];
        let windows = build_text_context_windows(&segments, "uppercase", 0);
        assert_eq!(windows.len(), 1);
    }

    #[test]
    fn build_text_context_windows_multiple_matches() {
        let segments = vec![
            make_segment(None, "First target"),
            make_segment(None, "Middle"),
            make_segment(None, "Second target"),
        ];
        let windows = build_text_context_windows(&segments, "target", 0);
        assert_eq!(windows.len(), 2);
    }

    #[test]
    fn build_text_context_windows_empty_segments() {
        let windows = build_text_context_windows(&[], "query", 1);
        assert!(windows.is_empty());
    }
}
