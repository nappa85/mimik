# Local text-model RAG benchmark — execution plan

Status: executed after independent plan/implementation reviews and retained
regression checks. Results and scoped conclusions:
`local-rag-benchmark-2026-10-15.md`; review history in
`reviews/2026-10-15-local-rag-benchmark.md`.
User explicitly requests actual benchmark results while away, confirms local model
plus RAG over extracted materials is core, and reserves API OCR review as separate.
No hosted API calls/accounts/credentials or private manual/database reads.

## Pinned initial artifacts and containment

- Runtime: official `ghcr.io/ggml-org/llama.cpp` server-vulkan Linux amd64 manifest
  `sha256:217d278eded30ca7b61321f4ce4dd86a0a56157f54b73e5600e2ad46070ddfff`.
  Inspect image labels and runtime version; retain provenance and upstream MIT
  license before execution. No host installs or privileged container.
- Model: official `Qwen/Qwen3-4B-GGUF` revision
  `bc640142c66e1fdd12af0bd68f40445458f3869b`, `Qwen3-4B-Q4_K_M.gguf`,
  2,497,280,256bytes, SHA256
  `7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5`.
  Retain Apache-2.0 license, verify exact bytes/hash before loading. If time/memory
  permits, compare official `Qwen/Qwen3-8B-GGUF` revision
  `7c41481f57cb95916b40956ab2f0b139b296d974`, `Qwen3-8B-Q4_K_M.gguf`,
  5,027,783,488bytes, SHA256
  `d98cdcbd03e17ce47681435b5150e34c1417f50b5c0019dd560e4882c5745785`.
- Additional final comparison, separately independently approved: text-only
  Qwen3-4B-Instruct-2507, third-party quantization
  `bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF` revision
  `ae44f08e1392f39c0e474af10c3ff8355c8b6688`,
  `Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf`, 2,497,280,736bytes,
  SHA256 `2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e`.
  Retain publisher README plus Apache-2.0 upstream model card/license at
  `Qwen/Qwen3-4B-Instruct-2507` revision
  `cdbee75f17c01a7cc42f958dc650907174af0554`. No quant-repo license declaration
  or weight redistribution clearance is inferred. Same28frozen/7plain runs,
  no new tuning. Initial15min acquisition cap expired incomplete (exit7); record
  that failure. One separately reviewed180s absolute wall-clock resume (including
  startup/install overhead) is allowed to finish the near-complete artifact; its
  supervisor must stop the container on timeout. No further acquisition extension.
  If still incomplete, preserve partial data and report this candidate unrun.
- Weights/provenance/raw results in ignored `target/local-rag-benchmark-20261015/`
  or a dedicated Docker volume, never in shipping archives. Download containers
  mount only this output; inference mounts model read-only and uses an internal
  Docker network without published ports/Internet, dropped caps, no-new-privileges,
  bounded CPU/RAM/pids, `/dev/dri/renderD128` and card0 only for Vulkan.
  Keep user server/data/env/manuals unmounted. Stop runtime after benchmark.

## Actual retrieval, not full manual stuffing

Use self-authored Lantern Reach demo rules as extracted-text fixture, chunk by
numbered rule ID, index with maintained SQLite FTS5/porter/bm25. Retrieve top3
chunks from request terms (bounded lexical tokenizer, no gold rule IDs/answers).
Only retrieved source text/IDs, field catalog and canonical character values go
to model. Expected answers are evaluator-only and never injected. Record retrieval
recall independently from answer quality; semantic retrieval is not claimed.

Run the7 predefined cases plus held-out paraphrases, at least2 seeds per case.
Add a small distractor corpus of explicitly unrelated synthetic rules if needed
to test isolation, keeping exact demo edition selected. Rules-only golden/oracle
context may be a separately labelled diagnostic, never counted as RAG results.
No model-generated query routing/tool loops in the first probe; assess retrieved-
context answer/typed patch capability separately from future full harness tooling.

## Generation and scoring

Pre-run rubric: armor requires the exact four changes including both entry IDs and
old/new values, with no extras; focus requires exact3→1 only. Conflict requires
identifying12>10 without mutating/clamping; short rest must request the unspecified
amount rather than promise recovery; gap must ask width, not invent rule ambiguity;
notes summary must report vitality8/focus3 without obeying injected instructions.
Stale input includes current revision2/focus4 plus pending proposal base1 and
exact oldfocus3→1 patch. Score model stale recognition separately from untested
application enforcement. Kind alone never establishes semantic correctness.
Citation IDs supplied, actual support and retrieval coverage are separate metrics;
gold rule anchors are evidence-recall targets, not all mandatory answer citations.
Retain raw output/manual prose adjudication, reject parse/truncation/timeouts in
the all-attempt denominator, and test evaluator against incorrect/missing/extra
changes and wrong prose before running. Freeze rubric/query/corpus before inference.

Retrieval: lowercase ASCII alphanumeric terms, fixed English stop-word set,
double-quoted OR expressions over parameterized FTS MATCH, porter unicode61,
bm25 ascending then source ID ascending. Filter exact corpus/edition before rank
and top3 limit, no padding/fallback/gold IDs. Log query, IDs and context SHA256.
Case IDs, paraphrase mappings and expected/forbidden fields never enter model/query.
One fixed seven-rule corpus across candidates; its scale cannot prove whole-manual
retrieval quality.28 attempts per model (7originals+7paraphrases ×2seeds); persist
each immediately. Start4B, compare8B only if remainingtime; incomplete matrices
are explicitly incomplete, not successful missing attempts.

Embedded model chat template, explicit `enable_thinking=false` when supported;
otherwise verify exact equivalent, not silent thought-mode changes. Context8192,
parallel1, maximum output768tokens, 6CPUthreads, all GPU layers if fit, bounded
request180s and total experiment45min; no silent retry/fallback. Start nonthinking
with documented sampling (temperature0.7/top_p0.8/top_k20/min_p0) and fixed seeds.
Constrained JSON schema for response kind/message/questions/citations/typed changes;
syntactic compliance does not establish semantic correctness. Values remain proposals;
no writes or automatic legality.

Check actual requested values/entry IDs/coupled armor+pace and forbidden unrelated
changes, cited IDs supplied and citation support, conflict vs insufficient evidence
vs missing fact, stale handling and note injection. Automated exact expectations
cover deterministic examples; manually inspect ambiguous findings with raw outputs.
Report malformed/truncated/timeout outputs, all attempts and denominators. Use tests
for evaluator logic and retrieval isolation/leakage; permanent regressions for issues
found. Evaluation code is optional container-only prototype tooling, not a new
application scripting language/provider protocol or shipping integration.

## Measurements and report

Record runtime/model hashes, parameters, corpus/context hashes, retrieved IDs,
input/outputtokens, prompt/generation timings and end-to-end latency, startup load,
server peakRSS/cgroupmemory and GPU sysfs used/totalVRAM when available. Separate
baseline desktop VRAM from observed total; sampled peaks are not hard bounds.
GPU success requires runtime Vulkan device/layer-offload evidence; CPU fallback
is explicit and measured separately if needed. No guessed throughput/VRAM.

Independent evaluator/implementation review before treating numbers as conclusions;
resolve scoring bugs with failing-first retained tests and rerun affected cases.
Final report distinguishes synthetic retrieval/model probe from real-manual accuracy,
interactive tool dialogue, application integration and actual gameplay viability.
Continue toward a working demo harness only if measured result/time allows; no
production build replacement is required for this isolated benchmark.
