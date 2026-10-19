# Experiment and benchmark results

Results whose conclusions are still in use, with exact scope, denominators and limitations.
Product-relevant measurements come first, then model and benchmark results. Raw data (ledgers,
logs, reports) stays local under `target/` and is not in the repository. The tooling is
described in [evaluation-tooling.md](evaluation-tooling.md).

## Catalogue publication lock budget (R8, 2026-10-19)

- **Question:** at which entry and unit-reference caps does a catalogue publication keep the
  SQLite write lock within 2 s, let a concurrent sheet-save-pattern writer wait and succeed,
  and stay under 256 MiB CLI peak RSS?
- **Setup:** `tools/catalogue_profile.py` against the release CLI, container
  `mimik-ci:latest` `--cpus 6`, Ryzen 5 1600 shared host, overlay filesystem, synthetic data
  (one finalized source of 1,024 × 16 units, all unreviewed). Calibrated shape v1–v3 plus five
  worst-case shapes (fan-out, FTS-heavy, fts3, 64-scalar keys, declaration-heavy), each
  published and updated. Holds are conservative upper bounds from `--no-contender` runs (three
  runs at 6,144, two more with the contender). Read latencies come from an in-process
  measurement (2026-10-18, same container).
- **Numbers** (closing runs at 6,144 entries / 6,144 references):
  - Calibrated shape (2.2 MB, 5,376 references): hold ≤ 764 ms; publish RSS ≤ 82 MiB.
  - Worst measured: fts3 update (4 MiB, 99-scalar names of 3-character tokens) 1,804.0 ms;
    keys update 1,714 ms; FTS-heavy update 1,481 ms; declarations update 1,111 ms (RSS
    ≤ 173 MiB, the peak); fan-out update 1,004 ms.
  - The contender waited through every publish and succeeded (wait up to 1,732 ms).
  - Cap selection (worst new shape): 8,192 entries failed 2 s at 8,192, 6,144 and (1 of 3
    runs) 4,096 references; 6,144 entries with 8,192 references passed by 0.01 s.
  - Other commands: import ≤ 644 ms, compare ≤ 629 ms, show ≤ 612 ms, `verify-catalogues`
    8.4 s over 13 versions, rebuild ≤ 1,043 ms.
  - Reads (in-process, 8 catalogues): browse p95 0.78 ms, search p95 29 ms, entry p95
    0.42 ms at 8,192 entries. Worst admitted browse page through `read::browse` (50 entries ×
    16 units of 256 KiB, `measure_the_worst_browse_page`): 416–441 ms.
- **Conclusion:** caps are 6,144 entries and 6,144 references (one per entry), owner text
  1 MiB in total. All R8 publish criteria are met with a margin of about 0.2 s, roughly the
  run-to-run spread. About 0.5 s of the worst hold is FTS re-tokenizing the previous names.
  The worst browse page misses the 250 ms page target. See [catalogues.md](catalogues.md).
- **Limitations:** one host, few runs per case; five shapes, not a proven worst case (many
  20,000-scalar owner texts and multilingual tokens unprofiled); the contender is the
  lock-level equivalent of a sheet save, not the HTTP route; real-route p95 not measured; a
  slower disk or busier host could exceed 2 s (not the 5 s busy timeout).

## Lexical lookup latency (2026-10-15)

- **Question:** does the Rust FTS5 lookup (`src/sources/search.rs`) meet a p95 under 50 ms?
- **Setup:** `measure_lookup_latency_on_100k_units` in `tests/source_search.rs` (`#[ignore]`),
  release build in the CI container with `--cpus 4`. Deliberately adverse synthetic corpus:
  24-word vocabulary, 40 words per unit, 10% unreviewed. 7 queries × 5 runs.
  `MIMIK_BENCH_UNITS` sets the corpus size (default 100,000).
- **Numbers** (latest, with the 1 s progress-handler budget checked every 10,000 VM ops):
  15,000 units p95 32 ms; 100,000 units p95 207 ms. Before the handler was added: 15,000 units
  p95 29 ms (max 31), 100,000 units p95 194 ms (max 202), a missing word 0.35 ms, a phrase
  37 ms.
- **Conclusion:** the target holds at about the legacy corpus size (12,956 chunks) but not for
  100,000 adverse units. See [sources-and-retrieval.md](sources-and-retrieval.md).
