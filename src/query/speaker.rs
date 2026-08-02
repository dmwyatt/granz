//! Filtering transcript utterances by who spoke.
//!
//! Granola gives two attribution signals, and they are not the same kind of
//! thing. The audio channel (`microphone`/`system`) is on every utterance and
//! only ever says "you" or "not you". A detected speaker name is on the system
//! channel, only for meetings recorded after 2026-07-21, and is a display name
//! with no stable identifier behind it.
//!
//! That second signal is why `--speaker` takes two passes. What the user types
//! is a [`SpeakerSelector`]; resolving it against the names actually present
//! produces a [`SpeakerFilter`] over concrete names. Resolving up front is what
//! lets a typo be an error instead of an empty result set that reads as
//! "nobody said that".

use anyhow::{Result, bail};
use rusqlite::Connection;

/// How many known speakers an error message lists before it truncates.
const MAX_LISTED_SPEAKERS: usize = 10;

/// What the user asked for on `--speaker`, before it is resolved against the
/// names present in the database.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeakerSelector {
    /// The local user: the microphone channel.
    Me,
    /// Everyone else: the system channel, named or not.
    Other,
    /// A case-insensitive name pattern.
    Name(String),
}

impl SpeakerSelector {
    /// Parse the `--speaker` value. `me` and `other` are reserved words;
    /// anything else is a name pattern. Only an empty pattern is rejected,
    /// since any other string could be somebody's name.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "" => None,
            "me" => Some(SpeakerSelector::Me),
            "other" => Some(SpeakerSelector::Other),
            _ => Some(SpeakerSelector::Name(s.trim().to_string())),
        }
    }
}

/// A resolved speaker filter: what an utterance is actually tested against.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeakerFilter {
    Me,
    Other,
    /// The concrete attributed names the pattern resolved to.
    Names(Vec<String>),
}

impl SpeakerFilter {
    /// Whether an utterance belongs to the filtered speaker.
    ///
    /// `Me` and `Other` test the audio channel alone, so they keep working on
    /// the ~500k utterances that predate attribution. `Names` tests the
    /// detected name, which never matches the microphone channel.
    pub fn matches(&self, source: Option<&str>, speaker_name: Option<&str>) -> bool {
        match self {
            SpeakerFilter::Me => source == Some("microphone"),
            SpeakerFilter::Other => source == Some("system"),
            SpeakerFilter::Names(names) => {
                speaker_name.is_some_and(|name| names.iter().any(|n| n.eq_ignore_ascii_case(name)))
            }
        }
    }
}

/// The names a pattern selects out of those present.
///
/// An exact case-insensitive hit wins outright, so `--speaker "Jane Doe"`
/// selects only her even when "Jane Doe Jr" is also in the corpus. Otherwise
/// every name containing the pattern is selected, because a first name is what
/// people actually type.
pub fn resolve_names(pattern: &str, available: &[String]) -> Vec<String> {
    let needle = pattern.trim().to_lowercase();

    let exact: Vec<String> = available
        .iter()
        .filter(|n| n.to_lowercase() == needle)
        .cloned()
        .collect();
    if !exact.is_empty() {
        return exact;
    }

    available
        .iter()
        .filter(|n| n.to_lowercase().contains(&needle))
        .cloned()
        .collect()
}

/// How to name a speaker in output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerLabel<'a> {
    /// The local user.
    You,
    /// Granola's detected name for the speaker.
    Named(&'a str),
    /// An unattributed remote speaker: everything before the cutover, and the
    /// 9% of system-channel utterances Granola declines to attribute.
    Other,
}

impl SpeakerLabel<'_> {
    pub fn as_str(&self) -> &str {
        match self {
            SpeakerLabel::You => "You",
            SpeakerLabel::Named(name) => name,
            SpeakerLabel::Other => "Other",
        }
    }
}

