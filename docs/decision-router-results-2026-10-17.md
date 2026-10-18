# Decision router SYS-R: dev results, R4 and R1 (2026-10-17)

Prototype tier, dev only, exploratory. Plan: `docs/decision-router-plan.md` (rev. 3.1). Status:
**reviewed** (Sol and Sonnet: CONFIRMED WITH CORRECTIONS, applied; third reviewer unavailable;
`docs/reviews/2026-10-17-router-results.md`).

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
| Outer-fold τ_i | 0.50 in all 5 folds (vacuous: no supported top-1 reaches 0.50) | NONE in all 5 folds |
| Min inner unsafe over the grid, per fold | 0, 0, 0, 0, 0 | 21, 21, 25, 25, 20 |
| Out-of-fold correct / unsafe / supported recall | 18/175, 0/175, 0/105 | 40/175, 0/175, 0/105 |
| Gate (recall ≥ 84/105, 0 unsafe, coverage) | fail (recall) | fail (recall) |
| Falsifier: at-risk top-1 vs B-risk 41.4% | 15/70, null | 21/70, null |
| Ablation full / empty / request-only (of 40) | 13 / 15 / 6 | 26 / 9 / 13 |
| Order flips (of 40) | 20 | 18 |
| Ablation full state, reversed option order (of 40) | — | 15 |
| ECE / Brier | 0.128 / 0.817 | 0.372 / 0.804 |
| Median s per decision / peak RSS | 14.1 s / 6.0 GiB | 152.5 s / 5.5 GiB |
| Claim | none | none |

Baselines on the same folds: B-ask 15/175 correct, 0 unsafe; B-risk 29/175, 0 unsafe; B-maj
(always proposal) 67/175, 108/175 unsafe.

## Reading

- **R4**: no case's top-1 reaches p ≥ 0.50 (highest 0.485 over all 175; 0.312 among supported
  top-1s; mean entropy 1.83 of a maximum 1.95; measured by the reviewer's independent
  recomputation from the raw records, not in the reports), so the smallest grid τ is
  "safe" on every inner set only because it routes nothing supported (recall 0). Its 68/175 raw
  top-1 is about the always-proposal baseline (67). Full state did not improve S40 accuracy over the
  empty state (13/40 against 15/40, overlapping intervals): priors are not excluded under the
  registered ablation rule. Its choice depends on
  option order in half the flip cases.
- **R1** is confidently wrong as a raw single-order readout: top-1 ≥ 0.5 on 171/175 cases; at
  τ = 0.99 it still makes 28 unsafe routes over all 175 (27 at-risk cases routed to proposal or
  answer), and every fold has ≥ 20 inner unsafe routes at 0.99, so no grid τ is feasible and the
  gate falls to NONE. Its at-risk top-1 does not beat the constant-risk baseline. Full context
  improved its S40 accuracy over both ablations (26/40 against 9 and 13), and it has the higher
  raw accuracy of these two routers; both hold in **canonical option order only**: with the order
  reversed, its full-state accuracy on the 40 flip cases drops from 26/40 to 15/40 and its
  proposal-against-answer choice follows list position (a primacy effect R4 shares). This
  strengthens the null.
- Neither router passes; per plan §3 the result is **null** for R4 and R1: no live SYS-R
  decision file. The S40 fixed-output replay (plan §3/§4, non-gating, out-of-fold routes, on
  the C0 9B S40 ledger `run-79fddee1d17d`, seed 17, which predates pack rev3): both routers
  route all 40 cases to an at-risk kind; resolved correct 4/40 (R4) and 10/40 (R1), false
  support 0, replay-unresolved 0; C0 alone was 25/40 correct, 8/40 false support. Reports:
  `docs/results/router-dev-2026-10-17/overlay-s40-r{4,1}.json`.
- **Decision this supports (provisional, configuration-specific):** no further work on these
  two raw-readout configurations; the router approach as a whole is not ruled out and waits
  for R3.

## Limitations

- **R4 was an out-of-distribution probe** (a 4-label-trained head asked a 7-option question).
  Per plan §2, a null R4 says nothing about Kev on its trained question, about a Kev head
  trained for this question, or about trained routers in general.
- **The router's "full state" is the C0 initial context, not every fact the tool loop can
  reach**: collection entries carry only `id` and `name` (`tools/tool_loop_benchmark.py`, C0
  context), retrieval is the request-only search, and the router cannot read or search further.
  The proposal option's wording mentions a `propose_change` preview that this screen does not
  provide. How much of the failure these explain is not measured.