- **Limitations:** synthetic vocabulary; the budget bounds VM-level work only (one long FTS5
  `xFilter` call is interrupted only at its next opcode). No vector search, fusion or
  recall@k measurement.

## Campaign departure freeze cost (2026-10-13)

- **Question:** how many retained sheets per custodian and campaign can one departure freeze
  inside a request, given that every sheet gets a full-snapshot freeze revision in one
  transaction?
- **Setup:** Python `sqlite3` prototype with synthetic 256-field snapshots of exactly 1 MiB,
  one custodian leaving one campaign, shared Ryzen 5 1600 Linux host, 3 samples per size.
  Then one sample of the Rust release-mode test (32 near-1 MiB sheets frozen at revision
  9 → 10, and a failure on the last sheet that rolls the whole departure back) via
  `tools/campaign_profile.py`.
- **Numbers** (probe, median transaction ms / DB growth): 8 sheets 295 ms / 8.0 MiB; 16 sheets
  579 ms / 16.0 MiB; 32 sheets 1,175 ms / 32.0 MiB; 64 sheets 2,022 ms / 64.1 MiB. Child peak
  RSS about 40 MiB at every size. Release workload at 32 sheets: freeze 3,051 ms,
  failed-departure rollback 2,948 ms, peak 86,012 KiB (including fixture setup), under the
  5 s / 128 MiB targets.
- **Conclusion:** 32 sheets per custodian per campaign leaves about 2 s of margin under the
  5 s request timeout. Time and storage grow linearly, so a higher safety limit needs the
  freeze re-measured or moved out of the request. See [campaigns.md](campaigns.md).
- **Limitations:** shared-host observations, not bounds; one sample of the release test.

## Harness assessment (2026-10-15)

- **Question:** why are the local RAG benchmark scores low?
- **Setup:** design review of `tools/local_rag_benchmark.py` and its reports (no new runs).
- **Conclusion (hypothesis, not measured):** the low scores mostly reflect the one-shot JSON
  patch contract, not retrieval. One call had to classify, derive values, repeat host-known
  old values and addresses, cite and read back (B1–B3), at temperature 0.7 (B5), over 28
  attempts. One attempt is about 3.6 points, so 6/28 versus 7/28 is noise (B4). This led to
  the tool-loop harness with host-filled metadata.
- **Limitations:** no experiment confirmed it; the tool-loop benchmark is the measurement.

## Local RAG benchmark (2026-10-15)

- **Question:** can a local model with RAG produce correct one-shot typed sheet patches?
- **Setup:** llama.cpp b11610 with Vulkan on an RX 6650 XT. Qwen3-4B, Qwen3-8B and
  Qwen3-4B-Instruct-2507, all Q4_K_M. Seven self-authored Lantern Reach rules, SQLite FTS5
  (porter) top-3 retrieval, temperature 0.7. 28 attempts per model (7 tasks ×
  original/paraphrase × seeds 17/29).
- **Numbers** (Qwen3-4B, Qwen3-8B, Qwen3-4B-Instruct-2507): gold rules in the top 3 for 24/28
  attempts (the four misses were note-summary cases missing LR-7). Mechanical contract passes
  6/28, 6/28, 5/28. Adjudicated acceptable outputs 6/28, 7/28, 5/28. Median latency 3.72 s,
  5.68 s, 4.23 s; p95 5.69 s, 20.89 s, 7.36 s. Plain-prose answers fully source-supported 4/7,
  5/7, 3/7. One CPU-only 4B case took 72.64 s (GPU about 4.10 s). 135 inference attempts in
  total.
- **Conclusion:** the tested contract is not reliable enough to ship. Most failures happened
  with the needed rule already retrieved. A thinking-mode 4B followed injected notes and
  proposed 999 vitality.
- **Limitations:** a tiny synthetic corpus that cannot test retrieval recall, an English
  stemmer, temperature 0.7 for strict output, no tool loop.

## Local decision benchmark (2026-10-15)

- **Question:** do Jev-style probability-readout decision models classify edits correctly
  (supported, conflict, missing fact, missing rule)?
- **Setup:** 18 synthetic Lantern Reach cases × 2 option orders × 2 repeats = 72 observations
  per configuration. Selected sheet fields plus FTS5 top-3 evidence. AnyJev (commit
  `b03158e9`) raw and L0 readout via llama-cpp-python 0.3.36 with Vulkan; trained Kev-0.8B on
  CPU in FP32.
