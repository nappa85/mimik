# Review record: decision router plan

2026-10-16. Scope: [docs/decision-router-plan.md](../decision-router-plan.md). Round 1
reviewed revision 1 (read-only planning draft, base master `46fd889`); revision 2
(`c960f45`) rewrote it in place. Round 2 re-reviewed revision 2; revision 3 rewrites it again
in place. Documentation only: no code, build, container or inference was run for this record.
Section references below are to revision 3.

## Independence

- **Round 1** (revision 1):
  - **OpenAI Sol** (`openai/gpt-6.1-sol`, `tools/external_review.py --chain main`), read-only,
    sanitized snapshot without `target/`: **NO-GO**, R1–R11.
  - **Sonnet**, fresh subagent, read-only: **GO-with-amendments**, B1–B3, M1–M7, L1–L3.
- **Round 2** (revision 2):
  - **Sol**: **NO-GO**. Blocking N1 (selection algorithm), Medium N2–N4; it confirmed the
    round-1 findings resolved in plan text, except for those carried into N1–N4, and asked for
    the stale "awaiting authorisation" status and §7 step 1 to be updated.
  - **Sonnet**: **GO-with-amendments**, N1–N7.
- **Third reviewer unavailable in both rounds:** Space Bunny, DeepSeek v4.1 Flash and Muse
  Spark all failed preflight (`--chain third`). Proceeded with the two main reviews, as
  AGENTS.md allows.

Adjudication by the implementer (a main-session subagent) against `harness-v3` at `b085aa4`
and, for S40, the 9B ledger of `run-79fddee1d17d` (`score` fields and outcomes only; no case
text). Round 2 re-read `local_decision_benchmark.py` (`:18-31`, `:168-181`, `:248-370`,
`:403-417`, `:485-545`) and `tool_loop_benchmark.py` (`:61-69`, `:846-877`, `:1127-1143`) on
`harness-v3`.

Since round 1: the GPU has a cooling hardware fault (junction 29→86 °C in 2 s under compute),
so all runs are CPU-only; CPU device mode exists on `harness-v3` (`6324fad`, `b085aa4`); a CPU
screen queue runs until ≈04:00 Saturday. **The user authorised the dev-only track** (v3 plan
§12, `39cb421`).

## Round 1 findings (revision 1)

| ID | Sev. | Verdict | Evidence | Disposition (as carried into revision 3) |
|---|---|---|---|---|
| Sol R1 / Son B2 | Block | Valid | Fixed-ledger only; `score()` judges kind and values separately (`tool_loop_benchmark.py:1736-1816`); live finisher removal changes trajectories | §0: one exact oracle definition, conditional S40 replay bounds, abstention trade-off, recompute on D0, narrow hypothesis |
| Son B2 (number) | — | Partly wrong | Recomputed: fixed replay gives correct ≤31/40 (16 at-risk + 15 right-kind supported correct; 4 unresolved), not ≈28/40 | §0 states 31/40, and 35/40 only under live rewrite |
| Sol R2 / Son M4 | Block / Med | Valid | The overlay kept C0 scores for routed-supported cases whose C0 kind was at-risk | §4: fixed-output replay, non-gating, `replay-unresolved` (round 2 N4); live 3-case smoke first |
| Sol R3 / Son B1 | Block | Valid | v3 plan §11.24 cut H10; §11.2 A0 sole verdict arm | User authorisation, v3 plan §12; §1 restates its limits |
| Son B3 / Sol R3 (gate) | Block | Valid | Revision 1's §0 rule evaluated 3/8 at the time it was written | §0 marked post-hoc |
| Sol R4 / Son B3 / Son M1 | Block / Med | Valid | The same 174 cases chose τ and router; Wilson 0/69 = 5.27%, 0/174 = 2.16% | §3: per-router nested CV, the gate on pooled out-of-fold routes, refit, Wilson bounds, no certification claim (completed by round 2 Sol N1) |
| Sol R5 / Son M2 | Med | Valid | The unsafe predicate is kind-boundary false support only (`:1744-1748`, `:1801-1802`) | §2 "cross-kind unsafe-route rate"; §6 49-pair test; terminals reported without citations |
| Sol R6 / Son M3 | Block / Med | Valid | `public_case` includes `id` (`:830-838`); context carries `sheet_id`; `interleave_by_outcome` uses expected | §2 isolated builder, sentinel scan, seeded shuffle, stateless decisions, ablations |
| Sol R7 | Block | Valid | `summarize` keys label × arm × split; `bar_verdict` judges any held-out group | §4 `arm_type`, `N/A` for system arms, relabel tests |
| Sol R7 (file) | Block | Valid | A state hash alone binds no router, policy or coverage | §4 decision-file integrity (completed by round 2 Sol N2) |
| Sol R8 / Son M5 | Block / Med | Valid | The AnyJev worker refuses without Vulkan, `n_gpu_layers=-1` (`:174-181`) | §2 CPU mode (completed by round 2 Son N6); §7 CPU budget |
| Sol R8 (thermal) | — | Outdated | The thermal guard exists on `harness-v3` (`run_tool_loop_benchmark.sh:426-444`, `run_queue.py:801-805`) | §5: the same sampling and watchdog limits; CPU temperature unavailable |
| Sol R9 / Son M7 | Block / Med | Valid | Registry per-GGUF `sha256` (`model_registry.py:241-271`); queue estimate assumes GGUF and episodes; watchdog fixed names | §5 `tools/decision_models.json`, router estimate and completeness, watchdog names, real-docker smoke |
| Sol R10 | Med | Valid | S40 numbers and inventory not checkable from Sol's snapshot | Current state split by verification status |
| Sol R11 / Son L2 | Med / Low | Valid | Test names only | §6 behavioural failing-first tests with on/off pairs |
| Sol R11 (scope) / Son L1 | Med / Low | Valid | A 5–7 h GPU queue before feasibility | Cut L0, Qwen3-8B, Kev-4B; CPU-first; strata only n ≥ 10 |
| Son M6 | Med | Valid | A 9B router shares the tool loop's backbone | §2 baselines and falsifier |
| Son L3 | Low | Valid | 8 stale cases; §11.23 governs H9 | Stale routes not claimed; §11.23 cross-referenced |

