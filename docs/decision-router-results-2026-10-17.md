# Decision router SYS-R: dev results, R4 and R1 (2026-10-17)

Prototype tier, dev only, exploratory. Plan: `docs/decision-router-plan.md` (rev. 3.1). Status:
**self-analysis, under independent review** (see `docs/reviews/2026-10-17-router-results.md`);
it does not drive a decision until that review is recorded.

## Scope

- Queue `tools/queues/router-cpu-dev.json`, frozen commit `b5a4f504b598faf3a861f9d40b74422c1c2115aa`,
  CPU only, 2026-10-17 10:14–21:39 (session `tool-loop-queue-router-cpu-dev-20261017T101449`).
- Dev pack rev3 (`examples/tool-loop-demo`): 175 cases, 105 supported (proposal 67, answer 38),
  70 at-risk. Ablation and order-flip units: the 40-case `dev-screen-40` subset.
- Routers: R4 = Kev 0.8B (`r4-kev-0.8b`), R1 = Qwen3-4B AnyJev raw (`r1-qwen3-4b`). R3 =
  Qwen3.5-9B was skipped by its 12 h projection cap (projection 39.1 h); it runs separately
  (`router-cpu-dev-r3.json`, frozen 0b7c694) and is not covered here.
- Analysis: `python3 tools/decision_router.py select --screen-attempt … --ablation-attempt …
  --router R4|R1 --out …`, run from the same frozen worktree. Full reports:
  `docs/results/router-dev-2026-10-17/select-r4.json`, `select-r1.json` (dev case ids, hashes
  and counts only). Both screens complete (215 decision records each: 175 screen + 40 order
  flips; 0 failures), fold file sha256 `97abdc5a…`, option hash `58eae72e…`.

## Results

| | R4 Kev 0.8B | R1 Qwen3-4B |
|---|---|---|
| Raw top-1 correct | 68/175 | 100/175 |
| Raw top-1 unsafe (all / at-risk) | 56/175 / 41/70 | 46/175 / 43/70 |
| Raw supported recall | 53/105 | 79/105 |
| Outer-fold τ_i | 0.50 in all 5 folds | NONE in all 5 folds |
| Min inner unsafe over the grid, per fold | 0, 0, 0, 0, 0 | 21, 21, 25, 25, 20 |
| Out-of-fold correct / unsafe / supported recall | 18/175, 0/175, 0/105 | 40/175, 0/175, 0/105 |
| Gate (recall ≥ 84/105, 0 unsafe, coverage) | fail (recall) | fail (recall) |
| Falsifier: at-risk top-1 vs B-risk 41.4% | 15/70, null | 21/70, null |
| Ablation full / empty / request-only (of 40) | 13 / 15 / 6 | 26 / 9 / 13 |
| Order flips (of 40) | 20 | 18 |
| ECE / Brier | 0.128 / 0.817 | 0.372 / 0.804 |
| Median s per decision / peak RSS | 14.1 s / 6.0 GiB | 152.5 s / 5.5 GiB |
| Claim | none | none |

Baselines on the same folds: B-ask 15/175 correct, 0 unsafe; B-risk 29/175, 0 unsafe; B-maj
(always proposal) 67/175, 108/175 unsafe.

## Reading

- **R4** is low-confidence everywhere: its supported top-1 never reaches p ≥ 0.50, so the
  smallest grid τ is "safe" on every inner set but routes nothing supported (recall 0). It does
  not use the state (empty state ≥ full state: priors) and its choice depends on option order
  in half the flip cases.
- **R1** uses the state (full 26/40 against empty 9/40) and has the best raw accuracy so far
  on CPU, but it is confidently wrong: every fold has ≥ 20 unsafe supported routes even at
  τ = 0.99, so no grid τ is feasible and the gate falls to NONE. Its at-risk top-1 does not
  beat the constant-risk baseline.
- Neither router passes; per plan §3 the result is **null** for R4 and R1: no live SYS-R
  decision file, no S40 replay from these routers.

## Limitations

- Dev only, 175 cases, 70 at-risk; folds grouped by metadata only (residual leakage possible).
- The ablation and flip units cover 40 cases.
- Only two of three planned routers; the R3 result may change the conclusion about the
  approach (not about R4 and R1).
- 152 s per decision for R1 on CPU would be impractical interactively even if it passed.