- **Numbers** (correct /72; cases always right /18; median time): 4B raw 58 (80.6%), 13,
  0.47 s. 4B L0 52 (72.2%), 12, 1.89 s. 8B raw 38 (52.8%), 7, 0.82 s. 8B L0 56 (77.8%), 13,
  3.34 s. Kev-0.8B 44 (61.1%), 10, 6.52 s cold and 2.12 s warm. Wrong answers among those with
  top probability ≥ 0.9: 4B raw 12/64, 8B raw 28/62, both L0 8/58, Kev 0/0 (no answer reached
  0.9). Kev's native state cache: 36/36 misses cold, 36/36 hits warm, probability delta 0.
- **Conclusion:** the approach runs locally without parsing generated output, but decisions
  are not reliably correct. L0 helps 8B and hurts 4B. Every configuration missed
  `focus-wrong-delta`. Option order still flips winners. Several AnyJev configurations
  endorsed the empty-evidence proposal.
- **Limitations:** the 72 observations are not independent and are not comparable with the
  28-attempt patch scores. The AnyJev GGUF backend clears the KV cache before every prompt, so
  warm GPU reuse is untested. Tacit was not run.

## Hierarchy screen (2026-10-15)

- **Question:** do local models use supplied multi-book hierarchy and scope facts correctly?
- **Setup:** `tools/evidence_forest.py` (validates supplied structure, does not extract it from
  PDFs) plus 24 unique cases (18 known and 6 new scope probes) × 2 option orders = 48
  observations. AnyJev raw readout, Vulkan.
- **Numbers** (correct /48; new-scope /12; false-supported / supported predictions; median
  time): Qwen3-4B-Instruct-2507 38/48, 9/12, 6/18, 0.460 s. Qwen3-8B 26/48, 7/12, 1/11,
  0.798 s. Qwen3-4B 28/48 (47/48 completed, 1 hit the deadline), 9/12, 8/16. Kev-0.8B,
  Mapika-2B (partial download) and Tacit-4B were not run.
- **Conclusion:** 4B-Instruct gets the most right; 8B has fewer false-supported predictions.
  No model was selected; the ten-candidate tournament is unfinished.
- **Limitations:** development cases, not held-out; oracle hierarchy facts, not PDF discovery.
  One setup phase overran its 900 s cap by 27.2 s; `tools/fetch_models.py` now enforces the
  elapsed deadline.

## Tool-loop benchmark v1 and v2, first held-out split (2026-10-15/16)

- **Question:** can a local 4B or 9B model, in a tool loop over the Lantern Reach demo pack
  (`examples/tool-loop-demo`), meet the bar of ≥90% correct and ≤2% false-supported on ≥100
  held-out cases?
- **Setup:** pinned llama.cpp b11610 (Vulkan, RX 6650 XT), Qwen3-4B-Instruct-2507 and
  Qwen3.5-9B Q4_K_M, thinking off, temperature 0.1. v2: 141 held-out cases × seeds 17 and 29 =
  282 episodes per model, 0 `not_run`, max_tokens 2048, 8 turns.
- **v1 (superseded):** a 72-case stratified subset, seed 17 (9B: 48 of 72 run). Correct 4/72
  (4B) and 16/72 (9B). Interface defects dominated (path syntax, optional answer values,
  implicit proposals) and were fixed in v2. v1 and v2 scores are not comparable.
- **v2 numbers:**

  | Model | Correct | False-supported, all | False-supported, at-risk |
  |---|---|---|---|
  | 4B | 79/282 (28.0%) | 58/282 (20.6%) | 33/112 (29.5%) |
  | 9B | 153/282 (54.3%) | 70/282 (24.8%) | 46/112 (41.1%) |

  At-risk correct: 9B 42/112, 4B 25/112 (112 = 56 at-risk cases × 2 seeds). 9B per outcome:
  answer 47/68, proposal 64/102, conflict 26/46, ask_fact 10/24, stale 5/12, ask_rule 1/24
  (search loops to the 8-turn limit), refuse 0/6.
