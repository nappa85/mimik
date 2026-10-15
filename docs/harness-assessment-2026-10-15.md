# Harness/RAG assessment — 2026-10-15

Status: **assessment and proposals, not executed experiments.** No inference,
build, test, download or application change was performed for this record.
Code findings come from a read-only fresh-context reviewer plus main-session spot
checks; they are not test-verified. External facts are web-sourced claims (links
below), not measured on this machine. User decisions on the proposals were recorded the same day; see
[User decisions](#user-decisions-2026-10-15). Review record: `reviews/2026-10-15-harness-assessment.md`.

## User question

The user asked why local benchmarks keep showing low correctness, which local
model to choose and how to wire it, and whether rule lookup over RPG manuals is
not already a largely solved document-analysis/retrieval problem.

## Main conclusion

Grounded retrieval and question answering over manuals is largely a solved,
well-documented pattern: layout-aware parsing → structured chunks with heading
path/page provenance → hybrid lexical+dense retrieval → cross-encoder reranking →
a tool-using model loop with a deterministic host. **Hypothesis (to be measured):
the low benchmark scores mostly reflect the task contract, not retrieval.** In the frozen RAG baseline, the
gold rules were in top-3 for 24/28 attempts
([report](local-rag-benchmark-2026-10-15.md#retrieval-versus-model-failures)),
and most failures happened with the rule present.

## Why the benchmarks score low (benchmark-design findings)

Evidence: `tools/local_rag_benchmark.py` and the 2026-10-15 reports.

| ID | Finding | Evidence |
|---|---|---|
| B1 | One-shot mega-contract: one call must classify into six kinds, compute derived values, emit typed patches with exact addresses and old values, cite IDs and produce readbacks. Small models fail this composition, not the lookup. | `SYSTEM` at `tools/local_rag_benchmark.py:38`, `response_schema()` |
| B2 | The model must re-serialise data the host already knows (old values, stable addresses, revision state). Any copying error fails the attempt. The host should fill these in and render the diff. | Same schema; report's own closing recommendation |
| B3 | Derived values (e.g. pace = 6 − armor, LR-4) are delegated to the model. The project forbids per-game compiled logic and a rules DSL, but also requires deterministic calculations, so no component is left to compute derivations. The recorded user decision allows generic JavaScript for *browser interaction*. Whether template-level derived-field scripts (the Roll20 sheet-worker / Foundry VTT derived-data pattern) are acceptable is an open user decision, and they may conflict with the "no per-game logic/templates" recommendation. **Decided 2026-10-15:** declarative formulas evaluated in Rust; template JavaScript rejected. | `AGENTS.md`, `docs/discovery.md` "Experience-first direction", `examples/sheet-harness-demo/rules.md` |
| B4 | Sample sizes cannot rank models: 28 attempts = 7 tasks × paraphrase × 2 seeds; one attempt ≈ 3.6 points. 6/28 vs 7/28 is noise. Decision screens are 18–24 unique cases. | Report tables |
| B5 | Temperature 0.7 / top_p 0.8 (Qwen general chat defaults) used for strict structured output. Use ~0–0.2 for patch/tool arguments. | `tools/local_rag_benchmark.py:391` |
| B6 | FTS5 `porter` stemming is English-only; manuals are Italian (MÖRK BORG IT 2025, Italian Pathfinder). Use `unicode61 remove_diacritics 2` or `trigram`, plus a multilingual embedder. | Report "What actually ran" |
| B7 | Seven rules fit in any context window, so the fixture cannot exercise retrieval recall; real retrieval failure modes only appear on a full corpus (e.g. the 12,956-chunk legacy Pathfinder set). | `examples/sheet-harness-demo/rules.md` |
| B8 | No tool calling or multi-step loop was evaluated; only single-turn JSON and plain prose. | `generate()` sends one system + one user message |

## Application code findings

From a fresh-context read-only reviewer and main-session spot checks.
Severity is the assessor's judgement.

| ID | Severity | Finding | Location |
|---|---|---|---|
| C1 | High | The Rust app contains **no retrieval index and no model/provider integration**. "Lookup" is a full-scan `LIKE '%q%'`, ordered by source/page/unit (not relevance), `LIMIT 50`, with no truncation signal. All RAG work exists only in Python prototypes. | `src/sources/store.rs:332` (spot-checked) |
| C2 | High (scope) | About 47% of ~12k production lines are auth/accounts/campaigns/LAN plumbing; retrieval/model harness is 0 lines. The viability-defining loop does not exist yet. | Reviewer line counts by module |
| C3 | Medium | Stored search text is hard-wrapped at 120 columns by `html2text`, so phrase queries that cross a wrap or formatting miss. SQLite `LIKE` case-folds ASCII only, so accented Italian text and "MÖRK" match case-sensitively. | `src/sources/import.rs:329` (spot-checked) |
| C4 | Medium | Lookup `Evidence` drops unit/block id and polygon, so a hit cannot be linked to its source-region crop for evidence comparison. | `src/sources/mod.rs:91-97`, `src/sources/store.rs:332` |
| C5 | Medium | Every prepared-source bundle row stores the full PDF blob (≤128 MiB); bundles are capped at 64 pages, so a 300-page manual is stored about five times. `pdf_hash` is not unique. | `src/sources/import.rs:447,465,555`, `migrations/003_sources.sql` |
| C6 | Medium | One `Arc<Mutex<Connection>>` serialises all database work; the app does not set `journal_mode`, so the default rollback journal applies. Large imports/blob reads block LAN/campaign requests. | `src/store.rs:10,46-48` (spot-checked) |
| C7 | Medium | Source-review routes are unauthenticated loopback, and the reviewer `actor` is a free-text field, so approvals are not bound to accounts. | `src/sources/web.rs:58-73`, `src/sources/mod.rs:75-89` |
| C8 | Low | Each region crop scans the whole `prepared_json` (≤24 MiB) for page dimensions. Mitigation: it deserialises into a geometry-only projection that skips unit data, so the cost is a full byte scan, not a full parse. | `src/sources/region.rs:14,36-63` |
| C9 | Low | Immutability enforced by large SQL triggers (with `json_extract`); migrations use a hand-written if-ladder rather than a maintained migration crate. | `migrations/007_source_finalization.sql`, `src/store.rs:64-117` |
| C10 | Low | Polygon validation checks bounds, not degeneracy/order. | `src/sources/import.rs:255-263` |
| C11 | Medium (direction) | MÖRK BORG mechanics, citations and adapter ids are hardcoded (`mork_borg.rs:115-170`, `mork_advice.rs`, string matches in `campaigns/snapshots.rs`, `campaigns/sheets.rs`, `lib.rs`, `games.rs`); the generic definitions pipeline is not used by sheet snapshots. This conflicts with the game-agnostic direction (already acknowledged in TODO as an exploration fixture). | Reviewer report |
| C12 | Low | `rusqlite` uses the `bundled` feature; FTS5 is expected to be available but no FTS5 table exists in the app. There is no HTTP/LLM client crate. | `Cargo.toml` |

## Process findings

- P1: `TODO.md` was 68 KB before this update (about 73 KB after). Many of its lines
  use space-stripped shorthand (the Stage line read "48/48correct38…" before it
  was rewritten here) that is hard to read or act on. `docs/reviews/` held 31
  records before this one, for a project whose core loop is not yet implemented.
- P2: Applying the full independent-review, packaging and offline-rebuild ritual to
  disposable experiments appears to be the dominant cost. *Proposed:* a lighter
  documented tier for throwaway prototypes, keeping full gates for shipped code.
  This requires a user decision because `AGENTS.md` is user policy.

## Proven solution landscape (external claims)

- **Ingestion:** layout-aware parsers (Marker 2, Docling, MinerU, olmOCR) handle
  multi-column layouts, tables and reading order. Marker already runs Surya vision
  models internally. Vision during **offline ingestion** is different from a
  multimodal **runtime** model; the user's "no vision" decision concerns the latter.
- **Retrieval:** hybrid BM25+dense with reciprocal-rank fusion, then a cross-encoder
  reranker on the top 20–50. Anthropic's Contextual Retrieval reports top-20
  retrieval failures falling from 5.7% to 2.9% with contextual embeddings + BM25,
  and to 1.9% with reranking.
- **Retrieval models (multilingual, small):** `bge-m3` + `bge-reranker-v2-m3`, or
  Qwen3-Embedding-0.6B / Qwen3-Reranker-0.6B (Apache-2.0). Qwen3 reranking in
  llama.cpp needed an unmerged fix at release; current mainline status unverified.
- **Storage in-process:** SQLite FTS5 plus `sqlite-vec` keeps the single-file
  database. Crate/extension fit has not been evaluated yet.
- **Rust abstraction:** `rig` (docs.rs lists a `rig` crate at 0.43; historically published as `rig-core`, identity unverified) and `genai` remain the
  candidates from `provider-abstraction.md`. llama.cpp server exposes an
  OpenAI-compatible API with tool calling, so local and hosted backends share one
  client path.
- **Prior art:** RPGX AI Assistant (Foundry VTT, Ollama + RAG server); academic
  TRPG RAG dialogue work (SBGames).

## Local model candidates (newer than the benchmarked Qwen3, not measured here)

Hardware: RX 6650 XT 8 GB, ~32 GB RAM, Ryzen 5 1600 (DDR4).

| Candidate | Why | Caveats |
|---|---|---|
| Qwen3.6-35B-A3B (Apr 2026, Apache-2.0) Q4 with llama.cpp `--n-cpu-moe` | MoE with ~3B active parameters. Expert weights live in system RAM and attention runs on the GPU. Reports show 17–40 tok/s on 6–8 GB GPUs. | Q4 is ~20 GB, tight in 32 GB with a desktop. The older CPU/DDR4 will likely be slower than reports. Unmeasured. |
| Qwen3.5-9B (Mar 2026) Q4 | Dense, fits fully in 8 GB VRAM | Hybrid architecture; check llama.cpp Vulkan support for the pinned build |
| Gemma 4 E4B / 26B-A4B (Apr 2026) | Native function calling; MoE option | Licence terms to check (Gemma licence, not Apache) |
| Qwen 4 27B | Announced 2026-09-22 | Not released as of 2026-10-12 |

Model size is expected to matter less than fixing B1–B3. That is a hypothesis to measure.

## User decisions (2026-10-15)

Recorded in [discovery](discovery.md#harness-assessment-decisions-user-2026-10-15). In summary: tool loop adopted; derived values use **declarative formulas** evaluated in Rust (template JavaScript rejected because of the browser/server round-trip); lighter process tier for experiments; vision allowed for offline ingestion only; the tool-loop benchmark comes first, with Qwen3.6-35B-A3B, Qwen3.5-9B and the 4B baseline; admin work continues in parallel; accuracy bar is ≥90% correct and ≤2% false-supported on ≥100 held-out cases.

## Proposed architecture (amended by user decisions)

1. Ingestion keeps the existing reviewed source store and hierarchy work; chunks
   carry heading path, page/region ids and revision pins (fixes C4).
2. Retrieval in Rust: FTS5 with a diacritic-insensitive tokenizer over unwrapped
   text (fixes C3/B6), plus a vector table and RRF. Optionally a reranker and
   contextual chunk prefixes. Exact-edition/source-set filtering stays before ranking.
3. **Declarative formulas** in the definition, evaluated server-side in Rust,
   compute derivations deterministically (B3; user decision). Template JavaScript was rejected. Rust stays authoritative for storage, revisions and access.
4. Tool loop instead of one JSON blob: `search_rules(query)`,
   `read_fields(paths)`, `propose_change(field, new_value, rule_ids, reason)`,
   `ask_user(question)`. Rust validates each call, fills old values and revision,
   runs derivations, renders the diff and requires explicit user confirmation.
   Constrained decoding is used only for tool arguments.
5. Provider via `rig` or `genai` against the llama.cpp OpenAI-compatible server.
6. Layered evaluation with ≥100 cases: retrieval recall@k against gold rule ids,
   answer correctness, then patch validity, each reported separately.

## Next steps (order set by user decision)

1. After the existing precondition is met (outer elapsed-deadline controller
   enforced before any new download), rebuild the benchmark as a low-temperature
   tool-calling loop on the existing demo. Run Qwen3.6-35B-A3B and Qwen3.5-9B side
   by side with the existing Qwen3-4B-Instruct-2507 baseline, on an enlarged case set.
2. Declarative derived-field formulas, evaluated in Rust. Evaluate maintained
   expression crates first; a custom parser built on `winnow` is the fallback.
3. Hybrid FTS5 (+vector) retrieval in the Rust app, with failing-first regressions
   for C3/C4/B6.
4. Admin work continues in parallel (user decision). The accuracy bar (≥90% correct,
   ≤2% false-supported on ≥100 held-out cases) gates model-dependent product work.

## Sources

- [Anthropic Contextual Retrieval summary (Maginative)](https://www.maginative.com/article/anthropics-contextual-retrieval-technique-enhances-rag-accuracy-by-67); [Gradient Flow](https://gradientflow.com/contextual-retrieval-rag/)
- [Hybrid search and reranking](https://wz-it.com/en/knowledge/ki/hybrid-search-reranking/)
- [Self-hosted Docling/Marker/MinerU guide](https://www.spheron.network/blog/self-host-document-intelligence-docling-marker-mineru-rag-guide/); [OmniDocBench](https://arxiv.org/pdf/2412.07626); [marker-pdf](https://pypi.org/project/marker-pdf/)
- [Qwen3 Embedding/Reranker paper](https://arxiv.org/abs/2506.05176v3); [Qwen3 reranker llama.cpp notes](https://www.morphllm.com/qwen3-reranker)
- [Gemma 4 vs Qwen3.5](https://codersera.com/blog/gemma-4-vs-qwen-3-5-comparison-2026/); [Gemma 4 12B vs Qwen 3.5 9B](https://www.betterclaw.io/blog/gemma-4-12b-vs-qwen-3-5-9b); [Qwen (Wikipedia)](https://en.wikipedia.org/wiki/Qwen); [Qwen 4 status](https://cellcog.ai/blog/qwen-4-release-date/)
- [Qwen3.6 35B MoE local guide](https://insiderllm.com/guides/best-way-run-qwen-3-6-35b-moe-locally/); [`--n-cpu-moe` on 8 GB](https://medium.com/coding-nexus/one-flag-tripled-my-tokens-per-second-on-an-8gb-gpu-238f55d1e3bd)
- [rig on docs.rs](https://docs.rs/crate/rig/0.43.0); [Rust AI in practice (JetBrains)](https://blog.jetbrains.com/rust/2026/09/09/rust-ai-in-practice/)
- [RPGX AI Assistant](https://foundryvtt.com/packages/rpgx-ai-assistant); [TRPG RAG dialogue (SBGames)](https://sol.sbc.org.br/index.php/sbgames/article/view/37366)
