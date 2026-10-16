# Decision router plan (prototype tier)

2026-10-16. Status: **plan draft, awaiting independent plan review.** Produced by a read-only
planning subagent; nothing built or run. Base: master `46fd889` (harness v3, registry, queue,
M3). This plan reopens H10, which v3 plan §11.24 cut. Held-out use requires a user decision
(Q1) and the review in Q9.

## Current state (verified)

**Scoring** (`tools/tool_loop_benchmark.py`)

- Outcome kinds: 7 (`:61-65`). At-risk set: `UNSUPPORTED_EXPECTED` (`:68`). Bar: held-out,
  ≥90% correct and ≤2% false support (`:69`).
- False support arises only when the outcome is `proposal` or `answer`:
  - `outcome_error` (`:1801-1802`);
  - any proposal when none was expected (`:1744-1748`);
  - value errors: `wrong_answer`, `over_cap`, `wrong_proposal`, `unreported_violation`
    (`:1803-1814`).
- So a route to ask, conflict, stale or refuse can never count as false support. `answer` on
  an expected proposal is incorrect but not false support (`:1745-1747`).

**Inputs and identity** (`tools/tool_loop_benchmark.py`)

- `public_case` exposes only id, character, request and setup (`:830-838`). The context JSON
  has no case id (`:1127-1143`).
- Retrieval is FTS5 BM25, top 5 (`:843-879`).
- The H8 merge gate is `GLOBAL_IDENTITY` / `GROUP_IDENTITY` (`:2736-2763`).

**Dev pack.** 174 cases:

| proposal | answer | conflict | ask_rule | ask_fact | stale | refuse |
|---|---|---|---|---|---|---|
| 67 | 38 | 29 | 14 | 14 | 8 | 4 |

That is 105 supported and 69 at-risk; 34 are Italian.

**v3 dev screen (S40, seed 17)**

| Model | Correct | False-supported | Notes |
|---|---|---|---|
| 9B | 25/40 | 8/40 | injection-passive 2/7; Dara 2/6; 40.5 s per episode |
| 4B | 14/40 | 17/40 | discarded as a tool-loop model |

**Oracle split of the 9B's 8 false-supported cases**

Source: the frozen run `run-79fddee1d17d`, `arms/s40-qwen35-9b/attempt-1/`, `score` fields
only.

- **3 have the wrong outcome kind.** A router can remove these: conflict→answer,
  ask_rule→answer, stale→proposal.
- **5 have the right kind but wrong values.** A router cannot remove these: `wrong_answer` 2,
  `wrong_proposal` 2, `over_cap` 1.
- **Bounds with a perfect router:** false support ≥ 5/40 (12.5%), and correct ≤ 35/40
  (87.5%).
- **Even a perfect router cannot meet the bar with the 9B on current evidence. It can only
  reduce false support.**

**v3 plan** (`docs/tool-loop-harness-v3-plan.md`)

- H9 and H10 were each their own system arm; H10 was cut.
- The router may use only product inputs, and must never read `expected`.
- §11.2: A0 is the sole verdict arm.
- §11.3: dev runs before held-out.

**Decision results** (`docs/local-decision-benchmark-2026-10-15.md`; 18 synthetic cases,
72 observations)

| Configuration | Correct | Median time | Wrong at p ≥ 0.9 |
|---|---|---|---|
| 4B raw | 58 | 0.47 s | 12/64 |
| 4B L0 | 52 | 1.89 s | 8/58 |
| 8B raw | 38 | 0.82 s | 28/62 |
| 8B L0 | 56 | 3.34 s | 8/58 |
| Kev-0.8B CPU | 44 | 6.52 s cold, 2.12 s warm | — |

The AnyJev GGUF backend clears the KV cache for every prompt.

**Tooling**

- `tools/local_decision_benchmark.py` has a fixed set of 4 `CHOICES` and a fixed `QUESTION`.
  Its AnyJev worker uses Vulkan with `n_ctx=4096`, and its Kev worker runs on CPU.
- Local images: `mimik-decision-anyjev:20261015` and `mimik-decision-kev:20261015`.
- Weights live under `/media/data/mimik/models/target/`.
- Kev-4B is not downloaded; a 14.3 GB footprint does not fit in 8 GB.
- Registry: `decision_readout` is already a capability, but entries are single-GGUF
  llama-server entries. Kev's torch snapshot does not fit that schema.
