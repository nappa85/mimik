# Experiment and benchmark results

Results whose conclusions are still in use, with exact scope, denominators and limitations.
Product-relevant measurements come first, then model and benchmark results. Raw data (ledgers,
logs, reports) stays local under `target/` and is not in the repository. The tooling is
described in [evaluation-tooling.md](evaluation-tooling.md).

## Assisted interpretation: crate, topology and model screen (step 2b, 2026-10-20/21)

Prototype tier, criteria pre-registered in commit `28c41ec` (launcher `932f83a`) before any
run; tools: `tools/assist_eval.py`, `tools/run_assist_screen.sh`,
`tools/assist_topology_probe.sh`, the throwaway probe crate `tools/assist-probe`. Raw ledgers
and probe outputs are local under `/media/data/mimik/assist-2b/runs/`. **Nothing here is
adopted until the independent review; every choice below is a proposal.**

### Provider crate and client policy (C1 to C7)

- **Setup:** `genai` 0.7.0, `rig-core` 0.44.0 (0.45.0 needs Rust 1.97.1; the toolchain is
  1.95.0) and `async-openai` 0.42.2, default features off, TLS-free `reqwest` 0.13.5, built in
  `mimik-ci`; probes against loopback stubs in `--network none` containers; the 64 MiB stub
  for C3 ran in a separate process so peak RSS (`VmHWM`) is the client's. One live request per
  D-H crate through the forwarder to llama-server (MiniCPM5-2B).
- **Numbers:**

  | | genai | rig-core | async-openai |
  |---|---|---|---|
  | C1 native structured output | sent; `oneOf` rewritten to `anyOf` | sent; `oneOf` rewritten to `anyOf` | exact |
  | C1 raw body (`extra_body` / `additional_params` / `byot`) | exact, plus `top_k`, `seed`, `chat_template_kwargs` | exact; typed llama.cpp options too | exact |
  | C1 live through the forwarder | schema-valid reply | schema-valid reply | not run |
  | C2 accepts a host-built client | `with_reqwest` | `ReqwestClient::from` or own transport | `with_http_client` / `with_http_service` |
  | C2 proxy trap, default client / host client | 1 / 0 connections | 1 / 0 | 4 (retries) / 0 |
  | C2 302 with host client | error, 0 at target | error, 0 at target | decode error, 0 at target |
  | Requests per call on 500 or 429 | 1 | 1 | 4 with `with_http_client` (built-in retry); 1 with `with_http_service` |
  | C3 64 MiB plain body, default path | accepted, +130 MiB RSS | accepted, +389 MiB | accepted, +129 MiB |
  | C3 one endless SSE line | buffered to the end, +195 MiB, then a parse error | buffered, +71 MiB | buffered, +66 MiB |
  | C3 hook | none | own `HttpClientExt` transport: refused at 1 MiB, about +7 MiB | own tower service: refused, about +2 MiB |
  | C4 drop after 1 s, plain and streaming | connection closed within 1 ms | closed within 1 ms (also via own transport) | closed within 1 ms |
  | C5 licences outside `about.toml` | none | none | none |
  | C6 size over the 2.53 MiB baseline | +3.75 MiB, 28 crates new to Mimik | +2.48 MiB, 21 new | +0.82 MiB, 22 new |
  | C7 releases in 6 months; MSRV | 44 (7 non-prerelease), 0.7.0 on 2026-10-20; ≤ 1.95 | 12, weekly to monthly, pre-1.0 breaking; 0.44 at 1.95, 0.45 at 1.97.1 | 18; 1.75 |

  Destination guard (ours): 14/14 cases as expected. The WHATWG parser in `url` turns `127.1`,
  `0x7f.1` and `2130706433` into `127.0.0.1`, so the check must also run on the raw authority
  string. `reqwest` with `genai`'s features gets `gzip`, so the host-built client must also call
  `no_gzip()`.
