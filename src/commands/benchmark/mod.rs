//! Benchmark commands: search performance and quality measurement.

mod perf;
mod quality;

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::args::BenchmarkAction;
use crate::output::format::OutputMode;

pub fn run(conn: &Connection, action: &BenchmarkAction, output_mode: OutputMode) -> Result<()> {
    match action {
        BenchmarkAction::SemanticSearch {
            queries,
            synthetic,
            vectors,
            warmup,
            min_score,
        } => perf::run_semantic_search_benchmark(
            conn,
            *queries,
            *synthetic,
            *vectors,
            *warmup,
            *min_score,
            output_mode,
        ),
        BenchmarkAction::Quality { file, k, detail } => {
            quality::run_quality_benchmark(conn, file, *k, *detail, output_mode)
        }
    }
}