/// How to name the speaker of an utterance, or `None` when the audio channel
/// is unknown and there is nothing truthful to say.
///
/// A detected name always wins over the channel label. The microphone channel
/// is the local user, so `You` holds even in the impossible case of a name on
/// it.
pub fn label<'a>(source: Option<&str>, speaker_name: Option<&'a str>) -> Option<SpeakerLabel<'a>> {
    match source {
        Some("microphone") => Some(SpeakerLabel::You),
        Some("system") => Some(match speaker_name.filter(|n| !n.trim().is_empty()) {
            Some(name) => SpeakerLabel::Named(name),
            None => SpeakerLabel::Other,
        }),
        _ => None,
    }
}

/// Resolve a selector against the database, reporting what the pattern hit.
///
/// `Me` and `Other` need no lookup. A name pattern that matches nothing is an
/// error rather than an empty result; one that matches several speakers takes
/// all of them and says so on stderr, since the union is usually what was
/// wanted and the note names the string needed to narrow.
pub fn resolve(conn: &Connection, selector: &SpeakerSelector) -> Result<SpeakerFilter> {
    let pattern = match selector {
        SpeakerSelector::Me => return Ok(SpeakerFilter::Me),
        SpeakerSelector::Other => return Ok(SpeakerFilter::Other),
        SpeakerSelector::Name(p) => p,
    };

    let available = crate::db::transcripts::distinct_speaker_names(conn)?;
    let matched = resolve_names(pattern, &available);

    match matched.len() {
        0 => bail!("{}", no_match_message(pattern, &available)),
        1 => Ok(SpeakerFilter::Names(matched)),
        n => {
            eprintln!(
                "[grans] --speaker \"{}\" matched {} speakers: {}",
                pattern,
                n,
                matched.join(", ")
            );
            Ok(SpeakerFilter::Names(matched))
        }
    }
}

/// [`resolve`] over an optional selector, for the `--speaker` flag.
pub fn resolve_opt(
    conn: &Connection,
    selector: Option<&SpeakerSelector>,
) -> Result<Option<SpeakerFilter>> {
    selector.map(|s| resolve(conn, s)).transpose()
}

