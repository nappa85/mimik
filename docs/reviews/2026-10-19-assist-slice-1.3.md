# Review: assisted sheet slice 1.3, switch and Undo (2026-10-19)

**Scope.** Branch `assist-1.3` from master `9d29dfe`: implementation `9ff30d8`, round-1 fixes
`f50d5ab` + `bbfb935`, round-2 fixes `967e99c` + `1606436`, round-3 tests/docs `a783248`.
Specification: `docs/assisted-sheet-plan.md` rev 3.1 §0 (D-Q, D-N, D-X1, D-X4, D-P), the slice
1.3 section and §8; implementation notes, decisions V3 and NBSP, deviations S1–S12 and the
round dispositions in §11, §11.1–§11.3. Contents: `src/campaigns/assist.rs`,
`src/campaigns/assist_web.rs` (POST `/private-sheets/{id}/switch` and `/undo`), the note binding
inside the writer's IMMEDIATE transaction, page-level action forms named by `aria-labelledby`,
CSS-only Hide, term-boundary elision of long expressions, index-based DOM suffixes for scalars,
the `notes::current` CROSS JOIN fix (8,192-note page: 22 s → 0.38 s), the slice-1.2 follow-ups.

**Reviewers.**

| Round | Sol (`--chain main`) | Sonnet (fresh subagent each round) | Third chain |
|---|---|---|---|
| 1 (`9ff30d8`) | FIX THEN MERGE | MERGE-READY | unavailable (all three preflights failed) |
| 2 (`bbfb935`) | FIX THEN MERGE | MERGE-READY | not run |
| 3 (`1606436`) | FIX THEN MERGE | MERGE-READY | not run |
| `a783248` (tests/docs only) | main-session check | — | — |

All rounds agreed: the S1 binding (the clicked note must still be the cell's current note and
the cell must still hold its value, rechecked in the IMMEDIATE transaction with auth, rights,
active state, revision and pin) is safe against double submits, stale tabs, ABA changes and
revoked sessions; 403 precedes any 400/409; offer text is rebuilt from parsed digits, escaped and
`<bdi>`-isolated; V3 and NBSP are sound; S1–S12 acceptable; S9 is a spec change, not a weakened
test. No reviewer found an authorization bypass or wrong-row write.

**Process note.** In round 2 the read-only Sonnet reviewer attempted `git checkout --detach` in
the worktree; the permission layer denied it and `git status` was clean at `bbfb935` before the
round-2 fixes. Later reviewer prompts forbid any git state change explicitly.

## Findings and disposition

| # | Round / source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | 1 Sol | Major | Worst-case page test used `-0`; long sign-led sums (note + offer) exceed D7 | Fixed: failing-first fixtures (red 20.97 MB / 23.08 MB for the 1.2 scribble case vs 18.95 MB); long text elided on the sheet, full in history; budget unchanged |
| 2 | 1 Sonnet M1 | Minor | Stale action on a removed row/list gave a bare 400 | Fixed: `assist::in_pin`, 409 with generic label; regression test |
| 3 | 1 Sol | Minor | Row action names omitted the list | Fixed: list legend in `aria-labelledby`; `row_actions_name_their_list` |
| 4 | 1 Sol, 2 both | Minor | Repeated row searches in rendering (O(P×R²)) | Fixed in actions (r1) and notes (r2): saved row by index, ID-checked |
| 5 | 1 Sonnet M4 | Minor | Possible DOM-id collision field vs row key | No change: field IDs start with a letter, row keys with a digit |
| 6 | 1 Sonnet M3 | Minor | Hide dropped keyboard focus | Fixed: checkbox stays focusable, named "Hide the suggestion" |
| 7 | 1 Sonnet M2, M5, M6 | Minor | Scribble hides earlier Undo; `target` unconstrained; offer lives until the cell changes | Recorded in §11.1 (known behaviour, L1, constraint for step 2c) |
| 8 | 2 Sol | Major | Scalar fields with 64-char IDs repeated per action exceed D7 | Fixed: failing-first (720,924 vs 589,824 bytes), scalar DOM suffix = field index; 492,970 bytes |
| 9 | 2 both | Minor | Character-count elision can cut inside a number next to a full result | Fixed: term-boundary elision with "… (+N more terms)", exact-position tests (red→green) |
| 10 | 2 both | Note | Scribble excerpt may split a grapheme | Recorded as cosmetic; no segmentation crate in the tree |
| 11 | 2 Sonnet | Note | 409 for any non-empty host on a valid list/property | Recorded (deliberate, no leak) |
| 12 | 3 Sol | Major | Field help (≤ 2,000 chars) is not covered by D7 | Adjudicated as a pre-existing D7 documentation gap: D7 already excludes definition text; the coverage paragraph in `derived-formulas-m4-plan.md` now names labels, help and group labels and states the additive term (≤ 2,621,440 bytes). No rendering change |
| 13 | 3 Sol | Minor | Host-ID bound ignored retained 64-char pre-M4 row IDs | Fixed: argument corrected; list fixture now uses 64-char retained IDs (tightest list case 4.3% headroom) |
| 14 | 3 Sonnet | Minor | No scalar scribble worst case | Added: 574,206 of 589,824 bytes (2.6% headroom, tightest overall), passed first run |

**Verification.** Author gates in §11–§11.3 (Monday 2026-10-19, containers, `--cpus 2`,
`CARGO_BUILD_JOBS=2`): on `967e99c` fmt, pedantic Clippy, 642 Rust tests passed, ruff, tools
unittest OK, actionlint, release build, `cargo package --offline` + verify, vendor archives,
`package_release.py`, unpacked checksums and `smoke_release.py --auth`, browser smoke on the
unpacked binary with a failing negative copy; `cargo audit` and `cargo about` on `f50d5ab`
(Cargo.lock unchanged since). On `a783248` (tests/docs only): fmt, Clippy, the affected test
modules (127 passed) and the six worst-case page tests. Not run: `lan_smoke.py`, the offline
corresponding-source rebuild, a hand-driven demo session. Reviews were static.

**Open after merge.** Release-gating items unchanged (L1 sealing, rate/storage guard); the page
bound headroom is thin (2.6% scalar scribble, 4.3% list sign-led) — any added per-cell markup in
later slices must rerun the worst-case fixtures.
