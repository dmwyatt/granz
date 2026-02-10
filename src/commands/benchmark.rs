//! Benchmark command: measure semantic search performance.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rand::prelude::*;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::cli::args::BenchmarkAction;
use crate::embed;
use crate::embed::search::cosine_similarity;
use crate::embed::store::load_all_vectors;
use crate::output::format::OutputMode;

/// Vector dimension for nomic-embed-text-v1.5
const EMBEDDING_DIM: usize = 768;

/// Benchmark results with statistics
#[derive(Debug, Serialize)]
pub struct BenchmarkResults {
    pub mode: String,
    pub vector_count: usize,
    pub query_count: usize,
    pub warmup_count: usize,
    pub min_score: f32,

    // Latency stats (in milliseconds)
    pub avg_latency_ms: f64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub min_latency_ms: f64,
    pub max_latency_ms: f64,

    // Throughput
    pub queries_per_sec: f64,
    pub vectors_per_sec: f64,
    pub total_time_ms: f64,
}

pub fn run(conn: &Connection, action: &BenchmarkAction, output_mode: OutputMode) -> Result<()> {
    match action {
        BenchmarkAction::SemanticSearch {
            queries,
            synthetic,
            vectors,
            warmup,
            min_score,
        } => run_semantic_search_benchmark(
            conn,
            *queries,
            *synthetic,
            *vectors,
            *warmup,
            *min_score,
            output_mode,
        ),
        BenchmarkAction::Quality { file, k, detail } => {
            run_quality_benchmark(conn, file, *k, *detail, output_mode)
        }
    }
}

fn run_semantic_search_benchmark(
    conn: &Connection,
    query_count: usize,
    synthetic: bool,
    vector_count: usize,
    warmup_count: usize,
    min_score: f32,
    output_mode: OutputMode,
) -> Result<()> {
    // Load or generate vectors
    let (vectors, mode) = if synthetic {
        let vecs = generate_synthetic_vectors(vector_count);
        (vecs, "Synthetic".to_string())
    } else {
        let vecs = load_real_vectors(conn)?;
        (vecs, "Real".to_string())
    };

    if vectors.is_empty() {
        bail!("No vectors available for benchmarking. Run `grans embed` first to generate embeddings, or use --synthetic mode.");
    }

    let actual_vector_count = vectors.len();

    // Generate query vectors
    let query_vectors = generate_query_vectors(query_count + warmup_count);

    // Warmup
    for query_vec in query_vectors.iter().take(warmup_count) {
        run_single_search(query_vec, &vectors, min_score);
    }

    // Benchmark
    let mut latencies: Vec<Duration> = Vec::with_capacity(query_count);

    for query_vec in query_vectors.iter().skip(warmup_count).take(query_count) {
        let start = Instant::now();
        run_single_search(query_vec, &vectors, min_score);
        latencies.push(start.elapsed());
    }

    // Calculate statistics
    let results = calculate_results(
        mode,
        actual_vector_count,
        query_count,
        warmup_count,
        min_score,
        &mut latencies,
    );

    // Output results
    match output_mode {
        OutputMode::Json => print_json(&results)?,
        OutputMode::Tty => print_tty(&results),
    }

    Ok(())
}

fn load_real_vectors(conn: &Connection) -> Result<Vec<Vec<f32>>> {
    let stored = load_all_vectors(conn)?;
    Ok(stored.into_iter().map(|sv| sv.vector).collect())
}

fn generate_synthetic_vectors(count: usize) -> Vec<Vec<f32>> {
    let mut rng = thread_rng();
    (0..count)
        .map(|_| {
            let mut vec: Vec<f32> = (0..EMBEDDING_DIM)
                .map(|_| rng.r#gen::<f32>() - 0.5)
                .collect();
            normalize(&mut vec);
            vec
        })
        .collect()
}

fn generate_query_vectors(count: usize) -> Vec<Vec<f32>> {
    generate_synthetic_vectors(count)
}

fn normalize(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in vec.iter_mut() {
            *x /= norm;
        }
    }
}

