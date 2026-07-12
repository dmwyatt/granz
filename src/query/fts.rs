//! Shared FTS5 query construction and term matching.
//!
//! A user query is parsed into tokens: bare words become individual terms
//! (implicit AND), and double-quoted spans become phrases that must match
//! contiguously. Every token is emitted double-quoted in the MATCH string,
//! which keeps FTS5 operators (AND, OR, NOT, NEAR, `-`, `*`, `:`) literal.

/// A parsed unit of a user search query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FtsToken {
    /// A single bare word; matches anywhere in the row.
    Term(String),
    /// A user-quoted span; must match contiguously.
    Phrase(String),
}

impl FtsToken {
    /// The token's text, phrase or term alike.
    pub fn text(&self) -> &str {
        match self {
            FtsToken::Term(s) | FtsToken::Phrase(s) => s,
        }
    }
}

/// Parse a raw user query into terms and quoted phrases.
///
/// A `"` starts a phrase (ending any bare word in progress); the phrase runs
/// to the next `"` or the end of the string, so unbalanced quotes are treated
/// as if closed at the end. Empty or whitespace-only phrases are dropped.
pub fn parse_query(query: &str) -> Vec<FtsToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_phrase = false;

    let flush = |buf: &mut String, in_phrase: bool, tokens: &mut Vec<FtsToken>| {
        if !buf.trim().is_empty() {
            let text = std::mem::take(buf);
            tokens.push(if in_phrase {
                FtsToken::Phrase(text)
            } else {
                FtsToken::Term(text)
            });
        } else {
            buf.clear();
        }
    };

    for ch in query.chars() {
        match ch {
            '"' => {
                flush(&mut current, in_phrase, &mut tokens);
                in_phrase = !in_phrase;
            }
            c if c.is_whitespace() && !in_phrase => {
                flush(&mut current, false, &mut tokens);
            }
            c => current.push(c),
        }
    }
    flush(&mut current, in_phrase, &mut tokens);

    tokens
}

