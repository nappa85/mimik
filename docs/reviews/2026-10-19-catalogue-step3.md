# Review: catalogue plan step 3 implementation (2026-10-19)

**Scope.**
- Branch `catalogue-step3` from master `b640fca`: `023252f` (step 3), round-1 fixes `611acc0`,
  round-2 minors ``6fe0b45``.
- Contents:
  - the `catalogues` definition member (1–8 `{"id": slug}` links, omitted when empty) and the
    three-site existence checks (draft save, publication compare/approval, readiness);
  - the read-only sheet routes `GET /private-sheets/{id}/catalogue/{catalogue}` (browse and
    search) and `/{key}` (entry), in `src/catalogues/read.rs`, `src/campaigns/catalogue.rs` and
    `src/campaigns/catalogue_web.rs`;
  - live badges and citations, the 20,000-character cut, quoted FTS search under the time budget,
    `(name, key)` cursor order;
  - `<bdi>`/pre-wrap rendering of owner and unit text with a Chromium check
    (`tools/catalogue_browser.py`), and a synthetic catalogue in `smoke_release.py`;
  - docs: `catalogue-publication.md` "Reading through a sheet", `runtime-definition-contract.md`,
    `definition-drafts.md`, plan §17–§18.
- Specification: `docs/catalogue-plan.md` step 3 and amendments R1–R28 (notably R6, R10, R12, R25,
  D11). Implementation notes, red→green tables and deviations are in plan §17 (step 3) and §18
  (review fixes).

**Reviewers.**

| Round | Commit | Sol (`--chain main`) | Sonnet (fresh subagent) | Third chain |
|---|---|---|---|---|
| 1 | `023252f` | FIX THEN MERGE | FIX THEN MERGE | Space Bunny: FIX THEN MERGE |
| 2 | `611acc0` | MERGE-READY | MERGE-READY | Space Bunny: MERGE-READY |
| 3 | `6fe0b45` | MERGE-READY | MERGE-READY | not run (round-2 minors only; recorded) |

- No reviewer reported a Blocking finding in either round.
- All reviewers found the authorization confinement, the FTS quoting and the HTML escaping sound
  by reading; nobody could construct a cross-catalogue, cross-edition or unsealed-version read.
- Sol's round-1 run had no Dockerfile in its snapshot (excluded by the sanitizer); not material.

## Findings and disposition

