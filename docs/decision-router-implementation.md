# Decision router SYS-R: implementation notes (prototype tier, dev-only)

2026-10-16. Spec: [decision-router-plan.md](decision-router-plan.md) **revision 3.1**. Branch
`harness-v3`. Prototype tier (AGENTS.md): self-check plus this note; an independent review is
still required before any router result drives a decision. **No router inference has run.**
The GPU is out of service, so everything here is CPU-only.

## What exists

| Step (plan order) | Files | Notes |
|---|---|---|
| §0 oracle tool | `tools/decision_router.py oracle` | Gold-kind fixed-output replay of a C0 ledger, per seed; non-gating |
| Isolated state builder, sentinel scan | `tools/router_state_builder.py`, `decision_router.py prepare` | Own process; derived file holds only key, character, request, setup; the pack copy holds no case file; C0 context (BASE_FLAGS) plus `retrieved_rules` |
| State-token measurement | `tools/router_tokens.py` | Vocab-only, no weights (loaders patched to raise), real templates, all variants and orders |
| Decision registry | `tools/decision_models.json`, `tools/decision_models.py` | Per-artefact path, size, sha256; pinned local image ids; separate from `tools/models.json` |
| Workers | `tools/local_decision_benchmark.py` | Question and choices from the request (defaults: the 4-label run); Kev checks the record against the request and returns the option hash; explicit AnyJev CPU mode; `--ready-timeout`/`--decision-timeout` (180 s defaults); shared tolerance in the request; `error_class`; appended worker logs; `count_only` |
| Router runner and driver | `tools/run_decision_router.sh`, `tools/router_driver.py` | Runner lock, window gates, hashes, states, hardened container `mimik-toolloop-v3-router`; driver: restart + retry, 3 restarts per unit (probe: 1), deadline only |
| Queue arm kind | `tools/run_queue.py` (`kind: router`), `tools/queues/router-cpu-dev.json` | Probe gate, §7 projection as the estimate, R3 cap and end margin, unit completeness, thermal records; watchdog fallback name added in `tools/toolloop_ops.py` |
| Nested CV selection | `decision_router.py select`, `choose` | §3 exactly: grouped folds (rev. 3.1 A7), smallest feasible τ per inner set, `NONE`, gate, refit, choice, baselines, falsifier, ECE/Brier, per-stratum counts, order flips, ablations |
| Offline overlay | `decision_router.py overlay` | Fixed-output replay with the refit routes (out-of-fold routes when no router passed); non-gating |
| Decision file | `decision_router.py decision-file`, `verify_decision_file` | Built only for a passing router; header carries the registry and worker provenance (§4); the harness verifies it again |
| Live SYS-R | `tools/tool_loop_benchmark.py --router-decisions FILE --router-decisions-sha256 HEX` | Off by default; C0 byte-identical when off (pinned digest test) |

## Measured: prompt tokens per decision (vocab-only, no inference)

All 174 dev states, rendered as each backend renders them (AnyJev raw prompt through the
GGUF's own chat template; Kev's native record encoding). Full state in canonical order; the
reversed order gives the same distribution.

| Router | Tokenizer | min | median | p90 | max | over n_ctx 8192 | request-only max | empty |
|---|---|---|---|---|---|---|---|---|
| R4 Kev-0.8B | Qwen3.5-0.8B-Base tokenizer (`load_tokenizer`) | 1,430 | 1,895.5 | 1,970 | 2,064 | 0 | 279 | 248 |
| R1 Qwen3-4B-Instruct-2507 Q4_K_M | GGUF vocab (151,936) | 1,485 | 1,950 | 2,026 | 2,118 | 0 | 333 | 302 |
| R3 Qwen3.5-9B Q4_K_M | GGUF vocab (248,320) | 1,496 | 1,961.5 | 2,036 | 2,130 | 0 | 345 | 316 |

The assumed 3k tokens per decision (§7) was high: the measured maximum is ≈2.1k. The
**sentinel scan** rendered 1,044 prompts per backend (174 sentinel-pack states × full,
request-only and empty × both orders) through the real templates: **0 sentinel hits** for all
three. Commands and raw reports: run in `mimik-decision-anyjev:20261015` and
`mimik-decision-kev:20261015`, `--cpus 2`, `--network none`, read-only mounts; reports kept in
the session scratchpad only (per-key counts, no case text).

