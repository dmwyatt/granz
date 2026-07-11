//! Terminal and JSON rendering for quality benchmark results.

use anyhow::Result;

use super::quality::{ModeRun, PairwiseComparison};

pub(super) fn print_json(runs: &[ModeRun], comparisons: &[PairwiseComparison]) -> Result<()> {
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

pub(super) fn print_tty(runs: &[ModeRun], comparisons: &[PairwiseComparison], detail: bool) {
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
