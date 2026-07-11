# Search Roadmap

A staged plan to evolve `grans search` from two separate modes (FTS5 keyword, semantic) into a single hybrid pipeline, with a measurement gate at every step so the merge cannot silently make results worse than what exists today.

## Current state (2026-07)

- **Keyword search**: FTS5 `MATCH` used as a boolean filter; results ordered by `created_at DESC`. No relevance ranking. Additionally, `sanitize_fts_query` wraps the whole query in double quotes, which FTS5 interprets as a phrase query: multi-word searches only match the words adjacent and in order, not AND semantics.
- **Semantic search** (`--semantic`): nomic-embed-text-v1.5 (768d) via fastembed, brute-force cosine over in-memory vectors, `min_score` cutoff. Chunkers for transcripts (adaptive window), panels (section), notes (paragraph).
- The two modes are mutually exclusive; `commands/search.rs` dispatches to one or the other.
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

Golden-set files live in the `benchmarks/` subdirectory of the grans data directory (the same directory `platform::data_dir()` resolves for the database, e.g. `~/.local/share/grans/benchmarks/`). They contain real meeting titles and MUST NEVER be committed to this repo or quoted in issues/PRs; this repo is public. The results ledger lives in the same directory.

- `search_benchmark_v2.json` (2026-07-10, primary): 93 queries. Built by agent generation over transcripts plus a verification pass that pooled hits from both retrievers, judged each hit, and completed the labels.
- `semantic_search_benchmark.json` (v1): the original 11 title-labeled queries, kept unchanged for longitudinal comparison.

v2 schema: top-level `{description, created, queries}`; each query is
`{query, query_type, provenance, relevant_meetings, relevant_meeting_ids, rationale}`
where `query_type` is `exact-term|paraphrase|mixed`, `provenance` is `v1|v2`, `relevant_meetings` holds exact titles, and `relevant_meeting_ids` holds document IDs. Since Phase 0 (#38) the harness matches by `relevant_meeting_ids` and stratifies by `query_type`; title matching remains only as the fallback for files without IDs (the v1 set).

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

## Phases

Each phase is one PR, independently shippable, gated by the benchmark.

### Phase 0: Evaluation harness

No search behavior changes. Everything else depends on this.

- Make `benchmark quality` retriever-agnostic: `--mode fts|semantic|hybrid|hybrid-rerank` (modes appear as they are implemented).
- Add `--compare <mode,mode,...>`: per-query rank table across modes with win/loss/tie summary.
- Metrics: rename the current precision@k to hit-rate@k (that is what it computes); add recall@k over each query's full `relevant_meetings` list; keep MRR; record per-mode query latency.
- Add `query_type` to the golden-set schema and label existing queries. Done 2026-07: see "Golden set" above.
- Match benchmark results to labels by document ID instead of exact title. Titles are heavily duplicated across recurring meetings (only 329 unique titles across 872 documents), so title matching over-credits any query targeting a recurring series.
- Keep growing the set with real queries captured from actual usage; every real-world search failure becomes a new labeled case.
- Record the FTS baseline alongside the semantic baseline; start the results ledger.

**Exit criteria:** FTS and semantic baselines recorded on the stratified set with ID-based matching.

### Phase 1: BM25 ranking and query semantics for keyword search

- Rank FTS matches by `bm25()` instead of `created_at DESC` (FTS5's bm25 is lower-is-better). Keep recency as a tiebreak.
- Fix `sanitize_fts_query`: quote each term individually instead of the whole query, so multi-word searches get implicit-AND semantics rather than strict phrase matching. Keep user-supplied quotes as explicit phrase syntax.
- Applies to transcripts, notes, and panels FTS queries.

**Gate:** FTS-mode metrics improve on the exact-term stratum with no per-query catastrophic regressions. This is a standalone win even if nothing else ships.

### Phase 2: Hybrid retrieval behind `--hybrid`

- Run both retrievers, fuse with RRF (`score = Σ 1/(k + rank)`, k≈60). RRF is rank-based, which sidesteps the known problem that cosine scores are only ordinal within a query and cannot be calibrated against bm25 scores.
- Dedupe chunk-level candidates to meetings before scoring (a meeting's score is its best chunk).
- Opt-in flag only; existing modes untouched.

**Gate:** the hybrid gate above (rule 2). Once passed, hybrid becomes the default for `grans search` in a follow-up PR, with `--keyword`/`--semantic` as forcing flags.

### Phase 3: Cross-encoder reranking

- Rerank the top 30-50 fused candidates with fastembed `TextRerank` (candidates: jina-reranker-v1-turbo-en, bge-reranker-base; pick by benchmark).
- Reranker score becomes the user-facing score; `--min-score` moves to this scale, which is more threshold-stable than cosine.
- `--fast` flag skips the reranker.

**Gate:** quality gain must justify the latency cost; both columns come from the same benchmark run. If the lift is marginal, reranking stays opt-in instead of default.

### Phase 4: Embedding-side experiments

Each experiment requires a re-embed, so A/B against a copy of the database before committing to re-embedding the real one.

- Contextual chunk headers: prepend meeting title/date/attendees to chunk text before embedding (the chunker already prefixes speaker labels; this extends the idea to meeting identity).
- Chunking strategy variations (window size, overlap, boundaries).
- Re-test nomic task prefixes (`search_query:`/`search_document:`) only if chunking changes materially. Tested 2026-07 on the current chunking: no measurable difference, so prefixes alone are not a win here despite the model card's guidance.

**Gate:** semantic-mode before/after on the golden set; keep only variants that move recall@k or MRR.

### Phase 5: Ranking polish

- Recency prior as a post-rerank tiebreak (meetings decay in relevance; a mild boost, not a hard sort).
- Title-match boost.
- Result shaping: group results by meeting, show the best chunk with `highlight()`/`snippet()` for lexical hits.

**Gate:** per-query comparison plus manual inspection; these changes are about presentation and tie-breaking, which binary relevance labels only partially capture. Add golden-set queries for cases these are meant to fix.

## Deliberately out of scope

- **ANN indexes (sqlite-vec, HNSW):** brute-force cosine is exact and fast at this corpus size. Revisit only if vector count or load time becomes a real problem; nomic v1.5 is Matryoshka-trained, so truncating to 256d is the first lever before any index.
- **LLM query expansion / HyDE:** requires an LLM call; wrong fit for an offline CLI.
- **Learned sparse retrieval (SPLADE), late interaction (ColBERT):** complexity out of proportion to a personal corpus.
- **nDCG:** with binary relevance labels and a small suite, hit-rate@k, recall@k, and MRR are sufficient.
