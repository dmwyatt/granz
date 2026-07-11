//! Search quality benchmark: score retrieval modes against a labeled golden set.
//!
//! The golden-set file provides labeled queries; each requested mode retrieves
//! a ranked document list per query, which metrics.rs scores. Labels match by
//! document ID when the file provides `relevant_meeting_ids`, falling back to
//! exact title for older files that only carry `relevant_meetings`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::metrics::{self, AggregateMetrics, LabelMatcher, QueryScore, RankedDoc, WinLossTie};
use super::perf::percentile;
use super::retriever::Retriever;
use crate::cli::args::QualityMode;
use crate::output::format::OutputMode;

/// CLI arguments for the quality benchmark.
pub struct QualityArgs<'a> {
    pub file: &'a Path,
    pub k: usize,
    pub mode: QualityMode,
    pub compare: &'a [QualityMode],
    pub detail: bool,
    pub record: bool,
    pub note: Option<&'a str>,
    /// --db override, when given; the default database path otherwise applies.
    pub db: Option<&'a Path>,
}

/// A single test query from the benchmark file.
#[derive(Debug, Deserialize)]
struct BenchmarkQuery {
    query: String,
    #[serde(default)]
    query_type: Option<String>,
    #[serde(default)]
    relevant_meetings: Vec<String>,
    #[serde(default)]
    relevant_meeting_ids: Vec<String>,
}

impl BenchmarkQuery {
    /// ID matching when the file provides document IDs, title matching
    /// otherwise (v1 golden set).
    fn matcher<'a>(&'a self, title_map: &'a HashMap<String, String>) -> LabelMatcher<'a> {
        if self.relevant_meeting_ids.is_empty() {
            LabelMatcher::Titles {
                titles: self.relevant_meetings.iter().map(String::as_str).collect(),
                title_map,
            }
        } else {
            LabelMatcher::Ids(
                self.relevant_meeting_ids
                    .iter()
                    .map(String::as_str)
                    .collect(),
            )
        }
    }

    /// Human-readable labels for output: titles when the file has them,
    /// document IDs otherwise.
    fn expected_display(&self) -> &[String] {
        if self.relevant_meetings.is_empty() {
            &self.relevant_meeting_ids
        } else {
            &self.relevant_meetings
        }
    }
}

/// The benchmark file format.
#[derive(Debug, Deserialize)]
struct BenchmarkFile {
    queries: Vec<BenchmarkQuery>,
}

/// A single result document in a query's top k, for the per-query output.
#[derive(Debug, Serialize)]
struct TopHit {
    rank: usize,
    document_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<f32>,
}

/// Full scored outcome for one query in one mode.
#[derive(Debug, Serialize)]
pub(super) struct QueryOutcome {
    query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    query_type: Option<String>,
    matching: &'static str,
    expected: Vec<String>,
    #[serde(flatten)]
    score: QueryScore,
    latency_ms: f64,
    top_k: Vec<TopHit>,
}

#[derive(Debug, Serialize)]
pub struct LatencyStats {
    pub avg_ms: f64,
    pub p50_ms: f64,
}

/// One mode's complete benchmark run over the golden set.
#[derive(Debug, Serialize)]
pub struct ModeRun {
    pub mode: &'static str,
    /// "id", "title", or "mixed" across the file's queries.
    pub matching: &'static str,
    pub k: usize,
    pub overall: AggregateMetrics,
    /// Empty when no query carries a `query_type` label (v1 golden set).
    pub strata: BTreeMap<String, AggregateMetrics>,
    pub latency: LatencyStats,
    pub(super) query_results: Vec<QueryOutcome>,
}

impl ModeRun {
    fn best_ranks(&self) -> Vec<Option<usize>> {
        self.query_results.iter().map(|o| o.score.best_rank).collect()
    }
}

/// Pairwise win/loss/tie between two modes, on per-query best rank.
#[derive(Debug, Serialize)]
pub struct PairwiseComparison {
    pub mode_a: &'static str,
    pub mode_b: &'static str,
    #[serde(flatten)]
    pub result: WinLossTie,
}

