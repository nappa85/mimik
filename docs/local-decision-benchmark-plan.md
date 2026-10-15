# Genuine local Jev-like decision benchmark

Status: independently reviewed and executed; actual source/evaluator/results
evidence in `local-decision-benchmark-2026-10-15.md` and
`reviews/2026-10-15-local-decision-benchmark.md`.
User explicitly requests actual local Jev-like tests, focused RAG context and
preload-aware timing. No hosted API, private material/database or credentials.

## Maintained implementations and runtime choices

1. **AnyJev raw versus L0** at upstream commit
   `b03158e9ba18e84c01054aaa393481925088db07` (Apache-2.0), using its actual
   `Decider`/`Question`/`LlamaCppBackend`, not chat-generated labels or a handwritten
   classifier. Reuse existing verified Qwen3-8B and Qwen3-4B-Instruct2507 GGUFs.
   Build `llama-cpp-python==0.3.36` with Vulkan and native-CPU flags disabled in
   an isolated container. Source tarball SHA256
   `832db0699007f1be95a7e41ef12e88926b02ba836461e36a36372db2760c1a2e`.
   Record underlying bundled llama.cpp revision and actual GPU-offload evidence;
   it is not assumed identical to the prior server build. Base Python3.12 amd64
   manifest `sha256:2b4f19dae3a777dfc3b76730bda1e82e1f66ab2a2686fa93ca78edbfb4f04ffe`.
2. **Kev-0.8B v1.0**, genuine trained adapter+pointer head through its maintained
   reference implementation at git `dd375f27fd61486c52204205e10a0d2e10f3b34a`.
   Checkpoint `jaredpalmer/kev-0.8b` revision
   `bf75a6a8848ea6960ff2ed108d9ed44c2941174f`; adapter SHA256
   `9b908623acb162118575f4e7a94524f9c139c335be4bfb74d6cfceca01e1885a`,
   head SHA256 `f400bd12802b2b105ae45d6b03774a158a3db4fccff42413734ddca2e5c920b6`.
   Base is `Qwen/Qwen3.5-0.8B-Base` revision
   `dc7cdfe2ee4154fa7e30f5b51ca41bfa40174e68`. Pin all downloaded base artifacts
   and hashes from that exact revision before loading; record `head.pt` metadata
   and verify its base/revision. CPU-only Torch2.8 reference FP32, threads6;
   no unsupported ROCm override or claim of Vulkan pointer-head support.
   Use `torch.load` default weights-only, no remote model-code trust.
   Its Hub base config is multimodal, but the reference decision loader is intended
   to load the text causal backbone. Verify the actual loaded module is text-only
   (no vision/image tower/processor) before scoring. If it retains vision components,
   stop and separately review a text-only checkpoint alternative; never silently
   use vision for this trial.
3. **Tacit fit:** review current reference engines/model provenance, but do not
   silently substitute generic Qwen for a trained checkpoint. Only add a pinned
   trained Tacit run if Kev is unavailable and time/disk permit; independently
   approve that artifact/runtime extension first. No success claim for unrun models.

Kev/AnyJev checkpoints are Apache-2.0; Python binding MIT and underlying llama.cpp
MIT, third-party dependencies retain their licenses. Libraries are evaluation-only
container tooling; shipping language remains Rust and no new game scripting DSL.
Pin direct packages before image build and retain resolved environment/provenance.
Use allowlisted build inputs in a dedicated temporary context, never the workspace
or `.env`/manuals. Keep weights/output outside source archives. Reserve at least
3GiB disk, cap new artifacts/images to fit currently14GiB free; no host installs,
no deletion of unrelated files. Install/download with Internet only in setup
containers; all inference network-disabled with readonly model/input mounts,
resource bounds and one model at a time. No published model ports required.

## Frozen decision task before inference

Author18 explicit synthetic input cases from existing Lantern Reach text rules:
four supported, four conflicting, four missing-fact, four missing-rule cases plus
a note-injection case and deliberate evidence-removal case. Each contains user
request, proposed values/claim/facts, explicit visible sheet-field paths and
separate evaluator-only expected label/minimal evidence anchors. Review labels
for ambiguity before freezing. Avoid unstated arithmetic/negative-rule assumptions.

Fixed choices describe **support in supplied evidence**, **conflict**, **missing
situation fact**, **insufficient/missing rule**. These are advisory findings, not
legality or permission. No model generates executable patches, field addresses,
or arbitrary JSON. Actual retrieval uses existing FTS5 BM25 top3 from input request
and target-field names, never gold rule IDs/labels. Canonical projection comes
from fixture's user-selected field paths (as UI section selection), not expected
outputs. Case IDs, gold labels and minimal anchors remain evaluator-only.

Keep all case-dependent request/facts/proposal data in **state**, with one generic
question and fixed option descriptions. This prevents content-free prior probes
from seeing actual case facts in question text. Record raw state, retrieval query/
ordered IDs, corpus/context hashes and actual rendered model prompts when possible.
Score end-to-end source-grounded decisions separately from evidence-conditioned
decision quality when retrieval missed the minimal applicable rule; freeze any
availability-adjusted target before inference, never after inspecting predictions.

## Matrix and controls

- AnyJev: raw (no prior) versus genuine L0, all four cyclic rotations and fixed
  content-free prior, no adaptive threshold fitting, canonical_order=False. Record
  actual achieved level/prior/forward counts, not a nominal L0 label if no correction.
- Every model/method sees the same states/options. Normal and reversed option
  orders, two repetitions each:72 decisions per method/model. Report probability
  normalization, top-label accuracy, per-class confusion, wrong answers with top
  probability>=0.8/0.9, order flip rate and repeated-call consistency. No probability
  is called measured RPG correctness/calibration from this tiny corpus.
- Label readout is deterministic; seeds/sampling-generation comparisons from the
  previous JSON benchmark are not equivalent tasks or denominators.
- Time model load/setup, retrieval, context rendering, first decision and repeated
  decision separately. AnyJev's existing GGUF backend clears KV per prompt and
  lacks `score_shared`; repeated calls are **not** a KV-warm benchmark. Record
  this explicitly rather than add an unreviewed custom cache implementation.
- Kev: use native reference `Server`/prefix-cache logic in-process, no web exposure.
  Repeat exact state to measure hit/miss effects; alter canonical value/revision
  and verify cache invalidation. Separate native state-cache warmup from resident
  model/kernel warmup. Its precision/backend/hardware differ from GGUF GPU runs;
  do not attribute differences solely to specialized training.

## Verification and stopping rules

Permanent helper tests cover no gold in inference inputs, scoped retrieval and
projection, fixed choice-ID mapping across permutations, finite normalized complete
distributions, scoring denominator/error persistence and cache hit/invalidation
claims using explicit native statistics rather than timing guesses. Cases/labels
and evaluator receive independent implementation review before scored runs.

Persist started/finished attempts immediately, errors and probabilities included.
Absolute request180s and method/run20min caps; no hidden CPU fallback or retries.
Log GPU/RAM metadata where available, actual backend/precision, startup and full
data/runtime/package identities. Setup/build/download failures remain unrun rather
than guessed scores. Scope this experiment to useful bounded local evidence review,
not free-form dialogue or whole-book retrieval quality. Independently review results,
record findings/regressions/handoff, stop runtimes and release GPU resources.
