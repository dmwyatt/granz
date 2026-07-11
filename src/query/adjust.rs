//! Ordering-only ranking adjustments applied after the cross-encoder: the
//! RRF fusion prior and the title-match boost. The user-facing score stays
//! [`RerankCandidate::rerank_score`]; these weights shape ordering only,
//! and can only reorder the fixed rerank pool, never add or remove
//! candidates or bury one below the pool.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::Connection;

use crate::query::rerank::RerankCandidate;

/// Words too common to signal which meeting a query is about. The Phase 5
/// sweep showed the boost is insensitive to the exact list, so it stays
/// minimal.
const STOPWORDS: &[&str] =
    &["a", "an", "the", "of", "in", "on", "at", "to", "for", "with", "and", "or"];

/// Weights for the ordering score. Defaults are the winners of recorded
/// sweeps on the 93-query golden set; a weight of 0.0 disables that
/// adjustment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankingConfig {
    /// Weight of the RRF fusion score, blended as
    /// `rerank_score + fusion_blend_weight * fused_score`. RRF scores top
    /// out near 2/(60+1), so 30 scales the prior to roughly the
    /// cross-encoder's [0, 1] range. Without the prior, queries where the
    /// cross-encoder is unconfident (max score well under 0.8) get
    /// noise-dominated orderings that can bury documents fusion ranked
    /// highly; the sweep picked 30 (hit-rate@10 0.935, MRR@10 0.804, no
    /// fusion top-3 document leaving the top 10).
    pub fusion_blend_weight: f32,
    /// Weight of the title-match boost: the fraction of query content
    /// tokens appearing in the document title, damped by
    /// `log2(1 + series count)` so recurring meeting series don't drown a
    /// query in same-titled siblings.
    pub title_boost_weight: f32,
}

impl Default for RankingConfig {
    fn default() -> Self {
        Self { fusion_blend_weight: 30.0, title_boost_weight: 0.0 }
    }
}

impl RankingConfig {
    /// Apply experiment-flag overrides; `None` keeps the default.
    pub fn with_overrides(self, title_boost_weight: Option<f32>) -> Self {
        Self { title_boost_weight: title_boost_weight.unwrap_or(self.title_boost_weight), ..self }
    }
}

/// Query-independent, per-database facts the adjustments consume. Load
/// once per search invocation or benchmark retriever build, not per query.
#[derive(Debug, Default)]
pub struct RankingContext {
    /// Normalized title -> count of non-deleted documents sharing it.
    pub title_counts: HashMap<String, u32>,
}

impl RankingContext {
    pub fn load(conn: &Connection) -> Result<Self> {
        Ok(Self { title_counts: crate::db::meetings::title_series_counts(conn)? })
    }
}

/// Normalize a title the same way [`crate::db::meetings::title_series_counts`]
/// keys its map: SQLite's `trim()` strips spaces only and `lower()` is
/// ASCII-only, so this must be a space-trim plus ASCII lowering.
pub fn normalize_title(title: &str) -> String {
    title.trim_matches(' ').to_ascii_lowercase()
}

/// Lowercased alphanumeric tokens of at least two characters, minus
/// [`STOPWORDS`].
fn content_tokens(s: &str) -> HashSet<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2 && !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect()
}

/// Title-match signal in [0, 1]: the fraction of query content tokens
/// found in the title, damped by log2(1 + series_count). Missing or
/// contentless titles and empty queries yield 0 — missing metadata must
/// never out-rank.
fn title_signal(query_tokens: &HashSet<String>, title: Option<&str>, series_count: u32) -> f32 {
    let Some(title) = title else { return 0.0 };
    if query_tokens.is_empty() {
        return 0.0;
    }
    let title_tokens = content_tokens(title);
    if title_tokens.is_empty() {
        return 0.0;
    }
    let overlap = query_tokens.intersection(&title_tokens).count() as f64
        / query_tokens.len() as f64;
    (overlap / f64::from(1 + series_count).log2()) as f32
}

/// The full ordering score for one candidate. Ordering only; never shown
/// to the user.
pub fn ordering_score(
    candidate: &RerankCandidate,
    query_tokens: &HashSet<String>,
    ctx: &RankingContext,
    cfg: &RankingConfig,
) -> f32 {
    let series_count = candidate
        .title
        .as_deref()
        .and_then(|t| ctx.title_counts.get(&normalize_title(t)).copied())
        .unwrap_or(1);
    candidate.rerank_score
        + cfg.fusion_blend_weight * candidate.fused_score as f32
        + cfg.title_boost_weight
            * title_signal(query_tokens, candidate.title.as_deref(), series_count)
}

