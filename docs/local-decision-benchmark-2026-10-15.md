# Genuine local Jev-like decisions — 2026-10-15

Status: actual local inference completed, outputs/report independently checked;
source-package checks and runtime cleanup completed. No hosted Jev/API calls, no private
manuals/database/credentials, no application model integration or sheet writes.

## Result

**The Jev-like path runs locally and removes generated-output parsing from the
decision step. It is promising, but it does not make decisions reliably correct.**
This experiment used actual AnyJev probability readout and a genuine trained Kev
checkpoint—not a prompt asking a chat model to print JSON.

| Configuration | Correct observations | Consistent success across all orders/repeats | Median decision time | Cases with order flips |
|---|---:|---:|---:|---:|
| Qwen3-4B-Instruct2507, AnyJev raw |58/72 (80.6%)|13/18|0.47s|4/18|
| Same4B, AnyJev L0 |52/72 (72.2%)|12/18|1.89s|2/18|
| Qwen3-8B, AnyJev raw |38/72 (52.8%)|7/18|0.82s|5/18|
| Same8B, AnyJev L0 |56/72 (77.8%)|13/18|3.34s|2/18|
| Trained Kev-0.8B, CPU FP32 |44/72 (61.1%)|10/18|6.52s cold /2.12s warm|3/18|

There are **18 unique synthetic cases**, each with normal/reversed options and
two repetitions.72 observations are not72 independent examples. These four-way
classification results must not be compared numerically with the earlier28-observation
free-form JSON/patch acceptance scores: the tasks and output requirements differ.
Model size, precision, architecture and hardware differ; this does not isolate
specialized training or establish a general model ranking.

L0 here specifically means **four rotations, content-free prior, strength1.0,
logmean combination**, not the default batch-prior configuration or every AnyJev
mode. It improved the8B result by18 observations and reduced option sensitivity,
but decreased4B accuracy by six observations. It is not a universal accuracy gain.

## Context and actual retrieval

The source is the original self-authored Lantern Reach fixture, not remembered
game knowledge or a developer-authored Rust game engine.18 source-reviewed cases:
four supported, four conflict, four missing-fact, four missing-rule cases plus
note-instruction and withheld-evidence controls. Expected labels/minimal evidence
IDs are evaluator-only.

Each model receives:

- only the selected sheet-section fields and relevant equipment IDs, with missing
  properties left absent rather than invented as zero;
- the actual request, proposed values/claim and supplied facts;
- up to three real FTS5/BM25 matches from request text plus selected field names;
- the same fixed question and four option descriptions.

Precedence is frozen: known supplied-rule conflict first; otherwise missing rule
before missing situation fact; otherwise support for the **specified check**, not
comprehensive legality. All case-dependent data is in state, not question text,
so AnyJev's content-free prior probes cannot see the actual case facts.

All necessary anchors were retrieved in the17 non-withheld cases. The18th
deliberately removes evidence and expects a missing-rule finding. `expected` and
`expected_context` therefore coincide in this matrix; equal scores do not establish
behavior under accidental retrieval misses. The empty-evidence control asks for
source-backed support, not merely whether an arithmetic result is plausible.

Independent review rebuilt all18 contexts/queries and confirmed logical state,
question and displayed option texts match across every backend. AnyJev's saved
prompts and Kev's native converted records/token counts were checked. No target
labels, case IDs, required-rule lists or truncation entered model inputs.
Equivalence is at the logical state/question/option-text boundary; AnyJev and Kev
use different prompt encoders and tokenized representations, which were not
required to match.

## Genuine implementations, not generic JSON substitutes

**AnyJev** commit `b03158e9ba18e84c01054aaa393481925088db07`:
actual `Decider`, `Question` and `LlamaCppBackend`. Raw reads next-token label
probabilities once; L0 reads every cyclic option rotation and applies the fixed
content-free correction. Returned L0 probabilities were independently reconstructed
from recorded position distributions/priors, agreeing within2.23e-16.

Content-free probe strings are `N/A`, empty string and `[MASK]`. They were primed
on empty state for both option orders before scored decisions; their setup time
is recorded separately. Normal/reversed question keys keep separate cached priors.
No adaptive threshold fitting, labelled calibration or running batch prior was used.
Canonical option ordering was disabled intentionally to measure external ordering
sensitivity. All real rows report the requested raw/L0 level and prior; one flat
prompt per raw decision and four per L0 decision. The single recorded backend
invocation can contain four actual prompt evaluations.