/// The error for a pattern that matches nobody. A corpus with no attribution
/// at all is a different problem from a misspelled name, so it says so.
fn no_match_message(pattern: &str, available: &[String]) -> String {
    if available.is_empty() {
        return format!(
            "no utterances have speaker attribution, so --speaker \"{pattern}\" cannot match; \
             Granola began attributing speakers on 2026-07-21, and only on meetings \
             recorded after that (run `grans sync` to pull recent ones)"
        );
    }

    let shown: Vec<&str> = available
        .iter()
        .take(MAX_LISTED_SPEAKERS)
        .map(String::as_str)
        .collect();
    let more = available.len().saturating_sub(shown.len());
    let suffix = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    format!(
        "no speaker matches \"{}\"; known speakers: {}{}",
        pattern,
        shown.join(", "),
        suffix
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // --- SpeakerSelector::parse ---

    #[test]
    fn parse_reserves_me_and_other_case_insensitively() {
        assert_eq!(SpeakerSelector::parse("me"), Some(SpeakerSelector::Me));
        assert_eq!(SpeakerSelector::parse("ME"), Some(SpeakerSelector::Me));
        assert_eq!(
            SpeakerSelector::parse("other"),
            Some(SpeakerSelector::Other)
        );
        assert_eq!(
            SpeakerSelector::parse("OTHER"),
            Some(SpeakerSelector::Other)
        );
    }

    #[test]
    fn parse_treats_anything_else_as_a_name_pattern() {
        assert_eq!(
            SpeakerSelector::parse("Jane Doe"),
            Some(SpeakerSelector::Name("Jane Doe".to_string()))
        );
        // Case is preserved for display; matching lowercases later.
        assert_eq!(
            SpeakerSelector::parse("  jane  "),
            Some(SpeakerSelector::Name("jane".to_string()))
        );
    }

    #[test]
    fn parse_rejects_an_empty_pattern() {
        assert_eq!(SpeakerSelector::parse(""), None);
        assert_eq!(SpeakerSelector::parse("   "), None);
    }

    // --- resolve_names ---

    #[test]
    fn resolve_names_matches_a_substring_case_insensitively() {
        let available = names(&["Jane Doe", "Marcus Webb"]);
        assert_eq!(resolve_names("jane", &available), names(&["Jane Doe"]));
        assert_eq!(resolve_names("WEBB", &available), names(&["Marcus Webb"]));
    }

    #[test]
    fn resolve_names_returns_every_substring_hit() {
        let available = names(&["Jane Doe", "Jane Smith", "Marcus Webb"]);
        assert_eq!(
            resolve_names("jane", &available),
            names(&["Jane Doe", "Jane Smith"])
        );
    }

    #[test]
    fn resolve_names_exact_hit_beats_substring_hits() {
        // "Jane Doe" is a substring of "Jane Doe Jr", so a naive substring
        // match would union the two and there would be no way to name just her.
        let available = names(&["Jane Doe", "Jane Doe Jr"]);
        assert_eq!(resolve_names("jane doe", &available), names(&["Jane Doe"]));
    }

    #[test]
    fn resolve_names_returns_empty_when_nothing_matches() {
        let available = names(&["Jane Doe"]);
        assert!(resolve_names("jayne", &available).is_empty());
    }

    // --- SpeakerFilter::matches ---

    #[test]
    fn me_and_other_test_the_audio_channel_only() {
        assert!(SpeakerFilter::Me.matches(Some("microphone"), None));
        assert!(!SpeakerFilter::Me.matches(Some("system"), None));
        assert!(!SpeakerFilter::Me.matches(None, None));

        assert!(SpeakerFilter::Other.matches(Some("system"), None));
        assert!(!SpeakerFilter::Other.matches(Some("microphone"), None));
        assert!(!SpeakerFilter::Other.matches(None, None));
    }

    #[test]
    fn other_keeps_meaning_not_me_when_a_name_is_present() {
        // Attribution must not silently narrow `other` to attributed rows.
        assert!(SpeakerFilter::Other.matches(Some("system"), Some("Jane Doe")));
    }

    #[test]
    fn names_test_the_detected_name_case_insensitively() {
        let filter = SpeakerFilter::Names(names(&["Jane Doe"]));
        assert!(filter.matches(Some("system"), Some("Jane Doe")));
        assert!(filter.matches(Some("system"), Some("jane doe")));
        assert!(!filter.matches(Some("system"), Some("Marcus Webb")));
    }

    #[test]
    fn names_never_match_an_unattributed_utterance() {
        // The microphone channel is never attributed, and neither is anything
        // recorded before the cutover.
        let filter = SpeakerFilter::Names(names(&["Jane Doe"]));
        assert!(!filter.matches(Some("microphone"), None));
        assert!(!filter.matches(Some("system"), None));
    }

    #[test]
    fn names_match_any_of_a_multi_speaker_resolution() {
        let filter = SpeakerFilter::Names(names(&["Jane Doe", "Jane Smith"]));
        assert!(filter.matches(Some("system"), Some("Jane Doe")));
        assert!(filter.matches(Some("system"), Some("Jane Smith")));
        assert!(!filter.matches(Some("system"), Some("Marcus Webb")));
    }

    // --- label ---

    #[test]
    fn label_names_the_detected_speaker_when_there_is_one() {
        assert_eq!(
            label(Some("system"), Some("Jane Doe")),
            Some(SpeakerLabel::Named("Jane Doe"))
        );
    }

    #[test]
    fn label_falls_back_to_other_for_an_unattributed_remote_speaker() {
        assert_eq!(label(Some("system"), None), Some(SpeakerLabel::Other));
        // A blank name is not a name.
        assert_eq!(label(Some("system"), Some("  ")), Some(SpeakerLabel::Other));
    }

    #[test]
    fn label_calls_the_microphone_channel_you() {
        assert_eq!(label(Some("microphone"), None), Some(SpeakerLabel::You));
        assert_eq!(
            label(Some("microphone"), Some("Jane Doe")),
            Some(SpeakerLabel::You)
        );
    }

    #[test]
    fn label_says_nothing_when_the_channel_is_unknown() {
        // Transcripts synced before v003 have no source at all.
        assert_eq!(label(None, None), None);
        assert_eq!(label(Some("weird"), None), None);
    }

    // --- no_match_message ---

    #[test]
    fn no_match_message_lists_known_speakers() {
        let msg = no_match_message("jayne", &names(&["Jane Doe", "Marcus Webb"]));
        assert!(msg.contains("no speaker matches \"jayne\""), "got: {msg}");
        assert!(msg.contains("Jane Doe, Marcus Webb"), "got: {msg}");
    }

    #[test]
    fn no_match_message_truncates_a_long_speaker_list() {
        let all: Vec<String> = (0..15).map(|i| format!("Speaker {i}")).collect();
        let msg = no_match_message("nobody", &all);
        assert!(msg.contains("Speaker 9"), "got: {msg}");
        assert!(!msg.contains("Speaker 10"), "got: {msg}");
        assert!(msg.contains("(+5 more)"), "got: {msg}");
    }

    #[test]
    fn no_match_message_distinguishes_a_corpus_with_no_attribution() {
        let msg = no_match_message("jane", &[]);
        assert!(
            msg.contains("no utterances have speaker attribution"),
            "got: {msg}"
        );
        assert!(msg.contains("2026-07-21"), "got: {msg}");
    }

    // --- resolve ---

    #[test]
    fn resolve_passes_me_and_other_through_without_a_lookup() {
        let conn = crate::db::test_fixtures::build_test_db(&serde_json::json!({}));
        assert_eq!(
            resolve(&conn, &SpeakerSelector::Me).unwrap(),
            SpeakerFilter::Me
        );
        assert_eq!(
            resolve(&conn, &SpeakerSelector::Other).unwrap(),
            SpeakerFilter::Other
        );
    }

    #[test]
    fn resolve_selects_the_matching_names() {
        let conn = crate::db::test_fixtures::build_test_db(&attributed_state());
        let filter = resolve(&conn, &SpeakerSelector::Name("jane".to_string())).unwrap();
        assert_eq!(filter, SpeakerFilter::Names(names(&["Jane Doe"])));
    }

    #[test]
    fn resolve_errors_when_the_pattern_matches_nobody() {
        let conn = crate::db::test_fixtures::build_test_db(&attributed_state());
        let err = resolve(&conn, &SpeakerSelector::Name("jayne".to_string()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no speaker matches \"jayne\""), "got: {err}");
        assert!(err.contains("Jane Doe"), "got: {err}");
    }

    #[test]
    fn resolve_takes_the_union_when_several_speakers_match() {
        let conn = crate::db::test_fixtures::build_test_db(&attributed_state());
        let filter = resolve(&conn, &SpeakerSelector::Name("a".to_string())).unwrap();
        let SpeakerFilter::Names(matched) = filter else {
            panic!("expected resolved names");
        };
        assert_eq!(matched, names(&["Jane Doe", "Marcus Webb"]));
    }

    /// One meeting whose system channel carries two attributed speakers and
    /// one utterance from before the cutover.
    fn attributed_state() -> serde_json::Value {
        serde_json::json!({
            "documents": {
                "doc-1": { "id": "doc-1", "title": "Sync", "created_at": "2026-07-22T10:00:00Z" }
            },
            "transcripts": {
                "doc-1": [
                    {"id": "u1", "document_id": "doc-1", "text": "Mine", "source": "microphone"},
                    {"id": "u2", "document_id": "doc-1", "text": "Hers", "source": "system",
                     "speaker_name": "Jane Doe"},
                    {"id": "u3", "document_id": "doc-1", "text": "Theirs", "source": "system",
                     "speaker_name": "Marcus Webb"},
                    {"id": "u4", "document_id": "doc-1", "text": "Unknown", "source": "system"}
                ]
            }
        })
    }
}
