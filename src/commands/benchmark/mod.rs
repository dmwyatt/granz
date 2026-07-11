//! Benchmark commands: search performance and quality measurement.

mod metrics;
mod perf;
mod quality;
mod retriever;

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
        BenchmarkAction::Quality {
            file,
            k,
            mode,
            compare,
            detail,
        } => {
            let args = quality::QualityArgs {
                file,
                k: *k,
                mode: *mode,
                compare,
                detail: *detail,
            };
            quality::run_quality_benchmark(conn, &args, output_mode)
        }
    }
}
