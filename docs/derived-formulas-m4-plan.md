# Derived-field formulas: milestone M4 plan (server-rendered list editing, browser and package gates)

2026-10-16. Status: **revision 2, amended after review (Sol NO-GO, Sonnet GO-with-amendments,
third unavailable); needs a short re-review before execution.** Review record:
[reviews/2026-10-16-derived-formulas-m4-plan.md](reviews/2026-10-16-derived-formulas-m4-plan.md).
Not implemented. §1 re-verified read-only against master `1ddbf17`; the worktree
`../mimik-wt/formula` starts with `git merge --ff-only master`. Parent plan:
[derived-formulas-plan.md](derived-formulas-plan.md) §6, §7, §9, §14.1 item 18, §16. Pattern:
[M3 plan](derived-formulas-m3-plan.md) §10–§10d.

**Scope (user decision 2026-10-16):** M4 ships **plain list editing**. Every list property the
current schema declares is editable. Non-editable properties and the item catalogue are a later
milestone (§8). Section 3 lists the remaining user questions; the design uses their recommended
defaults until answered.

## 1. Current state (verified)

- **Values.** `values::list_schema` (`src/campaigns/values.rs:26-49`): array of ≤ 256 items
  (`MAX_LIST_ITEMS`, `formula/mod.rs:32`), each with a required 1–64 character `id`, the
  non-optional properties required, `additionalProperties:false`, no null.
  `private_pin::list_values` (`private_pin.rs:97-121`) requires unique registry-slug IDs and
  exact i64 integers. The 1 MiB cap in `validate_values` (`:127-131`) applies only to pins with
  a `formula_profile`. `ItemProperty` (`definitions/mod.rs:173-182`, `deny_unknown_fields`) and
  the schema (`definition-v1.json:72-79`) have no `editable` member.
- **Writer and decoder.** `snapshots::write` (`snapshots.rs:260-345`) accepts any valid slug
  as an item ID. It checks the schema and the encoded size (`encode`, `:50-56`, 1 MiB, also for
  the frozen reserve at `:326-334`), **not** the decoder limits. Reopening uses
  `decode::parse_sheet` (`snapshots.rs:111-114`): a **32,768-node** cap (`decode.rs:38`), and
  256 keys per data object and 256 elements per array (`:80-112`).
- **Transport.** `values::form` refuses lists (`values.rs:107-111`, `:137`); `values::integer`
  (`:85-95`) takes exact decimal i64. `transport::Pairs` (`transport.rs:13-60`): ≤ 516 pairs,
  keys ≤ 70 bytes, no decoded duplicates. `admit` (`:50-59`) uses `keys.contains` per pair, so
  it is O(n·m), and it runs on the async worker (`web.rs:815`); only `parse` is in
  `spawn_blocking` (`:784-788`). Body limits: 1 MiB for all sheet routes (`web.rs:76`), 8 KiB
  elsewhere (`:86`).
- **Save.** `save_private` (`web.rs:773-880`) validates the values and **then** checks the
  revision (`:830`), so a stale invalid draft answers 422. `rejected_editor` (`:882-911`)
  re-renders with the save's own permit, except on the `None` path, which reacquires a permit
  and can answer 503 without the draft (`:896-901`). `rendered` marks only 409/422 responses
  `PrivateTrustedEditor` (`:663-668`).
- **Render.** `groups()` maps lists to `"list"` (`:602`). `private_sheet.html` has no list
  branch, so a list renders as an unsaveable `value-<list>` input (latent bug). The page has no
  JS, Name is `required` (line 6), there is one bottom "Save full private sheet" button
  (line 9), and the "Computed from saved revision N" heading is in the template (line 10). The
  test helper `submitted` (`formula_tests.rs:794-827`) ignores submit buttons and disabled
  controls.
- **Publication.** `publishable` (`publication.rs:139-152`) refuses lists. The test bypass has
  three parts: the thread-local `UNSHIPPED_FOR_TESTS` (`:12-17`), `publish_unshipped_for_tests`
  (`:19-23`, re-exported at `definitions/mod.rs:16`; one caller, `formula_tests.rs:629`, which
  seeds IDs `i-{n}`) and the early return (`:140-143`). `publication_tests.rs:188-217` asserts
  the refusal. **No shipped database holds a list sheet.** The admitted maxima are 256 fields,
  32 lists, 32 properties and 128 options (`formula/mod.rs:37-40`).