fn run_single_search(query: &[f32], vectors: &[Vec<f32>], min_score: f32) -> Vec<(usize, f32)> {
    vectors
        .iter()
        .enumerate()
        .filter_map(|(idx, v)| {
            let score = cosine_similarity(query, v);
            if score >= min_score {
                Some((idx, score))
            } else {
                None
            }
        })
        .collect()
}

fn calculate_results(
    mode: String,
    vector_count: usize,
    query_count: usize,
    warmup_count: usize,
    min_score: f32,
    latencies: &mut [Duration],
) -> BenchmarkResults {
    latencies.sort();

    let total_time: Duration = latencies.iter().sum();
    let total_time_ms = total_time.as_secs_f64() * 1000.0;

    let latencies_ms: Vec<f64> = latencies.iter().map(|d| d.as_secs_f64() * 1000.0).collect();

    let avg_latency_ms = latencies_ms.iter().sum::<f64>() / latencies_ms.len() as f64;
    let min_latency_ms = latencies_ms.first().copied().unwrap_or(0.0);
    let max_latency_ms = latencies_ms.last().copied().unwrap_or(0.0);

    let p50_latency_ms = percentile(&latencies_ms, 50.0);
    let p95_latency_ms = percentile(&latencies_ms, 95.0);
    let p99_latency_ms = percentile(&latencies_ms, 99.0);

    let queries_per_sec = if total_time_ms > 0.0 {
        query_count as f64 / (total_time_ms / 1000.0)
    } else {
        0.0
    };

    let vectors_per_sec = queries_per_sec * vector_count as f64;

    BenchmarkResults {
        mode,
        vector_count,
        query_count,
        warmup_count,
        min_score,
        avg_latency_ms,
        p50_latency_ms,
        p95_latency_ms,
        p99_latency_ms,
        min_latency_ms,
        max_latency_ms,
        queries_per_sec,
        vectors_per_sec,
        total_time_ms,
    }
}

fn percentile(sorted_values: &[f64], p: f64) -> f64 {
    if sorted_values.is_empty() {
        return 0.0;
    }
    let idx = (p / 100.0 * (sorted_values.len() - 1) as f64).round() as usize;
    sorted_values[idx.min(sorted_values.len() - 1)]
}

fn print_json(results: &BenchmarkResults) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(results)?);
    Ok(())
}

fn print_tty(results: &BenchmarkResults) {
    use colored::Colorize;

    println!("{}", "Semantic Search Benchmark".bold().cyan());
    println!("{}", "-".repeat(25).cyan());
    println!();
    println!("{:16} {}", "Mode:".bold(), results.mode);
    println!(
        "{:16} {:>12}",
        "Vectors:".bold(),
        format_number(results.vector_count)
    );
    println!(
        "{:16} {:>12}",
        "Queries:".bold(),
        format_number(results.query_count)
    );
    println!(
        "{:16} {:>12}",
        "Warmup:".bold(),
        format_number(results.warmup_count)
    );
    println!(
        "{:16} {:>12}",
        "Min Score:".bold(),
        format!("{:.2}", results.min_score)
    );
    println!();
    println!("{}", "Latency".bold().yellow());
    println!("{}", "-".repeat(7).yellow());
    println!(
        "{:16} {:>12}",
        "Average:",
        format!("{:.3} ms", results.avg_latency_ms)
    );
    println!(
        "{:16} {:>12}",
        "p50:",
        format!("{:.3} ms", results.p50_latency_ms)
    );
    println!(
        "{:16} {:>12}",
        "p95:",
        format!("{:.3} ms", results.p95_latency_ms)
    );
    println!(
        "{:16} {:>12}",
        "p99:",
        format!("{:.3} ms", results.p99_latency_ms)
    );
    println!(
        "{:16} {:>12}",
        "Min:",
        format!("{:.3} ms", results.min_latency_ms)
    );
    println!(
        "{:16} {:>12}",
        "Max:",
        format!("{:.3} ms", results.max_latency_ms)
    );
    println!();
    println!("{}", "Throughput".bold().green());
    println!("{}", "-".repeat(10).green());
    println!(
        "{:16} {:>12}",
        "Queries/sec:",
        format!("{:.1}", results.queries_per_sec)
    );
    println!(
        "{:16} {:>12}",
        "Vectors/sec:",
        format_number(results.vectors_per_sec as usize)
    );
    println!(
        "{:16} {:>12}",
        "Total time:",
        format!("{:.1} ms", results.total_time_ms)
    );
}

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// === Quality Benchmark ===

