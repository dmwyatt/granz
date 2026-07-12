//! Shaping search results for display: per-meeting cards with match evidence.
//!
//! A shaped result is one meeting with one or more evidence snippets showing
//! why it matched. This module holds the display-agnostic pieces: the card
//! data types and the pure text machinery that windows a snippet around the
//! first query-token match and locates highlight spans. Rendering lives in
//! `output::card`; evidence collection against the database lives in
//! `query::evidence`.

use crate::query::fts::FtsToken;

/// Where a piece of match evidence came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    Transcript,
    /// A Granola AI-notes panel section.
    Panel,
    /// The user's own notes.
    Notes,
}

/// A snippet of text with the query-term spans to emphasize.
/// Highlight offsets are character ranges `[start, end)` into `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    pub text: String,
    pub highlights: Vec<(usize, usize)>,
}

/// One place a meeting matched the query.
#[derive(Debug, Clone)]
pub struct MatchEvidence {
    pub source: EvidenceSource,
    pub excerpt: Excerpt,
    /// Raw utterance source for transcripts (`"microphone"`/`"system"`).
    pub speaker: Option<String>,
    /// ISO start timestamp for transcript evidence.
    pub timestamp: Option<String>,
    /// Section heading for panel evidence.
    pub section: Option<String>,
    /// Neighboring units before the match, oldest first (`--context`).
    pub context_before: Vec<ContextUnit>,
    /// Neighboring units after the match (`--context`).
    pub context_after: Vec<ContextUnit>,
}

/// A neighboring content unit shown around a match under `--context`: an
/// utterance for transcript evidence, a section for panel evidence, a
/// paragraph for notes evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextUnit {
    pub text: String,
    /// Raw utterance source for transcript neighbors.
    pub speaker: Option<String>,
    /// ISO start timestamp for transcript neighbors.
    pub timestamp: Option<String>,
    /// Section heading for panel neighbors.
    pub section: Option<String>,
}

/// Which retrieval signals surfaced a meeting.
#[derive(Debug, Clone, Copy, Default)]
pub struct Signals {
    pub keyword: bool,
    pub semantic: bool,
    pub title: bool,
}

/// One meeting shaped for display: identity, ranking facts, and evidence.
/// An empty `matches` with `signals.title` set renders as a title-only match.
#[derive(Debug, Clone)]
pub struct ShapedMeeting {
    pub document_id: String,
    pub title: Option<String>,
    pub created_at: Option<String>,
    /// Cross-encoder relevance when the rerank stage ran.
    pub score: Option<f32>,
    pub signals: Signals,
    /// Total match sites in the meeting's content, independent of how many
    /// are shown.
    pub total_matches: usize,
    pub matches: Vec<MatchEvidence>,
    /// Sources of the match sites beyond `matches`, deduped in display
    /// order, for the "+N more matches in …" collapse line.
    pub remaining_sources: Vec<EvidenceSource>,
}

/// Window `text` around the first query-token match.
///
/// Whitespace is normalized (all runs collapse to single spaces), the window
/// is truncated to roughly `max_chars` characters on word boundaries, and a
/// `…` marks each truncated edge. Every token occurrence inside the window is
/// reported as a highlight span. When no token occurs in the text (semantic
/// evidence), the window starts at the beginning and highlights are empty.
pub fn excerpt_around_match(text: &str, tokens: &[FtsToken], max_chars: usize) -> Excerpt {
    let normalized = normalize_whitespace(text);
    let chars: Vec<char> = normalized.chars().collect();

    let first_match = tokens
        .iter()
        .filter_map(|t| find_ignore_case(&chars, t.text()).first().copied())
        .min_by_key(|&(start, _)| start);

    let (window_start, window_end, leading, trailing) =
        window_bounds(&chars, first_match, max_chars);

    let mut text: String = String::new();
    if leading {
        text.push('…');
    }
    let body_offset = text.chars().count();
    text.extend(&chars[window_start..window_end]);
    if trailing {
        text.push('…');
    }

    let window_chars = &chars[window_start..window_end];
    let mut highlights = Vec::new();
    for token in tokens {
        for (start, end) in find_ignore_case(window_chars, token.text()) {
            highlights.push((start + body_offset, end + body_offset));
        }
    }
    highlights.sort_unstable();
    highlights.dedup();

    Excerpt { text, highlights }
}