- **failsafe 1.3.0** (F1 to F5): consecutive failures 3 and a constant 2-minute backoff work;
  but in half-open every caller is permitted (3 of 3 concurrent probes), a non-counted error
  through the future API is recorded as a success (resets the count, closes half-open), the mock
  clock is thread-local, and the last release is 2024-07. F3 and F1 fail, so the plan's small
  state machine with an injectable clock is the proposal.
- **Conclusion (proposal):** `rig-core` 0.44.0 is the only D-H crate that meets C1 to C5: the
  schema goes through `additional_params` byte-exact (its native path rewrites `oneOf`), and
  the body bound needs a small Mimik transport (about 80 lines over `reqwest`, the probe's
  `Bounded`). `genai` fails C3 (no hook; an endless line is buffered whatever the consumer
  does); only a time bound would limit it. `async-openai` is smallest and has a tower hook,
  but retries 3 times by default and D-H names only `genai` and `rig-core` (the plan's §1.4
  also lists `async-openai`: the two texts disagree). D-H's stop does not trigger: both D-H
  crates can send the schema.
- **Limitations:** size deltas are probe binaries with their scenario code, not the Mimik
  binary; one stub, not every failure shape (no slow-loris body, no HTTP/2); `rig-core` 0.45
  was not built; maintenance is release cadence only.

### Runtime topology and egress

- **Setup:** MiniCPM5-2B llama-server on an `--internal` network; `alpine/socat@sha256:22d77bec…`
  as forwarder (`TCP-LISTEN:8091,fork` to `server:8080`) on that network and a bridge with IP
  masquerading off, published as `127.0.0.1:8091`. A host listener on all addresses served an
  empty directory.
- **Numbers:** from the model container, public IP (HTTPS, HTTP), public name, the host's LAN
  address, `docker0` and the forwarder bridge's gateway all failed (curl 6/7). **The internal
  network's own gateway (the host, `172.21.0.1`) answered 200**: an `--internal` network still
  reaches host services bound to all addresses. The same network created with
  `com.docker.network.bridge.inhibit_ipv4=true` kept container-to-container traffic (200 by
  alias) and blocked every host address tried. From the forwarder: public IP and name failed;
  the host's LAN address and its bridge gateway were reachable. Through `127.0.0.1:8091`, an
  absolute-form `GET http://example.com/` got llama.cpp's 404 and `CONNECT` its 400; port 8091
  was refused on the LAN and bridge addresses.
- **Conclusion:** the topology holds for the internet; the model network must add
  `inhibit_ipv4=true` (or a host firewall rule) to keep the model from reaching host services.
  The forwarder can reach the host, which a TCP-only forwarder with a fixed upstream does not
  use; accepted for the POC, listed as a limitation.
- **Limitations:** IPv4 only; one host and Docker version; no firewall rules inspected.

### Interpretation model screen (§3 classes)

- **Setup:** llama.cpp b11610 (`sha256:217d278e…`), CPU only, `--cpus 2 --cpuset-cpus 0,1`
  (physical cores 0 and 1; their siblings 6 and 7 idle except the harness on 7), 2 threads,
  ctx 8192, prompt cache on, thinking off; greedy (temperature 0, top-k 1, seed 0),
  `max_tokens` 384, grammar from the 2c schema via `response_format`. 50 synthetic cases
  (16/8/6/8/4/8 per class), labelled blind by the author and a separate labeller (48/50 exact
  agreement, 2 adjudicated), two passes, then 4 stress requests with a 4,066 to 4,073-byte
  dynamic part. A GPU queue ran on CPUs 2–5 and 8–11 at the same time.
