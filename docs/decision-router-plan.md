# Decision router plan (prototype tier)

2026-10-16. Status: **revision 2 after review (Sol NO-GO, Sonnet GO-with-amendments, third
unavailable); requires the user's explicit authorisation to reopen H10 as a dev-only track,
plus a short re-review.** Nothing built or run. Review record:
[reviews/2026-10-16-decision-router-plan.md](reviews/2026-10-16-decision-router-plan.md).
Code base: branch `harness-v3` at `b085aa4` (v3 harness, registry, queue, CPU device mode).
**The GPU is out of service** (cooling fault, junction 29→86 °C in 2 s under compute), so
every step below is CPU-only; GPU items wait behind the *GPU repaired* gate (§7).

## Current state

**Source-verified** (`tools/tool_loop_benchmark.py` at `harness-v3`)
- 7 expected outcome kinds plus 4 harness outcomes (`:61-67`); at-risk set (`:68`); bar
  (`:69`): held-out, ≥100 cases, ≥90% correct, ≤2% false support.
- False support needs outcome `proposal` or `answer`: `outcome_error`, a proposal where none
  was expected, or a value reason (`wrong_answer`, `over_cap`, `wrong_proposal`,
  `unreported_violation`) (`:1736-1816`). Answer on an expected proposal is only incorrect.
- `public_case` returns `id`, character, request and setup (`:830-838`); the rendered context
  omits `id` but carries the character as `sheet_id` (`:1127-1143`).
  `interleave_by_outcome` orders by **expected** outcome (`:2719-2732`). `summarize` groups
  by label × arm × split, `bar_verdict` judges any held-out group (`:2038-2049`,
  `:2177-2217`), and `GROUP_IDENTITY` is checked only within a group.
- `tools/local_decision_benchmark.py:174-175`: the AnyJev worker **refuses without Vulkan**
  and requests `n_gpu_layers=-1`, `n_ctx=4096`; the Kev worker is CPU-only (`:248-293`).
- `tools/model_registry.py`: single-GGUF entries with `sha256` and pinned image (`:241-271`).
  `tools/run_queue.py` drives only the tool-loop runner; estimate and completeness assume
  GGUF and episodes (`:690-725`). Watchdog fallback: fixed names plus registered checks only
  (`toolloop_ops.py:240-250`). CPU device mode: `DEVICE=cpu`, no `/dev/dri`, `-ngl 0`; CPU
  temperature unreadable (no `k10temp`), recorded as such.

**Recomputed locally** (no case text read)
- Dev pack: 174 cases — proposal 67, answer 38, conflict 29, ask_rule 14, ask_fact 14, stale 8,
  refuse 4 (105 supported, 69 at-risk); 34 Italian.
- S40, 9B, seed 17 (`run-79fddee1d17d`, 9B ledger, `score` fields only): correct 25/40,
  false support 8/40. Cross-kind 3 (conflict→answer, ask_rule→answer, stale→proposal); value
  5 (wrong_answer 2, wrong_proposal 2, over_cap 1); 4 supported cases ended in the other
  supported kind.

**Reported, not re-verified:** 4B S40 (14/40, 17/40), latencies, the 2026-10-15 decision
numbers, local images and weights. **Measured on CPU** (Ryzen 5 1600, 6 threads,
LFM2.5-8B-A1B, one episode): prefill ≈59–67 tok/s, generation ≈23–24 tok/s, 122 s/episode.

## 0. Oracle replay (post-hoc, descriptive)

Revision 1's go rule (≥1/3 of the 9B's false support router-removable; S40: 3/8) was written
**after** seeing S40: post-hoc exploratory justification, not a pre-registered gate.
- **Oracle definition (one router only):** gold-kind routing, fixed-output replay. At-risk
  cases are replaced by the right kind; a supported case keeps its C0 score when C0 chose
  the right kind; a supported case where C0 chose the other supported kind is `unresolved`.
- **S40 bounds for that definition:** false support ≥5/40, correct ≤31/40 (35/40 only if all 4
  unresolved cases were rewritten correctly by a live rerun, which is not assumed).
