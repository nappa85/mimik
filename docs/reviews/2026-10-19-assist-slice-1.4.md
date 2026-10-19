# Review: assisted sheet slice 1.4, unknown values (2026-10-19)

**Scope.** Branch `assist-1.4` from master `5686980`: implementation `8e761d5` + `6127bcd`,
round-1 fixes `3d40e21` + `50ef803`, wording fixes in the merge-prep commit. Specification:
`docs/assisted-sheet-plan.md` rev 3.1 §0 (D-M, D-X1, D-N, D-Q; new D-Y, D-Z), §3a, §3f, the
slice 1.4 section; notes, decisions U1–U11, red→green and gates in §12, round-1 dispositions and
limitations L-U1–L-U4 in §12.1. Contents: `null` in every integer cell (idempotent nullable
wrapper), `private_pin::list_values`, entry rules (blank → unknown with `erased` note; unreadable
over a saved value keeps it; unreadable on a new row, newly present property or create → unknown
with `unresolved` note), `convert::inputs`/`record` null handling, the formula read-set guard,
the conformance fixture (content revision 2) and hash relock, "?" placeholder and the
"Unknown value" accessible hint, Undo/freeze/history with nulls, docs.

**Reviewers.**

| Round | Sol (`--chain main`) | Sonnet (fresh subagent) | Third chain |
|---|---|---|---|
| 1 (`6127bcd`) | FIX THEN MERGE (1 Blocking, 1 Major) | MERGE-READY | unavailable (all preflights failed) |
| 2 (`50ef803`) | MERGE-READY | MERGE-READY | not run |

**User decisions taken during review** (2026-10-19, recorded in plan §0):
- **D-Y:** formula profile `cel-mimik-1` may still change until the first release; "a pinned
  definition is never reinterpreted" applies from the first release on. No profile bump.
- **D-Z:** any formula or check that reads an unknown integer, nullable or not, is not evaluated
  ("not computed: a value it uses is unknown"). Nullable text, enum and boolean keep CEL null
  semantics.

## Findings and disposition

| # | Round / source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | 1 Sol | Blocking | Results change under the same profile ID for already-storable data (null copy of a nullable field: `type_mismatch` → `null_value`), violating the pinned-profile contract | Resolved by user decision D-Y; contract, profile note, U1 and fixture note updated |
| 2 | 1 Sonnet M1 | Minor | U1 claim "every outcome over pre-1.4 values unchanged" was false | Fixed: the changed outcomes are listed in the profile note |
| 3 | 1 Sol, Sonnet M2 | Major | Nullable integers exempt from the guard: `luck != 0` passes, `luck == 0 ? 10 : 20` gives 20 on unknown | Fixed per D-Z: exemption removed; failing-first `cel_forms_over_an_unknown_are_not_evaluated` + conformance cases; U3 superseded; L-U4 (no `== null` fallback on integers) |
| 4 | 1 Sol | Minor | No connected test for blank over a scribbled unknown | Added `a_blank_save_resolves_the_scribble_of_an_unknown_cell` (guard test) |
| 5 | 1 Sol | Note | No escaping test for scribbles on unknown cells | Added `a_scribble_on_an_unknown_cell_renders_escaped_inside_bdi` (guard test) |
| 6 | 1 Sonnet M5 | Minor | Empty unknown controls named only by `placeholder="?"` | Fixed: one hidden "Unknown value" hint per page, `aria-describedby` on empty integer controls; failing-first test; page bound rerun |
| 7 | 1 Sonnet M3, M4, M6 | Minor | `has()` true for present-but-unknown; > 256 notes per save is 422; no rollback for older binaries | Recorded as L-U3, L-U1, L-U2 |
| 8 | 2 Sol, Sonnet | Minor | Comments in `formula/mod.rs:24` and `formula/tests.rs:7-9`, and the literals row in `derived-formulas-plan.md:149`, did not reflect D-Y/D-Z | Fixed at merge prep (wording only) |
| 9 | 2 Sol | Note | No positive regression that nullable text/enum/boolean keep CEL null semantics | Follow-up (TODO); guard is typed on `ValueType::Int` |
| 10 | 2 Sonnet | Note | If nullable non-integer list properties are ever admitted, `unknowns.properties` would over-guard | Recorded for that future change |

**Process notes.** The implementer reported two slips in its own round-1 record: one page-size
rerun measured a stale build (mutation restore kept old mtimes; redone on a fresh build, the
script now touches restored files), and the round-1 smokes used the slice-1.3 source fixture
(a fresh one is identical by `diff -r`).

**Verification.** Author gates in §12 and §12.1 (Monday 2026-10-19; 2 CPUs until 18:00, then
6 for Cargo): fmt, pedantic Clippy, 658 Rust tests passed, ruff, tools unittest OK, actionlint,
release build, `smoke_release.py --auth`, `browser_smoke.py --auth` at both viewports with a
failing negative control; worst-case page fixtures (tightest 2.6%, unchanged). Merge prep
(main session, 4 CPUs): fmt, Clippy `-D warnings`, formula tests (85 passed). Not run (no
Cargo.lock or migration change): cargo audit, cargo about, packaging and the unpacked-artifact
smoke, `lan_smoke.py`, the offline source rebuild, a hand-driven demo. Reviews were static.
