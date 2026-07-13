use clap::ValueEnum;

use crate::models::Document;

/// The default `--in` target list for search and grep: every source.
pub const DEFAULT_SEARCH_TARGETS: &str = "titles,transcripts,notes,panels";

/// Where to search for text matches in meetings.
///
/// Parsed directly by clap as a `ValueEnum`, so `--in` rejects unknown
/// targets at parse time and names the valid ones, rather than silently
/// dropping typos.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "lowercase")]
pub enum SearchTarget {
    Titles,
    Transcripts,
    Notes,
    Panels,
}

impl SearchTarget {
    /// Every target, in canonical `--in` order. Matches
    /// [`DEFAULT_SEARCH_TARGETS`] and is the set used when `--in` is omitted.
    pub fn all() -> Vec<SearchTarget> {
        vec![
            SearchTarget::Titles,
            SearchTarget::Transcripts,
            SearchTarget::Notes,
            SearchTarget::Panels,
        ]
    }

    /// The canonical `--in` token for this target (the same spelling clap
    /// accepts on the command line).
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchTarget::Titles => "titles",
            SearchTarget::Transcripts => "transcripts",
            SearchTarget::Notes => "notes",
            SearchTarget::Panels => "panels",
        }
    }

    /// Map a search target to the corresponding chunk source type string.
    /// Returns `None` for targets that don't have embeddings (e.g. Titles).
    pub fn to_chunk_source_type(&self) -> Option<&'static str> {
        match self {
            SearchTarget::Titles => None,
            SearchTarget::Transcripts => Some("transcript_window"),
            SearchTarget::Notes => Some("notes_paragraph"),
            SearchTarget::Panels => Some("panel_section"),
        }
    }
}

/// Re-join parsed targets into an `--in` flag value, so a suggested command
/// can echo the active filter as the user could re-type it.
pub fn targets_to_flag_value(targets: &[SearchTarget]) -> String {
    targets.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(",")
}

/// Compute a source type filter for semantic search from search targets.
/// Returns `None` when all embeddable sources are included (= search everything).
/// Returns `Some(empty vec)` if only non-embeddable targets (titles) are selected.
pub fn semantic_source_filter(targets: &[SearchTarget]) -> Option<Vec<&'static str>> {
    let all_embeddable = [SearchTarget::Transcripts, SearchTarget::Notes, SearchTarget::Panels];
    if all_embeddable.iter().all(|t| targets.contains(t)) {
        return None; // default = no filter
    }
    let types: Vec<&str> = targets.iter().filter_map(|t| t.to_chunk_source_type()).collect();
    Some(types)
}

/// True when the title or id contains the lowercased filter.
pub fn meeting_filter_matches(filter_lower: &str, title: Option<&str>, id: Option<&str>) -> bool {
    title.map(|t| t.to_lowercase().contains(filter_lower)).unwrap_or(false)
        || id.map(|i| i.to_lowercase().contains(filter_lower)).unwrap_or(false)
}

/// Keep only documents whose title or id contains `filter` (case-insensitive).
/// No filter keeps everything.
pub fn filter_by_meeting(results: Vec<Document>, meeting_filter: Option<&str>) -> Vec<Document> {
    let Some(filter) = meeting_filter else {
        return results;
    };
    let filter_lower = filter.to_lowercase();
    results
        .into_iter()
        .filter(|doc| meeting_filter_matches(&filter_lower, doc.title.as_deref(), doc.id.as_deref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_matches_the_default_target_string() {
        // `all()` and DEFAULT_SEARCH_TARGETS must stay in lockstep so the
        // clap default and the footer's "is this the default?" check agree.
        assert_eq!(targets_to_flag_value(&SearchTarget::all()), DEFAULT_SEARCH_TARGETS);
    }

    #[test]
    fn test_as_str_round_trips_through_value_enum() {
        // Every target's `as_str` spelling is one clap accepts back.
        for target in SearchTarget::all() {
            let parsed = SearchTarget::from_str(target.as_str(), false).unwrap();
            assert_eq!(parsed, target);
        }
    }

    #[test]
    fn test_targets_to_flag_value_preserves_order() {
        let flag = targets_to_flag_value(&[SearchTarget::Notes, SearchTarget::Titles]);
        assert_eq!(flag, "notes,titles");
    }

    #[test]
    fn test_to_chunk_source_type() {
        assert_eq!(SearchTarget::Titles.to_chunk_source_type(), None);
        assert_eq!(SearchTarget::Transcripts.to_chunk_source_type(), Some("transcript_window"));
        assert_eq!(SearchTarget::Notes.to_chunk_source_type(), Some("notes_paragraph"));
        assert_eq!(SearchTarget::Panels.to_chunk_source_type(), Some("panel_section"));
    }

    #[test]
    fn test_semantic_source_filter_all_embeddable() {
        let targets = vec![SearchTarget::Transcripts, SearchTarget::Notes, SearchTarget::Panels];
        assert_eq!(semantic_source_filter(&targets), None);
    }

    #[test]
    fn test_semantic_source_filter_all_with_titles() {
        // All embeddable + titles = still None (search everything)
        let targets = vec![SearchTarget::Titles, SearchTarget::Transcripts, SearchTarget::Notes, SearchTarget::Panels];
        assert_eq!(semantic_source_filter(&targets), None);
    }

    #[test]
    fn test_semantic_source_filter_subset() {
        let targets = vec![SearchTarget::Panels];
        assert_eq!(semantic_source_filter(&targets), Some(vec!["panel_section"]));
    }

    #[test]
    fn test_semantic_source_filter_titles_only() {
        let targets = vec![SearchTarget::Titles];
        assert_eq!(semantic_source_filter(&targets), Some(vec![]));
    }

    #[test]
    fn test_semantic_source_filter_mixed() {
        let targets = vec![SearchTarget::Transcripts, SearchTarget::Panels];
        let filter = semantic_source_filter(&targets).unwrap();
        assert!(filter.contains(&"transcript_window"));
        assert!(filter.contains(&"panel_section"));
        assert_eq!(filter.len(), 2);
    }
}
