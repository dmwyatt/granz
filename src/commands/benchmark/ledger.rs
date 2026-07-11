//! Results ledger: persist quality benchmark runs next to the golden set.
//!
//! Each recorded run appends one JSON line to `ledger.jsonl` in the
//! benchmarks directory and writes the full per-query output under `runs/`,
//! so capture does not depend on remembering to do it by hand.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use super::metrics::AggregateMetrics;
use super::quality::{LatencyStats, ModeRun};

/// Run-independent context for recording: where, when, and what was measured.
pub(super) struct RecordContext<'a> {
    pub benchmarks_dir: &'a Path,
    /// Run date, YYYY-MM-DD.
    pub date: &'a str,
    /// Golden-set file name.
    pub set: &'a str,
    /// Binary version string.
    pub binary: &'a str,
    /// Database the queries ran against.
    pub db: &'a str,
    pub note: Option<&'a str>,
}

/// One line in ledger.jsonl.
#[derive(Serialize)]
struct LedgerEntry<'a> {
    date: &'a str,
    set: &'a str,
    mode: &'a str,
    matching: &'a str,
    k: usize,
    queries: usize,
    hit_rate_at_k: f64,
    recall_at_k: f64,
    mrr: f64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    strata: &'a BTreeMap<String, AggregateMetrics>,
    latency_ms: &'a LatencyStats,
    binary: &'a str,
    db: &'a str,
    /// Path of the full per-query output, relative to the benchmarks dir.
    per_query_results: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<&'a str>,
}

/// Full per-query output saved under runs/.
#[derive(Serialize)]
struct RunFile<'a> {
    date: &'a str,
    set: &'a str,
    binary: &'a str,
    db: &'a str,
    #[serde(flatten)]
    run: &'a ModeRun,
}

/// Record one mode's run: write the per-query output under `runs/` and
/// append a summary line to `ledger.jsonl`. Returns the run-file path.
pub(super) fn record_run(ctx: &RecordContext, run: &ModeRun) -> Result<PathBuf> {
    let runs_dir = ctx.benchmarks_dir.join("runs");
    fs::create_dir_all(&runs_dir)
        .with_context(|| format!("Failed to create runs directory: {}", runs_dir.display()))?;

    let set_stem = Path::new(ctx.set)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "set".to_string());
    let base = format!("{}-{}-{}-{}", ctx.date, run.mode, run.matching, set_stem);

    let run_file = RunFile {
        date: ctx.date,
        set: ctx.set,
        binary: ctx.binary,
        db: ctx.db,
        run,
    };
    let (run_path, rel_path) = write_unique(&runs_dir, &base, &serde_json::to_string_pretty(&run_file)?)?;

    let entry = LedgerEntry {
        date: ctx.date,
        set: ctx.set,
        mode: run.mode,
        matching: run.matching,
        k: run.k,
        queries: run.overall.n,
        hit_rate_at_k: run.overall.hit_rate_at_k,
        recall_at_k: run.overall.recall_at_k,
        mrr: run.overall.mrr,
        strata: &run.strata,
        latency_ms: &run.latency,
        binary: ctx.binary,
        db: ctx.db,
        per_query_results: rel_path,
        notes: ctx.note,
    };
    append_ledger_line(&ctx.benchmarks_dir.join("ledger.jsonl"), &entry)?;

    Ok(run_path)
}

/// Create `<base>.json` in `dir`, appending `-2`, `-3`, ... when a same-named
/// file already exists. Uses create_new so a concurrent run cannot clobber.
/// Returns the full path and the path relative to the benchmarks dir
/// (`runs/<name>.json`).
fn write_unique(dir: &Path, base: &str, content: &str) -> Result<(PathBuf, String)> {
    for attempt in 1..1000 {
        let name = if attempt == 1 {
            format!("{}.json", base)
        } else {
            format!("{}-{}.json", base, attempt)
        };
        let path = dir.join(&name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(content.as_bytes())
                    .with_context(|| format!("Failed to write run file: {}", path.display()))?;
                return Ok((path, format!("runs/{}", name)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to create run file: {}", path.display()))
            }
        }
    }
    anyhow::bail!("Could not find a free run-file name for {}", base);
}

fn append_ledger_line(ledger_path: &Path, entry: &LedgerEntry) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(ledger_path)
        .with_context(|| format!("Failed to open ledger: {}", ledger_path.display()))?;
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{}", line)
        .with_context(|| format!("Failed to append to ledger: {}", ledger_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::benchmark::metrics::AggregateMetrics;

    fn test_run(mode: &'static str) -> ModeRun {
        ModeRun {
            mode,
            matching: "id",
            k: 10,
            overall: AggregateMetrics {
                n: 2,
                queries_with_match: 1,
                hit_rate_at_k: 0.5,
                recall_at_k: 0.25,
                mrr: 0.5,
            },
            strata: BTreeMap::new(),
            latency: LatencyStats {
                avg_ms: 12.5,
                p50_ms: 11.0,
            },
            query_results: Vec::new(),
        }
    }

    fn test_ctx(dir: &Path) -> RecordContext<'_> {
        RecordContext {
            benchmarks_dir: dir,
            date: "2026-07-11",
            set: "golden.json",
            binary: "grans dev",
            db: "C:/data/grans.db",
            note: Some("test note"),
        }
    }

    #[test]
    fn record_run_appends_ledger_line_and_writes_run_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let run = test_run("fts");

        let run_path = record_run(&ctx, &run).unwrap();

        assert!(run_path.exists());
        let run_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&run_path).unwrap()).unwrap();
        assert_eq!(run_json["mode"], "fts");
        assert_eq!(run_json["date"], "2026-07-11");
        assert_eq!(run_json["set"], "golden.json");

        let ledger = fs::read_to_string(dir.path().join("ledger.jsonl")).unwrap();
        let lines: Vec<&str> = ledger.lines().collect();
        assert_eq!(lines.len(), 1);
        let entry: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(entry["date"], "2026-07-11");
        assert_eq!(entry["mode"], "fts");
        assert_eq!(entry["matching"], "id");
        assert_eq!(entry["k"], 10);
        assert_eq!(entry["queries"], 2);
        assert!((entry["hit_rate_at_k"].as_f64().unwrap() - 0.5).abs() < 1e-9);
        assert!((entry["recall_at_k"].as_f64().unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(entry["notes"], "test note");
        // Empty strata are omitted, matching hand-written v1 entries.
        assert!(entry.get("strata").is_none());

        // The ledger points at the run file, relative to the benchmarks dir.
        let rel = entry["per_query_results"].as_str().unwrap();
        assert!(dir.path().join(rel).exists());
    }

    #[test]
    fn record_run_appends_without_clobbering() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());

        let path_a = record_run(&ctx, &test_run("fts")).unwrap();
        let path_b = record_run(&ctx, &test_run("fts")).unwrap();

        assert_ne!(path_a, path_b, "same-day same-mode runs must not overwrite");
        let ledger = fs::read_to_string(dir.path().join("ledger.jsonl")).unwrap();
        assert_eq!(ledger.lines().count(), 2);
    }

    #[test]
    fn record_run_omits_missing_note() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.note = None;

        record_run(&ctx, &test_run("semantic")).unwrap();

        let ledger = fs::read_to_string(dir.path().join("ledger.jsonl")).unwrap();
        let entry: serde_json::Value = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
        assert!(entry.get("notes").is_none());
    }
}
