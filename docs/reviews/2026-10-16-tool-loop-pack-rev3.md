# Review record: Lantern Reach pack revision 3

2026-10-16. Scope: branch `pack-rev3` (`9216f79` → `c81c669` → `2b22ee5` → `36e220a` →
`3c2b059`). It applies the user's decisions on precedence rules 6–8, item bookkeeping, the
two-watch long-rest night, the plain "armor" convention and the blind-writer scope to
`examples/tool-loop-demo/`, plus harness checks in `tools/`. Decisions:
[adjudication record](../tool-loop-pack-adjudication-2026-10-16.md). Prototype tier, but
reviewed because this pack seeds the blind held-out writer.

## Reviewers

- **Round 1** (`c81c669`): Sol REJECT, Sonnet APPROVE-with-fixes, third reviewer unavailable.
- **Round 2** (`2b22ee5`): Sol REJECT, Sonnet APPROVE-with-fixes, third reviewer unavailable.
- **Round 3** (`36e220a`, confirmation, Sol only): REJECT. Every earlier item was confirmed
  resolved; two new test defects were fixed in `3c2b059`.

The free third-reviewer chain failed preflight in every round, as recorded per AGENTS.md.

## Findings and dispositions

| Round | Finding | Disposition |
|---|---|---|
| 1 | Sol R1: ho-048 contradicts rule 8 (short rest of undefined length, lamp lit) | Relabelled ask_rule |
| 1 | Sol R2: the expected replay is circular for semantics | Notes check (CEL-recomputed stated values) and oil check added; replay documented as structural only |
| 1 | Sol R3: policy only in the README, which the model never sees | New model-visible LR2-45 "Assistant conduct", plus a test that each README precedence line has a counterpart |
| 1 | Sol R4: ho-003 acquisition-like wording | Reworded |
| 1 | Sol R5: blind writer could generate unresolved labels | "Blind held-out writer scope" exclusions |
| 1 | Sonnet R1–R5, T1–T3 | Lamp at rest start (LR2-31, dev-034 added); no-field stat outcome; bookkeeping (user reversal); hypothetical questions get an answer; override-aware tests; no skip without the CLI |
| 2 | Sol RR1: bookkeeping test pinned to `["dev-019"]` | Property test over all cases; dev-019 has its own regression test |
| 2 | Sol RR2: LR2-45 time wording lacked the rule-fixed-duration exception | Reworded and tested |
| 2 | Sol RR3: ho-093 explanation | Fixed |
| 2 | Sonnet N1–N6 | Oil check plus mutation test; "set beyond a limit" is a conflict, with the resolving rules named; prices need a ruling; unresolved tokens fail; swap wording; ho-048 rule IDs |
| 3 | Sol: oil and notes tests pinned held-out results | Pins are dev-only; held-out keeps the general properties; synthetic held-out pass/fail tests |
| 3 | Sol N4 partial: unknown override names silently dropped | Reported as unresolved; `--validate-pack` fails; regression test |

User decisions taken during review: gaining and losing items are bookkeeping (this reversed
"acquiring needs a rule"); the last-unit removal is out of the writer's scope; buying is
bookkeeping while price questions are ask_rule.

## Verification (main session, `3c2b059`, `mimik-ci`, `--cpus 2`)

- ruff check and format: clean.
- Full tools unittest with the formula CLI: **450 OK, 11 skipped**. The skips are 7 host-only
  docker smoke tests, ShellCheck, and 3 v2-ledger tests left unmounted for blindness.
- `--validate-pack`: exit 0. 45 rules, 175 dev cases, replay 175/175; the notes check (55
  cases, 144 claims) and the oil check (dev-017, ho-020, dev-034) are clean.

**Known limit:** the oil check only recognises durations stated in watches or as a long rest.

Round 3's final fixes were verified by the main session rather than re-reviewed. They are
test-only and mechanical, and they exactly address the reviewer's findings.
