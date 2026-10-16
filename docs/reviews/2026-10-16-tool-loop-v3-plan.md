# Review record: tool-loop harness v3 plan

2026-10-16. Scope: [docs/tool-loop-harness-v3-plan.md](../tool-loop-harness-v3-plan.md)
§1–§10 (pre-amendment), including the user answers in §10. All reviewers worked from one
sanitized snapshot of the repo, without `target/`.

## Independence

There were six parallel reviewers with the same prompt, all read-only:

- **Sonnet 5.5**, a fresh subagent.
- Via opencode's `reviewer` agent (read/glob/grep only):
  - **Space Bunny** and **DeepSeek v4.1 Flash**, both free tier;
  - **Muse Spark 1.3**, free contributor tier;
  - **GPT-6.1 Sol** and **GPT-6 Luna**, the user's own OpenAI models. The user asked for
    them once, for this review and for reviewer-benchmark experiment 1 only.

A fork of the main session adjudicated against the code and the local `target/` artefacts.
Verdicts: GO-with-amendments from Sonnet and DeepSeek; NO-GO *as written* from Space Bunny,
Muse, Sol and Luna. Adjudicated result: **GO-with-amendments**. Every blocker is a plan-text
fix needed before freezing, and nothing has been built or run. Amendments are in plan §11
(1–33). The amended §6–§8 need a short re-review before the held-out is written.

## Findings and dispositions

| Issue | Raised by | Check | Disposition |
|---|---|---|---|
| 150×2 correlated seeds cannot certify ≤2%; no decision rule | all six | Wilson 0/70 = 5.2%, 0/150 = 2.5%; ≈188 cases needed | §11.1: three-valued verdict |
| H2 selection noisy, unpaired, D1 on a different denominator, untested combination | all six | Plan §7 | §11.4 |
| Held-out results visible before selection finishes | Sol | Progress lines `tool_loop_benchmark.py:1888-1892` | §11.3: dev before held-out |
| README leaks injection ids and "intended traps" to the writer | Son, Mu, Sol, Lu, DS | `README.md:93-97,224-237` | §11.9: sanitized brief |
| H2 (turns and search notes) in the base despite the expected false-support increase | DS, Mu, Lu, Bun | Correction item 6 | §11.5: out of base, split |
| `ctx` chosen after the smoke; computable offline | Son, Bun, Mu, Sol | `Episode.messages` | §11.6 |
| Merge identity omits prompt/tools hashes, `max_turns`, flags | Bun, Mu | `_ledger_identity` `:1938-1944` | §11.19 |
| S1 does not close the int hedge; strict must stay v2 | Son, Sol, Lu | `:1544-1558` | §11.15, plus differential test |
| Lenient must remove only eligible reasons | Sol | `:1574-1587` | §11.16 |
| S3 rubric absent; veto rules | Mu, DS, Lu, Sol, Bun | Plan §6 | §11.17 |
| Overlap check weak; flagged pairs leak dev text | Son, Bun, DS, Sol, Lu | Plan §4 | §11.10 |
| Gold-review record and summaries can expose held-out | Bun, DS | Plan §4 | §11.11 |
| No deterministic gold oracle | Son | — | §11.13 |
| Minima sum exactly to 150, no reserve | Lu | 45+35+24+14+14+8+10 = 150 | §11.14 |
| Queue: no state machine, completeness gate, deadlines or real preflight; lives in `target/` | all six | `CHECK_ONLY` exits before launch (`run_tool_loop_benchmark.sh:106-108`); `not_run` rows exit 0 | §11.26 |
| 35B: cold-read time over the health wait; request timeout hard-coded; G0 tie-break; speed-chosen prefix | Bun, Sol, Mu, Lu | `run_tool_loop_benchmark.sh:127-132,139`; results doc 12.6 MB/s | §11.27 |
| `max_tokens` 4096 is an unlisted change | DS, Bun | Plan §7 | §11.6 (H11) |
| H6 surfaces and refuse wording | Bun, Sol | `read_fields` raw values | §11.20, §11.21 |
| H9 needs an applicability predicate; cut H10 | Sol, Mu, Lu, Bun | — | §11.23, §11.24 |
| Weak or missing tests (H4 via CLI, H3 proxy, off/on pairs, nudge edges, Wilson) | all six | `run_episode` already accepts 12 (`:1352,1372`) | §11.25 |
| Thinking arm valid only at 4096 tokens and T = 0.1 | Son | — | §11.28 |
| A0 changes cases and flags together | Son | — | §11.29 |
| Arm/harness name collision | DS | H1–H10 vs H0–H2 | §11.7 |
| Model-visible strings not hash-pinned | Bun | — | §11.8 |
| No v3 results review scheduled; missing docs | DS, Bun | — | §11.31 |
| "Clamped" is wrong (values are rejected) | Sol | `:2055-2056` | Fixed in §1 |
| `target/` claims unverifiable from the snapshot | Bun, DS, Mu, Sol, Lu | Main session verified them locally: queue `:12`, load test `:22,32,38`, no 35B dir | §11.32 |
| Bar wording (correct over all cases; false support over both denominators) | Bun, Sol, Mu | `bar_verdict`, user decision | §11.1 |

## Wrong or harmful claims

| Reviewer | Claim | Why |
|---|---|---|
| Space Bunny | B1: S1 forgives invented extras that "v2 already flags" | v2 does not flag `{watches:12, lamp_oil:3}` against `{travel_watches:12}` (`need - have` is empty; within len+1). Its fix, extending the hedge guard in strict scoring, would break v2 identity (**harmful**) |
| Space Bunny | B2: 30% Italian makes the bar "undecidable by construction"; judge on the English stratum | Extrapolates v1 Italian rates as a ceiling. Restricting the bar to English would weaken the user's bar (**harmful**) |
| Space Bunny | 0/140 gives a 2.1% upper bound | Wilson gives 2.7% |
| Muse | F1 (Blocking): `README.md:221-223` has no substring rule; wrong tool-result and pending citations | The lines are the stale "value appears in the answer text" heuristic, which is what the plan meant. `:1411-1417` appends the tool result the model sees, and `:682-687` builds the path/to-only pending proposal. The citations are correct |
| Muse | F6: `--summarize --labels` "does not exist" | It is a planned new flag, not a current-state claim |
| Sonnet | Prompt stale line is "~:81-82" | It is `:82-83`, as the plan says |
| Luna | F4: strict is not identical to v2 | Under strict, both S1/S2 sub-classes remain false support, so the counts are identical. Valid only as "make it testable" (§11.15) |

Review findings are not proof of correctness.