- **Conclusion:** both models fail the bar on both denominators and under every labelling
  (excluding the 14 abstaining `answer` episodes and 8 ambiguous-gold episodes still leaves 9B
  at-risk false support at 24/112). The failure is systematic: the 9B is false-supported on
  both seeds in 26/141 cases. Main sources: following injected item text (9B obeyed it in
  27/44 Dara episodes), unsupported answers, wrong or incomplete at-risk proposals. The fair
  harness fixes considered look insufficient. Typed actions, untrusted-text handling and an
  abstention/routing stage are untested hypotheses.
- **Limitations:** the 141 cases were read during failure analysis, so they are now dev cases;
  a fresh blind held-out set was frozen
  ([tool-loop-heldout-v3-freeze.md](tool-loop-heldout-v3-freeze.md)). Untested levers:
  thinking on, more turns, other quantisations and temperatures. Scoring leniencies by design:
  value-only answer matching, empty citation lists count as valid, `answer` on an expected
  proposal is incorrect but not false support, `no_outcome` is never false support.
  Independently reviewed (three reviewers).

## Small-model candidates: CPU S40 screens (2026-10-17)

- **Question:** is any small open model (≤ ~8B) a better tool-loop candidate than the Qwen 9B?
- **Setup:** harness v3, C0 settings (thinking off, temperature 0.1, 8 turns, max_tokens
  4096, ctx 24576), dev subset `dev-screen-40` (40 cases, 16 at-risk), seed 17, pre-rev3 pack,
  frozen commit `b085aa4`, **CPU only** (6 threads). The 9B and 4B references ran on the GPU
  earlier, so the device differs. The 8B dense candidates (Granite 4.2 8B, Llama 3.1 8B,
  Ministral 3 8B) were not screened (too slow on CPU).
- **Numbers** (all of 40; at-risk false support of 16 where recorded):

  | Model | Correct | False support | At-risk FS |
  |---|---|---|---|
  | Qwen3.5-9B (GPU reference) | 25 | 8 | — |
  | Gemma 4 E4B (Q4_0) | 19 | 10 | 4/16 |
  | LFM2.5-2.6B | 17 | 18 | 10/16 |
  | Qwen3-4B-2507 (GPU reference) | 14 | 17 | — |
  | Granite 4.2 3B | 13 | 16 | 12/16 |
  | MiniCPM5-2B | 10 | 4 | 1/16 |
  | LFM2.5-8B-A1B | 8 | 9 | — |

- **Conclusion:** none beats the 9B; all are dropped as standalone candidates. Proposals are
  comparatively good (Gemma changes_exact 11/15) while outcome decisions are poor (Gemma
  answer 1/9), which motivated the decision-router screen.
- **Limitations:** unreviewed self-analysis; one seed, 40 cases; CPU versus GPU references;
  the 4B reference figures were reported, not re-verified. CPU speed: about 122 s per episode
  (LFM2.5-8B-A1B preflight).

## Decision router SYS-R: raw-readout routers on dev (2026-10-17/18)

- **Question:** can a separate model pick the outcome kind (7 options) from the C0 initial
  state, so that supported routes are safe (0 unsafe) with supported recall ≥ 84/105?
- **Setup:** dev pack rev3, 175 cases (105 supported, 70 at-risk), CPU only, grouped 5-fold
  nested CV for the threshold τ, pre-registered gate (recall ≥ 84/105, 0 unsafe, ≤ 8
  failures). Ablation and order-flip units on the 40 S40 ids. Routers: R4 Kev 0.8B
  (out-of-distribution 4-label head), R1 Qwen3-4B raw readout, R3 Qwen3.5-9B raw readout.
  Reports hold counts and hashes only.
- **Numbers:**

  | | R4 Kev 0.8B | R1 Qwen3-4B | R3 Qwen3.5-9B |
  |---|---|---|---|
  | Raw top-1 correct | 68/175 | 100/175 | 87/175 |
  | Raw unsafe, all / at-risk | 56/175, 41/70 | 46/175, 43/70 | 26/175, 26/70 |
  | Raw supported recall | 53/105 | 79/105 | 48/105 |
  | Out-of-fold supported recall (0 unsafe) | 0/105 | 0/105 (τ NONE) | 18/105 (τ 0.95) |
  | Gate | fail | fail | fail |
  | At-risk top-1 vs B-risk 29/70 | 15/70, null | 21/70, null | 39/70, not null |
  | Ablation full / empty / request-only (of 40) | 13 / 15 / 6 | 26 / 9 / 13 | 23 / 9 / 9 |
  | Order flips (of 40) | 20 | 18 | 11 |
  | Median s per decision | 14.1 | 152.5 | 241.9 |

  S40 fixed-output replay on the C0 9B ledger (non-gating; C0 alone 25/40 correct, 8/40 false
  support): R4 4/40 and 0, R1 10/40 and 0, R3 16/40 and 3.