GPU binding: `llama-cpp-python==0.3.36`, official sdist SHA256
`832db0699007f1be95a7e41ef12e88926b02ba836461e36a36372db2760c1a2e`, built with
Vulkan and `GGML_NATIVE=OFF`. The v0.3.36 source tag identifies bundled llama.cpp
submodule `0c1e57098bba43ac29e6e3b677cdceebdd22334f`; actual installed source is
pinned by the sdist hash. This is a different runtime build from the previous
llama-server benchmark. Vulkan device/offload evidence is in the worker logs.
Existing verified4B-Instruct/8B GGUF weights were reused, context4096, threads6,
full GPU layers requested. No sampling or generated decision strings.

**Kev** reference commit `dd375f27fd61486c52204205e10a0d2e10f3b34a`:
trained `jaredpalmer/kev-0.8b` checkpoint
`bf75a6a8848ea6960ff2ed108d9ed44c2941174f`, pinned adapter/head and
`Qwen/Qwen3.5-0.8B-Base` revision `dc7cdfe2ee4154fa7e30f5b51ca41bfa40174e68`.
Exact downloaded sizes/LFS or Git-blob hashes verified before loading; base and
adapter/head hashes checked again in the worker. Native `Checkpoint`/`Server.probs`
path, not a reimplementation of its pointer head. Raw unrounded probabilities are
used, not the public formatter's rounded values or rescaled `confidence` field.

Actual loaded backbone is **Qwen3_5TextModel**, CPU FP32, Torch2.8.0+cpu, no retained
vision modules, no image inputs/processors. The base Hub package has multimodal
metadata; this test uses the reference text-only causal backbone. GPU pointer-head
support was not assumed from ordinary GGUF compatibility. Checkpoint temperature
is2.3510958125672174; it is publisher-fitted, not calibrated on these RPG cases.
Tacit was assessed but **not run** in this experiment; no Tacit result is implied.

## Preload: an actual measured difference

Model weights stayed loaded in every scored configuration. But model residency,
retrieval-index reuse and neural prefix-cache reuse are different things.

### AnyJev's current GGUF backend

The maintained backend explicitly clears KV memory before every prompt and has
no `score_shared`. Repeated calls therefore do **not** test warm KV-prefix reuse.
Four-prefill L0 is correspondingly slower than one-prefill raw. The listed GPU
latencies are not the best possible cache-aware deployment latencies, and no
custom cache patch was added to obtain favorable numbers.

### Kev's native state cache

Adjacent controlled pairs clear only the native state cache for the first call,
then repeat the exact state/question. Native statistics verify:

-36/36 cold calls were state-cache misses.
-36/36 warm calls were state-cache hits.
- Median falls from **6.5171s to2.1183s**, approximately3.08× on this CPU run.
-36/36 paired distributions meet the frozen1e-4 parity gate.

Four separate, unscored invalidation probes each run cold, warm, changed state and
fresh-cache changed state. Changes to canonical value, sheet revision, evidence
revision and template pin all miss as expected. All four changed-versus-fresh and
probe cold/warm comparisons have **maximum probability delta0**.

This verifies content-keyed state reuse, **not application revision authorization**.
The repeated exact-question measurement is not a full interactive multi-question/
changed-sheet GPU preload test. Changing a value serialized into the cached state
requires recomputation; a future static-rules-prefix design may reuse more across
edits, but that optimization was not measured here.

## Confidence and failure findings

| Configuration | Observations with top probability≥0.9 | Wrong among them |
|---|---:|---:|
|4B raw|64|12|
|4B L0|58|8|
|8B raw|62|28|
|8B L0|58|8|
|Kev0.8B|0|0|

Kev's0/0 is **zero coverage at this threshold**, not zero risk or perfect calibration.
AnyJev's values are restricted-label probabilities/bias-corrected readouts, not a
measured probability of RPG correctness. No threshold was selected after seeing
outputs and no model finding authorized a sheet change.

Important cases:

- Every configuration misses `focus-wrong-delta` across its four observations:
  spending2 from3 while proposing4 conflicts with the supplied subtraction rule.
  A direct probability readout has no explanatory text revealing the internal cause.
