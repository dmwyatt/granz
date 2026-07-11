# Search Roadmap

A staged plan to evolve `grans search` from two separate modes (FTS5 keyword, semantic) into a single hybrid pipeline, with a measurement gate at every step so the merge cannot silently make results worse than what exists today.

## Current state (2026-07)

- **Keyword search** (Phase 1, #39): FTS5 `MATCH` with implicit-AND semantics (each term quoted individually; user-supplied quotes force phrase matching), ranked by `bm25()` with recency as tiebreak. Title substring matches rank as a tier above content matches; weighting them properly is Phase 5. Applies to the combined search and the standalone transcript/notes/panel searches.
- **Semantic search** (`--semantic`): nomic-embed-text-v1.5 (768d) via fastembed, brute-force cosine over in-memory vectors, `min_score` cutoff. Chunkers for transcripts (adaptive window), panels (section), notes (paragraph).
- **Hybrid search** (Phase 2, #40, opt-in `--hybrid`): runs both retrievers, truncates each ranked list to a 100-document candidate pool, and fuses by reciprocal rank fusion (k=60) in `query/fusion.rs` + `query/hybrid.rs`. Output is the fused meeting list; keyword and semantic remain the forcing modes.
- The modes are mutually exclusive; `commands/search.rs` dispatches on a `SearchMode` enum.
- **Evaluation** (Phase 0, #38): `grans benchmark quality --file <golden-set.json> --mode fts|semantic` scores any retrieval mode; `--compare fts,semantic` runs several with a per-query rank-of-first-relevant table and win/loss/tie summary. Results match labels by document ID (`relevant_meeting_ids`), falling back to exact title for the v1 file. Reports hit-rate@k, recall@k, MRR@k, and per-mode latency, with per-stratum breakdowns. `--record` appends the run to the results ledger. Implemented in `commands/benchmark/`.

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
4. **Results ledger.** Each benchmark run is appended to a dated ledger file kept next to the golden set (outside the repo): commit hash, mode, metrics, latency, notes. `benchmark quality --record` does this automatically. Trends stay visible across months.
5. **Failures feed the suite.** Any real-world search that returns bad results becomes a new labeled query in the golden set. The suite gets more trustworthy exactly where it was wrong.
6. **Escape hatches stay.** `--keyword` and `--semantic` remain after hybrid becomes the default, so a missed regression is recoverable and diagnosable (run the same query in all three modes).

## Golden set

Golden-set files live in the `benchmarks/` subdirectory of the grans data directory (the same directory `platform::data_dir()` resolves for the database, e.g. `~/.local/share/grans/benchmarks/`). They contain real meeting titles and MUST NEVER be committed to this repo or quoted in issues/PRs; this repo is public. The results ledger (`ledger.jsonl`, one JSON object per benchmark run: date, binary commit, mode, matching method, metrics with per-stratum breakdown, notes) lives in the same directory, with full per-query outputs saved under `runs/` for later win/loss comparison between runs.

- `search_benchmark_v2.json` (2026-07-10, primary): 93 queries. Built by agent generation over transcripts plus a verification pass that pooled hits from both retrievers, judged each hit, and completed the labels.
- `semantic_search_benchmark.json` (v1): the original 11 title-labeled queries, kept unchanged for longitudinal comparison.

v2 schema: top-level `{description, created, queries}`; each query is
`{query, query_type, provenance, relevant_meetings, relevant_meeting_ids, rationale}`
where `query_type` is `exact-term|paraphrase|mixed`, `provenance` is `v1|v2`, `relevant_meetings` holds exact titles, and `relevant_meeting_ids` holds document IDs. Since Phase 0 (#38) the harness matches by `relevant_meeting_ids` and stratifies by `query_type`; title matching remains only as the fallback for files without IDs (the v1 set).

Phase 0 baselines on v2 (2026-07-10, k=10, **ID matching**, commit 046d6d6):

| Mode | hit-rate@10 | recall@10 | MRR@10 | avg latency |
|---|---|---|---|---|
| fts | 0.05 | 0.03 | 0.04 | ~5 ms |
| semantic | 0.86 | 0.76 | 0.72 | ~58 ms |

Semantic per stratum (hit-rate / MRR): exact-term 0.92 / 0.81, mixed 0.86 / 0.77, paraphrase 0.84 / 0.64. FTS beats semantic on best rank for 1 of 93 queries (90 losses, 2 ties); its collapse outside exact-term was the phrase-quoting bug plus recency-only ordering, which Phase 1 fixed. Full per-stratum metrics for both modes are in the ledger.

Phase 1 results (2026-07-10, k=10, ID matching, same snapshot):

| Mode | hit-rate@10 | recall@10 | MRR@10 | avg latency |
|---|---|---|---|---|
| fts (Phase 0) | 0.05 | 0.03 | 0.04 | ~5 ms |
| fts (Phase 1) | 0.17 | 0.09 | 0.14 | ~5 ms |
| semantic (unchanged) | 0.86 | 0.76 | 0.72 | ~58 ms |

Per query, 11 flipped miss-to-hit, none hit-to-miss, worst rank change 7 to 9; FTS vs semantic on best rank moved from 1/90/2 to 2/82/9 (W/L/T). Under the re-audited strata (below), FTS scores hit-rate 0.94 / MRR 0.76 on exact-term (n=17) and zero on mixed and paraphrase; semantic scores 1.00 / 0.92 on exact-term. Note the implicit-AND granularity: all terms must co-occur within one FTS row (a single utterance, panel, or notes document), so multi-term queries whose terms are scattered across a transcript still miss.

Phase 2 results (2026-07-10, k=10, ID matching, same snapshot, re-audited strata):

| Mode | hit-rate@10 | recall@10 | MRR@10 | avg latency |
|---|---|---|---|---|
| fts (Phase 1) | 0.17 | 0.09 | 0.14 | ~5 ms |
| semantic | 0.86 | 0.76 | 0.72 | ~56 ms |
| hybrid (RRF) | 0.86 | 0.76 | 0.72 | ~65 ms |

Aggregates tie semantic, but the movement is where fusion should act: on exact-term queries hybrid reaches hit-rate 1.00 / MRR 1.000 (semantic 1.00 / 0.924, FTS 0.94 / 0.761), because FTS agreement pulls two semantically rank-2/rank-5 results to rank 1. Per query, hybrid matches or beats the better single mode on 90 of 93; the 3 losses are one-position slips (1→2, 2→3, 1→2) on mixed/paraphrase queries where an irrelevant FTS match fused above a relevant semantic one, costing ~0.015 MRR on those strata. No query with a top-3 result under either mode leaves the top 10, so the #40 gate passes. Against the single modes: hybrid vs FTS 69W/0L/24T, hybrid vs semantic 2W/3L/88T on best rank.

The ceiling here is FTS itself: with FTS scoring zero outside exact-term, fusion has only one useful signal for mixed/paraphrase queries. Larger hybrid gains wait on reranking (Phase 3) and embedding improvements (Phase 4). Promotion of `--hybrid` to the default is a follow-up PR per #40.

(v1-file baseline for reference: hit-rate@10 ~73%, MRR ~0.55, title matching.)

Caveats a maintainer must know:

- Ledger entries recorded before 2026-07-10 used title matching, which over-credits recurring-title meetings; do not compare them against ID-matched numbers.
- `query_type` strata were re-audited on 2026-07-10 after Phase 1 landed: 9 queries demoted to mixed only for the old phrase-match failure were promoted back to exact-term, and 4 exact-term labels that fail the operational test (all terms verbatim within one utterance/panel/notes row of a labeled doc) were demoted to mixed. Exact-term is now n=17, mixed n=38, paraphrase n=38. Ledger entries recorded before the re-audit used the old strata; their per-stratum numbers are not comparable to later entries (overall metrics are unaffected).
- For stable numbers across syncs, benchmark against the frozen snapshot rather than the live database: `grans --db <benchmarks-dir>/grans-snapshot-2026-07-09.db benchmark quality ...`. The snapshot is byte-identical to the state the Phase 0 baselines were recorded against (872 docs, 33,510 chunks); the live database drifts with every `grans sync`, which invalidates per-query comparison against earlier ledger entries.
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