- **History and pack.** `/history/{rev}` reuses the sheet template read-only and has no diff.
  The `{list, entry_id, field}` address (parent §6) is used only by
  `tools/tool_loop_benchmark.py:587-591`. The pack has `equipment` (name, kind enum of 8,
  quantity, weight, equipped, optional armor, optional category enum of 4) and `conditions`
  (kind enum of 4, marked `editable: false`, and stacks). The translator
  (`src/definitions/formula_pack_tests.rs:37-108`) drops `editable`. No expression reads
  `item.id`.
- **Smoke.** `smoke_release.py:943-984` publishes two versions, and the generic step picks
  `versions[-1]` (`:1132`). `browser_smoke.py` locates "Save full private sheet" by exact name
  (`:461`–`:661`). The CI browser runs with `--network=none --cpus=2` (`ci.yml:114-120`).

## 2. Design

**D1. Form encoding.** For list field index `f` (its index in `definition.fields`, stable
because pins are immutable), row `r` (0-based, contiguous) and property index `p`:
- `id-f-r` is hidden: the saved item ID, or empty for a new draft row;
- `row-f-r-p` is the value of property `p`;
- `set-f-r-p` is `present` / `absent` (optional properties only);
- `op` comes only from the clicked submitter: `save`, `add-f` or `remove-f-r`.

Indices use the canonical grammar `0|[1-9][0-9]{0,2}`. `f` must name a list field, `r < 256`,
and `p` must be below that list's property count. The longest key, `set-255-255-31`, is 14 bytes.
Rows are stored in ascending `r`, as submitted.

**D2. Admission: O(n), set-based, off the async worker.**
- `Pairs::parse_limited(bytes, limit)` replaces the hard-coded 516: `parse` keeps 516, and sheet
  POSTs use `SHEET_FORM_PAIRS = 65_536`. The decoded-duplicate and 70-byte checks stay.
- Admission moves into the existing `spawn_blocking` closure (`web.rs:818`). Only the CSRF
  lookup, one O(n) scan, stays async.
  1. One scan of the `id-` keys parses their indices. A bad grammar, a non-list `f`, `r ≥ 256`
     or a gap → 400. This bounds `n_f ≤ 256` **before** any key is generated.
  2. The expected keys go into a `HashSet<String>`: the scalars, the rows `0..n_f`, and an
     optional `op`.
  3. The form is admitted only if the counts match and every pair is in the set; otherwise → 400.
- `Pairs::admit` takes that set, so every route (`web.rs:201`, `save_private`) shares one path.

**D3. Host-managed IDs, enforced by the writer (decided: deterministic format).**
- A new row's ID is `r<N>-<k>`: N is the revision being written (`checked_add`), and k = 1..m
  numbers the **newly allocated** rows of that list in row order. The scope is one list of one
  sheet. Writer invariant (`snapshots::write`, kind `runtime`, `create` and `edit` only; `freeze`
  already requires identical data): every ID absent from the previous revision's list (empty on
  create) must be `r<new revision>-<k>`, with k contiguous from 1. So there are no client-chosen
  IDs, and a removed ID never returns.
- IDs label **row slots, not item identity**. Overwriting every property of a row, or a forged
  swap of hidden IDs between rows, is accepted and documented in the contract. Item identity
  belongs to §8. Future import and tool-loop writers must re-ID through the host.
- `values::form` reconciliation: an empty ID gets the next `r<N>-<k>`; a non-empty ID must exist
  in the saved list at the (by then current) revision; duplicates → 422.

**D4. Status precedence (all runtime and native sheet saves).** In order:
1. 413 body limit; 400 parse (pair cap, key length, duplicates).
2. 403 CSRF; authorization (`load_editor`); 403 if not `editable`.
3. 400 admission (D2), op grammar and op range.
4. An op (a non-integer `revision` → 400) → 200 draft re-render (D5); Add at 256 rows → 422.
   No value, ID or revision-match validation.
5. Save: a malformed `revision` → 422; **revision ≠ current → 409**. This now runs before values,
   so a stale draft with invalid values or unknown IDs answers 409. That is a deliberate change
   from `web.rs:830`, applied to scalar and native sheets too, and documented in the contract.
6. 422: `values::form`, reconciliation (an unknown ID at the current revision is tampering),
   `validate_values`.
7. Writer: 409 for a concurrent writer; 422 for an undecodable snapshot (D7) or the ID invariant.
- Every re-render (op, 409, 422) keeps the **submitted** hidden revision; nothing rebases a draft.

