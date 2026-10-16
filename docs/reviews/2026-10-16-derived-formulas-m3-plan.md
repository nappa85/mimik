# Review record: derived formulas M3 plan

2026-10-16. Scope: [derived-formulas-m3-plan.md](../derived-formulas-m3-plan.md) (§1–§9 and
the user answers in §3), against master `1166a1d`.

## Independence

- Planner: read-only Opus 5.5 subagent. Main session spot-checked three §1 claims.
- External reviewer `opencode/muse-spark-1.3-contributor-free`: unavailable (preflight
  failed, exit 3).
- Fallback reviewer: fresh-context Sonnet 5.5 subagent, read-only, no builds or containers.
  Same vendor family as the implementer; no cross-family review was available (as for M1/M2).

## Verdict

**GO-with-amendments.** All "Current state (verified)" claims checked out within 1–2 lines.
Two blocking plan defects, ten medium and five low findings. All are accepted and written into
plan §10 as binding amendments; none is implemented yet.

| ID | Sev. | Finding | Disposition |
|---|---|---|---|
| B1 | Blocking | Per-step commits leave `pub(crate)`/`pub(super)` items unused → `dead_code` under `clippy -D warnings` | §10.1: steps 1, 3, 4 merged into one commit; no `#[allow]` |
| B2 | Blocking | Create page (`data = {}`, `sheet_id == 0`) untested; would show bogus "unavailable" and derived inputs | §10.2: skip evaluation on create; three route tests |
| M1 | Medium | Test 4 compared `Compiled` values, which have no `PartialEq` | §10.3 |
| M2 | Medium | New cross-thread pin drop; non-pin `validate` callers would carry compiled ASTs | §10.4 |
| M3 | Medium | Q5 design: `deps` is a `BTreeSet`, constraints also report `dependency_failed`, digest risk | §10.5: accessor using `derived_order()`; no `Evaluation`/`Code` change |
| M4 | Medium | Permit held after disconnect; wall-clock deadline flakes tests under load | §10.6: document, test seam, record timings |
| M5 | Medium | History test could not distinguish historical from live data | §10.7 |
| M6 | Medium | Derived-key substitution not tested | §10.8 |
| M7 | Medium | Labels/help for derived fields; native markers | §10.9 |
| M8 | Medium | Unavailable state only unit-tested | §10.10 |
| M9 | Medium | Fixture admission unverified; escape test could pass for the wrong reason | §10.11 |
| M10 | Medium | Browser scope incomplete (publication path, POST names, long text, keyboard, native) | §10.12 |
| L1 | Low | `<output>` is an implicit live region and sits inside the disabled fieldset | §10.13 |
| L2 | Low | Derived text needs `pre-wrap` | §10.14 |
| L3 | Low | Render-time evaluation deviates from parent §7, unrecorded | §10.15 |
| L4–L5 | Low | `preview` doc correction; pack translation duplication | §10.16 |

Doc fixes applied with the amendments: the superseded §3 option list was removed and the Q5
work moved to step 2. Review findings are not proof of correctness. The implementation
needs its own independent review.

## Second round: reviewer comparison

Muse Spark, Space Bunny and a second Sonnet run then reviewed the same pre-amendment plan in
parallel. See [reviewer comparison](../reviewer-benchmark.md). The first
round's "Muse unavailable" was a wrapper bug, not quota: opencode used the launch `$PWD`
(the real repo) instead of the snapshot, so the snapshot-only `reviewer` agent did not exist.
Those runs never reached the model or a file. The wrapper is fixed (`--dir` plus `PWD`, and
`websearch`/`task`/`skill` now denied), with regression tests in
`tools/test_external_review.py`.

The new valid findings are plan §10b items 17–30. The false or harmful findings are listed
in the comparison record and rejected.