/// Collapse all whitespace runs (including newlines) to single spaces, the
/// single-line form every snippet and context unit renders in.
pub fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// True when every query token appears in the title (the FTS title-tier
/// match criterion, mirrored for display).
pub fn title_matches(title: &str, tokens: &[FtsToken]) -> bool {
    !tokens.is_empty() && crate::query::fts::matches_all_tokens(title, tokens)
}

/// Choose window bounds around `matched` (a char span) in `chars`, capped at
/// `max_chars` characters and snapped outward-in to word boundaries. Returns
/// `(start, end, leading_ellipsis, trailing_ellipsis)`.
fn window_bounds(
    chars: &[char],
    matched: Option<(usize, usize)>,
    max_chars: usize,
) -> (usize, usize, bool, bool) {
    let len = chars.len();
    if len <= max_chars {
        return (0, len, false, false);
    }

    // Place the match about a third of the way into the window so trailing
    // context (usually the more informative side) gets the larger share.
    let ideal_start = match matched {
        Some((match_start, _)) => match_start.saturating_sub(max_chars / 3),
        None => 0,
    };
    let mut start = ideal_start.min(len - max_chars);
    let mut end = start + max_chars;

    if start > 0 {
        // Snap forward to the char after the next space so the window opens
        // on a whole word, but never past the match itself.
        let limit = matched.map(|(s, _)| s).unwrap_or(end);
        if let Some(pos) = (start..limit.min(end)).find(|&i| chars[i] == ' ') {
            start = pos + 1;
        }
    }
    if end < len {
        // Snap back to the last space so the window closes on a whole word,
        // but never before the end of the match.
        let floor = matched.map(|(_, e)| e).unwrap_or(start).max(start + 1);
        if let Some(pos) = (floor..end).rev().find(|&i| chars[i] == ' ') {
            end = pos;
        }
    }

    (start, end, start > 0, end < len)
}