**D5. Draft operations (Q1 default: Add and Remove re-render, Save persists; Q2 default:
no reorder).**
- The buttons are `<button name="op" value="add-f" formnovalidate>`, "Add <List label> row",
  and `remove-f-r`, "Remove row R of <List label>". `formnovalidate` stops the `required` Name
  and half-typed rows from blocking an op; Save keeps browser validation.
- An op parses the draft into `ListDraft` (raw strings, never validated), applies itself and
  re-renders with **200** in `FormulaMode::Draft` (never evaluated). `auth::normalize` already
  sets `no-store` on every response (`auth/mod.rs:426-429`; test 18 guards it). The Add-at-256
  422 goes through `rendered`, so it carries `PrivateTrustedEditor` and is not replaced by the
  error document. The page shows a `role="status"` message ("Row added to Equipment. Not saved
  yet."). It never writes, never reveals saved IDs the draft lacked, and still needs CSRF,
  authorization, `editable` and the JOBS permit.
- At 256 rows the Add button is disabled with "limit reached" text. If the revision differs from
  the current one, an advisory warning reads "The saved sheet changed (revision N → M); saving
  this draft will be refused."
- `formaction="<action>#list-f"` (trusted numeric indices only) scrolls to the list. `autofocus`
  goes to the new row's first control after Add, and to the list's Add button after Remove.

**D6. Implicit submission.** List sheets also render a "Save full private sheet" (`op=save`)
button **before** the groups, so Enter saves (the default button is the first submitter in tree
order). Scalar sheets keep a single button and their browser locators; list-stage locators are
scoped.

**D7. Limits and the publication budget (Q3 default).** Owners: rows, `MAX_LIST_ITEMS`
(schema, `values::form`, D2); i64, slugs and uniqueness, `values::integer` and `list_values`;
text length, the schema (8,000 / 20,000) plus `maxlength`; stored bytes, `snapshots::encode`
(1 MiB); decodability, `snapshots::encode`; worst-case pairs, nodes and render units,
`values::form_budget` called by `publishable`.
- **Writer decodability (Sol B3).** `encode` re-parses its output with `decode::parse_sheet`.
  On failure it returns `Invalid("Private snapshot exceeds the sheet decoder limits.")` → 422,
  atomically. The frozen reserve is checked the same way, so an active sheet can always freeze.
  The decoder cap is not raised.
- **Publication budget.** `form_budget` assumes full lists with every optional property present.
  `publishable` refuses any count over its limit with `list-form-exceeds-limit`:
  - pairs = 5 + Σ non-derived scalars (1, or 2 if nullable) + Σ lists 256 × (1 + props +
    optional props) ≤ `SHEET_FORM_PAIRS`;
  - stored-JSON nodes = `ENVELOPE_NODES` + 1 + Σ non-derived scalars + Σ lists (1 + 256 × (2 +
    props)) ≤ `decode::MAX_NODES` (the 32,768 literal becomes a shared const). `ENVELOPE_NODES`
    bounds the snapshot's non-data nodes and is pinned by a boundary test: a definition exactly
    at the budget publishes, and its maximal snapshot saves, reopens and freezes. The writer
    check stays authoritative;
  - render units (Sol B5) = Σ scalars (1 + choices, + 2 if nullable) + Σ lists (4 + 256 × (4 +
    Σ props (1 + choices, + 2 if optional))) ≤ `RENDER_UNITS = 32_768`. Choices are the enum
    options + 1, or 3 for a boolean. A unit is one control, option or row element with its
    label, so its markup is bounded (labels ≤ 100 characters, escaped).
  - The pack is about 10,000 units, 3,400 nodes and 3,400 pairs. Four lists × 256 rows × 32
    integers is 34,820 nodes, about 250 KB of JSON, and is refused.
- **Body limit 5 MiB, on the create and edit POST routes only**; GETs keep 1 MiB. Stored values
  are ≤ 1 MiB, URL encoding expands them ≤ 3×, and keys add ≤ 65,536 × 16 bytes: ≤ 4 MiB, plus
  margin. A save over 1 MiB stored (or over the profile cap) → 422 with the draft kept. A page
  is ≤ units × per-unit markup + 6 × the echoed input; JOBS = 1 renders one at a time.

**D8. History (Q4 default).** `/history/N` shows each row as a read-only fieldset, legend
"Row 2 · ID r1-2", disabled controls (M3 L1), no op buttons and no top Save. The
contract defines the address textually as `<list id>/<item id>/<property id>`, scoped to one
sheet and its immutable pin. It uses IDs, never form indices. No diff view.

**D9. Derived values.** No engine change. Derived fields cannot be lists. Op and rejected
re-renders show the saved revision's results in the M3 block. `item.id` reaches formulas as
the host ID.

**D10. Markup and accessibility.** A list is `<fieldset class="list-field" id="list-f">` with
its label as the legend. Each row is a nested `fieldset id="row-f-r"`, legend "Row R of N · ID x"
or "New row · ID assigned on save". Every control has a `<label for>`; IDs are unique per page.
Optional properties get a labelled Set/Not set select (decided: explicit state), and required
boolean and enum selects start on "Select a value". Button names are visible text with the row
number (WCAG 2.5.3). An empty list shows "No rows yet.". No tables: rows stack, with
`fieldset{min-width:0}` and a flex-wrapping button row.

**D11. Escaping.** Askama defaults; no `|safe` (the source check stays). Witnesses go in their
real contexts: `</textarea>` in item text; `"` and `'` in attribute values (the hidden-ID echo,
text `value=`, invalid enum and Set/Not set echoes); `<script>` in list, property and option
labels and in the status message. These run on the view, op, 422, 409 and history pages. Tests
parse the DOM with the maintained `scraper` crate (a dev-dependency only, adopted after a
recorded licence, audit and `cargo about` check). They assert no injected elements or
attributes, and exact text and attribute values. `formaction` must equal the generated
same-origin path plus `#list-f`.

## 3. Questions for the user (recommended default first; the design uses it)

1. **Q1 Op semantics:** (a) Add and Remove re-render an unsaved draft, and Save persists; or
   (b) each op saves a new revision.
2. **Q2 Reorder:** (a) defer to a later milestone; rows keep their order (Sonnet R14); or (b) Move
   up and down buttons in M4, which adds `up-f-r`/`down-f-r` ops, edge-button rules and
   browser keyboard steps.
3. **Q3 Large forms:** (a) a 5 MiB POST limit plus the three-count budget (D7); or (b) keep 1 MiB,
   use a smaller budget and document 413.
4. **Q4 History:** (a) ID-labelled rows and the address grammar, no diff; or (b) a per-revision
   change summary in M4.

Decided by the review, with no product trade-off: explicit Set/Not set for optional properties,
deterministic `r<N>-<k>` IDs, and the D4 precedence.

## 4. Failing-first tests (in order)

"Guard" = passes when written; every other test runs red first with the stated failing
assertion, after a paired positive baseline on the same fixture passes (so it is red for the
intended reason). The `submitted` helper (rebuilt on `scraper` in step 5) includes exactly the
clicked submitter's `name=value` and excludes disabled controls and other buttons. Route tests
publish through the bypass until step 7.

| # | File | Test | Red because (failing assertion) |
|---|---|---|---|
| 1 | `campaigns/list_tests.rs` (new) | `writer_refuses_client_chosen_new_ids` (create and edit) | `write` returns Ok |
| 2 | same | `writer_refuses_removed_id_reintroduced`; `…_wrong_or_noncontiguous_k` (`r2-2` alone; `r2-2,r2-1`) | Ok |
| 3 | same | `writer_accepts_mixed_retained_new_rows_per_list_reuse_and_swaps` | guard |
| 4 | same | `departure_and_removal_freeze_keep_arbitrary_valid_ids`; `stale_direct_write_conflicts` | guard |
| 5 | same | `writer_refuses_undecodable_snapshot_atomically` (4 lists × 256 × 32 integers) | Ok; reopen then fails |
| 6 | same | `decoder_bound_snapshot_writes_reopens_and_freezes` (3 lists) | guard |
| 7 | `campaigns/tests.rs` | `list_row_keys_admit` baseline, then `…_rejects_gaps_aliases_overflow_substitution` (`id-01-0`, `id-+1-0`, non-list `f`, `r=256`, same-count swap, `%2D` duplicate, 70/71 bytes) | baseline 400 (no list keys) |
| 8 | same | `sheet_post_pair_cap_is_65536_other_routes_516` | cap 516 |
| 9 | same | `admission_of_65536_pairs_is_linear` (debug, < 5 s) | quadratic `contains` |
| 10 | same | `stale_invalid_draft_conflicts` (scalar, native, list) | 422 ≠ 409 |
| 11 | `values.rs` tests | `list_rows_parse_in_order_new_rows_get_host_ids`; `optional_state_is_explicit`; `item_integers_exact_i64`; `unknown_or_duplicate_ids_refused`; `257_rows_refused` | `form` returns the M2 refusal |
| 12 | `list_tests.rs` | `list_sheet_rendered_form_round_trips` (303, revision + 1) | renders `value-<list>`, 422 |
| 13 | same | `list_rows_render_as_labelled_fieldsets_unique_ids` | no `fieldset.list-row` |
| 14 | same | `failed_save_rerender_keeps_draft_and_submitted_revision` | same |
| 15 | same | `largest_budget_list_sheet_round_trips`; `body_5mib_plus_1_is_413`; `get_body_limit_unchanged` | 413 at 1 MiB |
| 16 | same | `stored_over_1mib_is_422_with_draft` | same |
| 17 | same | `competing_writer_loser_unchanged`; `op_409_op_save_never_rebases` | no ops |
| 18 | same | `add_rerenders_200_no_store_without_writing`; `remove_rerenders_without_writing` | no ops |
| 19 | same | `add_at_256_is_422_with_message`; `bad_op_or_index_is_400`; `two_ops_is_400` | no ops |
| 20 | same | `first_submitter_in_tree_order_is_save` (parsed, includes `input type=submit`) | single bottom button |
| 21 | same | `ops_need_csrf_and_edit_rights`; `stale_op_warns`; `ops_never_evaluate_draft`; `op_never_reveals_saved_ids` | no ops |
| 22 | same | `unknown_id_draft_op_rerenders_then_save_422` (current) and `409` (stale) | no ops |
| 23 | same | `op_buttons_have_formnovalidate_and_formaction`; `autofocus_after_add_and_remove` | absent |
| 24 | same | `create_page_adds_rows_then_creates` | no ops |
| 25 | same | `list_history_rows_have_stable_ids_and_no_ops` (incl. the test 6 snapshot); `history_address_grammar` | no rows |
| 26 | same | `derived_values_over_lists_update_after_save` | not saveable |
| 27 | same | `list_payloads_escaped_in_context` (D11, five pages) | not rendered |
| 28 | existing native and scalar tests | view, save and browser locators unchanged | guard |
| 29 | `tools/test_smoke_release.py` | form chosen by version ID and logged, not `versions[-1]` | picks last |
| 30 | `publication_tests.rs:188` renamed | list definition publishes; each budget count over → `list-form-exceeds-limit`; the same fixture at the limit publishes (guard) | `list-editing-unsupported` |
| 31 | `list_tests.rs` | `lantern_reach_list_sheet_end_to_end` (real publication; pack IDs re-ID'd to `r1-k`; derived results unchanged) | refused by the gate |
| 32 | `tools/browser_smoke.py` | list stage with a negative control | — |

Mutations for reviewers (each must be killed): writer invariant skipped; wrong `k` accepted;
writer decode check removed; reconcile accepts an unknown or duplicate ID; admission accepts a
row gap; admission reverted to `contains`; the pair cap reverted to 516; 5 MiB applied to
GETs; the revision check moved back after values; the op path writes; the draft is evaluated;
op skips CSRF; `formnovalidate` dropped; the first button is not Save; the top Save has the wrong
`op`; `set-` ignored; `|safe` on item text; a dropped kept permit; a re-render that rebases the
hidden revision; an op that silently does nothing; a budget term removed; the gate restored.

## 5. Commit-sized steps

Every commit passes fmt and strict clippy (`all` + `pedantic`, `-D warnings`) with no `#[allow]`.
Every new item lands with its production caller (named below), and shipped behaviour stays
unchanged until step 7, because lists cannot be published.

1. **Writer** (tests 1–6). `snapshots::host_item_ids` (private; caller `write`); the
   `parse_sheet` round check in `encode` (caller `write`). Seeds `i-{n}` → `r1-{n+1}`.
2. **Transport and precedence** (tests 7–10). `Pairs::parse_limited` (callers: `parse`, and
   `save_private` for sheet POSTs); set-based `Pairs::admit` (callers: the form helper at
   `web.rs:201` and `save_private`); `values::sheet_keys`
   (caller `save_private`, inside `spawn_blocking`). The revision check moves first. List rows
   are admitted and then refused by `form` until step 3.
3. **List save and render** (tests 11–16, 20). `ListDraft` and `values::reconcile` (callers
   `values::form`, and `groups`/`render_private`); template list branch and CSS; top Save on list
   sheets; 5 MiB layer on the create and edit POST routes; the M2 refusal strings removed.
4. **Draft ops** (tests 17–19, 21–24). `values::Op` parse and apply (caller `save_private`);
   `formnovalidate`, `formaction`, `autofocus`, messages, the 256 limit, the stale warning.
5. **History, derived values, escaping** (tests 25–28). History legends (caller
   `render_private`); the `scraper` dev-dependency and the rebuilt `submitted` helper.
6. **Docs and smoke preparation** (test 29). The contract gains the ID format and slot
   semantics, the address grammar, the optional state, D4 precedence, op behaviour and limits.
   The sheet-presentation and campaign-workspace docs are updated, but none of them claims list
   publication. `smoke_release.py` selects by version ID and logs it. `source_fixture.rs` writes
   `list-definition.json`: a derived `load` sum, a formula reading `item.id`, `load <= 10`, and a
   second list of 32 properties for 390 px. Its caller is a bypass route test.
7. **Gate lift, its own final commit** (tests 30–32):
   - `values::form_budget`, `ENVELOPE_NODES` and `RENDER_UNITS` (caller `publishable`);
     `decode::MAX_NODES` (callers: the decoder and `form_budget`);
   - `list-editing-unsupported` removed;
   - **deleted:** `publish_unshipped_for_tests`, its re-export, `UNSHIPPED_FOR_TESTS` and the
     early return; its callers move to real publication;
   - the smoke publishes a third version;
   - a `browser_smoke.py` list stage at both viewports: Add on an empty create page before Name
     (B6); fill, Enter saves; IDs; Remove; an op keeps an invalid row value; a violation; read-only
     history; no script; focus after ops; `no_overflow` with a 3,000-character note and the
     32-property row;
   - the scope docs flipped.

   Steps 1–6 are committed with their gates green. Step 7 (Sol M3, as M3 §10b.17) is assembled
   in the worktree. The **full §6 set, the smoke run and both browser viewports run on the
   uncommitted candidate**, and it is committed only when all pass, with the tree hash recorded.
   Any later fix re-runs the set.
8. **Implementation review** (Sol + Sonnet + the third chain) and a record with a red→green
   table (executed red, compile red, guards, mutations). Not complete while a blocker is open.

## 6. Container gates

The `mimik-ci` set of M3 §8: fmt, strict clippy, full tests, release formula tests, the
`formula_eval` build, `source_fixture`, ruff and unittest, actionlint, `cargo about` (including
`scraper`), a release build against a re-measured baseline, and `cargo audit` (the only
networked run). On the step 7 candidate, also: `cargo package --offline` plus verify;
`cargo vendor` and the release package; checksums, unpack and binary equality;
`smoke_release.py --auth`; `mimik-browser` with the CI flags; the fresh offline rebuild with an
empty `CARGO_HOME`; and `lan_smoke.py`. Record the bytes of the worst-budget GET, op and 422 pages (asserted ≤ the D7
bound), plus their time and permit hold (report-only). Use `--cpus 2` on weekdays 09:00–18:00.
Report every gate not run as not run.

## 7. Risks

1. Enter fires an op (D6, test 20, browser Enter check). Size is bounded by the D7 budget and
   measured.
2. Positional keys rely on immutable pins. Reloading after an op asks to resubmit, which is
   harmless (`no-store`, no writes).
3. JOBS = 1 contention means more 503s. The M3 rejected-save `None` path can still lose a draft
   on 503 (`web.rs:896-901`, documented).
4. D4 changes a scalar and native corner case (tested and documented). The writer invariant
   binds future writers (§8). The bypass must be fully deleted (mutation check).
5. Scope creep into reorder, diffs or the catalogue (§3, §8). Third reviewer: retry at
   implementation review.

## 8. Deferred to the item-catalogue milestone

- A per-property `editable: false` flag (Sol B1):
  - the schema and an `ItemProperty` field whose default keeps existing canonical bytes and
    hashes;
  - translator support and read-only rendering;
  - exact key sets for existing versus new rows;
  - reconstruction by saved ID, including optional absence;
  - writer-enforced immutability.
- An **item catalogue** in the definition. A new row picks an item and gets its fixed stats,
  under an authority the host checks. This settles new-row provenance (Sol B2) and the pack's
  acquisition rule (`ask_rule`, `tool-loop-pack-adjudication-2026-10-16.md:112-121`). Item
  identity beyond slot IDs (D3) belongs here too.
- Until then, every list property is editable, including pack `conditions.kind` and the
  equipment stats. The M4 pack test claims neither item-stat immutability nor a working
  acquisition workflow.
- Also deferred: reorder (if Q2 stays (a)), history diffs, and the import and tool-loop writers.