- These are **conditional S40 replay bounds**, not a live or held-out result: live SYS-R
  removes finishers and so changes trajectories and values. A thresholded router is another
  definition: each abstention on a supported case lowers correct and may remove a value-side
  false support (the abstention trade-off). Recompute on D0 (174) when it exists (§7).
- Narrow hypothesis: a router reduces cross-kind false support; value errors need other fixes.

## 1. Amendment proposal for v3 §11 (needs the user's authorisation)

Append, dated and with the authoriser, never editing frozen copies:
> **§11.24a.** H10 is reopened as `SYS-R`, a **dev-only system track**. A0 stays the only
> verdict arm (§11.2). SYS-R never enters the held-out verdict. A held-out SYS-R arm is
> admissible only if its router, policy, options, state builder and decision-file hashes are
> pre-registered and committed before A0's first held-out episode, and is then reported as a
> separately labelled system result. If held-out has started, SYS-R stays dev-only.
> Cross-reference §11.23 (H9: product-only applicability predicate).

Until authorised: no harness, registry or queue edit. Afterwards: a short re-review.

## 2. Router-only screen (dev, CPU, no tool loop)

**State builder** (`tools/decision_router.py`, isolated):
- It runs in its own process over a derived case file holding only character, request and
  setup (no `id`, expected, gold, tags, language or injection metadata), so `expected` never
  reaches it. It renders through the harness's C0 context path plus `retrieved_rules`
  (request-only top-5 BM25).
- **Sentinel scan:** a copy of the pack with unique sentinels in every id, expected, gold,
  tag and language field; every backend-rendered prompt (all orders) must contain none.
