//! Search quality benchmark: score search results against a labeled golden set.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::embed;
use crate::output::format::OutputMode;

/// A single test query from the benchmark file
#[derive(Debug, Deserialize)]
struct BenchmarkQuery {
    query: String,
    relevant_meetings: Vec<String>,
}

/// The benchmark file format
#[derive(Debug, Deserialize)]
struct BenchmarkFile {
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

pub fn run_quality_benchmark(
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