pub(super) fn run_quality_benchmark(
    conn: &Connection,
    args: &QualityArgs,
    output_mode: OutputMode,
) -> Result<()> {
    let content = fs::read_to_string(args.file)
        .with_context(|| format!("Failed to read benchmark file: {}", args.file.display()))?;
    let queries = parse_benchmark(&content)?;
    let title_map = build_title_map(conn)?;

    let modes: Vec<QualityMode> = if args.compare.is_empty() {
        vec![args.mode]
    } else {
        args.compare.to_vec()
    };

    let mut runs = Vec::with_capacity(modes.len());
    for mode in modes {
        let retriever = Retriever::build(mode, conn)?;
        runs.push(run_queries(
            |q| retriever.retrieve(q),
            &queries,
            &title_map,
            mode,
            args.k,
        )?);
    }

    let comparisons = pairwise_comparisons(&runs);

    match output_mode {
        OutputMode::Json => print_json(&runs, &comparisons)?,
        OutputMode::Tty => print_tty(&runs, &comparisons, args.detail),
    }

    if args.record {
        record_runs(&runs, args)?;
    }

    Ok(())
}

/// Persist every run to the results ledger in the benchmarks directory.
/// Confirmation goes to stderr so --json stdout stays parseable.
fn record_runs(runs: &[ModeRun], args: &QualityArgs) -> Result<()> {
    let benchmarks_dir = crate::platform::data_dir()?.join("benchmarks");
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let set = args
        .file
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| args.file.display().to_string());
    let binary = format!("grans {}", env!("GRANS_VERSION"));
    let db = match args.db {
        Some(path) => path.display().to_string(),
        None => crate::db::connection::default_db_path()?.display().to_string(),
    };

    let ctx = super::ledger::RecordContext {
        benchmarks_dir: &benchmarks_dir,
        date: &date,
        set: &set,
        binary: &binary,
        db: &db,
        note: args.note,
    };
    for run in runs {
        let run_path = super::ledger::record_run(&ctx, run)?;
        eprintln!("Recorded {} run: {}", run.mode, run_path.display());
    }
    eprintln!(
        "Ledger updated: {}",
        benchmarks_dir.join("ledger.jsonl").display()
    );
    Ok(())
}

/// Parse and validate a benchmark file's contents.
fn parse_benchmark(content: &str) -> Result<Vec<BenchmarkQuery>> {
    let file: BenchmarkFile =
        serde_json::from_str(content).with_context(|| "Failed to parse benchmark JSON")?;

    if file.queries.is_empty() {
        bail!("Benchmark file contains no queries");
    }
    for (i, q) in file.queries.iter().enumerate() {
        if q.query.trim().is_empty() {
            bail!("Benchmark query {} has an empty query string", i + 1);
        }
        if q.relevant_meetings.is_empty() && q.relevant_meeting_ids.is_empty() {
            bail!("Benchmark query {} ({:?}) has no relevance labels", i + 1, q.query);
        }
    }

    Ok(file.queries)
}

/// Run every query through one retrieval function and score the results.
fn run_queries<F>(
    retrieve: F,
    queries: &[BenchmarkQuery],
    title_map: &HashMap<String, String>,
    mode: QualityMode,
    k: usize,
) -> Result<ModeRun>
where
    F: Fn(&str) -> Result<Vec<RankedDoc>>,
{
    let mut outcomes = Vec::with_capacity(queries.len());
    for bq in queries {
        let start = Instant::now();
        let ranked = retrieve(&bq.query)?;
        let latency_ms = start.elapsed().as_secs_f64() * 1000.0;

        let matcher = bq.matcher(title_map);
        let score = metrics::score_query(&ranked, &matcher, k);
        outcomes.push(build_outcome(bq, &ranked, matcher.method(), score, latency_ms, title_map, k));
    }

    let overall = metrics::aggregate(outcomes.iter().map(|o| &o.score));
    let strata = build_strata(&outcomes);
    let latency = latency_stats(&outcomes);
    let methods: Vec<&'static str> = outcomes.iter().map(|o| o.matching).collect();

    Ok(ModeRun {
        mode: mode.as_str(),
        matching: matching_summary(&methods),
        k,
        overall,
        strata,
        latency,
        query_results: outcomes,
    })
}

fn build_outcome(
    bq: &BenchmarkQuery,
    ranked: &[RankedDoc],
    matching: &'static str,
    score: QueryScore,
    latency_ms: f64,
    title_map: &HashMap<String, String>,
    k: usize,
) -> QueryOutcome {
    let top_k = ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, doc)| TopHit {
            rank: i + 1,
            document_id: doc.document_id.clone(),
            title: title_map.get(&doc.document_id).cloned(),
            score: doc.score,
        })
        .collect();

    QueryOutcome {
        query: bq.query.clone(),
        query_type: bq.query_type.clone(),
        matching,
        expected: bq.expected_display().to_vec(),
        score,
        latency_ms,
        top_k,
    }
}

