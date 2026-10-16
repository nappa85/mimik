# Review record: derived formulas M4 plan

2026-10-16. Scope: [derived-formulas-m4-plan.md](../derived-formulas-m4-plan.md) revision 1
(draft committed in `9c79436`), reviewed against master `1ddbf17`. The plan was revised in place
to revision 2. Documentation only: no code, builds, tests or containers were run for this record.

## Independence

- Planner: a read-only Opus 5.5 subagent. Reviser and adjudicator: a separate Opus 5.5 subagent,
  which checked every finding against the code read-only (`decode.rs`, `transport.rs`, `web.rs`,
  `private_sheet.html`, `snapshots.rs`, `private_pin.rs`, `publication.rs`,
  `definition-v1.json`, `smoke_release.py`, `browser_smoke.py`, `definitions/mod.rs`,
  `auth/mod.rs`, `formula_tests.rs`, the pack).
- Main reviewers, in parallel with fresh contexts:
  - OpenAI Sol (`openai/gpt-6.1-sol`, `--chain main`): **NO-GO**. Six blocking, five medium and
    one low finding. Static read-only review.
  - Sonnet 5.5 subagent: **GO-with-amendments**, R1–R15 plus mutation gaps.
- Third reviewer (`--chain third`): **unavailable.** The preflights for Space Bunny, DeepSeek
  v4.1 Flash and Muse Spark all failed (the Muse attempts timed out; chain exit 3). Per
  `AGENTS.md`, the review proceeds with the two main reviews.
- User decision received during review (2026-10-16): M4 ships plain list editing with every list
  property editable. A later milestone adds a per-property `editable: false` flag and an item
  catalogue. D11, Q7 and test 29 are removed, and plan §8 is added.

## Findings, verdicts and dispositions

Verdicts: valid; valid-partial (the core is right, and a part is wrong or moot); wrong.

