use std::collections::HashMap;

use super::store::StoredVector;

/// A semantic search result.
#[derive(Debug, Clone)]
pub struct SemanticSearchResult {
    pub document_id: String,
    pub score: f32,
    pub source_type: String,
    pub matched_text: String,
    /// Start index of the matched window in the document's utterances.
    pub window_start_idx: Option<usize>,
    /// End index (inclusive) of the matched window in the document's utterances.
    pub window_end_idx: Option<usize>,
    /// Human-readable context for where the match came from (e.g. "AI notes: Budget Review").
    pub match_context: Option<String>,
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());

    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;

    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// Parse window indices from metadata JSON.
fn parse_window_indices(metadata_json: &Option<String>) -> (Option<usize>, Option<usize>) {
    if let Some(json_str) = metadata_json {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(json_str) {
            let start = meta.get("window_start_idx").and_then(|v| v.as_u64()).map(|v| v as usize);
            let end = meta.get("window_end_idx").and_then(|v| v.as_u64()).map(|v| v as usize);
            return (start, end);
        }
    }
    (None, None)
}

/// Extract human-readable context from a chunk's source type and metadata.
fn extract_match_context(source_type: &str, metadata_json: &Option<String>) -> Option<String> {
    match source_type {
        "panel_section" => {
            let heading = metadata_json
                .as_ref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                .and_then(|meta| meta.get("section_heading")?.as_str().map(|s| s.to_string()));
            Some(match heading {
                Some(h) => format!("AI notes: {}", h),
                None => "AI notes".to_string(),
            })
        }
        "notes_paragraph" => Some("your notes".to_string()),
        _ => None,
    }
}

