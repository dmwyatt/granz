//! --dump-candidates output: per-query rerank candidates as JSONL.
//!
//! One line per query: the query text and every reranked candidate with its
//! fusion components (fused rank, RRF score, passage, rerank score). Used
//! for offline ranking experiments (e.g. blend-weight sweeps). Dumps carry
//! real meeting content, so they belong outside the repo, alongside the
//! golden set.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::query::rerank::RerankCandidate;

#[derive(Serialize)]
struct DumpLine<'a> {
    query: &'a str,
    candidates: &'a [RerankCandidate],
}

pub(super) struct CandidateDumpWriter {
    writer: BufWriter<File>,
}

impl CandidateDumpWriter {
    pub(super) fn create(path: &Path) -> Result<Self> {
        let file = File::create(path)
            .with_context(|| format!("Failed to create dump file: {}", path.display()))?;
        Ok(Self { writer: BufWriter::new(file) })
    }

    pub(super) fn write_query(
        &mut self,
        query: &str,
        candidates: &[RerankCandidate],
    ) -> Result<()> {
        serde_json::to_writer(&mut self.writer, &DumpLine { query, candidates })?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    /// Flush and close. Explicit so write errors surface instead of being
    /// swallowed by a buffered writer's drop.
    pub(super) fn finish(mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, fused_rank: usize) -> RerankCandidate {
        RerankCandidate {
            document_id: id.to_string(),
            fused_rank,
            fused_score: 0.016,
            passage: format!("Title {id}\n\nchunk text"),
            rerank_score: 0.9,
        }
    }

    #[test]
    fn writes_one_json_line_per_query() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.jsonl");

        let mut writer = CandidateDumpWriter::create(&path).unwrap();
        writer.write_query("first query", &[candidate("doc-a", 1), candidate("doc-b", 2)]).unwrap();
        writer.write_query("second query", &[candidate("doc-c", 1)]).unwrap();
        writer.finish().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["query"], "first query");
        assert_eq!(first["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(first["candidates"][0]["document_id"], "doc-a");
        assert_eq!(first["candidates"][0]["fused_rank"], 1);
        assert_eq!(first["candidates"][0]["fused_score"], 0.016);
        assert_eq!(first["candidates"][0]["passage"], "Title doc-a\n\nchunk text");
        assert!((first["candidates"][0]["rerank_score"].as_f64().unwrap() - 0.9).abs() < 1e-6);

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["query"], "second query");
        assert_eq!(second["candidates"].as_array().unwrap().len(), 1);
    }
}
