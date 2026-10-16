# Review record: derived formulas M3 implementation

2026-10-16. Scope: branch `derived-formulas`, commits `b1c896b`…`81c4d01` (rebased on
master `2566040`), against [derived-formulas-m3-plan.md](../derived-formulas-m3-plan.md)
§1–§10d. Implementation summary and evidence: [parent plan §16](../derived-formulas-plan.md).

## Independence

- Implementer: Opus 5.5 subagent.
- Reviewers, run in parallel by the main session with fresh contexts:
  - Sonnet 5.5: **APPROVE-with-fixes**. It ran the tests and 7 mutations; 2 mutations
    survived.
  - OpenAI Sol: **REJECT**, on gate completeness.
- No third reviewer was available. Space Bunny timed out, and the DeepSeek and Muse
  preflights failed. Per `AGENTS.md`, the review proceeds with two reviewers.
- The main session adjudicated every finding. The entries below record the adjudicated
  findings as the main session relayed them; the full reviewer texts are kept by the
  main session.

## Findings and dispositions

| ID | Source | Finding | Disposition |
|---|---|---|---|
| F1 | Sonnet | Departure with formula pins untested; the mutation "depart uses `validate`" survived | Fixed in `3708836`: route/Store test for leaving and for removal (frozen state, values intact, `freeze` history row, advisory results still shown) plus a test-only departure-pins hook. The mutation is now killed. |
| F2 | Sonnet | The K39 test did not detect a dropped kept permit (mutation survived) | Fixed: the test asserts that `rejected_editor` received the save's own permit. The drop-and-readmit mutation is killed. |
| F3 | Sonnet | The write-only check came before the formula-free `None` | Fixed, with an executed-red test. |
| F4 | Sonnet | `undeclared_inputs` and the `formula_spec` declarations use separate predicates | Consistency test added (guard); a predicate mutation is killed. |
| F5/F6 | Sonnet | Residual 503 on the permit-less rejection path; 503 test name and shape | Documented in code (the auth layer adds `Retry-After: 1`). Test renamed and now preceded by a real edit save. |
| F7 | Sonnet | Branch predated master `2566040` | Rebased; `docs/small-model-candidates-2026-10-16.md` is kept. |
| F8 | Sonnet | Saved-mode markup could emit an empty `<dd>` | "Not computed: no result is available for this field." fallback. The case is unreachable in normal evaluation, so there is no route test. |
| R1 | Sol | Fresh delivered-source rebuild and the LAN/proxy gate were not run | Both ran and passed on revision `3708836` (artifact identity in §16). |
| R2 | Sol | Escaping lacked a quote-bearing attribute payload and block-scoped echo checks | `quote_payloads_stay_one_attribute_value`, plus echo assertions inside the view, saved-revision and history blocks. Two `|safe` mutations are killed. |
| R3 | Sol | Red→green evidence was not recorded | Table in §16, distinguishing executed red, compile red, guards and mutations. |

## Verification

See §16, "Verification after the review fixes". Review findings are not proof of
correctness.

## Status

All fixes have been applied. Re-review of the fixes is pending; the branch is not merged.