- **Conclusion:** null for all three; no live SYS-R decision file was built. The raw-readout
  router does not reach the gate with these models, this prompt and this state. For R3 even
  the raw ceiling (48/105) is below the floor. Strong option-order (primacy) effects.
- **Limitations:** the proposal option's precondition (a clean `propose_change` preview)
  cannot be met from the router input, while `answer` can, which predicts R3's
  proposal → answer errors (instrument defect; any follow-up must make every option decidable
  from the rendered state). The router sees the C0 initial context only (collection entries
  carry only id and name; request-only retrieval). Raw single-order readouts, no permutation
  averaging or calibration. R4 was out of distribution. R3 ran past its pre-registered 12 h cap
  by user decision (budget rule only). The S40 replay mixes a pre-rev3 ledger with rev3
  expected outcomes. A post hoc binary supported/at-risk readout for R3 admits only 10/105 at
  zero at-risk. The 175 records are now development data. Independently reviewed with
  recomputation.

## Reviewer benchmark (2026-10-16, concluded)

- **Question:** which models give accurate independent reviews of Mimik plans and results?
- **Setup:** 14 models on frozen, sanitized snapshots via `tools/external_review.py`
  (read-only opencode `reviewer` agent; Sonnet as a fresh subagent). Experiment 1: M3 plan
  review, scored against an adjudicated answer key (38 weighted points, extended to 44).
  Experiment 2: harness v2 results review (3 reviewers, no key). Experiment 3: harness v3 plan
  review (6 reviewers, adjudicated).
- **Numbers** (experiment 1, weighted recall /38): Sonnet 23 (61%, 0 wrong), Sol 20 (53%,
  0 wrong, the only one to find the weight-2 defect K40), Space Bunny 19 (50%, 3 wrong), Muse
  Spark 16 (42%, 2 wrong, 1 harmful), DeepSeek v4.1 Flash 14 (37%, 1 wrong), Luna 15 (39%),
  MiMo 15, GLM 10, Cat Jam 9, LongCat 7, Nemotron Super VL 7. LL-alpha, Nemotron Lightning and
  Nemotron Ultra gave no output. All free models together: 31/38. Experiment 3: Sol 18/18
  findings valid, DeepSeek 15/15, Sonnet 16/17, Space Bunny 25/28 with 2 harmful.
- **Conclusion:** Sonnet and Sol were consistently the most accurate. Space Bunny was the
  broadest free model and the most over-severe. Verdicts on their own are unreliable (four
  NO-GOs and two GO-with-amendments on one plan, all fixable). The reviewer setup in
  `AGENTS.md` follows from this.
- **Limitations:** one adjudicator, one task per experiment, mostly one run per model; two
  Sonnet runs on the same input overlapped by about half. Reviewers without a shell are at a
  disadvantage on data-heavy reviews (a grep failed on JSONL lines over 64 KiB).

## Legacy Pathfinder RAG diagnostic (2026-10-15)

- **Question:** can the old Pathfinder 2e RAG output be reused as Mimik evidence?
- **Setup:** read-only stdlib inspection of the legacy README, extractor, index and retriever,
  plus their files (counts only).
- **Numbers:** 7 Markdown files, 10,340,102 bytes. 12,956 chunk records (10,986,792 bytes)
  with only `id`, `source`, `section` and `text`. 39 records over 1,500 characters, the
  longest 68,888. 0 duplicate IDs.
- **Conclusion:** not reusable directly as evidence. Chunks drop page and region, source hash,
  extraction identity and review state. Breadcrumbs keep only heading levels 1–2. The embedder
  caps input at 512 tokens, so long chunks risk truncation. Source filtering happens after
  global retrieval. The existing FAISS+BM25 setup shows that adding vectors does not fix
  missing evidence.
- **Limitations:** no live retrieval was run, no PDF was compared visually, and no error rate
  was estimated.
