/// Where to search for text matches in meetings.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchTarget {
    Titles,
    Transcripts,
    Notes,
    Panels,
}

impl SearchTarget {
    pub fn parse_list(s: &str) -> Vec<SearchTarget> {
        s.split(',')
            .filter_map(|part| match part.trim().to_lowercase().as_str() {
                "titles" => Some(SearchTarget::Titles),
                "transcripts" => Some(SearchTarget::Transcripts),
                "notes" => Some(SearchTarget::Notes),
                "panels" => Some(SearchTarget::Panels),
                _ => None,
            })
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_target_parse_list() {
        let targets = SearchTarget::parse_list("titles,notes");
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&SearchTarget::Titles));
        assert!(targets.contains(&SearchTarget::Notes));
    }

    #[test]
    fn test_search_target_parse_all() {
        let targets = SearchTarget::parse_list("titles,transcripts,notes,panels");
        assert_eq!(targets.len(), 4);
        assert!(targets.contains(&SearchTarget::Panels));
    }

    #[test]
    fn test_search_target_parse_unknown() {
        let targets = SearchTarget::parse_list("titles,unknown");
        assert_eq!(targets.len(), 1);
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