/// Build an FTS5 MATCH expression from a user query.
///
/// Each token is double-quoted so FTS5 treats it literally; whitespace-joined
/// quoted tokens give implicit-AND semantics. An empty query yields `""`
/// (an empty phrase), which matches nothing.
pub fn sanitize_fts_query(query: &str) -> String {
    let tokens = parse_query(query);
    if tokens.is_empty() {
        return "\"\"".to_string();
    }
    tokens
        .iter()
        .map(|t| format!("\"{}\"", t.text()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when `text` contains every token, case-insensitively.
///
/// Mirrors the per-row semantics of the MATCH expression built by
/// `sanitize_fts_query`, for filtering display text (e.g. context windows).
/// An empty token list is vacuously true.
pub fn matches_all_tokens(text: &str, tokens: &[FtsToken]) -> bool {
    tokens
        .iter()
        .all(|t| crate::query::text::contains_ignore_case(text, t.text()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_query ---

    #[test]
    fn parse_single_word() {
        assert_eq!(parse_query("hello"), vec![FtsToken::Term("hello".into())]);
    }

    #[test]
    fn parse_multiple_words() {
        assert_eq!(
            parse_query("resource allocation"),
            vec![
                FtsToken::Term("resource".into()),
                FtsToken::Term("allocation".into())
            ]
        );
    }

    #[test]
    fn parse_quoted_phrase() {
        assert_eq!(
            parse_query("\"resource allocation\""),
            vec![FtsToken::Phrase("resource allocation".into())]
        );
    }

    #[test]
    fn parse_phrase_and_terms_mixed() {
        assert_eq!(
            parse_query("budget \"status update\" quarterly"),
            vec![
                FtsToken::Term("budget".into()),
                FtsToken::Phrase("status update".into()),
                FtsToken::Term("quarterly".into())
            ]
        );
    }

    #[test]
    fn parse_unbalanced_quote_runs_to_end() {
        assert_eq!(
            parse_query("foo \"bar baz"),
            vec![
                FtsToken::Term("foo".into()),
                FtsToken::Phrase("bar baz".into())
            ]
        );
    }

    #[test]
    fn parse_quote_attached_to_word() {
        assert_eq!(
            parse_query("foo\"bar baz\""),
            vec![
                FtsToken::Term("foo".into()),
                FtsToken::Phrase("bar baz".into())
            ]
        );
    }

    #[test]
    fn parse_empty_phrase_dropped() {
        assert_eq!(parse_query("foo \"\" bar"), vec![
            FtsToken::Term("foo".into()),
            FtsToken::Term("bar".into())
        ]);
    }

    #[test]
    fn parse_whitespace_only_phrase_dropped() {
        assert_eq!(parse_query("\"   \""), Vec::<FtsToken>::new());
    }

    #[test]
    fn parse_empty_query() {
        assert_eq!(parse_query(""), Vec::<FtsToken>::new());
        assert_eq!(parse_query("   "), Vec::<FtsToken>::new());
    }

    #[test]
    fn parse_keeps_punctuation_in_terms() {
        assert_eq!(
            parse_query("covid-19 v2.0"),
            vec![
                FtsToken::Term("covid-19".into()),
                FtsToken::Term("v2.0".into())
            ]
        );
    }

    // --- sanitize_fts_query ---

    #[test]
    fn sanitize_single_word_is_quoted() {
        assert_eq!(sanitize_fts_query("hello"), "\"hello\"");
    }

    #[test]
    fn sanitize_multi_word_quotes_each_term() {
        // Implicit AND: each word quoted separately, not one phrase.
        assert_eq!(
            sanitize_fts_query("resource allocation"),
            "\"resource\" \"allocation\""
        );
    }

    #[test]
    fn sanitize_preserves_user_phrases() {
        assert_eq!(
            sanitize_fts_query("budget \"status update\""),
            "\"budget\" \"status update\""
        );
    }

    #[test]
    fn sanitize_neutralizes_fts_operators() {
        assert_eq!(sanitize_fts_query("cats OR dogs"), "\"cats\" \"OR\" \"dogs\"");
        assert_eq!(sanitize_fts_query("foo NOT bar"), "\"foo\" \"NOT\" \"bar\"");
        assert_eq!(sanitize_fts_query("col:value"), "\"col:value\"");
        assert_eq!(sanitize_fts_query("wild*"), "\"wild*\"");
    }

    #[test]
    fn sanitize_empty_query_matches_nothing() {
        // Preserves the pre-existing behavior for empty input: an empty
        // phrase, which no row matches.
        assert_eq!(sanitize_fts_query(""), "\"\"");
    }

    // --- matches_all_tokens ---

    #[test]
    fn matches_all_terms_any_order() {
        let tokens = parse_query("allocation resource");
        assert!(matches_all_tokens(
            "We discussed resource allocation today",
            &tokens
        ));
    }

    #[test]
    fn matches_is_case_insensitive() {
        let tokens = parse_query("RESOURCE");
        assert!(matches_all_tokens("resource allocation", &tokens));
    }

    #[test]
    fn missing_term_fails_match() {
        let tokens = parse_query("resource headcount");
        assert!(!matches_all_tokens(
            "We discussed resource allocation today",
            &tokens
        ));
    }

    #[test]
    fn phrase_must_be_contiguous() {
        let tokens = parse_query("\"allocation resource\"");
        assert!(!matches_all_tokens(
            "We discussed resource allocation today",
            &tokens
        ));
        let tokens = parse_query("\"resource allocation\"");
        assert!(matches_all_tokens(
            "We discussed resource allocation today",
            &tokens
        ));
    }

    #[test]
    fn empty_tokens_match_vacuously() {
        assert!(matches_all_tokens("anything", &[]));
    }
}
