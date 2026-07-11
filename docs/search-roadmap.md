# Search Roadmap

A staged plan to evolve `grans search` from two separate modes (FTS5 keyword, semantic) into a single hybrid pipeline, with a measurement gate at every step so the merge cannot silently make results worse than what exists today.

## Current state (2026-07)

- **Keyword search**: FTS5 `MATCH` used as a boolean filter; results ordered by `created_at DESC`. No relevance ranking. Additionally, `sanitize_fts_query` wraps the whole query in double quotes, which FTS5 interprets as a phrase query: multi-word searches only match the words adjacent and in order, not AND semantics.
- **Semantic search** (`--semantic`): nomic-embed-text-v1.5 (768d) via fastembed, brute-force cosine over in-memory vectors, `min_score` cutoff. Chunkers for transcripts (adaptive window), panels (section), notes (paragraph).
- The two modes are mutually exclusive; `commands/search.rs` dispatches to one or the other.
- **Evaluation**: `grans benchmark quality --file <golden-set.json>` scores the semantic pipeline only (hardcoded in `commands/benchmark.rs`) and matches results to labels by exact title. See "Golden set" below for the dataset and baselines. No FTS baseline exists yet.

## Target pipeline

```
query
 ├─ lexical:  FTS5 MATCH ranked by bm25()          → top ~100
 └─ semantic: embed query, cosine over chunks       → top ~100
        ↓
 reciprocal rank fusion (RRF, k≈60)                 → top ~30-50
        ↓
 cross-encoder reranker (fastembed TextRerank)      → top N
        ↓
 per-meeting grouping, recency tiebreak, snippets
```

## How improvement is tracked

Every phase below ships with a before/after run of the quality benchmark, and the numbers go in the PR description. The rules:

1. **Per-query, not aggregate.** The benchmark reports rank-of-first-relevant per query per mode, plus win/loss/tie counts between modes. Aggregate MRR can improve while individual queries regress; the per-query table catches that.
2. **The hybrid gate.** Hybrid becomes the default only when, on the golden set:
   - per query, hybrid matches or beats the better of (FTS, semantic) for that query in most cases, and
   - no query whose first relevant result currently ranks in the top 3 falls out of the top k.
   One catastrophic regression outweighs several mild improvements.
3. **Strata.** Golden-set queries carry a `query_type` label (`exact-term`, `paraphrase`, `mixed`). Success reads as: hybrid ≈ FTS on exact-term, hybrid ≈ semantic on paraphrase, hybrid ≥ both on mixed.
4. **Results ledger.** Each benchmark run is appended to a dated ledger file kept next to the golden set (outside the repo): commit hash, mode, metrics, latency, notes. Trends stay visible across months.
5. **Failures feed the suite.** Any real-world search that returns bad results becomes a new labeled query in the golden set. The suite gets more trustworthy exactly where it was wrong.
6. **Escape hatches stay.** `--keyword` and `--semantic` remain after hybrid becomes the default, so a missed regression is recoverable and diagnosable (run the same query in all three modes).

## Golden set

Golden-set files live in the `benchmarks/` subdirectory of the grans data directory (the same directory `platform::data_dir()` resolves for the database, e.g. `~/.local/share/grans/benchmarks/`). They contain real meeting titles and MUST NEVER be committed to this repo or quoted in issues/PRs; this repo is public. The results ledger (`ledger.jsonl`, one JSON object per benchmark run: date, binary commit, mode, matching method, metrics with per-stratum breakdown, notes) lives in the same directory, with full per-query outputs saved under `runs/` for later win/loss comparison between runs.

- `search_benchmark_v2.json` (2026-07-10, primary): 93 queries. Built by agent generation over transcripts plus a verification pass that pooled hits from both retrievers, judged each hit, and completed the labels.
- `semantic_search_benchmark.json` (v1): the original 11 title-labeled queries, kept unchanged for longitudinal comparison.

v2 schema: top-level `{description, created, queries}`; each query is
`{query, query_type, provenance, relevant_meetings, relevant_meeting_ids, rationale}`
where `query_type` is `exact-term|paraphrase|mixed`, `provenance` is `v1|v2`, `relevant_meetings` holds exact titles (what the current harness matches on), and `relevant_meeting_ids` holds document IDs (what ID-based matching should use). The current harness ignores the extra fields, so v2 runs on the existing binary.

Semantic baseline on v2 (nomic-embed-text-v1.5, k=10, **title matching**):

| Stratum | n | hit-rate@10 | MRR |
|---|---|---|---|
| exact-term | 12 | 0.92 | 0.81 |
| mixed | 43 | 0.86 | 0.77 |
| paraphrase | 38 | 0.79 | 0.59 |
| overall | 93 | 0.84 | 0.70 |

(v1-file baseline for reference: hit-rate@10 ~73%, MRR ~0.55.)

Caveats a maintainer must know:

- These numbers were measured with title matching, which over-credits recurring-title meetings. After switching to ID-based matching (Phase 0), re-record the baseline; do not compare ID-matched numbers against this table.
- `query_type` labels were assigned relative to the current phrase-matching keyword behavior: several queries were demoted from exact-term to mixed only because the full query fails as a phrase. After Phase 1 lands implicit-AND semantics, re-audit the strata; the exact-term stratum (n=12) is currently thin and noisy.
- For stable numbers across syncs, benchmark against a frozen copy of the database via the global `--db` flag rather than the live one.
- Open review items: the 11 v1 queries' `query_type` values were hand-assigned and unreviewed, and two v1 queries ("AI phone agent...", "changing an intermittent caregiving leave...") had their ID labels expanded across recurring-title instances that may over-include.

## Work tracking

Implementation is tracked on GitHub: parent issue #37 with one sub-issue per phase. Each phase is one PR, independently shippable, gated by the benchmark as described above. Deliverables, implementation specifics, and gates live in the issues; this doc holds the methodology and reference material.

- Phase 0 (#38): retriever-agnostic benchmark harness with ID-based matching
- Phase 1 (#39): bm25 ranking and implicit-AND query semantics for keyword search
- Phase 2 (#40): hybrid retrieval (RRF fusion) behind `--hybrid`
- Phase 3 (#41): cross-encoder reranking
- Phase 4 (#42): embedding-side experiments (contextual chunk headers, chunking variants)
- Phase 5 (#43): ranking polish (recency tiebreak, title boost, per-meeting grouping)

One experiment result worth keeping here because it contradicts the model card's guidance: nomic task prefixes (`search_query:`/`search_document:`) were tested 2026-07 on the current chunking and made no measurable difference. Re-test only if chunking changes materially (tracked in #42).

## Deliberately out of scope

- **ANN indexes (sqlite-vec, HNSW):** brute-force cosine is exact and fast at this corpus size. Revisit only if vector count or load time becomes a real problem; nomic v1.5 is Matryoshka-trained, so truncating to 256d is the first lever before any index.
- **LLM query expansion / HyDE:** requires an LLM call; wrong fit for an offline CLI.
- **Learned sparse retrieval (SPLADE), late interaction (ColBERT):** complexity out of proportion to a personal corpus.
- **nDCG:** with binary relevance labels and a small suite, hit-rate@k, recall@k, and MRR are sufficient.