/// Find every case-insensitive occurrence of `needle` in `haystack`,
/// returned as char ranges `[start, end)` into `haystack`.
fn find_ignore_case(haystack: &[char], needle: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    if needle.is_empty() || haystack.is_empty() {
        return Vec::new();
    }

    // Lowercase per char, remembering each lowered char's original index.
    // One original char may lower to several chars (e.g. 'İ'), so spans are
    // computed through the mapping rather than by position arithmetic.
    let mut lowered: Vec<char> = Vec::with_capacity(haystack.len());
    let mut origin: Vec<usize> = Vec::with_capacity(haystack.len());
    for (i, &c) in haystack.iter().enumerate() {
        for lc in c.to_lowercase() {
            lowered.push(lc);
            origin.push(i);
        }
    }

    let mut spans = Vec::new();
    let mut i = 0;
    while i + needle.len() <= lowered.len() {
        if lowered[i..i + needle.len()] == needle[..] {
            let start = origin[i];
            let end = origin[i + needle.len() - 1] + 1;
            spans.push((start, end));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::fts::parse_query;

    fn excerpt(text: &str, query: &str, max: usize) -> Excerpt {
        excerpt_around_match(text, &parse_query(query), max)
    }

    fn highlighted<'a>(e: &'a Excerpt) -> Vec<&'a str> {
        let chars: Vec<char> = e.text.chars().collect();
        e.highlights
            .iter()
            .map(|&(s, t)| chars[s..t].iter().collect::<String>())
            .map(|s| Box::leak(s.into_boxed_str()) as &str)
            .collect()
    }

    // --- excerpt_around_match ---

    #[test]
    fn short_text_passes_through_whole() {
        let e = excerpt("the quick brown fox", "fox", 80);
        assert_eq!(e.text, "the quick brown fox");
        assert_eq!(highlighted(&e), vec!["fox"]);
    }

    #[test]
    fn whitespace_is_normalized() {
        let e = excerpt("line one\n\nline   two", "two", 80);
        assert_eq!(e.text, "line one line two");
    }

    #[test]
    fn long_text_windows_around_the_match() {
        let filler = "alpha beta gamma delta ".repeat(20);
        let text = format!("{filler}needle in the haystack {filler}");
        let e = excerpt(&text, "needle", 60);
        assert!(e.text.starts_with('…'), "leading ellipsis: {}", e.text);
        assert!(e.text.ends_with('…'), "trailing ellipsis: {}", e.text);
        assert!(e.text.contains("needle in the haystack"));
        assert!(e.text.chars().count() <= 62); // window + two ellipses
        assert_eq!(highlighted(&e), vec!["needle"]);
    }

    #[test]
    fn match_at_start_has_no_leading_ellipsis() {
        let text = format!("needle first {}", "word ".repeat(50));
        let e = excerpt(&text, "needle", 40);
        assert!(e.text.starts_with("needle"));
        assert!(e.text.ends_with('…'));
    }

    #[test]
    fn window_opens_and_closes_on_word_boundaries() {
        let filler = "abcdefgh ".repeat(30);
        let text = format!("{filler}needle {filler}");
        let e = excerpt(&text, "needle", 50);
        let inner = e.text.trim_matches('…');
        assert!(!inner.starts_with(' ') && !inner.ends_with(' '));
        // No partial filler word at the edges.
        for word in inner.split(' ') {
            assert!(
                word == "needle" || word == "abcdefgh",
                "partial word at window edge: {word:?} in {:?}",
                e.text
            );
        }
    }

    #[test]
    fn no_token_in_text_windows_from_start_with_no_highlights() {
        let text = "completely unrelated content ".repeat(10);
        let e = excerpt(&text, "needle", 40);
        assert!(e.text.starts_with("completely"));
        assert!(e.text.ends_with('…'));
        assert!(e.highlights.is_empty());
    }

    #[test]
    fn all_token_occurrences_in_window_are_highlighted() {
        let e = excerpt("rust code and rust tests", "rust", 80);
        assert_eq!(highlighted(&e), vec!["rust", "rust"]);
        assert_eq!(e.highlights, vec![(0, 4), (14, 18)]);
    }

    #[test]
    fn multiple_tokens_all_highlight() {
        let e = excerpt("the database migration ran fine", "database migration", 80);
        assert_eq!(highlighted(&e), vec!["database", "migration"]);
    }

    #[test]
    fn phrase_token_highlights_as_one_span() {
        let e = excerpt("the database migration ran fine", "\"database migration\"", 80);
        assert_eq!(highlighted(&e), vec!["database migration"]);
    }

    #[test]
    fn highlight_is_case_insensitive_and_preserves_original_case() {
        let e = excerpt("Database Migration plan", "database", 80);
        assert_eq!(highlighted(&e), vec!["Database"]);
    }

    #[test]
    fn earliest_token_occurrence_anchors_the_window() {
        let filler = "filler ".repeat(40);
        let text = format!("alpha here {filler} beta here");
        let e = excerpt(&text, "beta alpha", 30);
        // "alpha" occurs first, so the window anchors there.
        assert!(e.text.contains("alpha"), "window: {:?}", e.text);
        assert!(!e.text.contains("beta"));
    }

    #[test]
    fn unicode_text_windows_without_panicking() {
        let text = "café münchen straße ".repeat(20) + "needle";
        let e = excerpt(&text, "needle", 40);
        assert!(e.text.contains("needle"));
        assert_eq!(highlighted(&e), vec!["needle"]);
    }

    #[test]
    fn empty_query_yields_no_highlights() {
        let e = excerpt("some text", "", 80);
        assert_eq!(e.text, "some text");
        assert!(e.highlights.is_empty());
    }

    #[test]
    fn empty_text_yields_empty_excerpt() {
        let e = excerpt("", "needle", 80);
        assert_eq!(e.text, "");
        assert!(e.highlights.is_empty());
    }

    // --- title_matches ---

    #[test]
    fn title_matches_when_all_tokens_present() {
        let tokens = parse_query("infra sync");
        assert!(title_matches("Weekly Infra Sync", &tokens));
    }

    #[test]
    fn title_does_not_match_on_partial_tokens() {
        let tokens = parse_query("infra budget");
        assert!(!title_matches("Weekly Infra Sync", &tokens));
    }

    #[test]
    fn empty_query_never_title_matches() {
        assert!(!title_matches("Weekly Infra Sync", &[]));
    }

    // --- find_ignore_case (via excerpt highlights) ---

    #[test]
    fn overlapping_occurrences_do_not_overlap_spans() {
        let e = excerpt("aaaa", "aa", 80);
        assert_eq!(e.highlights, vec![(0, 2), (2, 4)]);
    }
}
