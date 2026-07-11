# Search Roadmap

A staged plan to evolve `grans search` from two separate modes (FTS5 keyword, semantic) into a single hybrid pipeline, with a measurement gate at every step so the merge cannot silently make results worse than what exists today.

## Current state (2026-07)

- **Keyword search** (Phase 1, #39): FTS5 `MATCH` with implicit-AND semantics (each term quoted individually; user-supplied quotes force phrase matching), ranked by `bm25()` with recency as tiebreak. Title substring matches rank as a tier above content matches in this mode; the hybrid pipeline weights title matches with the Phase 5 title boost instead. Applies to the combined search and the standalone transcript/notes/panel searches.
- **Semantic search** (`--semantic`): nomic-embed-text-v1.5 (768d) via fastembed, brute-force cosine over in-memory vectors, `min_score` cutoff. Chunkers for transcripts (adaptive window), panels (section), notes (paragraph).
- **Embedding spec** (Phase 4, #42): the chunking scheme (target/overlap tokens, overlap mode, contextual-headers toggle) is persisted in `embedding_metadata` and resolved stored-first by every search/benchmark/sync path (`embed/config.rs`), so a database embedded with a variant scheme is never silently re-chunked to the binary's defaults. `grans embed` has hidden experiment flags (`--chunk-target-tokens`, `--chunk-overlap-tokens`, `--overlap-mode`, `--contextual-headers[=bool]`) that override the stored scheme, and `embed status` reports the resolved scheme. Phase 4 tested header and chunking variants through this machinery; none beat the current defaults end-to-end (results below), so the defaults stand.
- **Hybrid search** (Phase 2, #40, opt-in `--hybrid`): runs both retrievers, truncates each ranked list to a 100-document candidate pool, and fuses by reciprocal rank fusion (k=60) in `query/fusion.rs` + `query/hybrid.rs`. Output is the fused meeting list; keyword and semantic remain the forcing modes.
- **Reranking** (Phase 3, #41 + follow-up, part of `--hybrid` by default): a cross-encoder (fastembed `TextRerank`, jina-reranker-v1-turbo-en) scores the top 50 fused candidates as title + best-chunk passages; the ordering formula lives in `query/adjust.rs` (see next bullet). The sigmoid relevance probability is the user-facing score; `--min-score` filters on it; `--fast` skips the stage for fusion-only ordering (~63 ms instead of ~2 s per query).
- **Ordering adjustments** (Phase 5 part 1, #43): the rerank-stage ordering is `rerank_score + 30 × RRF score + 0.2 × title_signal` (`query/adjust.rs`), where `title_signal` is the fraction of query content tokens found in the meeting title, damped by `log2(1 + series size)` so recurring series sharing one title (the largest spans 123 meetings) don't drown the query. Adjustments are ordering-only: they reorder the fixed 50-candidate pool and never touch the user-facing score. Weights live in `RankingConfig` (defaults are the sweep winners); `benchmark quality` has a hidden `--title-boost-weight` override so one binary can record before/after runs, and non-default weights are appended to the ledger note. A recency prior was swept alongside and rejected (results below).
- The modes are mutually exclusive; `commands/search.rs` dispatches on a `SearchMode` enum.
- **Evaluation** (Phase 0, #38): `grans benchmark quality --file <golden-set.json> --mode fts|semantic` scores any retrieval mode; `--compare fts,semantic` runs several with a per-query rank-of-first-relevant table and win/loss/tie summary. Results match labels by document ID (`relevant_meeting_ids`), falling back to exact title for the v1 file. Reports hit-rate@k, recall@k, MRR@k, and per-mode latency, with per-stratum breakdowns. `--record` appends the run to the results ledger. For rerank modes, `--dump-candidates <path>` writes each query's candidates (fused rank, RRF score, passage, rerank score) as JSONL for offline ranking experiments; dumps carry meeting content and stay outside the repo. Implemented in `commands/benchmark/`.

## Target pipeline

```
query
 ├─ lexical:  FTS5 MATCH ranked by bm25()          → top ~100
 └─ semantic: embed query, cosine over chunks       → top ~100
        ↓
 reciprocal rank fusion (RRF, k≈60)                 → top ~30-50
        ↓
 cross-encoder reranker + weighted fusion prior     → top N
        ↓
 per-meeting grouping, recency tiebreak, snippets
```

## How improvement is tracked

Every phase below ships with a before/after run of the quality benchmark, and the numbers go in the PR description. The rules:

1. **Per-query, not aggregate.** The benchmark reports rank-of-first-relevant per query per mode, plus win/loss/tie counts between modes. Aggregate MRR can improve while individual queries regress; the per-query table catches that.
2. **The promotion gate.** A change ships (or a mode becomes a default) when, per query on the golden set, the new behavior matches or beats the old in most cases with net wins clearly positive. Regressions are diagnostic signals, not automatic vetoes: a query whose top-3 result falls out of the top k gets diagnosed before shipping. A systematic cause (a failure mode that will recur in real use) blocks promotion until fixed; an idiosyncratic one-off weighs like any other loss. The strict no-regression standard is reserved for changes to a mode already in real daily use, where breaking a search someone relies on costs trust; during pre-adoption iteration the benchmark is the only user and there is no incumbent to protect. Phase 3 is the case study: three dropouts were first treated as a veto, but diagnosing them exposed one shared failure mode whose fix beat both the regressed and the protected configuration.
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

Phase 3 results (2026-07-10, k=10, ID matching, same snapshot, all modes in one `--compare` run):

| Mode | hit-rate@10 | recall@10 | MRR@10 | avg latency |
|---|---|---|---|---|
| hybrid (RRF) | 0.86 | 0.76 | 0.72 | ~63 ms |
| rerank-jina | 0.90 | 0.75 | 0.77 | ~2.0 s |
| rerank-bge | 0.89 | 0.70 | 0.70 | ~7.2 s |

jina-reranker-v1-turbo-en is the model pick: it beats bge-reranker-base on every aggregate and stratum at under a third of the latency, so it backs the rerank stage. Per stratum (hit / MRR), jina reaches mixed 0.87 / 0.759 and paraphrase 0.89 / 0.702 against hybrid's 0.82 / 0.694 and 0.84 / 0.626; exact-term keeps hit 1.00 but MRR slips 1.000 to 0.941 (two one-position slips). Per query at k=10 it goes 23W / 15L / 55T against the better of (hybrid, semantic), matching or beating it on 78 of 93.

The initial gate call: the lift was real but three queries whose relevant meeting ranked top-3 under fusion fell out of the top 10 (to ranks 13-15), so reranking first shipped opt-in rather than as part of plain `--hybrid`, pending a diagnosis of the dropouts.

Phase 3 follow-up (2026-07-11, same snapshot): the three dropouts shared one cause rather than being three one-offs. On queries where the cross-encoder is unconfident (best candidate scoring 0.40-0.62, against the 0.8+ it assigns to answers it recognizes), its ordering is noise-dominated: it buried documents fusion had ranked top-3 while promoting candidates from fused ranks 13-48. The fix blends the fusion prior into the final order (`rerank_score + 30 × RRF score`; the user-facing score stays the cross-encoder probability). The weight came from sweeping w offline over a `--dump-candidates` capture of all 93 queries:

| Mode | hit-rate@10 | recall@10 | MRR@10 | fusion-top-3 dropouts |
|---|---|---|---|---|
| hybrid (RRF only) | 0.86 | 0.76 | 0.72 | — |
| rerank, no prior (w=0) | 0.90 | 0.75 | 0.77 | 3 |
| rerank + prior (w=30) | 0.94 | 0.80 | 0.80 | 0 |

The blend beats both plain fusion and unblended reranking on every aggregate and every stratum: exact-term returns to hit 1.00 / MRR 1.000 (fixing the two slips), mixed reaches 0.92 / 0.768, paraphrase 0.92 / 0.754. Against hybrid at k=10 it goes 22W / 9L / 62T with no fusion-top-3 document leaving the top 10. With the quality objection gone, reranking became the `--hybrid` default; the remaining cost is latency, so `--fast` skips the stage for fusion-only ordering (~63 ms vs ~2 s per query).

Phase 4 results (2026-07-11, same snapshot, semantic mode, k=10, ID matching): embedding-side variants were each embedded into a copy of the snapshot via the hidden `grans embed` experiment flags and benchmarked as-is (the chunking scheme is persisted in `embedding_metadata` and resolved stored-first, so a variant database is never silently re-chunked back to the binary's defaults):

| Variant | hit-rate@10 | recall@10 | MRR@10 |
|---|---|---|---|
| baseline (348 target / 102 overlap, chars) | 0.86 | 0.76 | 0.721 |
| contextual headers (title/date/attendees) | 0.87 | 0.71 | 0.696 |
| small chunks (192/48) | 0.88 | 0.71 | 0.732 |
| large overlap (348/204) | 0.89 | 0.74 | 0.727 |
| big chunks (460/102) | 0.87 | 0.71 | 0.728 |
| utterance-boundary overlap | 0.85 | 0.70 | 0.702 |
| combo (192/114) | 0.87 | 0.73 | 0.723 |

No variant was adopted. The semantic-mode MRR gains (small chunks, large overlap) were rank shuffles rather than uniform lifts (both go 14W / 16L / 63T per query against baseline, with individual collapses like rank 19 → 318), and every variant loses recall. The decisive check was end-to-end: rerank-jina on the two survivors' databases regressed the production pipeline (baseline 0.94 / 0.80 / 0.804; large overlap 0.94 / 0.77 / 0.765; small chunks 0.93 / 0.77 / 0.751). Two mechanisms explain it: recall losses remove documents from the fused candidate pool where no reranker can recover them, and smaller/denser chunks shorten the passages the cross-encoder judges. The current chunking (348 target / 102 overlap / 512 max, character-boundary overlap) stands as a validated local optimum for the full pipeline, and the strict no-regression standard for the daily-driver mode (rule 2) is what rejected the variants.

The headers failure was diagnosed per query rather than written off as a suite artifact. Headers went 20W / 25L / 48T against baseline, with wins and losses concentrated in the *same* population: queries whose vocabulary overlaps meeting titles (69 of 93 by token overlap). The title-in-every-chunk mechanism is symmetric: it lifts a result when the labeled meeting's title matches the query (rank 34 → 1) and buries one when a *different* meeting's title matches (rank 1 → 29) — recurring meetings share titles, so wrong-instance chunks crowd out right-content matches. Queries with no metadata overlap paid a pure dilution tax (avg recall delta −0.076 vs −0.046 for overlapping queries). The attendee and date header lines went essentially untested (only ~8 queries contain attendee-name tokens, none reference dates), but neither warrants a follow-up: attendee data comes from calendar invitees, which routinely differs from actual attendance, and date constraints are already served precisely by `--from`/`--to`/`--date` filters that compose with every search mode. Both metadata dimensions belong in structured filters at the CLI boundary, not in the embedding space, so the golden set's content-query focus is correct as-is and no metadata stratum was added.

Phase 5 part 1 results (2026-07-11, same snapshot, rerank-jina, k=10, ID matching): ranking adjustments were swept offline over a recorded `--dump-candidates` file (each query's 50-candidate pool with fused and rerank scores) instead of re-running retrieval. Because the adjustments are ordering-only over a fixed pool, the simulation is exact: it reproduced the ledger baseline bit-for-bit as its fidelity gate, and after implementation the recorded before/after runs matched it per-query on all 93 queries (the before run with the boost zeroed reproduces the baseline exactly).

| Config | hit-rate@10 | recall@10 | MRR@10 |
|---|---|---|---|
| rerank-jina baseline | 0.935 | 0.798 | 0.804 |
| + title boost (overlap fraction / log2 series damping, w=0.2) | 0.935 | 0.805 | 0.813 |

The adopted weight sits on a plateau (every weight from 0.10 to 0.25 passes the no-regression gate; above it the boost starts overpowering the cross-encoder and hit-rate drops). Per query: 7 wins with jumps up to rank 9 → 3, 3 losses of 1-2 ranks each, and no query loses recall — the bounded additive form is why this succeeded where Phase 4's embedding-side headers failed, since a rank-time nudge cannot bury a document the way vector contamination did (19 → 318). The series damping earns its keep directly: the undamped variant at the same weight loses recall (0.801 vs 0.805). An all-terms-in-title binary signal was also tested and fires on only 2 of 93 queries (queries are sentences, titles are short), so it was dropped.

The recency prior (`w × 2^(-age/half-life)`, half-lives 30-365 days) was rejected across the whole grid. Its single nominally-passing configuration was noise: per-query inspection showed real recall losses (labels pushed from rank 10 to 11-12) traded for equally arbitrary gains, netting +0.0003 MRR on a knife-edge weight whose neighbors regress. At meaningful weights it is catastrophic (MRR 0.804 → 0.67 at w=1.0). Binary relevance labels cannot express "prefer the recent one among relevant meetings", and the golden set is content-focused, so this rejection is about the boost being unprincipled, not about the suite being blind: the losses were real relevant documents leaving the top 10. Recency stays where it already works — the FTS tiebreak and the `--from`/`--to`/`--date` structured filters.

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

One experiment result worth keeping here because it contradicts the model card's guidance: nomic task prefixes (`search_query:`/`search_document:`) were tested 2026-07 on the current chunking and made no measurable difference. Re-test only if chunking changes materially; Phase 4 kept the chunking unchanged, so the result stands.

## Deliberately out of scope

- **ANN indexes (sqlite-vec, HNSW):** brute-force cosine is exact and fast at this corpus size. Revisit only if vector count or load time becomes a real problem; nomic v1.5 is Matryoshka-trained, so truncating to 256d is the first lever before any index.
- **LLM query expansion / HyDE:** requires an LLM call; wrong fit for an offline CLI.
- **Learned sparse retrieval (SPLADE), late interaction (ColBERT):** complexity out of proportion to a personal corpus.
- **nDCG:** with binary relevance labels and a small suite, hit-rate@k, recall@k, and MRR are sufficient.