/// Per-stratum aggregation, keyed by `query_type`. Empty when no query in
/// the file carries a stratum label; queries missing one in a labeled file
/// land in "unlabeled".
fn build_strata(outcomes: &[QueryOutcome]) -> BTreeMap<String, AggregateMetrics> {
    if outcomes.iter().all(|o| o.query_type.is_none()) {
        return BTreeMap::new();
    }
    let pairs: Vec<(&str, &QueryScore)> = outcomes
        .iter()
        .map(|o| (o.query_type.as_deref().unwrap_or("unlabeled"), &o.score))
        .collect();
    metrics::aggregate_by_stratum(&pairs)
}

fn latency_stats(outcomes: &[QueryOutcome]) -> LatencyStats {
    let mut latencies: Vec<f64> = outcomes.iter().map(|o| o.latency_ms).collect();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    LatencyStats {
        avg_ms: latencies.iter().sum::<f64>() / latencies.len() as f64,
        p50_ms: percentile(&latencies, 50.0),
    }
}

/// Run-level matching method: "id" or "title" when uniform, "mixed" otherwise.
fn matching_summary(methods: &[&'static str]) -> &'static str {
    match methods.first() {
        Some(first) if methods.iter().all(|m| m == first) => first,
        Some(_) => "mixed",
        None => "mixed",
    }
}

/// Win/loss/tie for every ordered pair of runs (earlier mode as A).
fn pairwise_comparisons(runs: &[ModeRun]) -> Vec<PairwiseComparison> {
    let mut comparisons = Vec::new();
    for (i, a) in runs.iter().enumerate() {
        for b in &runs[i + 1..] {
            comparisons.push(PairwiseComparison {
                mode_a: a.mode,
                mode_b: b.mode,
                result: metrics::compare_ranks(&a.best_ranks(), &b.best_ranks()),
            });
        }
    }
    comparisons
}

fn build_title_map(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, title FROM documents")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (id, title) = row?;
        map.insert(id, title);
    }
    Ok(map)
}

// === Output ===

fn print_json(runs: &[ModeRun], comparisons: &[PairwiseComparison]) -> Result<()> {
    if runs.len() == 1 {
        println!("{}", serde_json::to_string_pretty(&runs[0])?);
    } else {
        let combined = serde_json::json!({
            "runs": runs,
            "comparisons": comparisons,
        });
        println!("{}", serde_json::to_string_pretty(&combined)?);
    }
    Ok(())
}

fn print_tty(runs: &[ModeRun], comparisons: &[PairwiseComparison], detail: bool) {
    for run in runs {
        print_run_tty(run, detail);
        println!();
    }
    if runs.len() > 1 {
        print_compare_tty(runs, comparisons);
    }
}

fn print_run_tty(run: &ModeRun, detail: bool) {
    use colored::Colorize;

    let heading = format!("Search Quality Benchmark ({})", run.mode);
    println!("{}", heading.bold().cyan());
    println!("{}", "=".repeat(heading.len()).cyan());
    println!();
    println!("{:24} {:>10}", "Queries:".bold(), run.overall.n);
    println!("{:24} {:>10}", "k:".bold(), run.k);
    println!("{:24} {:>10}", "Matching:".bold(), run.matching);
    println!(
        "{:24} {:>10}",
        "Matches in top k:".bold(),
        run.overall.queries_with_match
    );
    println!();
    println!(
        "{:24} {:>10.1}%",
        format!("hit-rate@{}:", run.k).bold().green(),
        run.overall.hit_rate_at_k * 100.0
    );
    println!(
        "{:24} {:>10.1}%",
        format!("recall@{}:", run.k).bold().green(),
        run.overall.recall_at_k * 100.0
    );
    println!(
        "{:24} {:>10.3}",
        format!("MRR@{}:", run.k).bold().green(),
        run.overall.mrr
    );
    println!(
        "{:24} {:>10}",
        "Latency (avg / p50):".bold(),
        format!("{:.1} / {:.1} ms", run.latency.avg_ms, run.latency.p50_ms)
    );

    if !run.strata.is_empty() {
        println!();
        println!(
            "{:<12} {:>4} {:>10} {:>10} {:>8}",
            "Stratum".bold().yellow(),
            "n",
            "hit-rate",
            "recall",
            "MRR"
        );
        for (stratum, agg) in &run.strata {
            println!(
                "{:<12} {:>4} {:>9.1}% {:>9.1}% {:>8.3}",
                stratum,
                agg.n,
                agg.hit_rate_at_k * 100.0,
                agg.recall_at_k * 100.0,
                agg.mrr
            );
        }
    }

    if detail {
        print_detail_tty(run);
    }
}

fn print_detail_tty(run: &ModeRun) {
    use colored::Colorize;

    println!();
    println!("{}", "Query Details".bold().yellow());
    println!("{}", "-".repeat(13).yellow());

    for qr in &run.query_results {
        let status = if qr.score.found_in_top_k {
            "PASS".green()
        } else {
            "MISS".red()
        };

        println!();
        println!("[{}] {}", status, qr.query.bold());
        println!("  Expected: {}", qr.expected.join(", "));
        match qr.score.best_rank {
            Some(rank) => {
                let score_note = qr
                    .score
                    .best_score
                    .map(|s| format!(" (score: {:.3})", s))
                    .unwrap_or_default();
                println!("  First relevant at rank {}{}", rank, score_note);
            }
            None => {
                println!("  Not found");
                let got = qr
                    .top_k
                    .first()
                    .and_then(|h| h.title.as_deref())
                    .unwrap_or("(no results)");
                println!("  Got: {}", got);
            }
        }
    }
}

fn print_compare_tty(runs: &[ModeRun], comparisons: &[PairwiseComparison]) {
    use colored::Colorize;

    println!("{}", "Per-query rank of first relevant".bold().cyan());
    println!("{}", "-".repeat(32).cyan());
    for run in runs {
        print!("{:>10}", run.mode);
    }
    println!("  query");

    let n = runs[0].query_results.len();
    for i in 0..n {
        for run in runs {
            let cell = run.query_results[i]
                .score
                .best_rank
                .map(|r| r.to_string())
                .unwrap_or_else(|| "-".to_string());
            print!("{:>10}", cell);
        }
        let query = &runs[0].query_results[i].query;
        let truncated: String = query.chars().take(60).collect();
        println!("  {}", truncated);
    }

    println!();
    for cmp in comparisons {
        println!(
            "{}: {} wins / {} losses / {} ties (on best rank)",
            format!("{} vs {}", cmp.mode_a, cmp.mode_b).bold(),
            cmp.result.wins,
            cmp.result.losses,
            cmp.result.ties
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2_JSON: &str = r#"{
        "description": "test set",
        "created": "2026-07-10",
        "queries": [
            {
                "query": "quarterly budget review",
                "query_type": "exact-term",
                "provenance": "v2",
                "relevant_meetings": ["Budget Review"],
                "relevant_meeting_ids": ["doc-a"],
                "rationale": "test"
            },
            {
                "query": "planning the offsite",
                "query_type": "paraphrase",
                "provenance": "v2",
                "relevant_meetings": ["Offsite Planning"],
                "relevant_meeting_ids": ["doc-b", "doc-c"],
                "rationale": "test"
            }
        ]
    }"#;

    const V1_JSON: &str = r#"{
        "description": "test set",
        "queries": [
            {
                "query": "quarterly budget review",
                "relevant_meetings": ["Budget Review"],
                "rationale": "test"
            }
        ]
    }"#;

    #[test]
    fn parse_v2_file_prefers_id_matching() {
        let queries = parse_benchmark(V2_JSON).unwrap();
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].query_type.as_deref(), Some("exact-term"));
        assert_eq!(queries[0].relevant_meeting_ids, vec!["doc-a"]);

        let title_map = HashMap::new();
        let matcher = queries[0].matcher(&title_map);
        assert_eq!(matcher.method(), "id");
        assert_eq!(matcher.label_count(), 1);
    }

    #[test]
    fn parse_v1_file_falls_back_to_title_matching() {
        let queries = parse_benchmark(V1_JSON).unwrap();
        assert_eq!(queries.len(), 1);
        assert!(queries[0].query_type.is_none());

        let title_map = HashMap::new();
        let matcher = queries[0].matcher(&title_map);
        assert_eq!(matcher.method(), "title");
    }

    #[test]
    fn parse_rejects_query_without_labels() {
        let json = r#"{"queries": [{"query": "anything", "rationale": "x"}]}"#;
        let err = parse_benchmark(json).unwrap_err();
        assert!(err.to_string().contains("no relevance labels"));
    }

    #[test]
    fn parse_rejects_empty_file() {
        let json = r#"{"queries": []}"#;
        assert!(parse_benchmark(json).is_err());
    }

    #[test]
    fn matching_summary_uniform_and_mixed() {
        assert_eq!(matching_summary(&["id", "id"]), "id");
        assert_eq!(matching_summary(&["title"]), "title");
        assert_eq!(matching_summary(&["id", "title"]), "mixed");
    }

    fn stub_ranked(ids: &[&str]) -> Vec<RankedDoc> {
        ids.iter()
            .map(|id| RankedDoc {
                document_id: id.to_string(),
                score: None,
            })
            .collect()
    }

    #[test]
    fn run_queries_scores_and_aggregates() {
        let queries = parse_benchmark(V2_JSON).unwrap();
        let title_map = HashMap::new();

        // First query: doc-a at rank 1. Second query: no relevant doc found.
        let run = run_queries(
            |q| {
                Ok(if q.starts_with("quarterly") {
                    stub_ranked(&["doc-a", "doc-x"])
                } else {
                    stub_ranked(&["doc-x", "doc-y"])
                })
            },
            &queries,
            &title_map,
            QualityMode::Fts,
            10,
        )
        .unwrap();

        assert_eq!(run.mode, "fts");
        assert_eq!(run.matching, "id");
        assert_eq!(run.k, 10);
        assert_eq!(run.overall.n, 2);
        assert_eq!(run.overall.queries_with_match, 1);
        assert!((run.overall.hit_rate_at_k - 0.5).abs() < 1e-9);
        assert!((run.overall.mrr - 0.5).abs() < 1e-9);

        assert_eq!(run.strata.len(), 2);
        assert_eq!(run.strata["exact-term"].n, 1);
        assert_eq!(run.strata["exact-term"].queries_with_match, 1);
        assert_eq!(run.strata["paraphrase"].queries_with_match, 0);

        assert_eq!(run.query_results.len(), 2);
        assert_eq!(run.query_results[0].score.best_rank, Some(1));
        assert_eq!(run.query_results[0].top_k.len(), 2);
        assert!(run.query_results[0].latency_ms >= 0.0);
    }

    #[test]
    fn run_queries_v1_has_no_strata() {
        let queries = parse_benchmark(V1_JSON).unwrap();
        let title_map: HashMap<String, String> =
            [("doc-a".to_string(), "Budget Review".to_string())].into();

        let run = run_queries(
            |_| Ok(stub_ranked(&["doc-a"])),
            &queries,
            &title_map,
            QualityMode::Semantic,
            10,
        )
        .unwrap();

        assert_eq!(run.matching, "title");
        assert!(run.strata.is_empty());
        assert_eq!(run.overall.queries_with_match, 1);
    }

    #[test]
    fn pairwise_comparisons_use_best_ranks() {
        let queries = parse_benchmark(V2_JSON).unwrap();
        let title_map = HashMap::new();

        // Mode A: hits query 1 at rank 1, misses query 2.
        let run_a = run_queries(
            |q| {
                Ok(if q.starts_with("quarterly") {
                    stub_ranked(&["doc-a"])
                } else {
                    stub_ranked(&["doc-x"])
                })
            },
            &queries,
            &title_map,
            QualityMode::Fts,
            10,
        )
        .unwrap();

        // Mode B: hits query 1 at rank 2, hits query 2 at rank 1.
        let run_b = run_queries(
            |q| {
                Ok(if q.starts_with("quarterly") {
                    stub_ranked(&["doc-x", "doc-a"])
                } else {
                    stub_ranked(&["doc-b"])
                })
            },
            &queries,
            &title_map,
            QualityMode::Semantic,
            10,
        )
        .unwrap();

        let comparisons = pairwise_comparisons(&[run_a, run_b]);

        assert_eq!(comparisons.len(), 1);
        let cmp = &comparisons[0];
        assert_eq!(cmp.mode_a, "fts");
        assert_eq!(cmp.mode_b, "semantic");
        assert_eq!(cmp.result.wins, 1);
        assert_eq!(cmp.result.losses, 1);
        assert_eq!(cmp.result.ties, 0);
    }

    #[test]
    fn single_run_has_no_comparisons() {
        let queries = parse_benchmark(V1_JSON).unwrap();
        let title_map = HashMap::new();
        let run = run_queries(
            |_| Ok(vec![]),
            &queries,
            &title_map,
            QualityMode::Fts,
            10,
        )
        .unwrap();

        assert!(pairwise_comparisons(&[run]).is_empty());
    }
}
