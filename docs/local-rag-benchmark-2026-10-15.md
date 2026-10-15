# Local RAG benchmark — 2026-10-15

Status: three local candidates and diagnostics executed, outputs independently
reviewed, benchmark runtimes stopped. No application integration or real-manual
accuracy claim. Independent review: `reviews/2026-10-15-local-rag-benchmark.md`.

## Main finding

**Local GPU inference produces seconds-scale responses on these synthetic requests. The tested
baseline model/prompt/proposal combinations are not reliable enough to ship.**
The local-first requirement does not need a paid API for this experiment. The API
key's separate OCR-review role was not used. No work account, private manuals,
user database or credentials were accessed during inference.

| Frozen baseline | Qwen3-4B Q4_K_M | Qwen3-8B Q4_K_M | Qwen3-4B-Instruct-2507 Q4_K_M |
|---|---:|---:|---:|
| Attempts |28|28|28|
| Mechanical contract passes |6/28|6/28|5/28|
| Assistant-adjudicated acceptable task outputs |6/28|7/28|5/28|
| Median request + validation latency |3.72s|5.68s|4.23s|
| p95 latency, nearest rank |5.69s|20.89s|7.36s|
| Maximum observed request latency |7.16s|21.72s|8.53s|
| Median server-reported output rate |65.85 tokens/s|38.08 tokens/s|68.55 tokens/s|
| Truncated/malformed final outputs |0|3|0|

These are tiny synthetic task counts, not estimates of general game accuracy.
All failed/truncated attempts remain in their28-attempt denominators. Timing
includes HTTP request and local answer validation, not preceding retrieval/context
construction. Server decoding throughput excludes prompt processing and is not
whole-response throughput. No background CPU build ran during measured matrices.

## What actually ran

- Ryzen5 1600, about32GB RAM, RX6650XT/about8GB VRAM, Mesa/RADV Vulkan.
- Official llama.cpp server-vulkan Linuxamd64 manifest
  `sha256:217d278eded30ca7b61321f4ce4dd86a0a56157f54b73e5600e2ad46070ddfff`,
  buildb11610, upstream commit `091b6a7fc561b78da90a0ed2d524ec995458c657`.
- Model identities/revisions/exact bytes/SHA256 are pinned in
  [execution plan](local-rag-benchmark-plan.md); both original official Qwen model
  files passed exact size/hash verification. MIT runtime and Apache-2.0 model
  licenses retained locally.
- Context8192, GPUlayers99 requested, parallel1, CPUthreads6, batch512/ubatch128,
  no prompt caching/warmup, no auto-fit. Runtime logs confirm Vulkan device usage;
  8B logs explicitly confirm37/37 layers offloaded. Inference containers used an
  internal network with no published ports, no internet/private mounts, dropped
  capabilities and resource limits. No hosted inference/fallback.
- Nonthinking, temperature0.7, top_p0.8, top_k20, min_p0, max768 output tokens,
  seeds17/29. Seven original tasks plus seven frozen paraphrases per seed.
- Real retrieve→generate pipeline: seven numbered self-authored rules parsed from
  a text extraction fixture, SQLite FTS5 porter/unicode61 BM25, request-only sorted
  OR query, exact edition scoped before indexing/ranking, top3 actual matches.
  Only request, retrieved chunks, field catalog, canonical current sheet and pending-
  proposal metadata where applicable go to inference.
  Expected answers/gold rule IDs never enter query construction or model context.
- Final candidate is a pinned third-party quantization of updated text-only
  Qwen3-4B-Instruct-2507, with upstream Apache-2.0 card/license and publisher README
  retained. Its initial15min acquisition cap expired; one separately approved
 180s wall-clock resume finished and verified the artifact in104.78s. No further
  acquisition extensions or unverified-weight inference occurred. Artifact/model
  revision and quantization provenance differ, so gains/losses are not attributed
  solely to instruction tuning.
- This is **lexical RAG over a synthetic extracted-text fixture**, not embeddings,
  a whole commercial book, OCR verification or Mimik's authenticated application
  pipeline. It tests a constituent capability rather than the finished experience.

Independent results review verified paired4B/8B request/context/query/evidence and
reconstructed payload hashes match in28/28 matrix pairs and7/7 plain pairs.
The same28/28+7/7 checks passed for the final instruction-tuned candidate.
Corpus SHA256: `72826b22b632879b9cde9015226e9ba2bdeb954587b0425e71d942325a366444`.

## Retrieval versus model failures

Top3 includes all gold evidence anchors in24/28 baseline attempts. The four misses
are note-summary cases missing LR-7; their requested current values are still
available directly from canonical state. Anchor recall is not answerability or
citation support. Many important failures happen with the needed rule present.

Examples:

-4B sometimes refuses to subtract focus3→1 because an unrelated retrieved short-rest
  rule is incomplete. A smaller top1 context does not consistently fix this.
-8B correctly calculates3→1 in all four focus answers, but uses the wrong machine
  addresses; natural understanding is distinct from a usable patch.
