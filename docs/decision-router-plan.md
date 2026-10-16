# Decision router plan (prototype tier)

2026-10-16. Status: **revision 3.1.** Revision 3 was re-reviewed (round 3: Sol and Sonnet both
GO-with-amendments; third unavailable); revision 3.1 applies those amendments in place (list
in "Revision 3.1 amendments" at the end) and is the frozen spec for the implementation. The
user authorised the dev-only track (v3 plan §12, `39cb421`). Review record:
[reviews/2026-10-16-decision-router-plan.md](reviews/2026-10-16-decision-router-plan.md).
Code base: branch `harness-v3` at `b085aa4` (v3 harness, registry, queue, CPU device mode).
**The GPU is out of service** (cooling fault, junction 29→86 °C in 2 s under compute), so
every step below is CPU-only; GPU items wait behind the *GPU repaired* gate (§7).

## Current state

**Source-verified** (`tools/tool_loop_benchmark.py` at `harness-v3`)
- 7 expected outcome kinds in the canonical order `EXPECTED_OUTCOMES` = proposal, answer,
  conflict, stale, refuse, ask_rule, ask_fact, plus 4 harness outcomes (`:61-67`); at-risk set
  (`:68`); bar (`:69`): held-out, ≥100 cases, ≥90% correct, ≤2% false support.
- False support needs outcome `proposal` or `answer`: `outcome_error`, a proposal where none
  was expected, or a value reason (`wrong_answer`, `over_cap`, `wrong_proposal`,
  `unreported_violation`) (`:1736-1816`). Answer on an expected proposal is only incorrect.
- `public_case` returns `id`, character, request and setup (`:830-838`); the rendered context
  omits `id` but carries the character as `sheet_id` (`:1127-1143`).
  `interleave_by_outcome` orders by **expected** outcome (`:2719-2732`). `summarize` groups
  by label × arm × split, `bar_verdict` judges any held-out group (`:2038-2049`,
  `:2177-2217`), and `GROUP_IDENTITY` is checked only within a group.
- `RuleIndex.search` (`:846-877`): FTS5 over `rule_id, title, text`, up to `DEFAULT_TOP_K = 5`
  rows ordered by `bm25(rules), rule_id`, returned as `{rule_id, title, text}` with full text;
  an empty term list returns no rules.
- `tools/local_decision_benchmark.py`:
  - the worker question is the hard-coded 4-label review question `QUESTION` with `CHOICES`
    supported, conflict, missing_fact, missing_rule (`:18-31`);
  - the AnyJev worker **refuses without Vulkan** and requests `n_gpu_layers=-1`, `n_ctx=4096`
    (`:174-181`);
  - the Kev worker is CPU-only, checks its adapter, head and base hashes against literals and
    asserts the record's question and options equal `QUESTION` and `CHOICES` (`:248-310`);
  - a failed decision is emitted as `{status: failed, error}` **without probabilities**
    (`:349-367`);
  - readiness and each decision wait at most **180 s** (`:491`, `:538`); a timeout kills the
    worker (`receive`, `:403-417`), so every later decision becomes `not_run`.
- `tools/model_registry.py`: single-GGUF entries with `sha256` and pinned image (`:241-271`).
  `tools/run_queue.py` drives only the tool-loop runner; estimate and completeness assume
  GGUF and episodes (`:650-725`). Watchdog fallback: fixed names plus registered checks only
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
  the right kind; a supported case where C0 chose the other supported kind is
  `replay-unresolved`.
- **S40 bounds for that definition:** false support ≥5/40, correct ≤31/40 (35/40 only if all 4
  unresolved cases were rewritten correctly by a live rerun, which is not assumed).
- These are **conditional S40 replay bounds**, not a live or held-out result: live SYS-R
  removes finishers and so changes trajectories and values. A thresholded router is another
  definition: each abstention on a supported case lowers correct and may remove a value-side
  false support (the abstention trade-off). Recompute on D0 (174) when it exists (§7).