| ID | Reviewer | Sev. | Verdict and evidence | Disposition |
|---|---|---|---|---|
| B1 | Sol | Blocking | **Valid.** `ItemProperty` is `deny_unknown_fields` with no `editable` (`definitions/mod.rs:173-182`); the schema's `item_property` has `additionalProperties:false` (`definition-v1.json:72-79`); the translator drops `editable` (`formula_pack_tests.rs:54-69`), although the pack marks `conditions.kind` non-editable. | D11, Q7 and test 29 removed (user decision); §8 lists the full flag contract for the later milestone. |
| B2 | Sol | Blocking | **Valid.** New-row provenance of immutable stats has no source; the adjudication needs a rule or `ask_rule` (`tool-loop-pack-adjudication-2026-10-16.md:112-121`). | Deferred with the catalogue (§8). M4 claims neither immutability nor an acquisition workflow (Scope, §8). |
| B3 | Sol | Blocking | **Valid.** `encode` checks only the byte count (`snapshots.rs:50-56`, reserve `:326-334`); `decode` uses `parse_sheet` with a 32,768-node cap (`decode.rs:38`). Four lists × 256 rows × 32 integers = 4 × (1 + 256 × 34) = 34,820 nodes at about 250 KB, under the old 65,536-pair gate. | D7: `encode` re-parses its own output and the reserve (→ 422, atomic); the publication node budget shares `decode::MAX_NODES`; the cap is not raised. Tests 5, 6, 30; mutation "writer decode check removed". |
| B4 | Sol | Blocking | **Valid.** `admit` runs `keys.contains` per pair (`transport.rs:53`), O(n·m), on the async worker (`web.rs:815`). The parse visitor already uses a `BTreeSet`, so only admission is quadratic. | D2: `HashSet` admission inside the existing `spawn_blocking`; `n_f ≤ 256` bounded by an index scan first; canonical index grammar. Tests 7–9; mutations "reverted to `contains`", "row gap". |
| B5 | Sol | Blocking | **Valid.** The admitted maxima (`formula/mod.rs:37-40`; schema 128 options) allow 2 lists × 32 enum properties × 128 options × 256 rows ≈ 2.1 M `<option>`s. The old pair cap did not count options. | D7 render-unit budget (`RENDER_UNITS = 32_768`) in `publishable`; §6 records bytes, time and permit hold for the worst admitted GET, op and 422 pages. Test 30. |
| B6 | Sol | Blocking | **Valid.** Name is `required` (`private_sheet.html:6`), so an op button without `formnovalidate` never POSTs on an empty create page. | D5 `formnovalidate` on op buttons only; test 23; browser stage "Add on an empty create page before Name" (step 7); mutation "`formnovalidate` dropped". |
| M1 | Sol | Medium | **Valid.** `values::form` and `validate_values` run before the revision check (`web.rs:821-831`), so a stale invalid draft gets 422. | D4: explicit precedence; the revision check moves first (scalar and native too; documented change); re-renders keep the submitted revision. Tests 10, 17, 22; mutations "revision check moved back", "re-render rebases". |
| M2 | Sol | Medium | **Valid.** The plan left `k` and the scope undefined; freeze reuses the writer (`domain.rs:264`). Tool and import writers are out of scope, as Sol says. | D3: `k` counts newly allocated rows in row order, contiguous; per list and sheet; `checked_add`. Tests 1–4 (create and edit, wrong k, reintroduction, mixed, swaps, departure and removal freeze, stale direct write), 31 (pack re-ID, derived results unchanged). |
| M3 | Sol | Medium | **Valid.** Revision 1 step 6 combined the lift with the fixture, browser and docs work; M3 §10b.17 requires the lift last, after docs and browser. The bypass has three parts (`publication.rs:12-23`, `:140-143`). | §5: docs and smoke preparation in step 6; step 7 is the lift alone, committed only after the full §6 set, the smoke and both browser viewports pass on the uncommitted candidate; all bypass parts deleted. |
| M4 | Sol | Medium | **Valid.** `submitted` ignores buttons and disabled controls (`formula_tests.rs:794-827`); many negatives were red only because lists were unsupported. | §4 preamble: paired positive baselines, and the helper submits exactly one clicked submitter. Tests 7, 19 (`two_ops_is_400`), 30 (at-limit guard); mutations for no-op ops, rebasing and dropped permits. Read-only reconstruction is moot (B1 deferred). |
| M5 | Sol | Medium | **Valid-partial.** Context-specific witnesses and the two-Save locator clash are right (exact-name locators at `browser_smoke.py:461`, `:484`, …). The `formaction` part is moot: it is built from numeric indices only, and putting hostile text into it was a revision 1 error. | D11: witnesses in real contexts, with DOM-parsed assertions (`scraper` dev-dependency, licence and audit check); `formaction` asserted equal to the generated path. D6: the top Save only on list sheets, so scalar locators are unchanged; list locators are scoped. |
| L1 | Sol | Low | **Valid.** All five corrections confirmed: the heading is in the template (line 10); the cap is profile-conditional (`private_pin.rs:127-131`); the `None` path reacquires a permit (`web.rs:896-901`); the pack test is at `src/definitions/formula_pack_tests.rs`; `versions[-1]` is at `smoke_release.py:1132`. | §1 rewritten; D8 address uses list, item and property IDs, scoped to the sheet and pin. |
| R1 | Sonnet | Blocking | **Valid** (same as B1). | §8, user decision. |
| R2 | Sonnet | Medium | **Valid.** IDs label slots: content can be overwritten and hidden IDs swapped. | D3 states slot semantics for the contract; test 3 pins the accepted swap; item identity goes to §8. |
| R3 | Sonnet | Medium | **Valid.** | D3: the invariant applies to the runtime writer, `create` and `edit` only, and on create all IDs must be `r1-k`. Tests 1 and 4. |
| R4 | Sonnet | Medium | **Valid.** | D1 grammar and D2 step 1 bound indices before key generation. Test 7 (`id-01-0`, `id-+1-0`, `f` beyond the fields, `r=256`, 70/71 bytes). |
| R5 | Sonnet | Medium | **Valid.** The 1 MiB snapshot cap is `snapshots.rs:52`. | D7: derivation of 5 MiB, POST create and edit only; tests 15 (413, GET unchanged) and 16 (stored over 1 MiB → 422 with the draft). |
| R6 | Sonnet | Medium | **Valid-partial.** Scroll and focus needed definition (valid). `no-store` is already global: `auth::normalize` sets it on every response (`auth/mod.rs:426-429`). | D5: fragment `formaction` plus `autofocus`; test 23; test 18 guards `no-store`. |
| R7 | Sonnet | Medium | **Valid.** | D4 step 4 and D5: ops never validate IDs, the warning is advisory, and no saved IDs are revealed. Tests 21 and 22. |
| R8 | Sonnet | Medium | **Valid.** | Test 20 parses the first submitter in tree order, including `input type=submit`; mutation "top Save has the wrong `op`". |
| R9 | Sonnet | Medium | **Valid.** | D11 covers the hidden-ID echo, option labels, invalid echoes and every attribute context on the op, 422 and 409 pages. |
| R10 | Sonnet | Medium | **Valid.** Strict clippy denies `dead_code`. | §5: every step names each new item's production caller. `form_budget` lands with `publishable` in step 7. The lift and the bypass deletion are in one commit, with all callers moved. |
| R11 | Sonnet | Low | **Valid.** The gate means no shipped list sheets or history exist. | §1 states it; D8 defines the address grammar textually; test 25. |
| R12 | Sonnet | Low | **Valid-partial.** Edge buttons are moot under the Q2 default (no reorder); focus and the 32-property mobile layout are valid. | D5 focus; the fixture's 32-property list and the 390 px `no_overflow` (steps 6–7). |
| R13 | Sonnet | Low | **Valid.** | Step 6 selects by version ID and logs it before the third publish; test 29. |
| R14 | Sonnet | Low | **Valid** (a recommendation). | Q2 default (a): defer reorder. The browser stage is trimmed accordingly. |
| R15 | Sonnet | Low | **Valid-partial.** The §4 column now names the failing assertion; generic "no ops" entries name the absent behaviour, not one assertion line. | The step 8 red→green table must record each exact failing assertion. |
| Mut. | Sonnet | — | **Valid.** Gaps: duplicate ID in reconcile, wrong `k`, op skipping CSRF, admission with a row gap, 5 MiB on GET, top Save with a wrong value. | All six added to the §4 mutation list. |
| A1 | Adjudicator | Medium | **New, valid.** `auth` replaces 4xx documents with an error page unless the response carries `PrivateTrustedEditor` (`auth/mod.rs:249-266`); `rendered` adds it only for 409 and 422 (`web.rs:663-668`). An Add-at-256 response sent outside `rendered` would lose the draft. | D5: the Add-at-256 422 goes through `rendered`; test 19. |

## Wrong findings

None of the findings was wrong. Two parts are rejected: Sol M5's hostile-text-in-`formaction`
item, because the attribute is built only from numeric indices (replaced by an equality
assertion), and Sonnet R6's `no-store` requirement, which the auth layer already meets (kept
as a guard).

## Questions

Revision 2 renumbers the user questions. The old Q1 (ops) stays **Q1**. The old Q3 (reorder)
becomes **Q2**, with the recommended default flipped to "defer", per R14. The old Q4 (limits)
becomes **Q3**, and the old Q5 (history) becomes **Q4**. The old Q2 (explicit optional state)
and Q6 (deterministic IDs) are decided, because neither carries a product trade-off. The old Q7 is
removed.

## Status

The plan is **not approved for execution**. It needs a short re-review of revision 2 (Sol and
Sonnet, plus the third chain if available), and user answers to Q1–Q4 or acceptance of the
defaults. Review findings are not proof of correctness.