| # | Round | Source | Severity | Finding | Disposition |
|---|---|---|---|---|---|
| 1 | 1 | Sol 1 (Major), Sonnet 1, Space Bunny M2, m8 | Major | The second-transaction guards (`corresponds` → 409, `pin.unchanged`, the transaction-B session rechecks) had no test of their own; only a role demotion was exercised | Fixed in `611acc0`: a pause-hook test with four cases (unchanged, sheet saved, pin row rewritten, custodian epoch bumped). Each guard's removal turns only its own case red |
| 2 | 1 | Space Bunny M1 | Major | A malformed cited unit kept the "Reviewed" badge while the description was hidden; browse could not detect malformed units at all | Fixed: one `MALFORMED` predicate feeds browse and entry, one `Badge::of_units` rule (rejected, then malformed or no unit → `Badge::Malformed`, "Description unavailable"); the entry description follows the badge. Red: badge from review state only |
| 3 | 1 | Sonnet 2 | Minor | Type labels, property labels and `<option>` labels were not bidi-isolated | Fixed: `<bdi>` on labels, `dir="auto"` on options (cannot hold elements). RLO labels are impossible (admission refuses Cf in labels), so the Chromium fixture uses Hebrew labels and checks the following badge |
| 4 | 1 | Sol 2, Space Bunny m3 | Minor | Confinement inferred from SQL, not pinned: other edition, sealed v1 + unsealed v2, foreign cursor key, corrupt pair; the corrupt-pin test only asserted `!= 200` | Fixed: fixtures added, exact statuses asserted; removing the edition predicate, the seal join or the cursor confinement each turns red |
| 5 | 1 | Sol 3 | Minor | Malformed unit after the 20,000 cut only tested for NUL in a standalone entry | Fixed: 25,000-character unit followed by NUL, BLOB or oversized unit shows none of the first unit's text |
| 6 | 1 | Sol 4 | Minor | Browser check comment overstated a following-text witness | Fixed: "Current text" paragraph after the description, checked left-to-right below it |
| 7 | 1 | Sonnet 4, Space Bunny m4 | Minor | Every `AppError::Invalid` from browse became the 422 query-problem page, including integrity damage | Fixed: `BrowseFailure { Query, Other }`; round 2 narrowed it further (see #13) |
| 8 | 1 | Space Bunny m5 | Minor | History pages listed links from the historical pin while the routes use the current sheet | Fixed: no catalogue section on history pages (create page asserted in round 2) |
| 9 | 1 | Space Bunny m6 | Minor | Property values had no pre-wrap/overflow-wrap | Fixed; 400-character unbroken value in the smoke fixture, computed-style assertion |
| 10 | 1 | Space Bunny m7 | Minor | A no-unit entry: Unchecked on browse, 422 on the entry route | Fixed: both show `Badge::Malformed`; the retired entry page is tested readable |
| 11 | 1 | Sonnet 3 | Minor | A damaged catalogue title failed the whole sheet view | Fixed: title optional, link shown as unavailable, pages fall back to the id |
| 12 | 1 | Space Bunny m9, Sol (R10) | Minor | No comment on why `catalogues: []` is refused; R10 test evidence worded more broadly than its formula-free fixture | Fixed in comments and §17 |
| 13 | 2 | Space Bunny F2, Sonnet note | Minor | The new malformed probe scans full unit text under the search budget; an interruption there gave a plain browse the "Query too broad" message | Fixed in `6fe0b45`: only the name-search statement's interruption is `Query`; plain browse, types, cursor and badge-probe failures are `Other` (still an error, fail-closed). Red: old mapping. Worst badge-probe volume (50 entries × 16 units × 256 KiB) measured at 416–441 ms warm, release, `--cpus 6`: inside the 1 s budget, above the 250 ms page target; a cheaper probe is a follow-up. Sol round 3 (Minor, open): the types, cursor and badge-probe interruptions have no direct test |
| 14 | 2 | Space Bunny F1, Sol note | Minor | The record claimed the loader refuses an unsupported runtime pin first and that the route guard is unreachable | Settled by mutation in `6fe0b45`: isolated pin-column damage is refused by the loader (`private_sheet_events_valid` compares the history pin with `private_sheets.pin`), as Sol and Sonnet said; a coordinated rewrite of pin, snapshots and hashes passes the loader and is refused only by the route arm. New test pins the arm as a whole (each half is redundant for schema 2). The round-1 relabel claimed in `611acc0` had been lost by a mutation-restore; corrected and recorded |
| 15 | 2 | Sol, Sonnet, Space Bunny F3 | Minor | `Badge::Malformed` wording speaks of a cited passage also when an entry cites none | Fixed: "…cannot be read, or the citations of this entry are missing…" and "(source text malformed or citations missing)"; publication doc matches |
| 16 | 2 | Sol, Space Bunny F5 | Minor | Plan §17 decisions 8–9 and the D4 badge table were stale after round 1 | Fixed: §17 decisions 8–9 marked superseded by §18; D4 table updated |
| 17 | 2 | Space Bunny F6, F8, Sol note | Note | Small test gaps: browse badge on the cut rows, create page catalogue section, route-level control for integrity errors | Fixed: cut-row browse badges, create page without catalogue section, `index_identity_damage_never_renders_the_query_problem_form` |
| 18 | 2 | Space Bunny F7 | Note | Browse `unit_states` does not join `sources`; `unit_metadata` does | Fixed: `unit_states` joins `sources`; `a_lost_source_row_gives_the_same_badge_on_both_routes` (red without the join) |

**Accepted author decisions (all reviewers):**
- refusals answer 403, byte-identical to an unreadable sheet (D11 "same shape as today"), rather
  than the literal 404 in the test list; malformed input answers 400 before authentication;
- stale wiring keeps the sheet's catalogues readable (readiness gates creation, not reading);
- retired entries stay listed and readable, with a "Retired" badge;
- a sheet save between the two transactions answers 409;
- `catalogues: []` is refused (one spelling of "no links").

**Recorded, not fixed:**
- n10: up to about 3 × 8 `latest_sealed` lookups per sheet render; not measured.
- n12: the catalogue routes have no rate limit; a search loop delays sheet saves (documented).
- n13: `src/definitions` now depends on `src/catalogues::read`.
- Browse without a type filter sorts up to 6,144 rows per page; the real-route p95 ≤ 250 ms
  criterion has not been measured.
- Round 3 (Sol, Minor): no direct interruption test for the types, cursor and badge-probe statements; the measurement is maximum badge-probe text volume, not an end-to-end p95.
- Step-2 review row 16 (profiler commit-phase regression) remains open.

**Verification.**
- Author gates (plan §17–§18), Monday 2026-10-19 before 09:00 unless noted: fmt, pedantic Clippy,
  `cargo test --locked --all-targets --all-features` (557 → 563 → 566 passed, 0 failed,
  1 pre-existing ignored; round 2 adds the ignored measurement test), ruff, tools unittest (751 run, 1 known error
  `test_run_queue.ProcessLauncherTests.test_fake_runner_script`), release build +
  `browser_smoke.py --auth` (catalogue stage at 1280×720 and 390×844), `smoke_release.py --auth`.
- Reviews were static; Sonnet ran one SQLite timing probe. No reviewer reran the Rust gate or the
  browser check.
- Not run: `cargo audit`, `cargo about`, packaging, unpacked-artifact and LAN smoke, route
  profiling (step 5).
