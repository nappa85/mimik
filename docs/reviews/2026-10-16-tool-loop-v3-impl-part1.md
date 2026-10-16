# Review record: tool-loop harness v3, first half (implementation)

2026-10-16. Scope: commits `cded70d`…`5869e00` on branch `harness-v3`:
- pack and prompt fixes (§3, §11 item 21);
- H1–H6, H8, H11;
- the ctx audit (§11 item 6);
- H7 reporting;
- the v2 differential test.

Spec: [v3 plan](../tool-loop-harness-v3-plan.md) §11. Prototype tier.

## Reviewers (standing setup, `AGENTS.md`)

- **Sonnet 5.5 subagent: APPROVE-with-fixes.** It ran the gates in `mimik-ci` and 6
  mutations; the tests caught 5. The surviving mutant removed H6 marking of derived
  previews.
- **OpenAI Sol (`--chain main`): REJECT.** Static review only.
- **Space Bunny (`--chain third`, first in the fallback order): APPROVE-with-fixes.**

The main session checked Sol's unique claims R3 and R6 against the code and confirmed both.

## Findings and dispositions

| Finding | Raised by | Disposition |
|---|---|---|
| Ledger identity gaps: `dry_run`/`dry_run_expected`, `harness_sha256`, `visible_text_hashes`, `length_rule` and the case set were not compared; dry-run and live ledgers could merge; rows were not checked against headers or the job set | all three | Fixed in `04aea7f`, failing-first test per field |
| Case-level all-seeds metrics counted cases missing a seed | Sol, Space Bunny | Fixed; reports `complete_cases`/`incomplete_cases` |
| ctx audit measured only short replays | all three | Fixed. The upper bound for 8/12/16 turns with 4096 completion tokens and a 1.25 template margin gives **≈22.3k tokens at 8 turns, above the planned ctx 16384**; the exact count needs the tokenizer at G0 |
| `not_run` rows counted in exposure denominators | Sonnet, Space Bunny | Fixed; a `coverage` summary is added and incomplete coverage fails the bar |
| H6: the no-op rejection error repeated injected text unmarked | Sol | Fixed; flags-off wording unchanged |
| H6 derived-preview marking untested (surviving mutant) | Sonnet | Test added; shown red by deleting the marking |
| `--dry-run-expected` allowed on held-out | Space Bunny | Refused unless `--allow-heldout-gold-replay` is given (recorded in the header) |
| `truncated_calls` mislabels non-object JSON | Space Bunny | Renamed `sanitised_calls` |
| Docs: flags-off is not byte-identical to v2; test skip counts; `empty_citation` denominator; H9 wording; H5 × stale interaction | Sonnet, Space Bunny | Fixed in the docs; `empty_citation_of` added |
| README counts and hashes not mechanically checked | Space Bunny | Test added; it found a wrong recorded hash tail (`010b` → `d10b`), now fixed |
| Answer-gold validation is circular (the replay copies `answer_values`) | Sol | **Deferred** to the held-out gold-check phase (§11 item 13); recorded in TODO |
| Lenient CSV columns; runner `--ctx`/`--arm` pass-through; H3 live smoke | Sonnet, Space Bunny | Deferred (second half / needs inference) |

**Deviation accepted by the main session:** seeds are not a strict identity field. Ledgers
with different seeds may combine when every seed covers the same case set, as seed-29
confirmations require. This is checked through the job set and the per-seed case-set test.

## Verification

Main session, `mimik-ci`, v2 ledgers mounted read-only:
- ruff check and format clean;
- **259 tests OK**; 3 skip without `MIMIK_V2_LEDGERS`;
- `--validate-pack`: 174 dev cases, 0 held-out; expected replay 174/174.

No inference was run. The fixes were not re-reviewed; each has a failing-first test.
Review findings are not proof of correctness.
