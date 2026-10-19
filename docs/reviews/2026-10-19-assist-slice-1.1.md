# Review: assisted sheet slice 1.1 (2026-10-19)

**Scope.** Branch `assist-1.1` from master `4d07932`: `813a9d9` (slice 1.1: sums on the existing
save, demo fixture and launcher) and review fixes `9200fe8`. Specification:
`docs/assisted-sheet-plan.md` rev 3.1, §0 (D-Q: sign-led input is literal) and the slice 1.1
section; implementation notes, deviations V1–V7 and gates in §9–§9.1.

**Reviewers.**

| Round | Commit | Sol (`--chain main`) | Sonnet (fresh subagent) | Third chain |
|---|---|---|---|---|
| 1 | `813a9d9` | FIX THEN MERGE (release gates unrun; two Minors) | MERGE-READY | unavailable |
| 2 | `9200fe8` | MERGE-READY | MERGE-READY | not run (verification round) |

No reviewer found a functional defect in the parser or its integration. Both confirmed
`entry::number` is reached only through `values::scalar_value` (runtime scalar fields and list
rows, create and edit), while revisions and native sheets stay strict.

## Findings and disposition

| # | Round | Source | Severity | Finding | Disposition |
|---|---|---|---|---|---|
| 1 | 1 | Sol | Major | Release gates (audit, about, package, unpacked smoke, offline rebuild, LAN smoke) not run although the change adds locked dependencies | Run in `9200fe8` (§9.1). `cargo audit` found RUSTSEC-2026-0373 in `ammonia 4.2.3`; updated to 4.2.4 (lockfile only). Not reachable: the sanitizer drops every `style` attribute; guard test `style_attributes_never_survive_in_any_case` added. All gates then passed |
| 2 | 1 | Sol | Minor | The i128 overflow test did not distinguish checked from wrapping accumulation | Fixed: `accumulation_overflow_is_not_cancelled_by_later_terms` (`A+A-A-A`), red under a wrapping mutation |
| 3 | 1 | Sol | Minor | `Range` doc described the wrong bound | Fixed |
| 4 | 1 | Sonnet | Minor | Demo data dir 0755 and account-input files left on disk | Fixed: 0700, inputs removed in `finally`, three tests |
| 5 | 1 | Sonnet | Minor | 422 message does not name the field; NBSP refused | Unchanged behaviour; NBSP noted for slice 1.3 |
| 6 | 2 | Sonnet | Minor | Guard test's bare `<td>` is dropped by the parser, so the table branch is not exercised | Follow-up (slice 1.2): wrap in a table and assert an allowed `colspan` survives |
| 7 | 2 | Sonnet | Minor | `fresh_directory` chmods a caller-supplied empty directory | Follow-up: document in the docstring |
| 8 | 2 | Sol | Minor | `test_assist_demo.py:68-69` catches every `AssertionError`, which can swallow the mock's own permission assertions | Follow-up (slice 1.2): a dedicated exception type for the injected failure |
| 9 | 2 | Sonnet | Note | The release record names gated commit `3aca432`, folded into `9200fe8` (docs-only difference) | Recorded |
| 10 | 1 | main session | Note | `tools/lan_smoke.py` starts two containers without a CPU cap, against the weekday 2-core rule; the gate run used a local wrapper | Follow-up: cap them in the script |

**Verification.** Author gates in plan §9 and §9.1 (Monday 2026-10-19, `--cpus 2`): fmt,
pedantic Clippy, 587 Rust tests passed, ruff, tools unittest OK, actionlint, `cargo audit`
(clean after the fix), `cargo about`, release build, source package, vendor archives, unpacked
`smoke_release.py --auth`, offline rebuild, `browser_smoke.py --auth` at both viewports with the
assisted stage, `lan_smoke.py`. No physical phone or router. Reviews were static.