/// A single test query from the benchmark file
#[derive(Debug, Deserialize)]
struct BenchmarkQuery {
    query: String,
    relevant_meetings: Vec<String>,
    #[allow(dead_code)]
    rationale: String,
}

/// The benchmark file format
#[derive(Debug, Deserialize)]
struct BenchmarkFile {
    #[allow(dead_code)]
    description: String,
    queries: Vec<BenchmarkQuery>,
}

/// Results for a single query
#[derive(Debug, Serialize)]
struct QueryResult {
    query: String,
    expected: Vec<String>,
    found: Vec<String>,
    found_in_top_k: bool,
    best_rank: Option<usize>,
    best_score: Option<f32>,
}

/// Overall quality benchmark results
#[derive(Debug, Serialize)]
struct QualityResults {
    total_queries: usize,
    queries_with_match: usize,
    precision_at_k: f64,
    mean_reciprocal_rank: f64,
    k: usize,
    query_results: Vec<QueryResult>,
}

fn run_quality_benchmark(
    conn: &Connection,
    file: &Path,
    k: usize,
    detail: bool,
    output_mode: OutputMode,
) -> Result<()> {
    // Load benchmark file
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read benchmark file: {}", file.display()))?;
    let benchmark: BenchmarkFile = serde_json::from_str(&content)
        .with_context(|| "Failed to parse benchmark JSON")?;

    if benchmark.queries.is_empty() {
        bail!("Benchmark file contains no queries");
    }

    // Build a map of meeting titles for lookup
    let title_map = build_title_map(conn)?;

    let mut query_results = Vec::new();
    let mut total_reciprocal_rank = 0.0;

    for bq in &benchmark.queries {
        // Run semantic search
        let (results, _total) = embed::semantic_search(conn, &bq.query, None, k, None, false)?;

        // Get result titles
        let result_titles: Vec<String> = results
            .iter()
            .filter_map(|r| title_map.get(&r.document_id).cloned())
            .collect();

        // Check if any expected meeting is in results
        let expected_set: HashSet<&str> = bq.relevant_meetings.iter().map(|s| s.as_str()).collect();

        let mut best_rank: Option<usize> = None;
        let mut best_score: Option<f32> = None;

        for (i, result) in results.iter().enumerate() {
            if let Some(title) = title_map.get(&result.document_id) {
                if expected_set.contains(title.as_str()) {
                    if best_rank.is_none() {
                        best_rank = Some(i + 1);
                        best_score = Some(result.score);
                    }
                }
            }
        }

        let found_in_top_k = best_rank.is_some();

        // Calculate reciprocal rank (1/rank if found, 0 if not)
        if let Some(rank) = best_rank {
            total_reciprocal_rank += 1.0 / rank as f64;
        }

        query_results.push(QueryResult {
            query: bq.query.clone(),
            expected: bq.relevant_meetings.clone(),
            found: result_titles,
            found_in_top_k,
            best_rank,
            best_score,
        });
    }

    let queries_with_match = query_results.iter().filter(|r| r.found_in_top_k).count();
    let precision_at_k = queries_with_match as f64 / query_results.len() as f64;
    let mean_reciprocal_rank = total_reciprocal_rank / query_results.len() as f64;

    let results = QualityResults {
        total_queries: query_results.len(),
        queries_with_match,
        precision_at_k,
        mean_reciprocal_rank,
        k,
        query_results,
    };

    match output_mode {
        OutputMode::Json => print_quality_json(&results)?,
        OutputMode::Tty => print_quality_tty(&results, detail),
    }

    Ok(())
}

fn build_title_map(conn: &Connection) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, title FROM documents")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (id, title) = row?;
        map.insert(id, title);
    }
    Ok(map)
}

fn print_quality_json(results: &QualityResults) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(results)?);
    Ok(())
}