- **R1 is a raw, single-order readout**: no permutation averaging, no prior or calibration
  correction (the calibrated levels were cut from the plan). Debiased or permutation-averaged
  readouts are untested; the null is about this readout, not about the 4B model. Its 0.99 is
  a normalised readout score, not a validated probability of correctness (ECE 0.37); the
  per-fold inner counts overlap (28 distinct unsafe routes at τ = 0.99 over all 175).
- The plan text predates the 175-case rev3 pack and says 174. The code derives everything from
  the pack; the recall floor (84/105), coverage (≤ 8 failures) and fold sizes are unchanged.
  Only the Wilson bounds (2.15% over 175, 5.20% over 70) and the B-risk point (29/70 = 41.4%)
  differ slightly from the plan's numbers. No consequence for the verdict.
- Dev only, 175 cases, 70 at-risk; folds grouped by metadata only (residual leakage possible).
- The ablation and flip units cover 40 cases.
- Only two of three planned routers; the R3 result may change the conclusion about raw-readout
  routers (not about R4 and R1).
- The S40 replay uses a C0 ledger from before pack rev3; it is descriptive only.
- 152 s per decision for R1 on CPU would be impractical interactively even if it passed.

## Addendum (2026-10-18): R3 Qwen3.5-9B

Status: **self-analysis, under independent review** (`docs/reviews/2026-10-18-router-r3-results.md`);
it does not drive a decision until that review is recorded.

- Queue `tools/queues/router-cpu-dev-r3.json`, frozen `0b7c694edf8518f91cb96386051d8697e84db57c`,
  CPU, 2026-10-17 22:50 – 2026-10-18 14:54, under the user's time-limited schedule override
  (removed automatically when the queue ended). Probe, screen (215 records, 0 failures) and
  ablation (80, 0 failures) complete.
- Analysis from the run's own frozen worktree (`decision_router.py select --router R3`, then
  `overlay` on the same C0 9B S40 ledger). A first attempt from the b5a4f50 worktree was
  correctly refused by the code-identity check (`router_sha256`, `render_sha256` differ); it
  is not used. Reports: `docs/results/router-dev-2026-10-17/select-r3.json`,
  `overlay-s40-r3.json`.

| | R3 Qwen3.5-9B |
|---|---|
| Raw top-1 correct | 87/175 |
| Raw top-1 unsafe (all / at-risk) | 26/175 / 26/70 |
| Raw supported recall | 48/105 (proposal 19/67, answer 29/38) |
| Outer-fold τ_i | 0.95 in all 5 folds |
| Inner unsafe per fold at 0.90 / 0.95 | 3, 4, 3, 2, 4 / 0 in every fold |
| Out-of-fold correct / unsafe / supported recall | 73/175, 0/175, 18/105 (proposal 2/67, answer 16/38) |
| Gate | fail (recall 18/105 < 84) |
| Falsifier: at-risk top-1 vs B-risk 41.4% | 39/70 (Wilson 44.1–66.8%), **not null** |
| Ablation full / empty / request-only (of 40) | 23 / 9 / 9 |
| Order flips (of 40) | 11 |
| ECE / Brier | 0.237 / 0.703 |
| Median s per decision / peak RSS | 241.9 s / 8.5 GiB |
| S40 replay (out-of-fold routes, C0 9B ledger) | 16/40 correct, 3/40 false support, 0 unresolved, 35/40 routed at-risk (C0 alone 25/40, 8/40) |
| Claim | none (gate failed) |

Reading:
- R3 is the first router with a feasible threshold: τ = 0.95 gives zero unsafe routes on every
  inner set and zero over the 175 out-of-fold routes, and its at-risk top-1 beats the
  constant-risk baseline. It improves with the state over both ablations, and its choice is
  less order-sensitive than R4's or R1's (11/40 flips).
- It fails on recall. Its raw top-1 labels 38 of the 67 proposal cases as `answer`; below τ,
  those cases fall to the at-risk argmax, mostly `conflict` (44 of 67 out of fold). Only 2 of
  67 proposals are routed `proposal`. The proposal-against-answer split, not the
  supported-against-at-risk split, is where it loses most supported cases. Possible
  contributors (unmeasured): the proposal option's `propose_change` preview wording and the C0
  entries without values (see Limitations above).
- Per plan §3 the result is **null** for R3 too: no live SYS-R decision file.
- Exploratory and not pre-registered: whether a router asked only "supported or at-risk"
  (leaving proposal against answer to the tool loop) would pass is a new question. It would
  need its own plan, frozen before any rerun, and these dev data are now seen.
- 242 s per decision on CPU is far from interactive use.