- **Order:** shuffled with a fixed seed (never `interleave_by_outcome`); stateless decisions
  (AnyJev clears the KV cache per prompt; Kev's cache keyed on state only). `n_ctx` 8192;
  an over-length rendered state fails as `over_context`, never truncated.

**Question.** One generic question; 7 options = the `SYSTEM_PROMPT` outcome lines, verbatim,
hashed, fixed IDs. Routes to `stale` are reported, never claimed (8 cases).

**Candidates (CPU-first):**

| ID | Router | Condition |
|---|---|---|
| R1 | AnyJev raw over Qwen3-4B-Instruct, CPU | Router role only (the 4B stays discarded as a tool-loop model) |
| R4 | Kev-0.8B, CPU-native | — |
| R3 | AnyJev raw over Qwen3.5-9B, CPU | Only if its single-decision CPU probe fits the budget (§7) |
| B-ask, B-maj, B-risk | Constant baselines | always `ask_rule`; majority class (`proposal`); majority at-risk (`conflict`) |

Cut: L0 (R5), Qwen3-8B (R2), Kev-4B (R6); reconsidered only after a router beats baselines.

**AnyJev CPU mode** (explicit, tested): worker flag `--device cpu|gpu`. CPU: container
without `/dev/dri`, `n_gpu_layers=0`, threads 6, and the worker records
`llama_supports_gpu_offload()`, `device=cpu`, layers and threads in provenance. GPU mode
keeps the Vulkan requirement. No silent fallback either way: an unknown device, or CPU mode
with a GPU device visible, is refused. Default behaviour of the existing benchmark unchanged.

**Metrics** (case denominators, Wilson intervals): top-1 accuracy and 7×7 confusion; the
**cross-kind unsafe-route rate** (route = proposal ∧ expected ≠ proposal, or route = answer
∧ expected at-risk) over all 174 and over the 69 at-risk; supported-route recall over 105;
safe-wrong rate; ECE, Brier, risk–coverage; order flips (reversed order on the S40 ids only);
strata only where n ≥ 10; latency and RSS; failures counted as coverage loss. Route risk is
kept distinct from completed-system false support.

**Falsifiers:** each router is reported beside every baseline; one that does not beat B-risk
on at-risk top-1 (Wilson lower bound above the baseline point) is null. Ablations on the S40
ids (short prompts, minutes): empty state and request only; no accuracy drop = priors.

## 3. Threshold and selection (pre-registered, frozen before inference)

- **Policy:** route to k ∈ {answer, proposal} only if k is the argmax and p(k) ≥ τ; otherwise
  the renormalised argmax among the at-risk kinds. Failure, timeout, invalid or zero-mass
  distribution → `ask_rule`, flagged.
- **Grid:** τ ∈ {.50, .60, .70, .80, .85, .90, .95, .98, .99}. **Nested CV:** 5 outer folds
  stratified by outcome (fixed seed); in each, choose (router, τ) on the other 4 folds (the
  smallest τ with zero cross-kind unsafe routes over **all** inner cases, then most supported
  cases routed correctly, ties to fewer unsafe routes, then lower cost) and apply it to the
  held fold. The gate uses the pooled out-of-fold routes only.
- **Gate (named set: all 174 dev cases, out-of-fold):** zero cross-kind unsafe routes **and**
  supported-route recall ≥80% (≥84/105). Report the Wilson 95% upper bounds (0/174 → 2.16%,
  0/69 → 5.27%). τ\* is a point estimate; this is no statistical certification of ≤2%.
- **Freeze:** script, grid, policy, folds, seeds and hashes committed before inference.

## 4. Combined system (`SYS-R`, after authorisation)

**Fixed-output replay diagnostic** on a C0 ledger (S40 now, D0 later), never a live bound:
routed at-risk → the routed outcome; routed supported with the C0 kind equal to the route
→ the C0 score; every other routed-supported case `unresolved`, counted as incorrect *and*
as false support. Router terminals carry no message or citations and are reported as such.
It is exploratory and non-gating for any live claim.

**Live SYS-R** (`--router-decisions FILE`; off by default, v3 byte-identical when off).
Routed at-risk: host-written terminal (`terminal.tool="router"`, no model call). Routed
supported: `allowed_finishers` drops the other supported finisher; `respond` and `ask_user`
stay; no hint. First a 3-case live smoke with forced restriction, then dev 174; dev-only.

**Decision-file integrity.** Header: router runtime, image digest, model artefact hashes,
library revision, device, threads, `n_ctx`; policy hash (τ\*, grid, fallback); question and
option hash; retrieval config; state-builder sha; pack hashes. One entry per case: case id,
state hash, 7 probabilities, route. The harness records the file's sha256 in the ledger
header and refuses on: file hash mismatch, missing or duplicate cases, invalid
distributions (non-finite, wrong keys, sum ≠ 1 ± 1e-6), a route that the recomputed policy
does not reproduce, or a state hash mismatch.

**Summary and bar isolation.** Row-header `arm_type: model|system` joins the summary key and
merge identity; `bar_verdict` gives `N/A (system arm)`; selection and verdict accept only
`arm_type=model`, arm A0; `SYS-` labels required on system rows, forbidden on model rows.

## 5. How it runs

**Decision models: `tools/decision_models.json`** (separate file; `tools/models.json` and its
consumers unchanged). Per entry: `runtime` (anyjev-llamacpp | kev-torch), pinned
`image@sha256`, `library_revision`, `files` each with path, size and sha256 (GGUF; or base,
head, tokenizer, config for Kev), `devices`, `threads`, `n_ctx`, `memory_limit`,
`seconds_per_decision` (measured or null), `licence`. Paths stay under the model root; every
artefact is hashed before load.

**Runner `tools/run_decision_router.sh`:** the tool loop's runner lock and window gate,
`--deadline-epoch`; container `mimik-toolloop-v3-router` with ownership labels, `--network
none --pull never --init`, read-only mounts, memory and `--cpus 6` caps, no `/dev/dri` while
the GPU is out. Workers take `choices`/`question`/`n_ctx`/`device`, defaults unchanged.

**Queue:** arm `kind: toolloop|router`; router estimate = load + hash I/O + decisions ×
orders × `seconds_per_decision` × factor; completeness = exact case coverage. CPU inference
and loads are heavy: window only, serialised by the lock after the running CPU queue.

**Watchdog and thermal:** the router container joins the watchdog's fallback names; queue 5 s
sampling and watchdog hard limits apply; CPU temperature recorded as unavailable.

## 6. Failing-first tests (each red before its fix; on/off pairs)

- **State:** `test_sentinels_absent_from_every_prompt` (pair: planted leak caught),
  `test_builder_has_no_expected_import`, `test_retrieval_query_is_request_only`,
  `test_order_shuffled_seeded` (pair: interleave refused), `test_over_context_refused`.
- **Routing:** option IDs stable across orders; cross-kind unsafe on all 49 pairs with
  value-correct fixtures through the real `score()`, paired with value errors (wrong answer,
  over cap, wrong proposal, violation) that are *not* router-unsafe; non-trivial threshold
  boundaries; invalid or zero-mass distribution → `ask_rule`; failures as coverage loss.
- **Selection:** nested CV never sees the held fold (pair: planted leakage detected); gate
  needs zero unsafe *and* recall; baselines and falsifier; ECE/Brier known values; oracle
  replay on a synthetic ledger.
- **CPU mode:** CPU → zero layers, no `/dev/dri` / GPU → Vulkan still required; unknown
  device and CPU-with-GPU-visible refused; CPU provenance recorded; worker defaults unchanged.
- **Harness (after authorisation):** router off byte-identical / on changes only routed
  cases; router terminal makes no model call; a removed finisher is rejected through real
  dispatch; decision-file hash, missing, duplicate and invalid entries, and recomputed-route
  mismatch refused; system arm never gets the bar / model arm still does; forged relabel refused.
- **Ops:** decision-model schema and hashes; `tools/models.json` consumers untouched; queue
  router arm (window, lock, estimate, completeness); watchdog fallback removes the router
  container with docker listing failing; real-docker smoke (no model): labels, `--network
  none`, watchdog removal, cleanup verified.

## 7. CPU budget and order

Assumed state ≈3k tokens per decision; raw readout ≈ one prefill. **All rows unmeasured
estimates** except the LFM reference; dense models prefill more slowly than the MoE.

| Item | Assumed rate | Per decision | 174 + 40 reversed |
|---|---|---|---|
| LFM2.5-8B-A1B (reference, measured) | 59–67 tok/s | ≈45–50 s | — |
| R4 Kev-0.8B torch FP32 | 40–80 tok/s | ≈40–75 s | ≈2.4–4.5 h |
| R1 Qwen3-4B dense | 15–25 tok/s | ≈120–200 s | ≈7–12 h |
| R3 Qwen3.5-9B dense | 7–12 tok/s | ≈250–430 s | ≈15–26 h |

Probes (3 states × 3 backbones, incl. load and HDD hash): ≈1–1.5 h.

1. **Now (no inference):** authorisation, re-review, then code and tests; freeze §3.
2. **After the CPU screen queue (≈04:00 Saturday):** probes, R4, R1; R3 only if its probe
   projects ≤12 h and it fits before Monday 09:00. Then (minutes) CV, baselines, S40 replay.
3. **GPU repaired gate:** D0 (9B on 174, ≈2.4 h on GPU), oracle recompute, full replay,
   live SYS-R smoke and dev run; reconsider L0, Qwen3-8B and Kev-4B.
4. **Before any held-out use:** independent results review; pre-register (§1) or record null.

## Questions for the user (recommended defaults)

1. **Authorise §11.24a** (H10 reopened as dev-only SYS-R)? *Default: yes, dev only.*
2. **Fallback** at-risk argmax; **routed-supported** removes the other finisher, no hint.
   *Default: yes to both.*
3. **CPU inference counts as heavy work** (window only). *Default: yes.*
4. **Recall floor** ≥80% of supported cases at zero unsafe. *Default: 80%.*
5. **9B router on CPU** only if the probe projects ≤12 h. *Default: yes.*
6. **If AnyJev cannot load Qwen3.5-9B:** a reviewed, separately labelled llama-server
   top-logprob readout? *Default: yes, behind the GPU gate.*
7. **Keep the discarded 4B as a router backbone?** *Default: yes, router role only.*