fn print_quality_tty(results: &QualityResults, detail: bool) {
    use colored::Colorize;

    println!("{}", "Semantic Search Quality Benchmark".bold().cyan());
    println!("{}", "=".repeat(35).cyan());
    println!();
    println!("{:24} {:>10}", "Total queries:".bold(), results.total_queries);
    println!("{:24} {:>10}", "Matches in top k:".bold(), results.queries_with_match);
    println!("{:24} {:>10}", "k:".bold(), results.k);
    println!();
    println!(
        "{:24} {:>10.1}%",
        "Precision@k:".bold().green(),
        results.precision_at_k * 100.0
    );
    println!(
        "{:24} {:>10.3}",
        "Mean Reciprocal Rank:".bold().green(),
        results.mean_reciprocal_rank
    );

    if detail {
        println!();
        println!("{}", "Query Details".bold().yellow());
        println!("{}", "-".repeat(13).yellow());

        for qr in &results.query_results {
            let status = if qr.found_in_top_k {
                "PASS".green()
            } else {
                "MISS".red()
            };

            println!();
            println!("[{}] {}", status, qr.query.bold());
            println!("  Expected: {}", qr.expected.join(", "));
            if let (Some(rank), Some(score)) = (qr.best_rank, qr.best_score) {
                println!("  Found at rank {} (score: {:.3})", rank, score);
            } else {
                println!("  Not found in top {}", results.k);
                println!("  Got: {}", qr.found.first().unwrap_or(&"(no results)".to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_synthetic_vectors() {
        let vecs = generate_synthetic_vectors(10);
        assert_eq!(vecs.len(), 10);
        for v in &vecs {
            assert_eq!(v.len(), EMBEDDING_DIM);
            // Check normalization (L2 norm should be ~1.0)
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(
                (norm - 1.0).abs() < 1e-5,
                "Vector not normalized: norm = {}",
                norm
            );
        }
    }

    #[test]
    fn test_normalize() {
        let mut vec = vec![3.0, 4.0];
        normalize(&mut vec);
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((vec[0] - 0.6).abs() < 1e-6);
        assert!((vec[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_zero_vector() {
        let mut vec = vec![0.0, 0.0, 0.0];
        normalize(&mut vec); // Should not panic
        assert_eq!(vec, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_percentile() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        // p50 of [1..10]: index = (0.50 * 9).round() = 5, value = 6.0
        assert!((percentile(&values, 50.0) - 6.0).abs() < 1e-6);
        assert!((percentile(&values, 0.0) - 1.0).abs() < 1e-6);
        assert!((percentile(&values, 100.0) - 10.0).abs() < 1e-6);
        // p90: index = (0.90 * 9).round() = 8, value = 9.0
        assert!((percentile(&values, 90.0) - 9.0).abs() < 1e-6);
    }

    #[test]
    fn test_percentile_empty() {
        let values: Vec<f64> = vec![];
        assert_eq!(percentile(&values, 50.0), 0.0);
    }

    #[test]
    fn test_percentile_single() {
        let values = vec![42.0];
        assert_eq!(percentile(&values, 50.0), 42.0);
        assert_eq!(percentile(&values, 0.0), 42.0);
        assert_eq!(percentile(&values, 100.0), 42.0);
    }

    #[test]
    fn test_run_single_search() {
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.7071, 0.7071, 0.0],
        ];
        let query = vec![1.0, 0.0, 0.0];

        let results = run_single_search(&query, &vectors, 0.0);
        assert_eq!(results.len(), 3);

        // With high min_score, only first vector should match
        let results = run_single_search(&query, &vectors, 0.9);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 0);
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(12345), "12,345");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn test_calculate_results() {
        let mut latencies = vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
            Duration::from_millis(40),
            Duration::from_millis(50),
        ];

        let results = calculate_results("Test".to_string(), 1000, 5, 2, 0.0, &mut latencies);

        assert_eq!(results.mode, "Test");
        assert_eq!(results.vector_count, 1000);
        assert_eq!(results.query_count, 5);
        assert_eq!(results.warmup_count, 2);
        assert!((results.avg_latency_ms - 30.0).abs() < 0.1);
        assert!((results.min_latency_ms - 10.0).abs() < 0.1);
        assert!((results.max_latency_ms - 50.0).abs() < 0.1);
        assert!((results.total_time_ms - 150.0).abs() < 0.1);
    }
}