**Projection with the measured tokens** (rates still the §7 assumptions, not measured; the
probe replaces them). Worst-case admission figures from `unit_projection` (3 restarts each
charged with T + 15 s + a full reload at 5 MB/s, retries ×1.05, safety ×1.25):

| Router | Assumed prefill | s/decision | Screen (214) | Ablation (80) | Total | Probe cap |
|---|---|---|---|---|---|---|
| R4 | 80 → 40 tok/s | 25 → 49 | 3.2 → 5.1 h | 2.0 → 2.7 h | 5.1 → 7.8 h | 2.5 h |
| R1 | 25 → 15 tok/s | 81 → 135 | 7.8 → 12.1 h | 3.8 → 5.5 h | 11.6 → 17.6 h | 2.6 h |
| R3 | 12 → 7 tok/s | 170 → 291 | 15.6 → 25.4 h | 7.3 → 11.2 h | 22.9 → 36.5 h | 3.0 h |

R3 stays over its 12 h cap at these rates, so the queue is expected to skip it.

## Other measured or recomputed facts

- **§0 oracle** on the S40 9B ledger (`run-79fddee1d17d`, seed 17, the ledger §0 cites):
  resolved correct 31/40, resolved false support 5/40, replay-unresolved 4, worst-case bound
  9/40. This reproduces §0's bounds.
- **Folds** (grouped, seed 20261016; fold file sha256 `8ad758e7…8de6`): sizes 39/33/33/35/34.
  Held per kind (folds 0–4): proposal 17/13/12/13/12, answer 7/8/7/8/8, conflict 5/5/7/6/6,
  stale 3/1/2/2/0, refuse 1/1/1/0/1, ask_rule 3/3/1/3/4, ask_fact 3/2/3/3/3. Fold 4 holds no
  stale case and fold 3 no refuse case.
- **Option hash** `58eae72e…d8aa` (question plus the 7 SYSTEM_PROMPT outcome lines; the
  ask_user line split into its two kinds).
- **C0 byte identity:** the normalised dry-run ledger of the demo dev split (seeds 17 and 29,
  expected replay; harness sha and engine path removed) has sha256 `9ea39fda…d676`, from the
  unchanged harness (`b085aa4`), and the same with the router code present and off.

## Red → green (failing-first)

| Test file | Red before the change | Green |
|---|---|---|
| `test_router_workers.py` | 21 of 25 failed against the unchanged worker (no question/choices from the request, Vulkan-only, hard-coded 180 s, truncating log, no error class); 4 default-guard tests passed | 25/25 |
| `test_router_driver.py`, `test_decision_router.py`, `test_router_tokens.py`, `test_decision_models.py`, `test_run_decision_router.py` | New modules (import error); pair tests carry their own negative controls (planted leak, interleave order, planted fold leakage, forged entries) | all pass |
| `test_run_tool_loop_runner.py` (existing) | 5 window tests failed once `run_queue.py` imported the router modules at load (the copied test repo lacks them) | lazy imports: green |
| `test_run_decision_router.py` mount check | The first runner mounted the whole `prep/`, host-only mapping included | only `prep/run/prepared` and the summary extra are mounted |
| `test_run_queue_router.py` | 12 of 12 failed (unknown arm keys; router name not in the watchdog fallback) | 12/12 |
| `test_sys_router.py` | 9 of 10 failed (no `--router-decisions`, no system arm); the byte-identity guard passed before and after | 10/10 |

## Launch (main session, after the CPU candidate screens finish)

The launcher refuses while any `run_queue.py` or `run_watchdog.py` process runs, so it cannot
start before the CPU screen queue has ended. From any checkout of the repository, with
`MIMIK_MODEL_ROOT` unset and the default Docker context:

```sh
tools/launch_frozen_queue.sh --print --session-end "2026-10-19 08:30" <commit> tools/queues/router-cpu-dev.json
tools/launch_frozen_queue.sh --session-end "2026-10-19 08:30" <commit> tools/queues/router-cpu-dev.json
```

