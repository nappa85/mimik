# Review: assisted sheet slice 1.5a, per-field save for metadata and scalar fields (2026-10-19)

**Scope.** Branch `assist-1.5a` from master `21b6559`: implementation `ae16632`, round-1 fixes
`3198d42`, round-2 fixes `cc3cfa6`, merge-prep test and wording. Specification:
`docs/assisted-sheet-plan.md` rev 3.1 §0 (D-C, D-X3, D-N, D-P, D-Q), §3a "Per-cell save", §3g,
§3h, the slice 1.5a section; notes, decisions P1–P12, red→green, mutations and gates in §13,
§13.1, §13.2. Contents: `POST /private-sheets/{id}/meta/{name|notes}` and `/fields/{field_id}`
(`unit_web.rs`, `unit.rs`), exact keys, value-based conflicts with `base` and L read in one
transaction, one form per unit, the D-X3 bounded permit wait and styled busy page (unit routes
and, since round 1, the full-form/list-form edit route), interim per-list forms (P1), accessible
Save names, the scribble quote shortened to 32 characters (P9).

**Reviewers.**

| Round | Sol (`--chain main`) | Sonnet (fresh subagent) | Third chain |
|---|---|---|---|
| 1 (`ae16632`) | FIX THEN MERGE (1 Major) | FIX THEN MERGE (1 Major) | unavailable (all preflights failed) |
| 2 (`3198d42`) | MERGE-READY (3 Minors) | FIX THEN MERGE (3 Minors) | not run |
| 3 (`cc3cfa6`, targeted) | MERGE-READY (1 Minor) | MERGE-READY (1 Note) | not run |

All rounds agreed: the value-based conflict design is sound (base and L in one read, patch on
L, auth/rights/state/revision rechecked in the IMMEDIATE writer); unit saves do not disturb
other units' notes or the 1.3 switch/Undo bindings; P1 list forms cannot drop or overwrite other
fields or lists; 403 precedes 400; ABA is accepted per L3.

## Findings and disposition

| # | Round / source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | 1 Sol (confirmed in code) | Major | The document-error handler (`auth/mod.rs:257-265`) replaced the styled busy 503 with the generic page in real browsers, losing the draft | Fixed: `sheet_busy` exemption; failing-first `the_busy_page_survives_a_browser_navigation` through the full router with document headers; guard for 409/422 re-renders |
| 2 | 1 Sonnet M1 | Major | Full-form/list-form route had no D-X3 wait: busy list saves lost typed rows | Fixed: bounded wait and a busy page re-posting the submitted pairs; failing-first test |
| 3 | 1 Sonnet m2 | Minor | All Save buttons named "Save" | Fixed: `aria-labelledby` ("Save Current HP", "Save list Equipment"); failing-first test; page bound rerun (tightest 3.1%) |
| 4 | 1 Sol | Minor | Unsupported pins answer 403, not the documented 400 | Documented as 403 (read-only); guard test |
| 5 | 1 Sonnet m3, m4, m6 | Minor | Busy page did not name the field; no default-submitter assertion; no large-draft re-render bound | Fixed (field named by ID/Name/General notes; enum/boolean still free text, recorded); assertion added; bound test added |
| 6 | 1 Sonnet m1, m5; Sol P4 | Minor | Same-value save replaces another tab's scribble; signed-in users can queue for the global permit | Recorded in §13.1 (L3; bounded, no leak; per-account fairness later) |
| 7 | 1 → 2 | — | The round-1 change made the existing slow-upload test loop forever; it was rewritten for the edit route | Reviewed in round 2: a legitimate strengthening for edit, but create lost coverage (#8) |
| 8 | 2 Sonnet F1, Sol 3 | Minor | Create route no longer covered by a slow-upload test | Fixed: renamed edit test; `slow_create_upload_holds_the_runtime_permit_at_most_the_request_timeout` keeps the original assertion; 20 s deadlines on every loop |
| 9 | 2 Sol 1 | Minor | Full-form saves waited before CSRF and full auth | Fixed: parse, CSRF, auth before the wait; failing-first `a_forged_edit_is_refused_at_once_while_busy` |
| 10 | 2 Sonnet F3 | Minor | Busy echo bound counted values only (about 10 MB pages possible) | Fixed: keys + values + 48 bytes per pair ≤ 1 MiB; failing-first `the_busy_page_of_a_form_is_bounded` |
| 11 | 2 Sonnet F2, Sol 2 | Minor | Plan P4 row and runtime contract described the old buffering | Fixed (create vs edit; up to 8 × 5 MiB edit bodies) |
| 12 | 2 Sonnet F4, F5 | Note | Slow body + wait can end in 408; `busy_form` checks no sheet rights | Recorded in §13.2; code comment |
| 13 | 3 Sol | Minor | The busy-bound test did not protect the per-pair markup charge | Fixed at merge prep: a 30,000-short-pair case (keys alone within 1 MiB) must be dropped; mutation (charge × 0) fails it at `unit_tests.rs:1315`, restored and checked with `cmp` |
| 14 | 3 Sonnet | Note | "About 40 MiB raw" counts bodies only | Wording fixed at merge prep (decoded pairs and busy page included) |

**Verification.** Author gates (Monday 2026-10-19 evening, 10 CPUs): on `cc3cfa6` fmt,
pedantic Clippy, 682 Rust tests passed, ruff, tools unittest OK, actionlint, release build,
`smoke_release.py --auth`, `browser_smoke.py --auth` at 1280 and 390 px; negative browser
controls failed as intended in rounds 0–1. Merge prep (main session, 6 CPUs): fmt, Clippy
`-D warnings`, `unit_tests` (20 passed), the mutation above. Not run (no Cargo.lock or
migration change): cargo audit, cargo about, packaging, the unpacked-artifact smoke,
`lan_smoke.py`, the offline source rebuild, a hand-driven demo. No browser stage for the busy
page (it would need a test hook in the shipped binary). Reviews were static.

**Open after merge.** The migration-020 `meta:` length follow-up stays open (1.5a writes no
`meta:` notes). Interim per-list forms (P1) are replaced in 1.5b. Page-bound headroom 3.1%.