- Queue: it is hard-wired to `run_tool_loop_benchmark.sh`. The watchdog relies on the
  `OWNED_PREFIX` and the ownership labels.

## 0. Go/no-go before any router work (no GPU)

- `decision_router.py oracle LEDGER…` reads only `score` fields and prints counts, never case
  text.
- **Pre-registered rule:** proceed only if the router could remove at least 1/3 of the 9B's
  false support. On S40 it could remove 3/8, so the rule passes.
- Always state the result as "reduction only; the bar also needs value-side fixes".

## 1. Router-only screen (174 dev cases, no tool loop)

**State.** It consists of the C0 context plus `retrieved_rules` (the request's top-5 search).
It never includes expected, gold, the loop's searches, tags, case ID or language.

**Question.** One generic question. The 7 options are the `SYSTEM_PROMPT` outcome lines
copied verbatim and hashed, with fixed IDs.

**Candidates, in this order:**

| ID | Router | Notes |
|---|---|---|
| R3 | AnyJev raw over Qwen3.5-9B | Load probe first (Q6) |
| R1 | AnyJev raw over Qwen3-4B-Instruct | Router role only (Q11) |
| R2 | AnyJev raw over Qwen3-8B | |
| R4 | Kev-0.8B, CPU | |
| R5 | AnyJev L0 | 7 rotations, on the 2 best raw routers |
| R6 | Kev-4B | Deferred (Q5) |

- `n_ctx` is 8192. An over-length state fails as `over_context`; it is never truncated.

**Metrics.** Case denominators with Wilson intervals:
- top-1 accuracy and the 7×7 confusion matrix;
- **unsafe-route rate**: (route = proposal ∧ expected ≠ proposal) or (route = answer ∧
  expected at-risk), over all cases and over at-risk cases;
- safe-wrong rate;
- ECE, Brier score, risk–coverage;
- order flips;
- strata by injection tag, Dara and language;
- latency and RSS.

## 2. Threshold (pre-registered; dev only)

- **Policy:** route to k ∈ {answer, proposal} only if k is the argmax and p(k) ≥ τ. Otherwise
  route to the renormalised argmax among the at-risk kinds. A failure or timeout routes to
  `ask_rule` and is flagged.
- **Grid:** τ ∈ {.50, .60, .70, .80, .85, .90, .95, .98, .99}.
- **τ\*:** the smallest τ with zero dev unsafe routes (Q7). If none qualifies, the router
  fails.
- **Choice:** the router with the most supported cases routed correctly. Ties go to fewer
  unsafe routes, then to lower cost.
- **Folds:** 5 deterministic folds stratified by outcome; informational only.
- **Freeze:** commit the script, grid, policy and hashes before inference.

## 3. Combined system arm (behind a flag)

**Offline overlay first.** Applied to a C0 ledger:
- routed at-risk → the routed outcome;
- routed supported, with the C0 kind equal to the route or at-risk → the C0 score;
- routed supported, with C0 choosing the other supported kind → `unresolved`, reported both
  as incorrect and as false support.

**Go rule for the live arm.**
- Worst-case false support is at most half of C0's, over all cases and over at-risk cases.
- Correct drops by no more than 10 pp.

**Live `SYS-R`** (`--router-decisions FILE`; off by default, so v3 stays byte-identical)
- Routed at-risk: the host writes the terminal (`terminal.tool="router"`, no model call).
- Routed supported: the run uses `allowed_finishers`, removing the other supported finisher.
  `respond` and `ask_user` stay.
- The header records the router identity, τ\* and option hash, with `system_arm: true` in
  `GROUP_IDENTITY`.
- Safeguards:
  - the state hash is checked against the decisions file;
  - `SYS-` labels are required;
  - the bar is labelled as a system bar.
- Held-out only if pre-registered before A0.

## 4. How it runs

**Registry (schema 2).** A `decision_models` map with these fields:
- `runtime` (anyjev-llamacpp | kev-torch), `image`, `library_revision`;
- `backbone` or `files`;
- `device`, `n_ctx`, `levels`, `threads`, `memory_limit`, `seconds_per_decision`, `licence`.

Also add a `qwen3-8b` entry.

**Runner** (`tools/run_decision_router.sh`)
- Uses the same runner lock and window gates as the tool loop.
- `--deadline-epoch`.
- Container `mimik-toolloop-v3-router`, with ownership labels so the watchdog covers it.
- `--network none`, `--pull never --init`, read-only mounts.
- Uses the same **thermal and fan guard** as the tool loop (added 2026-10-16 after the GPU
  overheating shutdown).
- The workers take `choices`/`question`/`n_ctx` parameters; their defaults are unchanged.

**Queue.**
- Arm field `kind: toolloop|router`.
- The same estimate, fit, edge stop, retry, heartbeat, manifest and thermal rules.
- Kev on CPU counts as heavy work, so it runs only in the window and never concurrently with
  a GPU arm.

## 5. Failing-first tests

**Router module:**
- `test_state_never_contains_gold`
- `test_retrieval_query_is_request_only`
- `test_option_ids_stable_across_orders`
- `test_route_unsafe_matches_score`: all 49 pairs against the real `score()`
- `test_threshold_policy`
- `test_failure_routes_ask_rule`
- `test_selection_rule`
- `test_cv_folds_deterministic`
- `test_ece_brier_known_values`
- `test_overlay_bounds_unresolved`
- `test_oracle_split_counts`
- `test_over_context_refused`

**Harness:**
- `test_router_off_byte_identical`
- `test_router_terminal_makes_no_model_call`
- `test_routed_supported_restricts_finishers`
- `test_removed_finisher_is_malformed`
- `test_state_hash_mismatch_refused`
- `test_router_never_merges_with_model_arm`
- `test_sys_label_required`
- `test_bar_labelled_system`

**Registry and queue:**
- `test_decision_entry_schema`
- `test_shell_refuses_decision_entry`
- `test_queue_router_arm_window_lock_watchdog`
- `test_router_runner_args`
- `test_worker_defaults_unchanged`

## 6. Budget and order

1. **Now** (CPU-light): §0 (passes, 3/8); code and tests; freeze §2.
2. **Weekend:** the candidate screens (5 small, 3 8B), then the router queue:

   | Step | Time |
   |---|---|
   | Probes | ≈0.5 h |
   | R3 | ≈0.35 h |
   | R1 | ≈0.2 h |
   | R2 | ≈0.35 h |
   | R4 (Kev, CPU) | ≈1.5–2.5 h |
   | R5 (L0) | ≈0.7–1.2 h each |
   | **Total** | **≈5–7 h** |

   Skip order when short of time: L0, then Kev, then R2.
3. **CPU, minutes:** selection, folds, overlay on S40.
4. **Next window:** D0 (9B on all 174 dev cases, ≈2.4 h); then the full overlay; then, if the
   go rule passes, live `SYS-R`.
5. **Before A0:** independent results review; pre-register or record the null result.

## Questions for the user (recommended defaults)

1. **Reopen H10 as the separate track `SYS-R`,** knowing that even a perfect router leaves the
   9B at ≥12.5% false support on S40? *Default: yes, as a false-support reduction, dev only
   until reviewed, paired with value-side work.*
2. **Low-confidence fallback.** *Default: the argmax among the at-risk kinds.*
3. **Routed-supported mode.** *Default: remove the other supported finisher, keep `respond`
   and `ask_user`, no hint text.*
4. **Does Kev CPU inference count as heavy work?** *Default: yes, window only.*
5. **Kev-4B.** *Default: defer.*
6. **If AnyJev cannot load Qwen3.5-9B**, allow a reviewed llama-server top-logprob readout?
   *Default: yes, labelled separately.*
7. **Threshold target.** *Default: zero unsafe routes on the dev at-risk cases.*
8. **D0 in the next window.** *Default: yes.*
9. **Independent plan review before amending the v3 pre-registration.** *Default: yes.*
10. **Weekend order.** *Default: candidate screens, then G0/35B (held for thermal reasons
    since 2026-10-16), then the router.*
11. **Keep the discarded 4B as a router backbone?** *Default: yes, router role only.*