No round-1 finding was rejected. One numeric claim (Sonnet's ≈28/40) did not reproduce, and
one part of Sol R8 was outdated rather than wrong.

## Round 2 findings (revision 2)

| ID | Sev. | Verdict | Evidence | Disposition (revision 3) |
|---|---|---|---|---|
| Sol status | — | Valid | Revision 2 still said "requires the user's explicit authorisation"; §7 step 1 listed authorisation | Status and §1 cite v3 plan §12; §7 step 1 and the questions updated |
| **Sol N1** | **Block** | Valid | Revision 2 pooled five independently chosen (router, τ) pairs with no refit, no infeasible outcome, no final tie-break and no rare-class allocation | §3 frozen algorithm: `route()` with canonical-ID ties; `NONE` for infeasible folds; deterministic seeded folds with the pointer carried over (refuse in four folds); per-router CV; the gate labelled as validating the procedure; full-dev refit of τ\* with `NONE` → null; router choice rules among passers labelled exploratory; a null outcome. §6 tests for infeasible folds and differing τ_i |
| Sol N2 | Med | Valid | The worker emits failures without probabilities (`local_decision_benchmark.py:349-367`), but revision 2's file contract required seven probabilities for every entry; zero at-risk mass was undefined | §3 success and failure records, `invalid_distribution`, `not_run` = incomplete; policy steps 1 and 4 (`fallback_failure`, `zero_at_risk_mass`); failures never correct or unsafe; §4 file schema and refusals; replay and live failure columns |
| Sol N3 | Med | Valid | 180 s readiness and decision caps (`:491`, `:538`); a timeout kills the worker (`:403-417`), so later decisions become `not_run` | §5 `--ready-timeout` and `--decision-timeout` (defaults 180 s), bounded by the deadline; one restart per unit; §6 fake slow decision with an injectable legacy cap |
| Sol N4 | Med | Valid | Recomputed: 80 ablation decisions at §7's rates are 0.9–1.7 h (R4), 2.7–4.4 h (R1), 5.6–9.6 h (R3) | §7: 294 decisions, two loads and hashes, 5% retries, ×1.25 safety; probes separate; R3's cap covers its whole evaluation (≤ 112 s per decision at the cap) |
| Son N1 | Med | Valid | Kev's `QUESTION` and `CHOICES` are the 4-label review question (`:18-31`); the Kev worker asserts them (`:309-310`) | §2: R4 labelled out of distribution, a null R4 not general; workers take the requested question and options, asserting against the request; artefact hash checks unchanged; question 7 |
| Son N2 | Med | Valid-partial | A τ = 1.0 sentinel is right in intent, but a probability can equal 1.0, so a numeric 1.0 can still route | §3 `NONE` sentinel (never routes a supported kind); per-fold τ_i reported; the gate validates the procedure; CV per router |
| Son N3 | Med | Valid | Revision 2 projected from an assumed 3k tokens and had no hard abort | §2 vocab-only token measurement of all 174 prompts; §5 deadline at the earlier of the window end and Monday 09:00, container killed, incomplete units never gate-evaluable; §7 R3 cap as total wall time incl. the 40 reversed decisions, load and hash |
| Son N4 | Med | Valid | Revision 2 counted unresolved cases as false support, unlike `score()` | §0 and §4 `replay-unresolved`, never in false support; a labelled worst-case bound line; §6 test |
| Son N5 | Low | Valid | Revision 2 named stratified folds without allocation or seed | §3 `fold_seed`, allocation, per-fold 5 × 7 table, rare-class limitation; §2 per-stratum counts |
| Son N6 | Med | Valid | Revision 2 said "CPU with a GPU device visible is refused" without a check. Also, `llama_supports_gpu_offload()` (`:174`) is a build property, true for a Vulkan build even without a device | §2 device-node check, `n_gpu_layers=0`, the load-log offload check, the build property recorded only; verified in the first probe; §6 tests |
| Son N7 | Low | Valid | Revision 2 said "top-5 BM25" only; `RuleIndex.search` returns 0–5 full-text rules (`:846-877`) | §2 format, count and order; included in the token measurement; §6 test |

No round-2 finding was judged wrong. Sonnet N2's numeric sentinel was replaced by `NONE` for
the reason given.

## Round 3 findings (revision 3)

Re-review of revision 3 (`0b5fcc2`), read-only. **Sol** (`openai/gpt-6.1-sol`, `--chain main`):
**GO-with-amendments**; N1/N2 closed, N3 and N4 medium. **Sonnet** (fresh subagent):
**GO-with-amendments**, A1–A7. Third reviewer not retried for this short round. Adjudicated by
the implementer against `harness-v3` at `fd35140` (merge of master). Revision 3.1 applies every
amendment in place; table at the end of the plan.

| ID | Sev. | Verdict | Evidence | Disposition (revision 3.1) |
|---|---|---|---|---|
| Sol N1/N2 | — | Closed | §3 algorithm and failure records | None |
| Sol N3 | Med | Valid | §5 derived the decision timeout from the probe but gave the probe no cap of its own; legacy waits are 180 s (`local_decision_benchmark.py:491`, `:538`) | §5 probe caps (bytes / 5 MB/s + 600 s; 1,800 s), deadline-bounded; failed or incomplete probe → router ineligible |
| Sol N4 | Med | Valid | One restart per unit but two units, and §7 budgeted one restart in total, without the timeout wait | §5 restart accounting per unit; §7 wall_u formula charging T + K + L per allowed restart in every unit; R3 cap includes both |
| Son A1 | Med | Valid | One timeout plus one restart per unit makes one slow case end the unit | §5 sequence: restart + retry, second timeout = `timeout` + restart, N = 3 per unit; §6 test |
| Son A2 | Med | Valid | Retry result vs original not specified | §5 final = retry, attempt log keeps all; §7 charges |
| Son A3 | Med | Valid | Worker validates at 1e-5 (`:96-110`) while §3 says 1e-6; legacy ties are alphabetical (`:110`); the worker's failure carries no class (`:349-367`) | §3 `SUM_TOLERANCE` shared and sent in the request; canonical ties; `error_class` |
| Son A4 | Low | Valid | `Worker` opens its log with mode `w` (`:376`), so a restart truncates it | §5 per-start logs; §6 test |
| Son A5 | Med | Valid | `--budget-seconds` defaults to 1,200 s (`:472`), below every CPU unit | §5 router driver without the legacy budget; test |
| Son A6 | Low | Valid | A failure routed to `ask_rule` could score correct on an `ask_rule` case | §4 never correct (replay and live) |
| Son A7 | Med | Valid-partial | Recomputed from non-text metadata: 19 multi-case groups, 42 cases (9 en/it pairs) | §3 grouped folds; text-level near-duplicates not detectable without reading case text, recorded as residual |

No round-3 finding was rejected. One clarification was added by the implementer: §5's "floor
40 MB/s" is read as never assuming a disk faster than 40 MB/s for the ready timeout.

## Regression checks and verification

- Documentation only. Code line citations re-read on `harness-v3`; dev pack counts recomputed
  from `harness-v3:examples/tool-loop-demo/cases-dev.json` outcome fields (round 1); S40 split
  recomputed from the 9B ledger's `score` fields (round 1); Wilson bounds computed (z = 1.96).
  Round 2: fold sizes and rare-class spread derived from the dev counts and the canonical order;
  §7 projections computed from the stated formula and the unmeasured rates.
- Not run: any test, build, container, tokenizer pass or inference. The CPU budget is an
  unmeasured estimate except the LFM reference episode.

## Blockers and handoff

- **Re-review:** done (round 3, both GO-with-amendments); revision 3.1 applies the amendments.
  Implementation proceeds under the prototype tier; an independent review is still required
  before router results drive any decision.
- **User:** confirm or change the defaults in the plan's questions, including the new coverage
  floor (95%) and running Kev-0.8B as an out-of-distribution probe.
- Router inference starts only after the CPU screen queue (≈04:00 Saturday), via the queue
  lock, and stops by Monday 2026-10-19 09:00.