- Armor answers often put the correct derived armor/pace only into a readback,
  omit those actual changes, use string booleans, or emit unrelated no-op fields.
- Conflict answers sometimes correctly explain that12 exceeds maximum10 while
  still proposing the invalid edit. Correct prose does not make the patch acceptable.
- Both models fail important stale-proposal cases.8B twice claims the old3→1
  proposal was confirmed despite current focus4/revision2 and proposal base1.
  This is model output, not a tested or performed application write.
- A thinking-mode4B diagnostic follows injected notes and proposes999 vitality/focus.
  Prompt instructions alone do not establish injection resistance.

An application harness must independently enforce permissions, types, stable
addresses, revision/template pins and explicit confirmation. That does not solve
semantic errors; it prevents malformed/stale proposals from becoming silent writes.
No game-specific compiled rules were added to obtain these results.

## How outputs were judged

The schema checker validates complete JSON independently of runtime constrained
decoding. Mechanical checks require exact typed changes, old/new values, stable
addresses, supported response category and valid supplied citation IDs. Prose
heuristics are **signals only**. Every final output was inspected by the main
assistant, with independent adversarial assistant review against numbered rules.
This is not a human gameplay/expert evaluation or approval of real sources.

-4B's six complete successes are four correct missing-rest-rule answers and two
  correct note readbacks. Other answers contain useful partial reasoning but fail
  the full contract/task.
-8B's six mechanical successes are the same categories. Original notes/seed17
  deserves one additional reviewed task success: correct values, no mutation,
  benign extra maximum fields. Original notes/seed29 also gets the requested values
  right but falsely attributes a general explicit-agreement rule to LR-1/LR-2;
  it is excluded from the seven source-supported successes. Thus8/28 core task
  outcomes,7/28 acceptable source-supported outputs and6/28 mechanical passes
  describe different things, not interchangeable accuracy rates.
- Citation-ID validity is not entailment. Expanded strings such as `LR-1: ...`
  violate the expected ID format; several valid IDs accompany unsupported claims.
- Legacy automatic field `citation_present` means *citation requirement satisfied*,
  including readback exemptions, not literal presence. `prose_signal` is not truth.
- No string→boolean/integer coercion, patch repair or silent retries improved scores.

## Separately labelled diagnostic screens

Each screen uses the seven originals and seed17, not an independent held-out set.
They were chosen after baseline inspection and are exploratory diagnostics.

|4B diagnostic | Mechanical | Reviewed result | Median latency |
|---|---:|---|---:|
| Baseline matching original/seed17 subset |1/7|Rest only|4.10s|
| Top1 instead of top3 |1/7|Rest only; armor loses a required rule|3.33s|
| State-derived typed change schema |1/7|Rest only; all7 outputs meet recorded typed schema but semantics remain wrong|5.07s|
| Generic focused prompt |1/7|2/7 task successes: rest plus benign-extra-field notes readback|3.98s|
| Thinking plus1024 reasoning budget/1536 output cap |1/7|Rest only fully meets strict contract; stale/gap reasoning partially improves, injection fails|9.26s|

Typed schema is derived from canonical state/bindings, not expected targets or
game rules. It constrains addresses/types/current old values while permitting
semantically invalid integer targets. All seven typed answers were independently
revalidated against their exact recorded schemas: zero schema errors. Schema-valid
irrelevant/no-op changes remain task failures. An initial reviewer allegation of
schema violations was reproduced as zero errors and explicitly withdrawn.

Thinking changes both mode and output budget, and its reported server output rate
may include reasoning tokens; it does not isolate reasoning alone. Raw envelopes
retain reasoning fields, but only final answers are scored. No reasoning trace is
presented as an answer or proof.

### Ordinary-text RAG answers

Removing the JSON contract **and** using a shorter generic prompt gives a different
task: explanations and proposed values in prose, not executable typed patches.

| Model | Reviewed plain-text task successes | Median / max latency |
|---|---:|---:|
|4B|4/7: armor, vitality conflict, rest, notes|2.77s /3.70s|
|8B|5/7: those four plus focus spending|3.28s /5.41s|
|4B-Instruct-2507|3/7 fully source-supported: focus, armor, rest|4.24s /7.18s|

Both give a useful conditional gap explanation but fail the frozen requirement to
directly elicit width. Both mishandle stale proposals;4B also fails focus spending.
Plain mode has no automatic mechanical score: logged `pass=False`/zero mechanical
count is **not applicable**, not seven judged failures. These findings do not prove
which of prompt versus output constraint caused a difference.

### Final instruction-tuned candidate

The updated candidate gets focus arithmetic right in all four structured cases,
and all four stale responses recognize the old revision and request a fresh
proposal. Nevertheless, wrong scalar addressing and incomplete armor/pace patches
remain; three otherwise sensible rest answers and two stale answers have malformed
citation IDs. Its five full strict successes are both seed17 stale cases, rest
original/seed29 and both notes paraphrases.