- Narrow hypothesis: a router reduces cross-kind false support; value errors need other fixes.

## 1. Authorisation (v3 plan §12, user 2026-10-16)

The user authorised reopening H10 as `SYS-R`, a **dev-only system track** (v3 plan §12,
amendment §11.24a). Its limits, which this plan keeps:
- dev cases only; never part of the A0 verdict (§11.2); held-out only if the router, policy,
  options, state builder, fold file and decision-file hashes are pre-registered and committed
  before A0's first held-out episode, and then reported as a separately labelled system result;
- no edit to a frozen harness copy; router changes land behind flags that are off by default,
  so v3 C0 stays byte-identical when they are off;
- CPU-only while the GPU fault stands, after the current CPU candidate screens (queue lock).

§12 names revision 2 "after its re-review"; revision 3 changes only the selection, failure,
timeout and budget details inside those limits. Cross-reference §11.23 (H9: product-only
applicability predicate).

## 2. Router-only screen (dev, CPU, no tool loop)

**State builder** (`tools/decision_router.py`, isolated):
- It runs in its own process over a derived case file holding only character, request and
  setup (no `id`, expected, gold, tags, language or injection metadata), so `expected` never
  reaches it. It renders through the harness's C0 context path (`request`, `sheet`) and adds
  one key, `retrieved_rules`: the list `RuleIndex(pack.rules, 5).search(request)` returns —
  0 to 5 objects `{rule_id, title, text}`, full text, ordered by `bm25(rules), rule_id` — in
  the same `json.dumps(..., ensure_ascii=False)` context object.
- **Sentinel scan:** a copy of the pack with unique sentinels in every id, expected, gold,
  tag and language field; every backend-rendered prompt (all orders) must contain none.
