//! Benchmark commands: search performance and quality measurement.

mod ledger;
mod metrics;
mod perf;
mod quality;
mod report;
mod retriever;

use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

use crate::cli::args::BenchmarkAction;
use crate::output::format::OutputMode;

pub fn run(
    conn: &Connection,
    action: &BenchmarkAction,
    db_path: Option<&Path>,
    output_mode: OutputMode,
) -> Result<()> {
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
            record,
            note,
        } => {
            let args = quality::QualityArgs {
                file,
                k: *k,
                mode: *mode,
                compare,
                detail: *detail,
                record: *record,
                note: note.as_deref(),
                db: db_path,
            };
            quality::run_quality_benchmark(conn, &args, output_mode)
        }
    }
}
