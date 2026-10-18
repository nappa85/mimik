# Review: decision router R3 (Qwen3.5-9B) dev result (2026-10-18)

**Scope.**
- The "Addendum (2026-10-18): R3 Qwen3.5-9B" of `docs/decision-router-results-2026-10-17.md`,
  as committed in 31bc7aa.
- The reports `docs/results/router-dev-2026-10-17/select-r3.json` and `overlay-s40-r3.json`.
- Plan rev. 3.1 §2–§4, and the `select`/`overlay` paths of `tools/decision_router.py`. The run
  came from frozen 0b7c694.

Prompt: the session scratchpad `r3-prompt.md`.

**Reviewers.**
- **Sol** (`--chain main`): static review. CONFIRMED WITH CORRECTIONS.
- **Fresh Sonnet subagent:** review plus an independent stdlib recomputation from the raw
  records, without `select`. Every reported number matched, including folds, per-fold inner
  unsafe, out-of-fold confusion, falsifier, ablation, flips, ECE/Brier and the S40 replay.
  CONFIRMED WITH CORRECTIONS.
- **Space Bunny** (`--chain third`): static review. CONFIRMED WITH CORRECTIONS, with one
  Blocking finding that concerns disclosure.

**Verdict.** R3 is null under the pre-registered gate, and no live SYS-R decision file is built.
- No reviewer found a deviation from §3 that could flip the gate.
- All three note that the raw exact-kind recall of 48/105 is already below the 84/105 floor.

## Findings and disposition

| # | Source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | Space Bunny F1 | Blocking (disclosure) | The pre-registered §2/§7 12 h cap on R3 was overridden without the addendum saying so (projection 39.1 h; realised screen 14.9 h) | Fixed: the addendum states the cap, the projection, the user's 2026-10-17 decision and that it is separate from the schedule override. The cap is a budget rule; the gate does not depend on it |
| 2 | Sonnet M1, Space Bunny F5 | Major | Binary follow-up: Space Bunny calls it promising (raw collapsed labels give 86/105 supported); Sonnet measured the zero-unsafe version: p(prop)+p(ans) ≥ 0.98 gives 10/105 supported, and ≥ 0.95 gives 29/105 with 2/70 at-risk | Adjudicated for Sonnet: 86/105 carries 26/70 at-risk cases called supported, which is not a zero-unsafe result. Recorded as "not promising for this model and prompt", reporting both numbers. Space Bunny's caveat accepted: any re-scoring of these retained records is post hoc and never confirmatory |
| 3 | Sonnet M2, Sol 1, Space Bunny F3 | Major | The Reading attributes the recall loss to proposal → answer; out of fold, threshold loss (30) is comparable to raw errors (57), and the 48/105 ceiling means no τ can pass | Fixed: both effects and the ceiling stated |
| 4 | Sonnet M3, Space Bunny F4 | Major | Order effect omitted: on proposal flips, canonical order gives answer 10 / proposal 5, reversed gives answer 3 / proposal 6 / conflict 5; reversed accuracy 25/40 against 23/40 | Fixed, labelled as review recomputation |
| 5 | Space Bunny F2 | Major | "Zero unsafe" stated without coverage; 18/18 committed precision omitted | Fixed: coverage 18/175, 18/18 correct, 0 unsafe, no certification |
| 6 | Space Bunny F8, Sol 2, Sonnet M4 | Major | The proposal option's precondition (a clean preview) cannot be met from the router's input, while `answer` can; "C0 entries without values" was inaccurate | Fixed: named as an instrument defect and carried as a requirement for any follow-up; corrected to "only collection entries lack values"; dev-015 and dev-017 cited |
| 7 | Space Bunny F6, Sonnet M5, Sol 4 | Major | The S40 replay mixes rev3 expected kinds with pre-rev3 ledger scores; `overlay` keeps totals only and does not assert the ledger ids against the subset | Disclosed in the addendum. Sonnet checked that all 40 expected outcomes equal rev3's (ho-048 included). Per-case rows and an id assertion in `overlay`: **open follow-up** |
| 8 | Space Bunny F7 | Major | Plan §0 trade-off omitted: −5 false support, −9 correct against C0 | Fixed |
| 9 | Sonnet M5 | Minor | The replay's 3 false supports are C0 value errors on routed-answer cases, not unsafe routes | Fixed |
| 10 | Sonnet M6 | Minor | Empty-state ablation is one prompt answered `answer` 40 times; full state beats constant proposal (15/40) only modestly | Fixed |
| 11 | Sonnet M7, Space Bunny F12 | Note | The falsifier passes marginally (44.1% against 41.4%), with the gain on non-conflict at-risk kinds | Fixed: "marginally", "not a positive claim" |
| 12 | Space Bunny F9 | Minor | Probe completion not evidenced in the reports | Fixed: cites the queue log and the 3 probe records |
| 13 | Space Bunny F10, F11 | Minor | `--router` is not checked against `bindings.runtime.router_id`; the report omits the code-identity hashes it checks | **Open follow-up** (`decision_router.py`); the values agree for R3 |
| 14 | Space Bunny F13 | Note | `llama_supports_gpu_offload: False` on a Vulkan build | **Open**: reconcile before any GPU router run |
| 15 | Space Bunny F14, Sol 3 | Note | `per_fold_strata` is held-only | Open, cosmetic (carried from the R4/R1 review) |
| 16 | Space Bunny F15 | Note | `outcome_options` docstring mismatch | Open, cosmetic |
| 17 | Sonnet M8 | Note | 141 dev ids are named `ho-*` | Noted in the addendum |

**Verification.**
- The numbers were recomputed independently by Sonnet; Sol and Space Bunny checked them
  statically against the reports.
- The analysis ran from the run's frozen worktree. The first attempt, from b5a4f50, was refused
  by the code-identity check and is not used.
- No inference was run for this review.
