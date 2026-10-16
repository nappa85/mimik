# Review record: decision router plan

2026-10-16. Scope: [docs/decision-router-plan.md](../decision-router-plan.md) revision 1
(read-only planning draft, base master `46fd889`). Revision 2 rewrites it in place.

## Independence

- **OpenAI Sol** (`openai/gpt-6.1-sol`, `tools/external_review.py --chain main`), read-only,
  sanitized snapshot without `target/`: **NO-GO**, R1–R11.
- **Sonnet**, fresh subagent, read-only: **GO-with-amendments**, B1–B3, M1–M7, L1–L3.
- **Third reviewer unavailable:** Space Bunny, DeepSeek v4.1 Flash and Muse Spark all failed
  preflight (`--chain third`). Proceeded with the two main reviews, as AGENTS.md allows.

Adjudication by the implementer (main-session subagent) against `harness-v3` at `b085aa4`
and, for S40, the 9B ledger of `run-79fddee1d17d` (`score` fields and outcomes only; no case
text). No code, build, container or inference was run. Adjudicated result:
**revision 2 resolves every Blocking and Medium finding in plan text; the track stays
blocked** until the user authorises reopening H10 (plan §1) and a short re-review passes.

New facts since revision 1: the GPU has a cooling hardware fault (junction 29→86 °C in 2 s
under compute), so all runs are CPU-only; CPU device mode exists on `harness-v3`
(`6324fad`, `b085aa4`); CPU measurements for LFM2.5-8B-A1B; a CPU screen queue runs until
≈04:00 Saturday. The item-catalogue deferral (user, 2026-10-16) does not affect this plan.

## Findings and dispositions