/// Rank all stored vectors against a query vector.
/// Returns results deduplicated by document_id (highest score per doc), sorted by score descending.
/// `source_type_filter`: if `Some`, only score vectors whose source_type is in the list.
/// `None` means search everything.
pub fn rank_results(
    query_vec: &[f32],
    stored: &[StoredVector],
    min_score: f32,
    source_type_filter: Option<&[&str]>,
) -> Vec<SemanticSearchResult> {
    let mut doc_best: HashMap<&str, SemanticSearchResult> = HashMap::new();

    for sv in stored {
        // Apply source type filter
        if let Some(filter) = source_type_filter {
            if !filter.contains(&sv.source_type.as_str()) {
                continue;
            }
        }

        let score = cosine_similarity(query_vec, &sv.vector);
        if score < min_score {
            continue;
        }

        let (window_start_idx, window_end_idx) = parse_window_indices(&sv.metadata_json);
        let match_context = extract_match_context(&sv.source_type, &sv.metadata_json);

        let entry = doc_best
            .entry(&sv.document_id)
            .or_insert_with(|| SemanticSearchResult {
                document_id: sv.document_id.clone(),
                score,
                source_type: sv.source_type.clone(),
                matched_text: sv.text.clone(),
                window_start_idx,
                window_end_idx,
                match_context,
            });

        if score > entry.score {
            entry.score = score;
            entry.source_type = sv.source_type.clone();
            entry.matched_text = sv.text.clone();
            entry.window_start_idx = window_start_idx;
            entry.window_end_idx = window_end_idx;
            entry.match_context = extract_match_context(&sv.source_type, &sv.metadata_json);
        }
    }

    let mut results: Vec<SemanticSearchResult> = doc_best.into_values().collect();
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![0.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_rank_results_deduplicates_by_document() {
        let stored = vec![
            StoredVector {
                chunk_id: 1,
                document_id: "doc1".to_string(),
                source_type: "transcript_window".to_string(),
                text: "chunk 1".to_string(),
                vector: vec![1.0, 0.0, 0.0],
                metadata_json: None,
            },
            StoredVector {
                chunk_id: 2,
                document_id: "doc1".to_string(),
                source_type: "transcript_window".to_string(),
                text: "chunk 2".to_string(),
                vector: vec![0.9, 0.1, 0.0],
                metadata_json: None,
            },
            StoredVector {
                chunk_id: 3,
                document_id: "doc2".to_string(),
                source_type: "transcript_window".to_string(),
                text: "chunk 3".to_string(),
                vector: vec![0.5, 0.5, 0.0],
                metadata_json: None,
            },
        ];

        let query = vec![1.0, 0.0, 0.0];
        let results = rank_results(&query, &stored, 0.0, None);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].document_id, "doc1");
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn test_rank_results_min_score_filter() {
        let stored = vec![
            StoredVector {
                chunk_id: 1,
                document_id: "doc1".to_string(),
                source_type: "transcript_window".to_string(),
                text: "relevant".to_string(),
                vector: vec![1.0, 0.0, 0.0],
                metadata_json: None,
            },
            StoredVector {
                chunk_id: 2,
                document_id: "doc2".to_string(),
                source_type: "transcript_window".to_string(),
                text: "irrelevant".to_string(),
                vector: vec![0.0, 1.0, 0.0],
                metadata_json: None,
            },
        ];

        let query = vec![1.0, 0.0, 0.0];
        let results = rank_results(&query, &stored, 0.5, None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].document_id, "doc1");
    }

    #[test]
    fn test_rank_results_sorted_descending() {
        let stored = vec![
            StoredVector {
                chunk_id: 1,
                document_id: "doc1".to_string(),
                source_type: "transcript_window".to_string(),
                text: "low".to_string(),
                vector: vec![0.1, 0.9, 0.0],
                metadata_json: None,
            },
            StoredVector {
                chunk_id: 2,
                document_id: "doc2".to_string(),
                source_type: "transcript_window".to_string(),
                text: "high".to_string(),
                vector: vec![0.95, 0.05, 0.0],
                metadata_json: None,
            },
            StoredVector {
                chunk_id: 3,
                document_id: "doc3".to_string(),
                source_type: "transcript_window".to_string(),
                text: "mid".to_string(),
                vector: vec![0.7, 0.3, 0.0],
                metadata_json: None,
            },
        ];

        let query = vec![1.0, 0.0, 0.0];
        let results = rank_results(&query, &stored, 0.0, None);

        assert_eq!(results.len(), 3);
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }

    #[test]
    fn test_rank_results_parses_window_indices() {
        let metadata_with_indices = serde_json::json!({
            "window_start_idx": 5,
            "window_end_idx": 10,
        })
        .to_string();

        let stored = vec![
            StoredVector {
                chunk_id: 1,
                document_id: "doc1".to_string(),
                source_type: "transcript_window".to_string(),
                text: "chunk with metadata".to_string(),
                vector: vec![1.0, 0.0, 0.0],
                metadata_json: Some(metadata_with_indices),
            },
            StoredVector {
                chunk_id: 2,
                document_id: "doc2".to_string(),
                source_type: "transcript_window".to_string(),
                text: "chunk without metadata".to_string(),
                vector: vec![0.8, 0.2, 0.0],
                metadata_json: None,
            },
        ];

        let query = vec![1.0, 0.0, 0.0];
        let results = rank_results(&query, &stored, 0.0, None);

        assert_eq!(results.len(), 2);

        // Find doc1 result (should have indices)
        let doc1_result = results.iter().find(|r| r.document_id == "doc1").unwrap();
        assert_eq!(doc1_result.window_start_idx, Some(5));
        assert_eq!(doc1_result.window_end_idx, Some(10));

        // Find doc2 result (should have no indices)
        let doc2_result = results.iter().find(|r| r.document_id == "doc2").unwrap();
        assert!(doc2_result.window_start_idx.is_none());
        assert!(doc2_result.window_end_idx.is_none());
    }

    #[test]
    fn test_parse_window_indices_missing_fields() {
        // Missing fields should return None
        let partial_meta = serde_json::json!({
            "window_start_idx": 3,
        })
        .to_string();

        let (start, end) = parse_window_indices(&Some(partial_meta));
        assert_eq!(start, Some(3));
        assert!(end.is_none());
    }

    #[test]
    fn test_parse_window_indices_invalid_json() {
        let (start, end) = parse_window_indices(&Some("not valid json".to_string()));
        assert!(start.is_none());
        assert!(end.is_none());
    }

    #[test]
    fn test_source_type_filter_includes_matching() {
        let stored = vec![
            StoredVector {
                chunk_id: 1,
                document_id: "doc1".to_string(),
                source_type: "transcript_window".to_string(),
                text: "transcript text".to_string(),
                vector: vec![1.0, 0.0, 0.0],
                metadata_json: None,
            },
            StoredVector {
                chunk_id: 2,
                document_id: "doc2".to_string(),
                source_type: "panel_section".to_string(),
                text: "panel text".to_string(),
                vector: vec![0.9, 0.1, 0.0],
                metadata_json: Some(serde_json::json!({"section_heading": "Budget"}).to_string()),
            },
            StoredVector {
                chunk_id: 3,
                document_id: "doc3".to_string(),
                source_type: "notes_paragraph".to_string(),
                text: "notes text".to_string(),
                vector: vec![0.8, 0.2, 0.0],
                metadata_json: None,
            },
        ];

        let query = vec![1.0, 0.0, 0.0];

        // Filter to only panel_section
        let results = rank_results(&query, &stored, 0.0, Some(&["panel_section"]));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].document_id, "doc2");

        // Filter to transcript + notes
        let results = rank_results(&query, &stored, 0.0, Some(&["transcript_window", "notes_paragraph"]));
        assert_eq!(results.len(), 2);

        // No filter = all results
        let results = rank_results(&query, &stored, 0.0, None);
        assert_eq!(results.len(), 3);

        // Empty filter = no results
        let results = rank_results(&query, &stored, 0.0, Some(&[]));
        assert!(results.is_empty());
    }

    #[test]
    fn test_match_context_panel_section() {
        let ctx = extract_match_context(
            "panel_section",
            &Some(serde_json::json!({"section_heading": "Budget Review"}).to_string()),
        );
        assert_eq!(ctx, Some("AI notes: Budget Review".to_string()));
    }

    #[test]
    fn test_match_context_panel_section_no_heading() {
        let ctx = extract_match_context("panel_section", &None);
        assert_eq!(ctx, Some("AI notes".to_string()));
    }

    #[test]
    fn test_match_context_notes_paragraph() {
        let ctx = extract_match_context("notes_paragraph", &None);
        assert_eq!(ctx, Some("your notes".to_string()));
    }

    #[test]
    fn test_match_context_transcript_window() {
        let ctx = extract_match_context("transcript_window", &None);
        assert_eq!(ctx, None);
    }
}