/// Stable sort best-first by ordering score. Input must be in fused order
/// so ties keep the fused order; scores are computed once per candidate.
pub fn sort_candidates(
    candidates: Vec<RerankCandidate>,
    query: &str,
    ctx: &RankingContext,
    cfg: &RankingConfig,
) -> Vec<RerankCandidate> {
    let query_tokens = content_tokens(query);
    let mut scored: Vec<(f32, RerankCandidate)> = candidates
        .into_iter()
        .map(|c| (ordering_score(&c, &query_tokens, ctx, cfg), c))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_fixtures::build_test_db;
    use serde_json::json;

    fn cand(id: &str, fused_score: f64, rerank_score: f32, title: Option<&str>) -> RerankCandidate {
        RerankCandidate {
            document_id: id.to_string(),
            fused_rank: 0,
            fused_score,
            passage: String::new(),
            title: title.map(str::to_string),
            created_at: None,
            rerank_score,
        }
    }

    fn no_boost() -> RankingConfig {
        RankingConfig { title_boost_weight: 0.0, ..Default::default() }
    }

    #[test]
    fn default_fusion_blend_weight_is_thirty() {
        assert_eq!(RankingConfig::default().fusion_blend_weight, 30.0);
    }

    #[test]
    fn with_overrides_none_keeps_default_some_overrides() {
        let cfg = RankingConfig::default();
        assert_eq!(cfg.with_overrides(None), cfg);
        assert_eq!(cfg.with_overrides(Some(0.7)).title_boost_weight, 0.7);
    }

    #[test]
    fn content_tokens_drop_stopwords_short_tokens_and_case() {
        let tokens = content_tokens("The Kumquat & Q1 Review of x");
        let expected: HashSet<String> =
            ["kumquat", "q1", "review"].into_iter().map(str::to_string).collect();
        assert_eq!(tokens, expected);
    }

    #[test]
    fn normalize_title_matches_sqlite_semantics() {
        // Space-trim plus ASCII-only lowering, exactly like lower(trim(x)).
        assert_eq!(normalize_title("  Weekly Standup  "), "weekly standup");
        assert_eq!(normalize_title("CAFÉ"), "cafÉ");
    }

    #[test]
    fn title_signal_is_overlap_fraction_damped_by_series() {
        let q: HashSet<String> =
            ["kumquat", "sync"].into_iter().map(str::to_string).collect();
        // Full overlap, unique title: log2(1 + 1) = 1, no damping.
        assert_eq!(title_signal(&q, Some("Kumquat Sync"), 1), 1.0);
        // Full overlap in a 3-meeting series: damped by log2(4) = 2.
        assert_eq!(title_signal(&q, Some("Kumquat Sync"), 3), 0.5);
        // Half the query tokens in the title.
        assert_eq!(title_signal(&q, Some("Kumquat Retro"), 1), 0.5);
    }

    #[test]
    fn title_signal_zero_for_missing_or_contentless_inputs() {
        let q: HashSet<String> = ["kumquat"].into_iter().map(str::to_string).collect();
        assert_eq!(title_signal(&q, None, 1), 0.0);
        assert_eq!(title_signal(&q, Some("of the"), 1), 0.0);
        assert_eq!(title_signal(&HashSet::new(), Some("Kumquat Sync"), 1), 0.0);
    }

    #[test]
    fn zero_boost_weight_reproduces_fusion_blend_bit_for_bit() {
        let ctx = RankingContext::default();
        let cfg = no_boost();
        let q = content_tokens("kumquat sync");
        for c in [
            cand("a", 0.0163, 0.72, Some("Kumquat Sync")),
            cand("b", 0.032786885, 0.0, None),
            cand("c", 0.0, -1.5, Some("Unrelated")),
        ] {
            let expected = c.rerank_score + 30.0 * c.fused_score as f32;
            assert_eq!(ordering_score(&c, &q, &ctx, &cfg).to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn boost_reorders_but_never_touches_the_user_facing_score() {
        // Equal rerank and fused scores; only the title distinguishes them.
        let candidates = vec![
            cand("plain", 0.016, 0.5, Some("Unrelated Retro")),
            cand("titled", 0.016, 0.5, Some("Kumquat Sync")),
        ];
        let ctx = RankingContext::default();
        let cfg = RankingConfig { title_boost_weight: 0.2, ..Default::default() };

        let sorted = sort_candidates(candidates, "kumquat sync", &ctx, &cfg);

        assert_eq!(sorted[0].document_id, "titled");
        assert!(sorted.iter().all(|c| c.rerank_score == 0.5));
    }

    #[test]
    fn series_damping_prefers_the_unique_title() {
        // Same title overlap, but one title names a 15-meeting series.
        let candidates = vec![
            cand("series", 0.016, 0.5, Some("Kumquat Sync")),
            cand("unique", 0.016, 0.5, Some("Kumquat Deep Dive")),
        ];
        let ctx = RankingContext {
            title_counts: HashMap::from([
                ("kumquat sync".to_string(), 15),
                ("kumquat deep dive".to_string(), 1),
            ]),
        };
        let cfg = RankingConfig { title_boost_weight: 0.2, ..Default::default() };

        let sorted = sort_candidates(candidates, "kumquat", &ctx, &cfg);

        assert_eq!(sorted[0].document_id, "unique");
    }

    #[test]
    fn ties_keep_input_order() {
        let candidates = vec![
            cand("first", 0.02, 0.5, None),
            cand("second", 0.02, 0.5, None),
            cand("third", 0.02, 0.5, None),
        ];
        let sorted =
            sort_candidates(candidates, "kumquat", &RankingContext::default(), &no_boost());
        let ids: Vec<&str> = sorted.iter().map(|c| c.document_id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second", "third"]);
    }

    #[test]
    fn context_load_reads_series_counts() {
        let conn = build_test_db(&json!({
            "documents": {
                "doc-1": {"id": "doc-1", "title": "Weekly Standup", "created_at": "2026-01-05T10:00:00Z"},
                "doc-2": {"id": "doc-2", "title": "weekly standup", "created_at": "2026-01-12T10:00:00Z"}
            }
        }));
        let ctx = RankingContext::load(&conn).unwrap();
        assert_eq!(ctx.title_counts.get("weekly standup"), Some(&2));
    }
}