| ID | Sev. | Verdict | Evidence | Disposition (rev. 2) |
|---|---|---|---|---|
| Sol R1 / Son B2 | Block | Valid | Fixed-ledger only; `score()` judges kind and values separately (`tool_loop_benchmark.py:1736-1816`); live finisher removal changes trajectories | §0: one exact oracle definition, "conditional S40 replay bounds", abstention trade-off, recompute on D0, narrow hypothesis |
| Son B2 (number) | — | Partly wrong | Recomputed: fixed replay gives correct ≤31/40 (16 at-risk + 15 right-kind supported correct; 4 unresolved), not ≈28/40; rev. 1's 35/40 assumed unresolved cases fixed | §0 states 31/40, and 35/40 only under live rewrite |
| Sol R2 / Son M4 | Block / Med | Valid | Overlay kept C0 scores for routed-supported cases whose C0 kind was at-risk; counterfactual not bounded | §4: fixed-output replay diagnostic, non-gating, all other routed-supported cases unresolved; live 3-case smoke first |
| Sol R3 / Son B1 | Block | Valid | v3 plan §11.24 cuts H10; §11.2 A0 sole verdict arm; §11.3 dev first | §1: proposed §11.24a (dev-only SYS-R; held-out only if pre-registered before A0's first held-out episode); no edits until authorised |
| Son B3 / Sol R3 (gate) | Block | Valid | Rev. 1 §0 rule evaluated 3/8 at the time it was written | §0 marked post-hoc |
| Sol R4 / Son B3 / Son M1 | Block / Med | Valid | Same 174 cases chose τ and router; Wilson 0/69 = 5.27%, 0/174 = 2.16% (computed); rev. 1 Q7 named only at-risk cases | §3: nested 5-fold CV, gate on pooled out-of-fold routes; zero cross-kind unsafe over all 174 plus recall ≥84/105; Wilson bounds reported; no certification claim |
| Sol R5 / Son M2 | Med | Valid | Unsafe predicate = kind-boundary false support only (`:1744-1748`, `:1801-1802`); router terminals have no message or citations | §2 renamed "cross-kind unsafe-route rate"; route vs system false support kept apart; §6 49-pair test with value-correct fixtures plus value-error pair; router terminals reported as lacking citations (§4) |
| Sol R6 / Son M3 | Block / Med | Valid | `public_case` includes `id` (`:830-838`); context carries `sheet_id` (`:1133`); `interleave_by_outcome` uses expected (`:2719-2732`) | §2: isolated builder over a derived file without id/expected/tags/language; sentinel scan of every backend prompt; seeded shuffle; stateless decisions; ablations |
| Sol R7 | Block | Valid | `summarize` keys label × arm × split (`:2038-2049`); `bar_verdict` judges any held-out group (`:2177-2217`); `GROUP_IDENTITY` within-group only (`:2752`) | §4: `arm_type` in summary key and identity; system arms get `N/A`; only model A0 for verdict; relabel tests |
| Sol R7 (file) | Block | Valid | State hash alone binds no router, policy, coverage | §4 decision-file integrity: header hashes, file sha256 in ledger, coverage, duplicates, distribution validity, recomputed route |
| Sol R8 / Son M5 | Block / Med | Valid | AnyJev worker refuses without Vulkan, `n_gpu_layers=-1` (`local_decision_benchmark.py:174-181`); GPU now out of service | §2 explicit AnyJev CPU mode with paired tests and provenance; §7 CPU budget (unmeasured), GPU items behind the gate; G0/D0 deferred |
| Sol R8 (thermal) | — | Outdated | The thermal and fan guard exists on `harness-v3` (`run_tool_loop_benchmark.sh:426-444`, `run_queue.py:801-805`); Sol's snapshot predated it. CPU temperature is unreadable (no `k10temp`) | §5: router container under the same queue sampling and watchdog limits; CPU temperature recorded as unavailable |
| Sol R9 / Son M7 | Block / Med | Valid | Registry checks per-GGUF `sha256` and pinned image (`model_registry.py:241-271`); queue estimate assumes GGUF and episodes (`run_queue.py:690-725`); watchdog fallback knows fixed names only (`toolloop_ops.py:240-250`) | §5: separate `tools/decision_models.json` with per-artefact path, size, sha256 and pinned image; router estimate and completeness; router container in fallback names; real-docker smoke |
| Sol R10 | Med | Valid | S40 numbers and inventory were not independently checkable from Sol's snapshot | Current state split into source-verified, recomputed (dev counts 67/38/29/14/14/8/4, 34 Italian; S40 25/40, 8/40, 3 cross-kind, 5 value) and reported-not-reverified |
| Sol R11 / Son L2 | Med / Low | Valid | Names only; module-missing failures prove nothing | §6: behavioural failing-first tests with on/off pairs, planted-leak and leakage detectors, real dispatch, synthetic ledger |
| Sol R11 (scope) / Son L1 | Med / Low | Valid | 5–7 h GPU queue with L0, 8B, Kev-4B before feasibility | Cut L0, Qwen3-8B, Kev-4B; CPU-first R1, R4, R3 only after a probe; strata only n ≥ 10 |
| Son M6 | Med | Valid | 9B router shares the tool loop's backbone (correlated errors) | §2 baselines B-ask, B-maj, B-risk and falsifier; 4B and Kev offer non-identical backbones |
| Son L3 | Low | Valid | 8 stale cases; §11.23 governs H9 | Stale routes not claimed; §11.23 cross-referenced in §11.24a |

No finding was rejected. One numeric claim (Sonnet's ≈28/40) did not reproduce, and one
part of Sol R8 (no thermal guard) is outdated rather than wrong.

## Regression checks and verification

- Documentation only. Code line citations re-read on `harness-v3`; dev pack counts recomputed
  from `harness-v3:examples/tool-loop-demo/cases-dev.json` outcome fields; S40 split
  recomputed from the 9B ledger's `score` fields; Wilson bounds computed (z = 1.96).
- Not run: any test, build, container or inference. The CPU budget in plan §7 is an
  unmeasured estimate except the LFM reference episode.

## Blockers and handoff

- **User:** authorise §11.24a (H10 as dev-only SYS-R) or keep H10 cut; confirm the recall
  floor (80%), CPU inference as heavy work, and the 9B CPU probe limit (≤12 h).
- **Re-review:** a short independent review of revision 2 before any code.
- Router inference starts only after the CPU screen queue (≈04:00 Saturday), via the queue lock.