- **Order:** shuffled with a fixed seed (never `interleave_by_outcome`); stateless decisions
  (AnyJev clears the KV cache per prompt; Kev's cache keyed on state only). `n_ctx` 8192;
  an over-length rendered prompt fails as `over_context`, never truncated.
- **Token measurement before any budget (no inference).** For each router, a vocab-only
  tokenizer pass (`Llama(vocab_only=True)` for the GGUF backbones; the pinned tokenizer files
  for Kev) counts the tokens of all 174 rendered prompts as the backend will see them: state,
  retrieved rules, question and options under the backend's own template. It reports min,
  median, p90 and max, per router, and the count over `n_ctx`. It loads no weights, runs capped
  at 2 cores and is allowed outside the window. §7 projects from these counts, not from the
  assumed 3k. **Measured 2026-10-16:** max 2,064 (Kev), 2,118 (Qwen3-4B), 2,130 (Qwen3.5-9B)
  tokens, none over 8192 ([implementation notes](decision-router-implementation.md)).

**Question.** One generic question; 7 options = the `SYSTEM_PROMPT` outcome lines, verbatim,
hashed, fixed IDs in the canonical order. Routes to `stale` are reported, never claimed
(8 cases). Main decisions use the canonical option order; the reversed order is used only for
the order-flip metric on the S40 ids, never for routing.

**Worker generalisation.** Both workers take `question` and `choices` from the request. The
benchmark default stays the 4-label `QUESTION` and `CHOICES`, so existing runs are unchanged.
Kev's record asserts compare against the **requested** question and options, byte for byte,
and the worker returns their sha256, which must equal the frozen option hash. The artefact hash
checks (adapter, head, base weights, base revision) are unchanged and still refuse.

**Candidates (CPU-first):**

| ID | Router | Condition |
|---|---|---|
| R1 | AnyJev raw over Qwen3-4B-Instruct, CPU | Router role only (the 4B stays discarded as a tool-loop model) |
| R4 | Kev-0.8B, CPU-native | **Out of distribution:** trained for the 4-label review question, not this 7-option question |
| R3 | AnyJev raw over Qwen3.5-9B, CPU | Only if its whole evaluation projects ≤12 h (§7) |
| B-ask, B-maj, B-risk | Constant baselines | always `ask_rule`; majority class (`proposal`); majority at-risk (`conflict`) |

R4's result is an out-of-distribution probe. A null R4 result says nothing about Kev on its
trained question, about a Kev head trained for this question, or about trained routers in
general. Cut: L0 (R5), Qwen3-8B (R2), Kev-4B (R6); reconsidered only after a router beats
baselines.

**AnyJev CPU mode** (explicit, tested): worker flag `--device cpu|gpu`. GPU mode keeps the
Vulkan requirement. In CPU mode the worker refuses unless all of these hold, and records each:
- no `/dev/dri`, `/dev/kfd` or `/dev/nvidia*` node exists in the container;
- `n_gpu_layers=0` is passed, with threads 6;
- the llama.cpp load log reports no offloaded layer (a line reporting more than 0 refuses; an
  absent line is recorded as absent).
`llama_supports_gpu_offload()` is recorded as a **build property only**: a Vulkan build returns
true even with no device visible, so it is not the CPU check. No silent fallback either way; an
unknown device is refused. The first probe in the real container checks the recorded
provenance. Default behaviour of the existing benchmark unchanged.

**Metrics** (case denominators, Wilson intervals): top-1 accuracy and 7×7 confusion; the
**cross-kind unsafe-route rate** (route = proposal ∧ expected ≠ proposal, or route = answer
∧ expected at-risk) over all 174 and over the 69 at-risk; supported-route recall over 105;
safe-wrong rate; ECE, Brier, risk–coverage; order flips (reversed order on the S40 ids only);
latency and RSS. Route risk is kept distinct from completed-system false support.
- **Per-stratum counts** for every outcome kind: n and its confusion row, overall and per
  fold. Wilson intervals only where n ≥ 10; refuse (4) and stale (8) are counts only, and no
  per-class claim is made for them.
- **Failures** are counted as coverage loss and reported by status (§3).

**Falsifiers:** each router is reported beside every baseline. A router whose raw argmax
(τ-free) at-risk top-1 over the 69 does not beat B-risk (Wilson lower bound above the baseline
point) is null. **Ablations** on the S40 ids: empty state, and request only (80 decisions per
router, budgeted in §7). No accuracy drop against the full state means priors. Without a
complete ablation unit, a router's result is reported as "priors not excluded" and supports no
positive claim.

## 3. Policy, folds and selection (pre-registered, frozen before inference)

**Decision records.** Every (case, order) gets exactly one record:
- success: `status: completed`, 7 probabilities keyed by the canonical IDs, finite, ≥ 0, sum
  1 ± 1e-6. That tolerance is **one shared constant** (`SUM_TOLERANCE`): the runner sends it
  in every request, the worker validates with it, and the runner validates again with the same
  value (the legacy 4-label benchmark keeps its own 1e-5 default);
- failure: `status` ∈ {`failed`, `timeout`, `invalid_distribution`, `over_context`}, an
  `error_class` field, `probabilities: null`. A worker output that fails distribution
  validation (in the worker, reported as `error_class: invalid_distribution`, or in the
  runner) becomes `invalid_distribution`; it is never repaired into a prediction.
- The router's canonical tie rule (earlier canonical ID) replaces the legacy alphabetical one
  everywhere in the router path; the legacy benchmark's own scoring is unchanged.
- `not_run` (deadline or a stopped worker) is not a decision: a unit holding one is
  **incomplete**.

**Policy** `route(record, τ)`, where τ is a grid value or `NONE`:
1. A failure record → `ask_rule`, reason `fallback_failure`.
2. s = argmax over all 7, ties to the earlier canonical ID. If s ∈ {answer, proposal}, τ ≠
   `NONE` and p(s) ≥ τ → s, reason `supported_threshold`.
3. Otherwise, if the at-risk mass Σ p over {conflict, stale, refuse, ask_rule, ask_fact} > 0 →
   the at-risk argmax (renormalising does not change it), ties to the earlier canonical ID,
   reason `at_risk_argmax`.
4. Otherwise (zero at-risk mass, so s was supported and below τ) → `ask_rule`, reason
   `zero_at_risk_mass`.
- `NONE` means "never route a supported kind". It is a sentinel, not τ = 1.0, because a
  probability can equal 1.0 exactly.
- Scoring: a failure is never a correct route, even when the expected kind is `ask_rule`; it is
  never unsafe; on a supported case it is a recall miss.

**Grid:** τ ∈ {.50, .60, .70, .80, .85, .90, .95, .98, .99}. Raising τ only removes supported
routes, so both supported-correct and unsafe counts are non-increasing in τ: the smallest
feasible τ is the unique best choice.

**Folds (deterministic, seed recorded, grouped).** `fold_seed = 20261016`.
- **Groups (rev. 3.1, Sonnet A7).** Non-text metadata only: cases sharing (character, sorted
  `gold_rule_ids`, expected outcome) form one group. On the dev pack, 42 cases fall in 19
  multi-case groups (16 of 2, 2 of 3, 1 of 4; 9 of them pair an English and an Italian case),
  so they may be translations or near-duplicates. A group is never split across folds. No case
  text was read to form or check the groups; groups that share text but not this key are not
  detected (residual leakage, reported).
- For each kind in canonical order, its groups are sorted by
  `sha256(f"{fold_seed}:{first case id of the group}")` and dealt round-robin, whole, into folds
  0–4; the fold pointer carries over between kinds and is never reset.
- Fold sizes are therefore those the deal produces (recorded in the fold file and the report;
  the ungrouped 35/35/35/35/34 no longer holds). Computed on the dev pack: 39/33/33/35/34; fold
  4 holds no stale case and fold 3 no refuse case (fold file sha256 `8ad758e7…8de6`). Refuse and stale land as the deal gives them
  (each listed in the 5 × 7 table), so some fold may hold none of them.
- The fold file (case id → fold, plus the group key of each case) is hashed and frozen. The
  report shows a 5 × 7 table of held and inner counts per kind. Limitation: rare classes are
  too small for per-fold claims; a held fold without refuse says nothing about refuse.

**Per-router procedure** (run for each router separately; baselines are evaluated on the same
folds without τ):
1. For outer fold i, with inner set T_i = the other four folds: τ_i = the smallest grid τ with
   zero cross-kind unsafe routes over T_i; if none, τ_i = `NONE`.
2. Apply τ_i to fold i. Pool the 174 out-of-fold routes.
3. Report every τ_i, the inner unsafe count at each grid value, and the out-of-fold metrics.

**Gate** (per router; it validates the **selection procedure**, not the final τ\*):
- the router's screen unit is complete (no `not_run`);
- zero cross-kind unsafe routes over the 174 out-of-fold routes;
- out-of-fold supported-route recall ≥ 80% (≥ 84/105);
- coverage: completed decisions ≥ 95% of 174 (≤ 8 failures).
A fold with τ_i = `NONE` routes no supported case, so a router with infeasible folds usually
fails the recall floor; that is the intended outcome. Report the Wilson 95% upper bounds of
zero unsafe (0/174 → 2.16%, 0/69 → 5.27%). This is no statistical certification of ≤2%.

**Refit.** For a router that passes, τ\* = the smallest grid τ with zero unsafe routes over all
174 (the same rule). If τ\* = `NONE`, the router is null. τ\*'s full-dev numbers are
resubstitution and are reported only as such, never as the gate.

**Router choice** (only among passing routers, made after seeing out-of-fold results with at
most three candidates, labelled exploratory): the highest out-of-fold supported recall; then the
lower measured median seconds per decision; then the fixed order R1, R4, R3. If no router
passes, the result is **null**: it is recorded, no live SYS-R decision file is built, and the
S40 replay is reported per router as a non-gating diagnostic.

**Freeze:** the scripts, grid, policy, canonical IDs and option hash, fold seed and fold file,
gate, refit and choice rules, and all hashes are committed before inference.

## 4. Combined system (`SYS-R`, dev-only)

**Fixed-output replay diagnostic** on a C0 ledger (S40 now, D0 later), never a live bound:
- routed at-risk → the routed outcome (correct only if it equals the expected kind; never false
  support, since it is neither proposal nor answer);
- routed supported with the C0 kind equal to the route → the C0 score, including its false
  support;
- every other routed-supported case → **`replay-unresolved`**: counted as incorrect, kept in
  its own column, and **never added to false support**;
- a router failure → routed `ask_rule`, flagged `router_failure` in its own column, and
  **never correct** (even when the expected kind is `ask_rule`; rev. 3.1, Sonnet A6). The
  same holds in live SYS-R ledgers.
The report gives resolved correct, resolved false support (as `score()` computes it), the
replay-unresolved count, and one line labelled as a worst-case bound: "false support ≤
resolved + replay-unresolved". Router terminals carry no message or citations and are reported
as such. It is exploratory and non-gating for any live claim.

**Live SYS-R** (`--router-decisions FILE`; off by default, v3 byte-identical when off).
Routed at-risk: host-written terminal (`terminal.tool="router"`, no model call). Routed
supported: `allowed_finishers` drops the other supported finisher; `respond` and `ask_user`
stay; no hint. A failure record routes to the `ask_rule` host terminal, is flagged
`router_failure` in the ledger and is scored as never correct. First a 3-case live smoke with forced restriction, then dev
174; dev-only.

**Decision-file integrity.** Header: router runtime, image digest, model artefact hashes,
library revision, device and its CPU provenance, threads, `n_ctx`; policy hash (τ\*, grid,
`NONE` rule, fallback rules); question and option hash; retrieval config; state-builder sha;
fold seed and fold-file hash; gate result; pack hashes. One main-order entry per dev case:
case id, state hash, then either a success record with its route and reason, or a failure
record (`probabilities: null`, its status, route `ask_rule`, reason `fallback_failure`). The
harness records the file's sha256 in the ledger header and refuses on: file hash mismatch;
missing or duplicate cases; a `not_run` entry; a success record with an invalid distribution;
a failure record carrying probabilities, or a success record without them; a route or reason
that `route(record, τ*)` does not reproduce; or a state hash mismatch.

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
the GPU is out. Workers take `choices`, `question`, `n_ctx` and `device`, defaults unchanged.

**Timeouts and deadline.** `local_decision_benchmark.py` gains `--ready-timeout` and
`--decision-timeout`; their defaults stay 180 s, so the existing benchmark is unchanged.
- **Probe caps (rev. 3.1, Sol N3).** The probe unit's own caps never depend on probe results:
  ready timeout = artefact bytes / 5 MB/s + 600 s, decision timeout 1,800 s, both bounded by
  the deadline as below. A probe unit that fails or is incomplete makes that router
  **ineligible**: its screen and ablation units are refused, never estimated from the probe
  decisions that did succeed.
- After a complete probe, the router runner sets the ready timeout (load, hash, readiness) to
  the artefact bytes over min(probe-measured disk rate, 40 MB/s) plus 600 s (never assuming a
  faster disk than 40 MB/s, even when the probe read from the page cache), and the decision
  timeout to 3 × the probe's slowest decision, clamped to 180–1,800 s.
- Both are bounded by `deadline_epoch − now − 120 s`. The runner does not start a decision
  whose timeout would cross the deadline; it marks the rest `not_run` and the unit incomplete.
- `deadline_epoch` = the earlier of the window end, the queue's session end and **Monday
  2026-10-19 09:00** local. At the deadline the runner kills the container. An incomplete unit
  is never gate-evaluable.
- **No legacy run budget (rev. 3.1, Sonnet A5).** The router driver does not use the legacy
  benchmark's `--budget-seconds` (default 1,200 s); its only time limits are the timeouts above
  and the deadline. A test runs a unit whose total time exceeds 1,200 s on a fake clock.
- **Timeout sequence (rev. 3.1, Sonnet A1/A2).** A decision timeout still kills the worker.
  - First timeout on a case: the runner restarts the worker (re-hashing the artefacts) and
    retries that case once.
  - A second timeout on that case: the case is recorded as `timeout` and the worker is
    restarted again; the unit continues with the next case.
  - A worker that exits during a decision is handled the same way, with status `failed` and
    `error_class: worker_exit`.
  - Restarts are capped at **N = 3 per unit** (screen and ablation each have their own cap). The
    probe's own cap is 1: any failed probe decision already makes the router ineligible. When a fourth restart would be needed, the rest is `not_run` and the
    unit incomplete. A test shows that one deterministic timeout does not collapse the unit.
  - The final record of a retried case is the retry's result; every attempt, including the
    original, is kept in the unit's attempt log.
  - **Worker logs (Sonnet A4):** one log per worker start, `worker-<n>.log`, never truncated by
    a restart (tested).
- **Restart accounting (Sol N4).** Restarts are counted per unit and never shared between
  units. The §7 projection charges every allowed restart in every unit with its worst case: the
  decision timeout waited out, the kill wait (15 s), and a full reload and re-hash.

**Queue:** arm `kind: toolloop|router`; router estimate = the §7 projection; completeness =
exact case coverage with no `not_run`. CPU inference and model loads are heavy work under the
machine schedule: window only, serialised by the lock after the running CPU queue.

**Watchdog and thermal:** the router container joins the watchdog's fallback names; queue 5 s
sampling and watchdog hard limits apply; CPU temperature recorded as unavailable.

## 6. Failing-first tests (each red before its fix; on/off pairs)

- **State:** `test_sentinels_absent_from_every_prompt` (pair: planted leak caught),
  `test_builder_has_no_expected_import`, `test_retrieval_query_is_request_only`,
  `test_retrieved_rules_format_order_and_count` (0–5, `bm25` then `rule_id`, full text),
  `test_order_shuffled_seeded` (pair: interleave refused), `test_over_context_refused`.
- **Tokens:** the measurement covers all 174 prompts with retrieved rules and the template;
  it loads vocabulary only (a weight load is refused); over-`n_ctx` counts reported.
- **Workers:** the requested question and options reach both workers; Kev asserts against the
  request and refuses a mismatch; the option hash is returned; defaults reproduce the 4-label
  run; every artefact hash mismatch still refuses.
- **Routing:** option IDs stable across orders; cross-kind unsafe on all 49 pairs with
  value-correct fixtures through the real `score()`, paired with value errors (wrong answer,
  over cap, wrong proposal, violation) that are *not* router-unsafe; threshold boundaries
  (p = τ routes, p just below does not); argmax ties go to the earlier canonical ID; `NONE`
  routes no supported kind even at p = 1.0; zero at-risk mass → `ask_rule`; a failure is never
  correct, never unsafe and a recall miss.
- **Records:** success and failure schemas; an invalid distribution becomes
  `invalid_distribution`, never a prediction; failures preserved through CV, completeness and
  replay; a `not_run` record makes the unit incomplete and the gate refuses it.
- **Selection:** the fold allocation is reproducible from the seed, groups are never split
  (rev. 3.1), the recorded sizes match the deal, and the per-stratum table matches; nested CV never sees the held fold (pair:
  planted leakage detected); folds with different τ_i pool correctly; an infeasible fold gets
  `NONE` and the gate fails on recall; the gate needs completeness, zero unsafe, recall and
  coverage; refit uses all 174 and `NONE` makes the router null; router-choice ties follow the
  fixed rules; baselines and falsifier; ECE/Brier known values; oracle replay on a synthetic
  ledger.
- **Replay:** `replay-unresolved` never enters the false-support field; the bound line equals
  resolved + unresolved; router failures get their own column.
- **Timeouts:** with the legacy cap made injectable and set to 1 s, a fake decision taking 2 s
  under a 5 s decision timeout completes and the next decision runs on the same process (the
  legacy path fails it); readiness uses the ready timeout; one deterministic timeout restarts
  the worker, retries the case and the unit completes; a timeout on the retry records the case
  as `timeout` and the unit continues; the fourth needed restart marks the rest `not_run`; the
  attempt log keeps the original; worker logs survive restarts; a unit longer than 1,200 s
  completes; the probe caps do not depend on probe results; the deadline stops new decisions
  and the unit is incomplete; the defaults stay 180 s (rev. 3.1).
- **CPU mode:** CPU → zero layers / GPU → Vulkan still required; a device node present (fake
  `/dev` root) refuses; an offload line above 0 refuses; unknown device refused; CPU provenance
  recorded, with `llama_supports_gpu_offload()` labelled a build property; worker defaults
  unchanged.
- **Harness:** router off byte-identical / on changes only routed cases; router terminal makes
  no model call; a removed finisher is rejected through real dispatch; decision-file hash,
  missing, duplicate, `not_run`, invalid entries, a failure with probabilities, and a
  recomputed-route or reason mismatch refused; a failure record routes to the `ask_rule`
  terminal and is flagged; system arm never gets the bar / model arm still does; forged
  relabel refused.
- **Ops:** decision-model schema and hashes; `tools/models.json` consumers untouched; queue
  router arm (window, lock, §7 estimate, completeness); watchdog fallback removes the router
  container with docker listing failing; real-docker smoke (no model): labels, `--network
  none`, watchdog removal, cleanup verified.

## 7. CPU budget and order

**Rates.** Assumed state ≈3k tokens per decision; raw readout ≈ one prefill. **All rows are
unmeasured estimates** except the LFM reference; dense models prefill more slowly than the MoE.
Before any run they are replaced by: the §2 token counts × the probe-measured prefill rate.

| Item | Assumed rate | Per decision | Whole unit, projected (below) |
|---|---|---|---|
| LFM2.5-8B-A1B (reference, measured) | 59–67 tok/s | ≈45–50 s | — |
| R4 Kev-0.8B torch FP32 | 40–80 tok/s | ≈40–75 s | ≈4.3–8.0 h |
| R1 Qwen3-4B dense | 15–25 tok/s | ≈120–200 s | ≈12.9–21.4 h |
| R3 Qwen3.5-9B dense | 7–12 tok/s | ≈250–430 s | ≈26.8–46.1 h |

**What a router's projection covers (rev. 3.1).** Decisions: 174 main + 40 reversed + 80
ablation = 294 (ablations budgeted at the full-state rate, which is conservative). Per unit u
with n_u decisions (214 screen, 80 ablation), N = 3 allowed restarts, L = load + hash at the
ready-timeout rate, T = the decision timeout and K = 15 s kill wait:

  wall_u = L + N × (T + K + L) + n_u × 1.05 × 1.25 × s,

where s is the probe-measured seconds per decision (its slowest probe decision, not the
median). A router's evaluation is wall_screen + wall_ablation; the queue admits a unit only if
its own wall_u fits before the deadline. The table's last column uses only the n × 1.05 × 1.25
× s term. Probes (3 states at the measured token p50, p90 and max × 3 backbones, incl. load
and HDD hash) are budgeted separately at their own caps (§5).

**Units and evaluability.** Per router, the screen unit (214) and the ablation unit (80) run in
that order. Only a complete screen unit is gate-evaluable; a complete ablation unit is needed
for any positive claim (§2).

**R3's 12 h cap** covers its whole evaluation: both units' wall_u above (load, hash, every
allowed restart with its timeout and reload, retries and the safety factor), but not its probe.
R3 runs only if that projection is ≤ 12 h **and** its projected end, after the actual R1 and R4
runs, is ≥ 1 h before Monday 09:00. Before rev. 3.1 the cap needed ≤ 112 s per decision; the
restart charges (2 units × 3 × (T + K + L)) lower that further, so R3 is not expected to run on
CPU.

1. **Now (no inference):** the short re-review of revision 3, then code and tests; freeze §3;
   the token measurement (§2).
2. **After the CPU screen queue (≈04:00 Saturday 2026-10-17)**, window ≈53 h to Monday 09:00
   (session end 08:30): the probes; then R4 (Kev-0.8B, the cheapest unit, first by the main
   session's instruction of 2026-10-16), then R1, each only if its projection fits the remaining
   window; R3 under its cap. Then, in minutes and without inference: CV, baselines, the S40 replay.
3. **GPU repaired gate:** D0 (9B on 174, ≈2.4 h on GPU), oracle recompute, full replay,
   live SYS-R smoke and dev run; reconsider L0, Qwen3-8B and Kev-4B.
4. **Before any held-out use:** independent results review; pre-register (§1) or record null.

## Questions for the user (recommended defaults)

Answered: the authorisation (v3 plan §12), and CPU inference counting as heavy work (the
machine schedule names model loads and inference as heavy).

1. **Fallback** at-risk argmax; **routed-supported** removes the other finisher, no hint.
   *Default: yes to both.*
2. **Recall floor** ≥80% of supported cases at zero unsafe. *Default: 80%.*
3. **Coverage floor** (new): at most 8 of 174 decisions may fail for the gate. *Default: 95%.*
4. **9B router on CPU** only if its whole evaluation projects ≤12 h (§7). *Default: yes.*
5. **If AnyJev cannot load Qwen3.5-9B:** a reviewed, separately labelled llama-server
   top-logprob readout? *Default: yes, behind the GPU gate.*
6. **Keep the discarded 4B as a router backbone?** *Default: yes, router role only.*
7. **Run Kev-0.8B as an out-of-distribution probe** (§2)? *Default: yes; it is the cheapest
   unit.*

## Revision 3.1 amendments (round 3 re-review, both GO-with-amendments)

| ID | Source | Amendment (where) |
|---|---|---|
| A1 | Sonnet | Timeout sequence: restart + one retry, a second timeout records `timeout` and restarts again; cap N = 3 restarts per unit; one deterministic timeout never collapses a unit (§5, §6) |
| A2 | Sonnet | The final record is the retry's, the original stays in the attempt log; the projection charges worst-case timeout + reload per allowed restart (§5, §7) |
| A3 | Sonnet | One shared tolerance `SUM_TOLERANCE` (1e-6) for worker and runner; canonical tie rule replaces the alphabetical one in the router path; `error_class` maps worker failures to `invalid_distribution` (§3) |
| A4 | Sonnet | Per-start worker logs `worker-<n>.log`, never truncated (§5, §6) |
| A5 | Sonnet | No legacy 1,200 s budget in router mode; deadline only; tested (§5) |
| A6 | Sonnet | `router_failure` is never correct, in replay and live (§4) |
| A7 | Sonnet | Metadata groups (character, gold rule ids, outcome) are never split across folds; residual text-level leakage reported (§3) |
| N3 | Sol | The probe has its own caps, independent of probe results; a failed or incomplete probe makes the router ineligible (§5) |
| N4 | Sol | Restarts counted per unit, both units' restart costs (timeout wait, kill wait, reload) in the projection and R3's cap (§5, §7) |
| — | Sol | Closed: N1 and N2 (selection algorithm, failure records) |
| — | main session | Ready timeout rate = min(measured, 40 MB/s) (clarifies "floor 40 MB/s"); R4 runs before R1 (§5, §7) |
| — | implementer | Probe restart cap 1; without a measured disk rate (artefacts read from the session hash cache) the unit ready timeout uses the probe's 5 MB/s; an ablation unit runs only after its router's screen unit completed; the decision file header must carry the §4 provenance and a passed gate (§4, §5) |