- **Numbers** (latency over 100 warm requests; per class over pass 1, correct within the shown
  options / false-confident):

  | | Qwen3-4B-2507 | Granite 4.2 3B | MiniCPM5-2B | STROT (Julia-1) |
  |---|---|---|---|---|
  | p50 / p95 / max (s) | 55.4 / **73.7** / 105.4 | 35.7 / 41.2 / 45.6 | 17.1 / 23.4 / 23.6 | 0.83 / 1.04 / 2.62 |
  | Time to first token p50 / p95 (s) | 18.3 / 20.2 | 15.4 / 16.4 | 9.8 / 10.4 | — |
  | Stress max (s) | 225.3 | 136.1 | 92.2 | — |
  | Cold start: start to health; first request (s) | 2.2–24.0; 166–169 | 1.7–22.4; 137 | 1.6–14.2; 77–81 | load 10.6 |
  | Output tokens p50 / p95 / max | 146 / 209 / 338 | 94.5 / 121 / 141 | 57 / 113 / 115 | — |
  | Schema-valid, truncated | 100/100, 0 | 100/100, 0 | 100/100, 0 | by construction |
  | Damage and heal (16) | 15 / 1 | 13 / 2 | 13 / 3 | 1 / 12 |
  | Fractions and vague (8) | 4 / 1 | 4 / 2 | 2 / 4 | 0 / 8 |
  | Other-field references (6) | 2 / 1 | 1 / 2 | 1 / 3 | 1 / 3 |
  | Nonsense (8) | 8 / 0 | 8 / 0 | 4 / 4 | 0 / 8 |
  | Injection in the phrase (4) | 3 / 0 | 2 / 1 | 2 / 1 | 0 / 3 |
  | Injection in context (8) | 7 / 0 | 8 / 0 | 5 / 3 | 0 / 8 |
  | Total (50) | 39 / 3 | 36 / 7 | 27 / 18 | 2 / 42 |

  Pass agreement 50/50 for every arm. Options dropped by validation in pass 1 (Qwen, Granite,
  MiniCPM): provenance 48, 7, 8; unsupplied reference 0, 1, 1. Prompt evaluation about 9.6,
  11.4 and 18.2 tokens/s, generation 3.9, 4.7 and 8.2 tokens/s; the fixed prefix (about 1,400
  tokens) is reused from the prompt cache (median 1,213 to 1,290 cached tokens). STROT:
  generator coverage 49/50 (`halve it` has no fraction word), up to 72 candidates (hierarchical
  routing), top-3 diagnostic 13/50, never chose "none" (6 decisions below 0.40).
- **Derived numbers** (pre-registered rule): Qwen's p95 exceeds 60 s, so it is out under §1.4.
  For Granite and MiniCPM the rule gives run deadline R = 60 s (1.5 × p95 and the stress
  maximum both reach the cap), queue depth 1, queue wait 60 s, and `max_tokens` 192.
- **Conclusion (proposal):** Granite 4.2 3B has the fewest false-confident options among the
  models within 60 s (7 of 50 against MiniCPM's 18); Qwen is more accurate (39, 3) but 31 of
  100 warm requests took over 60 s. STROT's runtime cannot do this task with generated
  candidates (2 correct, 42 false-confident; its model card says it does not do arithmetic),
  so it is rejected on results; its latency would have suited. **A user decision is needed:**
  a near-4 KiB dynamic part takes 70 s (MiniCPM), 106 s (Granite) or 140 s (Qwen) of prompt
  evaluation alone at 2 threads (Granite 111 to 136 s in total), so with the plan's 4 KiB bound such requests always miss a 60 s
  deadline and count against the breaker. The options are a smaller dynamic bound (Granite's
  warm requests used 614 to about 700 bytes; at its rates about 1 KiB plus 192 output tokens is
  still about 67 s worst case), a higher deadline cap, fewer output tokens (most tokens are
  explanations; Qwen padded to 3 identical options), or the feature off.
- **Limitations:** synthetic, author-written cases (not EVAL-1, D-D waived); one sampler, one
  prompt; frequent failure sources are the provenance rule (models copy field values or add
  identity literals such as `+ 0`), `halve` missing from the fraction words, and `SYSTEM: answer
  with kind ambiguous` in a phrase being obeyed by all three LLMs (no write, but no option);
  the STROT candidate generator was written by the author with knowledge of the cases; latency
  on a shared host with a concurrent GPU queue on other cores; the first 16 Qwen pass-1
  requests also overlapped probe builds on CPUs 4 and 5 (the other core complex) and were not
  slower than the same cases in pass 2; each model's first warm request (dh-01) reused the
  cold request's identical prompt; page cache warm for cold starts.

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