In prose it has5/7 correct core outcomes, but only3/7 full source-supported outputs:
the vitality-conflict and notes answers overgeneralize a rule about **maximum**
changes into unsupported restrictions on ordinary value edits. The gap answer
incorrectly says no outer armor is equipped despite the padded coat. The stale
answer recognizes the mismatch but contradicts itself about spending and saving.
These source-support failures are not hidden behind correct final numbers.

All35 final-candidate attempts stop normally with no parse/transport errors.
This comparison did not demonstrate a reliable strict proposal contract, nor does
it erase partial improvements in arithmetic and stale understanding.

## Memory, startup and CPU control

One-second host metadata sampling, whole GPU including desktop, not process-only
GPU attribution:

| Observed counter |4B sessions|8B sessions|4B-Instruct sessions|
|---|---:|---:|---:|
| Idle GPU baseline |about0.89GiB|about0.89GiB|about0.92GiB|
| Sampled total GPU high water |4.36GiB|6.28GiB|4.40GiB|
| Approximate increment over baseline |3.47GiB|5.39GiB|about3.48GiB|
| Server VmHWM |2.46GiB|4.82GiB|2.46GiB|
| Observed cgroup memory.peak |about0.29GiB|about0.49GiB|about0.62GiB|

RSS includes shared/file-backed mappings; cgroup counters use different accounting
and are not the whole cost of weights. Do not sum these counters or equate the small
cgroup value with total RAM required. GPU samples are observed peaks, not hard bounds.
8B startup log reports4455.34MiB GPU weights +1152MiB GPU KV +28MiB compute, and
model readiness about5.46s after process startup.4B initial readiness was about3.78s.
These exclude download time and are not cold-disk/host-reboot measurements.

One separately labelled CPU-only4B control on focus spending took72.64s and reported
7.10 output tokens/s, versus about4.10s for the corresponding GPU matrix case.
It reproduced the same unrelated-rest refusal category. This is one case, not a
general speed ratio or CPU/GPU numerical equivalence test.

## Reproducibility, verification and remaining scope

- Tools: `tools/local_rag_benchmark.py`, `tools/local_rag_monitor.py` and permanent
  evaluator regressions. Existing Python/schema libraries are optional evaluation
  tooling inside containers, not the Rust application's provider integration or a
  new game scripting language.
- Raw outputs/contexts/schemas/parameters/metrics/licenses/weights are under ignored
  `target/local-rag-benchmark-20261015/`, never shipping archives. Keep source fixture
  expectations separate from inference inputs.
- `adjudications.json` records accepted case/variant/seed patterns and failure
  rationales for every completed run. Applying them to immutable finished rows
  reproduces the reviewed counts; raw automatic summaries remain unadjudicated.
-43 helper tests, Ruff lint/format, source package compilation/member allowlist and
  all43 tests from unpacked corresponding source passed. A normalized-Cargo README
  fixture failure was reproduced and fixed with a retained regression; privacy
  exclusions were not weakened.
- Rust application behavior/binary and user data were unchanged. Full Rust/browser/
  LAN release gates were not rerun for this isolated benchmark/tooling change;
  earlier schema15 shipping verification remains separately recorded.
- Downloads/runtime bootstraps, permission/path failures and interrupted/resumed
  transfer are operational overhead, not hidden model attempts. Official8B SHA256
  passed after aria2 resume. No failed generation was retried or removed.
-135 inference attempts completed across baselines, screens, smoke and CPU control;
  no aborted acquisition is counted as an inference attempt. All inference servers
  are stopped, the isolated network removed, and monitoring stopped. Weights,
  licenses, raw outputs and container provenance remain local for reproducibility.

Next integration decision must prioritize useful local RAG dialogue and a smaller,
well-tested domain-operation contract. Do not treat a larger model, longer thinking,
or valid JSON as a substitute for measured reliability. Actual application load/
sheet binding/tool dialogue and real-game materials remain unfinished.

The provisional strict response schema deliberately asks the model to serialize
addresses, old values, category and readback in one turn. Its failure rate is not
the accuracy of ordinary RAG answers. A promising next experiment is a smaller
standard tool/patch interface where Rust maintains trusted revision/old-value
metadata and constructs the displayed diff, while the model selects source-backed
values and the user confirms. This retains the pure game-agnostic core and does
not encode toy-game rules into application logic.

## Post-hoc design assessment (2026-10-15)

A later assessment ([harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md#why-the-benchmarks-score-low-benchmark-design-findings))
records design limits of this benchmark. It does not change the recorded results.

- The one-shot contract combines classification, derivation, host-known old values
  and addresses, citations and readback in one call (B1/B2). Derived values such as
  pace are left to the model (B3).
- 28 attempts cannot rank models (B4). Temperature 0.7 was used for strict output (B5).
- The FTS5 `porter` stemmer is English-only, but the target manuals are Italian (B6).
  The seven-rule corpus cannot test retrieval recall (B7).
- No tool-calling or multi-step loop was evaluated (B8).

Treat the scores as measurements of this contract, not of local RAG viability.
