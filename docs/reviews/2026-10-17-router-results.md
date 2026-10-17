# Review: decision router dev results, R4 and R1 (2026-10-17)

**Scope.** `docs/decision-router-results-2026-10-17.md` as committed in 683a7f8, the select reports
`docs/results/router-dev-2026-10-17/select-r{4,1}.json`, plan `docs/decision-router-plan.md`
rev. 3.1 §2–§4, and the `select` path of `tools/decision_router.py`. The run came from frozen
b5a4f50; between b5a4f50 and the snapshot, `decision_router.py` differs only in the
decision-file `router_sha256` identity, which `select` does not use. Prompt: the session
scratchpad `results-prompt.md`.

**Reviewers.**
- Sol (`openai/gpt-6.1-sol`, `--chain main`): static review. **CONFIRMED WITH CORRECTIONS.**
- Fresh Sonnet subagent: review plus an independent stdlib recomputation from the raw
  `decisions.jsonl`, mapping, subset and dev pack, without importing `select`. It re-derived
  the folds from the plan text. Every reported number matched. **CONFIRMED WITH CORRECTIONS.**
- Third reviewer (`--chain third`): unavailable. Every model failed preflight with server
  errors. The review proceeded with the two main reviewers, as `AGENTS.md` allows.

**Verdict.** The null result for R4 and R1 stands. No Blocking finding. No defect in `select`,
the probability mapping or the state builder was found that could flip a gate outcome. Sol
adds that both raw supported recalls (53/105, 79/105) are already below the 84/105 floor, and
thresholding can only lower them.

## Findings and disposition

| # | Source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | Sonnet M1, Sol 1 | Major | Note omits the plan's R4 out-of-distribution caveat; the deprioritisation is broader than the evidence | Fixed: Limitations quote §2; decision framed as provisional and configuration-specific, approach waits for R3 |
| 2 | Sol 1 | Major | "Full state" is the C0 initial context: collection entries carry only `id`/`name`; request-only retrieval; no follow-up reads; the proposal option mentions a preview the screen lacks | Verified against `tool_loop_benchmark.py` (C0 context entries) and `router_state_builder.render_state`; disclosed in Limitations, effect not measured |
| 3 | Sonnet M2 | Major | R1's state use and accuracy hold in canonical order only (reversed full-state 15/40 vs 26/40; position-following proposal/answer) | Fixed: Reading and a table row state it; strengthens the null |
| 4 | Sol 4 | Minor | Note said "no S40 replay"; plan §3 requires the replay per router as a non-gating diagnostic on a null | Fixed: `overlay` run from the frozen worktree on the C0 9B S40 ledger (pre-rev3); R4 4/40, R1 10/40 correct, 0 false support, 0 unresolved; reports committed |
| 5 | Sonnet m3 | Minor | R1 null is about the raw single-order readout, not the 4B model | Fixed in Limitations |
| 6 | Sonnet m4, Sol 3 | Minor | "Does not use the state" overstated for R4 (13 vs 15 of 40) | Fixed: registered wording (priors not excluded) |
| 7 | Sonnet m5, Sol 2 | Minor | R4 τ = 0.50 row is vacuous; "low-confidence everywhere" not shown by the reports | Fixed: "vacuous" in the table; the 0.485 maximum is cited as the reviewer's recomputation |
| 8 | Sol 5 | Minor | "Best raw accuracy so far on CPU" compares different tasks | Fixed: "higher raw accuracy of these two routers" |
| 9 | Sol 6 | Note | 0.99 is a readout score, not a calibrated probability; the inner counts overlap (28 distinct) | Added to Limitations |
| 10 | Sonnet N6, Sol | Note | 174 vs 175: no consequence (floor 84/105, ≤ 8 failures, folds unchanged; Wilson 2.15%/5.20%, B-risk 41.4%) | Recorded in Limitations |
| 11 | Sol | Note | `per_fold_strata` holds held counts only; the plan asks for held and inner | Open, cosmetic: inner counts derive from the totals; no effect on selection |
| 12 | Sonnet N7 | Note | 141 dev ids are prefixed `ho-` | Open, naming only; no held-out path touched |

**Verification.** The note's numbers were checked against the reports by both reviewers and
recomputed independently by Sonnet. The S40 replay came from the frozen b5a4f50 worktree
(`decision_router.py overlay`). No inference was run for this review.
