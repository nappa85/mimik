# Decision router plan (prototype tier)

2026-10-16. Status: **revision 3.** Revision 2 was re-reviewed (Sol NO-GO on N1, with N2–N4
medium; Sonnet GO-with-amendments N1–N7; third unavailable); revision 3 resolves those findings
in plan text and needs a short re-review of §3, §4 and §7 before any code. The user authorised
the dev-only track (v3 plan §12, `39cb421`). Nothing built or run. Review record:
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
  assumed 3k.

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
  1 ± 1e-6;
- failure: `status` ∈ {`failed`, `timeout`, `invalid_distribution`, `over_context`}, an error
  class, `probabilities: null`. A worker output that fails distribution validation becomes
  `invalid_distribution`; it is never repaired into a prediction.
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

**Folds (deterministic, seed recorded).** `fold_seed = 20261016`. For each kind in canonical
order, its cases are sorted by `sha256(f"{fold_seed}:{case_id}")` and dealt round-robin into
folds 0–4; the fold pointer carries over between kinds and is never reset. Fold sizes are
35, 35, 35, 35, 34. Refuse (4) lands in four folds, one each, so one fold holds no refuse case;
stale (8) splits 2/2/2/1/1 or a rotation of it. The fold file (case id → fold) is hashed and
frozen. The report shows a 5 × 7 table of held and inner counts per kind. Limitation: rare
classes are too small for per-fold claims; with refuse absent from one held fold, that fold
says nothing about refuse.

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
- a router failure → routed `ask_rule`, flagged `router_failure` in its own column.
The report gives resolved correct, resolved false support (as `score()` computes it), the
replay-unresolved count, and one line labelled as a worst-case bound: "false support ≤
resolved + replay-unresolved". Router terminals carry no message or citations and are reported
as such. It is exploratory and non-gating for any live claim.

**Live SYS-R** (`--router-decisions FILE`; off by default, v3 byte-identical when off).
Routed at-risk: host-written terminal (`terminal.tool="router"`, no model call). Routed
supported: `allowed_finishers` drops the other supported finisher; `respond` and `ask_user`
stay; no hint. A failure record routes to the `ask_rule` host terminal and is flagged
`router_failure` in the ledger. First a 3-case live smoke with forced restriction, then dev
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
- The router runner sets the ready timeout (load, hash, readiness) to the artefact bytes over
  the probe-measured disk rate (floor 40 MB/s) plus 600 s, and the decision timeout to 3 × the
  probe's slowest decision, clamped to 180–1,800 s.
- Both are bounded by `deadline_epoch − now − 120 s`. The runner does not start a decision
  whose timeout would cross the deadline; it marks the rest `not_run` and the unit incomplete.
- `deadline_epoch` = the earlier of the window end and **Monday 2026-10-19 09:00** local. At the
  deadline the runner kills the container. An incomplete unit is never gate-evaluable.
- A decision timeout still kills the worker. The runner restarts it **at most once per unit**
  (re-hashing the artefacts) and retries that decision once. A second kill makes the rest
  `not_run`.

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
- **Selection:** the fold allocation is reproducible from the seed, sizes 35/35/35/35/34, refuse
  in four folds, and the per-stratum table matches; nested CV never sees the held fold (pair:
  planted leakage detected); folds with different τ_i pool correctly; an infeasible fold gets
  `NONE` and the gate fails on recall; the gate needs completeness, zero unsafe, recall and
  coverage; refit uses all 174 and `NONE` makes the router null; router-choice ties follow the
  fixed rules; baselines and falsifier; ECE/Brier known values; oracle replay on a synthetic
  ledger.
- **Replay:** `replay-unresolved` never enters the false-support field; the bound line equals
  resolved + unresolved; router failures get their own column.
- **Timeouts:** with the legacy cap made injectable and set to 1 s, a fake decision taking 2 s
  under a 5 s decision timeout completes and the next decision runs on the same process (the
  legacy path fails it); readiness uses the ready timeout; one restart after a timeout, and a
  second timeout marks the rest `not_run`; the deadline stops new decisions and the unit is
  incomplete; the defaults stay 180 s.
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

**What a router's projection covers.** Decisions: 174 main + 40 reversed + 80 ablation = 294
(ablations budgeted at the full-state rate, which is conservative). Projected wall time =
2 × (load + hash) [the second for the one allowed restart] + 294 × 1.05 [the retried
decisions] × 1.25 [safety factor] × the measured seconds per decision. The table's last column
uses that formula without load and hash. Probes (3 states at the measured token p50, p90 and
max × 3 backbones, incl. load and HDD hash): ≈1–1.5 h, budgeted separately.

**Units and evaluability.** Per router, the screen unit (214) and the ablation unit (80) run in
that order. Only a complete screen unit is gate-evaluable; a complete ablation unit is needed
for any positive claim (§2).

**R3's 12 h cap** covers its whole evaluation: load, hash, restart allowance, screen and
ablation units, retries and the safety factor, but not its probe. R3 runs only if that
projection is ≤ 12 h **and** its projected end, after the actual R1 and R4 runs, is ≥ 1 h before
Monday 09:00. With the table's rates the cap needs ≤ 112 s per decision, so R3 is not expected
to run on CPU.

1. **Now (no inference):** the short re-review of revision 3, then code and tests; freeze §3;
   the token measurement (§2).
2. **After the CPU screen queue (≈04:00 Saturday 2026-10-17)**, window ≈53 h to Monday 09:00:
   the probes; then R1, then R4, each only if its projection fits the remaining window; R3 under
   its cap. Then, in minutes and without inference: CV, baselines, the S40 replay.
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
