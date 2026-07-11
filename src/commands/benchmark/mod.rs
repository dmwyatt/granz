//! Benchmark commands: search performance and quality measurement.

mod dump;
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
            dump_candidates,
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
                dump_candidates: dump_candidates.as_deref(),
            };
            quality::run_quality_benchmark(conn, &args, output_mode)
        }
    }
}