- Several AnyJev configurations endorse the deliberately empty-evidence proposal;
  they cannot be trusted to refuse unsupported rule judgments merely from prompting.
  Kev correctly returns missing rule on this control, but makes other false
  missing-rule/conflict judgments on supported edits.
- Normal/reversed option order still changes winners. Counts include cases where
  **both** winners are wrong; counting only changes in correctness would undercount.
- All same-order repeated winners agree (36/36 pairs per configuration), but stable
  wrong answers remain wrong. No exact top-probability ties occurred; the evaluator's
  semantic-ID tie rule would not manufacture presentation-order flips.
- Note-instruction results do not establish general injection resistance. Adding
  the notes field also changes the retrieval query/evidence relative to the clean
  case, so it is not a fully isolated causal injection experiment.

Four-way confusion is in the raw records. L0's reduced order sensitivity is a real
benefit, but it does not remove the task's semantic errors or make model outputs
safe for automatic legality decisions.

## Timing boundaries and environment

Table timings are **resident, in-process library decision wall time** including
their tokenization/model/readout path, excluding earlier retrieval/projection and
later prompt/token logging. Kev includes its native queue/encoding/cache path.
There is no generated answer or HTTP transport to time. Full phase separation
between neural prefill and label readout was not instrumented; no decomposition
is inferred from the wall times.

Supplemental non-neural preparation microprobe (20 fresh indexes,360 requests,
same Python/SQLite image, all reconstructed state hashes equal the scored inputs):

| Preparation phase | Median |
|---|---:|
| Fresh seven-rule in-memory index construction |0.633ms|
| Selected-state projection |0.032ms|
| Actual BM25 lookup |0.084ms|
| Serialization plus context hash |0.040ms|

This is not whole-manual indexing or cold-disk OCR performance. Neural context
processing/readout dominates these tiny fixture preparations.

Execution environment: Linux/Ryzen5 1600/about32GB RAM/RX6650XT8GB. GPU container
images use Python3.12.15, SQLite3.46.1, AnyJev0.3.0, NumPy2.5.4, Mesa25.0.7;
resolved environments are retained inside the images and pinned build recipe.
Only one model was benchmarked at a time; no concurrent compiler workload.
Observed worker peakRSS: approximately2.46GiB4B,4.56GiB8B,4.99GiBKev CPU,
including load/merge high water and shared mappings. Whole-GPU peak was not sampled
in this probe; earlier server-run GPU peaks are not relabelled as this run's values.

All360 scored observations completed;16 native invalidation-probe calls completed
separately. Actual library prior-setup empty-state calls are unscored setup, not
extra independent cases. All three execution/cache-verification records passed.
The Kev container exited0 after8m44s without OOM; its console attachment later
timed out, but persisted timestamps/complete ledgers establish no neural timeout.

## Artifacts, checks and next step

- Source/evaluator: `tools/local_decision_benchmark.py`, `prepare_decision_model.py`,
  their permanent regressions and `examples/sheet-harness-demo/decision-cases.json`.
- Optional evaluation Docker recipe: `containers/decision-benchmark.Dockerfile`;
  builds used an empty allowlisted context and container-only tools. No large host
  `/tmp` build tree was recreated after the authorized cleanup.
- Local ignored artifacts: `target/local-decision-benchmark-20261015/`; input
  manifests, model hashes/provenance, actual state/prompts/token IDs, unrounded
  probabilities, native cache counters, timestamps, verification records and logs.
- Independent plan, evaluator/case and result reviews recorded in
  `reviews/2026-10-15-local-decision-benchmark.md`. Results review recomputed all
  scores, order flips, L0 transforms and cache flags, not just tool summaries.
-55 helper regressions, Ruff lint/format, Cargo formatting, locked offline package
  compilation/member allowlist and all55 helper tests from unpacked corresponding
  source passed. All inference containers are stopped; no shipping app behavior,
  authentication policy or user database changed.

**Recommended next step:** use this evidence to build a small user-testable local
RAG/sheet harness with bounded advisory decisions, not more admin panels or an
automatic validation guarantee. Keep addressing, state versions, source availability
and explicit application of edits deterministic and game-agnostic. A cache-aware
GPU decision backend and better trained small checkpoints remain candidates, not
assumed solved capabilities. This pilot is not an RPG accuracy, production-readiness,
full dialogue, citation-generation or practical gameplay certificate.