`<commit>` is the final implementation commit on `harness-v3`. The queue runs, in order:
probes R4, R1, R3; then R4 screen and ablation; R1 screen and ablation; R3 screen and ablation
only under the 12 h cap and the 1 h end margin (expected to be skipped). Router arms are
CPU-only by validation (no `/dev/dri`); the router container uses `--cpus 6` and 6 threads
(registry `threads`, plan §5), the same cap as the CPU screens' server. Every unit is bounded
by the session end 08:30 and the runner's own Monday 09:00 hard deadline.

## After the queue: selection, overlay, decision file (minutes, no inference)

Host Python, stdlib only. Per router, with `S` the queue session directory and `A` an attempt
directory (`$S/arms/<arm>/attempt-<n>`):

```sh
python3 tools/decision_router.py select --router R4 \
  --mapping $A_screen/prep/run/host/mapping.json \
  --screen $A_screen/unit --ablation $A_ablation/unit --out $S/report-R4.json
python3 tools/decision_router.py choose $S/report-R*.json        # passing routers only
python3 tools/decision_router.py overlay --report $S/report-R4.json --ledger <C0 S40 ledger>
python3 tools/decision_router.py decision-file --router-key r4-kev-0.8b \
  --report $S/report-R4.json --mapping $A_screen/prep/run/host/mapping.json \
  --screen $A_screen/unit --states $A_screen/prep/run/prepared/states.jsonl --out $S/sys-r-R4.json
```

`select` refuses to judge an incomplete screen unit; `decision-file` refuses a router that did
not pass the gate or whose τ\* is `NONE`. The overlay uses the refit routes, or the out-of-fold
routes when no router passed, and is labelled non-gating. Live SYS-R
(`--router-decisions FILE --router-decisions-sha256 HEX`, a `SYS-` label, dev only) waits behind
the GPU-repaired gate.

## Verification (helper/unit only)

- Gate in `mimik-toolloop-py:20261015` (`--cpus 2`, `--network none`, uid 1000, worktree and
  git dir mounted, `MIMIK_FORMULA_EVAL` mounted): `ruff check` and `ruff format --check`
  clean; `python -m unittest discover -s tools -p "test_*.py"`: **603 tests, 11 skipped,
  1 error**. The error is `test_package` (`cargo package --list` without network or a cargo
  registry in the container); the unchanged branch (`46e93b2`) gives the same single error in
  the same container (479 tests, 11 skipped), so it is environmental and not caused here.
- 124 new tests (workers 25, driver 18, router core 39, tokens 4, registry 7, runner 9, queue
  router arms 12, live SYS-R 10).
- ShellCheck (koalaman/shellcheck:stable): `run_decision_router.sh`,
  `run_tool_loop_benchmark.sh`, `launch_frozen_queue.sh` clean.
- `run_queue.py check tools/queues/router-cpu-dev.json` and `decision_models.py check --files`
  pass on the host (3 routers, 0 file problems).

## Not done or not run

- No router inference, probe or live SYS-R run (the CPU screen queue holds the machine).
- No real-docker smoke for the router runner (it would share owned names with the running
  queue's cleanup and watchdog); the fake-docker runner tests cover the plumbing only.
- The live 3-case SYS-R smoke and the D0 recompute wait behind the GPU-repaired gate.
- Text-level near-duplicates across folds are not detectable without reading case text.
- Changing `tool_loop_benchmark.py` changes `harness_sha256`: new C0 ledgers do not merge
  with ledgers from `b085aa4` (the running CPU screens), by the existing identity rule.
- **Master moved after the merge** (`fd35140` merged master at `8174be6`): master now has pack
  revision 3 (`34bbd46`, changes to `cases-dev.json`, `rules.md`, `definition.json`, the S40
  subset and `tool_loop_benchmark.py`) and the held-out freeze record (`f9d34e0`). A trial
  `git merge-tree` shows no conflict. Everything measured here (tokens, folds, oracle, the C0
  digest) is on the **pre-rev3 pack**, the pack the running CPU screens use. Merging master
  before the router launch would change the dev states; then recompute the C0 digest (with
  the pre-merge harness on the new pack), the folds and the token counts (minutes, CPU-light).
