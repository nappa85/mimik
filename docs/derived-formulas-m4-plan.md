# Derived-field formulas: milestone M4 plan (server-rendered list editing, browser and package gates)

2026-10-16. Status: **draft, awaiting independent plan review** (AGENTS.md). Produced by a
read-only planning subagent; not implemented. Base: master `46fd889` (M3 merged). The
worktree `../mimik-wt/formula` starts with `git merge --ff-only master`. Parent plan:
[derived-formulas-plan.md](derived-formulas-plan.md) §6, §7, §9, §14.1 item 18, §16. Pattern:
[M3 plan](derived-formulas-m3-plan.md) §10–§10d.

## 1. Current state (verified)

**Value schema and pin checks**

- `values::list_schema` (`src/campaigns/values.rs:26-49`) builds an array with
  `maxItems = formula::MAX_LIST_ITEMS` (256, `formula/mod.rs:32`):
  - required `id` (a 1–64 character string) plus the declared properties;
  - `required` holds `id` plus the non-optional properties;
  - `additionalProperties:false`; no null inside items.
- `private_pin::list_values` (`src/definitions/private_pin.rs:95-121`) checks that item IDs
  are registry slugs (`registry.rs:41-47`: `[a-z][a-z0-9-]{0,63}`), that they are unique per
  list, and that item integers are exact i64.
  - `validate_values` calls it (`:124-149`), after the 1 MiB profile-pin cap (`:127-131`).
- Item IDs are not host-managed: the writer `snapshots::write`
  (`src/campaigns/snapshots.rs:260-311`) accepts any valid slug, so a client can rename or
  reuse IDs.

**Forms and transport**

- `values::form` refuses every list field ("List editing is unsupported until milestone M4.",
  `values.rs:107-111`, `:137`).
- Integers go through `values::integer` (`:85-95`): exact decimal i64, with no leading zero
  and no `+`.
- Form transport (`src/campaigns/transport.rs:21-58`):
  - at most **516** pairs, keys of at most **70** bytes, no duplicate keys;
  - `admit` requires the key set to match exactly;
  - `tests.rs:1036-1052` pins the 516 limit.
- Body limits: 1 MiB on sheet routes (`web.rs:76`) and 8 KiB on the rest (`:86`).
- Worst case: 256 rows × (1 + 32 + 32) keys = 16,640 pairs per list. Lists therefore cannot
  fit under 516.

**Save and render**

- `save_private` (`web.rs:773-880`):
  - builds the fixed `value-<id>`/`null-<id>` key list (`:793-814`) and admits it (`:815`);
  - in `spawn_blocking`, runs `values::form`, then `validate_values`, then the revision check
    (`:824-833`);
  - on success, returns 303; Invalid/Conflict re-render as 422/409 through
    `rejected_editor`, which keeps the permit (`:882-911`).
- `groups()` (`web.rs:563-640`) maps `Control::List` to `"list"` (`:602`). The template has
  no list branch, so a list renders through the generic `<input name="value-<list>">` else
  branch (`templates/private_sheet.html:8`).
  - **Latent bug**, analogous to M3's: a list sheet published through the test path renders
    a control that cannot save.
- `render_private` (`web.rs:471-553`) evaluates only the saved snapshot. `FormulaMode::Draft`
  labels results "Computed from saved revision N" (`:423-443`).
- The page has no JS and one bottom submit button, "Save full private sheet"
  (`private_sheet.html:9`).

**Formula admission and publication**

- `formula_admission::items` (`src/definitions/formula_admission.rs:42-81`):
  - lists are never nullable and never a formula;
  - list and property IDs are CEL identifiers, and the property `id` is reserved;
  - no nested lists; valid enum choices.
- The decoder caps lists at 32 properties and definitions at 256 fields (`decode.rs:19,23`).
- Derived fields cannot be lists, so derived values over lists are scalars computed by the M3
  path. `ImmutablePin::evaluate` passes list values through, because list IDs are always
  declared (`private_pin.rs:162-191`).
- `publication::publishable` (`src/definitions/publication.rs:136-152`) refuses lists with
  `list-editing-unsupported`.
  - The test-thread bypass `publish_unshipped_for_tests` (`:10-23`, re-exported at
    `definitions/mod.rs:16`) has one caller, `campaigns/formula_tests.rs:629`, which seeds
    item IDs `i-{n}` (`:643`).
  - `publication_tests.rs:188-217` asserts the refusal.

**History**

- `private_history.html` is a list of revision links. `/history/{rev}` reuses
  `private_sheet.html` read-only, with controls disabled. There is no diff view.
- The "history addressing" deferred by §14.1 item 18 is the `{list, entry_id, field}` address
  of parent §6 (`derived-formulas-plan.md:296-300`). Only the Python benchmark tools use it
  (`tools/tool_loop_benchmark.py:587-591`).

**Pack**

- `examples/tool-loop-demo/definition.json` has two lists:
  - `equipment`: name, kind enum, quantity, weight, equipped, and optional armor and
    category;
  - `conditions`: kind and stacks.
- `bram-holt` has 5 equipment and 4 condition rows, with pack IDs (`ringshirt-1`, …). No pack
  expression reads `item.id`.
- `formula_pack_tests.rs:37-108` translates the pack into an envelope with `"sources": []`,
  as far as a pin and without a database.
- **New input (user decision, 2026-10-16):** item stats are not editable. The pack revision
  will mark the equipment fields `armor`, `category`, `kind` and `weight` as non-editable; see
  [adjudication](tool-loop-pack-adjudication-2026-10-16.md). M4 must therefore handle
  non-editable list properties (§2 D11).

**Browser and smoke**

- `tests/support/source_fixture.rs:93-132` holds the scalar formula fixture, written as
  `formula-definition.json` (`:86-89`).
- `smoke_release.py:943-984` publishes it as the second version, and the generic-form step
  picks `versions[-1]` (`:1130`). Adding a third version of the same game would silently
  switch it.
- `browser_smoke.py:441-517` holds the scalar formula stage, run at 1280×720 and 390×844
  (`:1129`).
- The CI browser job runs with `--network=none --cpus=2` (`ci.yml:114-120`).

## 2. Design decisions

**D1. Form encoding: positional keys plus a hidden ID per row.**

For list field index `f` (its index in `definition.fields`, stable because the pin is
immutable), row `r` (0-based, contiguous) and property index `p`:

| Key | Meaning |
|---|---|
| `id-f-r` | Hidden. The saved item ID, or empty for a new draft row. |
| `row-f-r-p` | The value of property `p`. |
| `set-f-r-p` | Optional properties only: a select, `present` or `absent` (Q2). |

- The longest key is `set-255-255-31` (14 bytes). Raw IDs would break the 70-byte cap and
  put user text into names.
- Row order is ascending `r`, and it is stored as submitted.
- **Admission:**
  - `n_f` is the number of `id-f-*` keys;
  - the expected set is the scalar keys, plus all row keys for `0..n_f`, plus an optional
    `op`;
  - a gap, a missing key, an extra key, or an `id-` key on a non-list index → 400;
  - `Pairs::admit` gains an optional-key slice;
  - cap `SHEET_FORM_PAIRS = 65_536` through a new `Pairs::parse_limited`. Other routes keep
    516.

**D2. Host-managed IDs, enforced in the writer and generated deterministically.**

- New rows get `r<N>-<k>`, where N is the revision being written and k = 1, 2, … in row order
  within that list.
- New `snapshots::write` invariant (runtime create and edit): every item ID absent from the
  previous revision's list must be exactly `r<new revision>-<k>`. Consequences:
  - existing IDs cannot be renamed into fresh ones;
  - a removed ID never returns, because its revision is in the past;
  - two drafts on the same revision collide only on the revision check.
  - Freeze is unchanged.
- `values::form` reconciliation:
  - an empty ID gets a generated ID;
  - a non-empty ID must exist in the saved list, else 422 (409 when the revision is stale);
  - duplicates → 422.
- Reconciliation runs after `form` and before `validate_values`, so the scalar order is
  unchanged.

**D3. Add, remove and reorder are draft operations (Q1, Q3).**

- Per row: "Move up", "Move down" and "Remove row R". Per list: "Add <List label> row".
- All are submit buttons with `name="op"` and values `add-f`, `remove-f-r`, `up-f-r` and
  `down-f-r`.
- An op POST:
  - parses the draft into `ListDraft` (raw strings, never validated, so half-typed rows
    survive);
  - applies the op;
  - re-renders with **200** in `FormulaMode::Draft` (never evaluated), with "Row added to
    Equipment. Not saved yet.";
  - never writes, but still runs CSRF, the `load_editor` authorization, `editable` (403
    otherwise) and the JOBS permit.
- `op=save`, or no op, saves.
- Errors:
  - an out-of-range or unknown op → 400;
  - Add at 256 rows → 422 with a message; the Add button is disabled, with "limit reached"
    text.
- An op on a stale revision warns: "The saved sheet changed (revision N → M); saving this
  draft will be refused."

**D4. Implicit submission (Enter) must save.**

- Browsers submit with the first submit button, so list sheets render a "Save full private
  sheet" (`op=save`) button **before** the groups, as well as at the bottom.
- Test: the first `<button` inside the form is `op=save`.
- JS-free progressive enhancement: op buttons get `formaction="<action>#list-f"`, so the page
  scrolls to the affected list.

**D5. Limits, each with one owner.**

| Limit | Owner |
|---|---|
| 256 rows | `MAX_LIST_ITEMS` in `values::form` and the schema |
| i64 | `values::integer` on input, `list_values` in storage |
| slug and uniqueness | `list_values` |
| text length | schema (8,000 / 20,000) plus `maxlength` |

- Sheet-route body limit: 1 MiB → **5 MiB** (Q4). Stored JSON is ≤ 1 MiB; URL encoding expands
  it ≤ 3×; keys add ≤ ~1.3 MiB.
- Publication refuses `list-form-exceeds-limit` when the worst-case pair count exceeds
  65,536.
  - The count is 4 + 1 + Σ scalar (1–2) + Σ lists 256 × (1 + props + optional props).
  - It is a pure function in `campaigns::values`, called by `publishable` in the gate-lift
    commit.
- One body at a time: the `runtime` layer takes the permit first.

**D6. Stale and concurrent edits.**

- A stale save → 409, and the re-render keeps the draft rows.
- An unknown ID at the current revision is tampering → 422; at a stale revision → 409.
- Last writer loses, never merges.

**D7. History (Q5).**

- `/history/N` shows each row as a read-only fieldset with the ID in the legend ("Row 2 · ID
  r1-2"). It has no op buttons and no top Save.
- With D2's never-reused IDs, `<list>/<entry_id>/<property>` is a stable address. It is
  documented in `runtime-definition-contract.md`.
- No diff view in M4.

**D8. Derived values over lists.**

- No engine change. The view shows the M3 states. Draft and op re-renders show the saved
  revision's results in the separate M3 block.
- `item.id` reaches formulas as the generated ID.

**D9. Markup, accessibility and reflow.**

- A list is `<fieldset class="list-field" id="list-f"><legend>Label</legend>`. Each row is a
  nested `<fieldset class="list-row" id="row-f-r">`, with the legend "Row R of N · ID x" or
  "New row · ID assigned on save".
- Every control has a `<label for>`. Optional properties have a labelled Set/Not set select.
  Required boolean and enum selects start on "Select a value".
- Button names are visible text including the row number (WCAG 2.5.3).
- "No rows yet." marks an empty list. Status messages use the existing `role="status"`.
- No tables; rows stack. CSS adds `fieldset{min-width:0}` and a flex-wrap button row.
- Read-only history disables each control (M3 L1).

**D10. Escaping.**

- Askama defaults; no `|safe`.
- Test payloads (`<script>`, `"`, `'`) go into list labels, property labels, enum option
  labels, item text, the status message and `formaction`.
- Assert the escaped form is present **and** the raw form absent, in text and attribute
  contexts, on four pages: view, op re-render, 422 re-render and history.

**D11. Non-editable list properties** (new, from the user's item-stats decision).

- A property marked `editable: false` renders as read-only text in existing rows and is not
  submitted.
- New rows take its value from where the item comes from. The pack has no item catalogue, so
  for M4 a non-editable property is either set when the row is created and immutable
  afterwards, or the whole list is append-only from an approved source. **Q7.**
- The writer rejects any change to a non-editable property of an existing item (409/422).

## 3. Questions for the user (recommended default first)

1. **Op semantics:** (a) add, remove and move re-render an unsaved draft, and Save persists;
   or (b) each op saves immediately as a new revision.
2. **Optional properties:** (a) an explicit Set/Not set select; or (b) empty means absent,
   with a select only for text.
3. **Reorder in M4:** (a) Move up and down buttons; or (b) defer.
4. **Large forms:** (a) a 5 MiB body, a 65,536-pair cap and `list-form-exceeds-limit`; or
   (b) keep 1 MiB and document 413.
5. **History:** (a) ID-labelled rows plus the address grammar, with no diff; or (b) a
   per-revision change summary in M4.
6. **ID format:** (a) deterministic `r<revision>-<k>`; or (b) random slugs.
7. **Non-editable properties on new rows:** (a) set once at row creation, then immutable; or
   (b) new rows only from an approved item list (a catalogue, which is future work).

## 4. Failing-first test list (in order)

"Guard" means the test passes when written. Every other test is run red first. Route tests
publish through the test bypass until step 6.

| # | File | Test | Red because |
|---|---|---|---|
| 1 | `src/campaigns/list_tests.rs` (new) | `writer_refuses_client_chosen_new_item_ids` | writer accepts |
| 2 | same | `writer_refuses_reintroducing_a_removed_item_id` | writer accepts |
| 3 | same | `writer_accepts_host_ids_and_per_list_reuse` | guard |
| 4 | same | `freeze_preserves_list_ids` | guard |
| 5 | `src/campaigns/values.rs` tests | `list_rows_parse_in_order_and_new_rows_get_host_ids` | `form` refuses lists |
| 6 | same | `optional_property_state_is_explicit` | same |
| 7 | same | `item_integers_use_exact_i64_text` | same |
| 8 | same | `unknown_id_is_invalid_when_current_conflict_when_stale`; `duplicate_row_ids_refused`; `257_rows_refused` | same |
| 9 | `src/campaigns/tests.rs` | `sheet_routes_admit_list_keys_exactly`; `sheet_pair_cap_is_65536` | no list keys / cap 516 |
| 10 | `list_tests.rs` | `list_sheet_rendered_form_round_trips` | renders `value-<list>`, 422 |
| 11 | same | `list_rows_render_as_labelled_fieldsets` | same |
| 12 | same | `failed_save_rerender_keeps_draft_rows` | same |
| 13 | same | `stale_list_save_conflicts_and_keeps_draft` | same |
| 14 | same | `largest_list_sheet_round_trips` | 413/400 |
| 15 | same | `add_row_rerenders_without_saving` | no ops |
| 16 | same | `remove_and_move_rerender_without_saving` | no ops |
| 17 | same | `add_at_256_rows_refused_with_message`; `unknown_or_out_of_range_op_is_400` | no ops |
| 18 | same | `first_submit_button_saves` | single bottom button |
| 19 | same | `ops_need_csrf_and_edit_rights`; `op_on_stale_revision_warns`; `ops_never_evaluate_the_draft` | no ops |
| 20 | same | `create_page_adds_rows_then_creates` | no ops |
| 21 | same | `list_history_shows_rows_with_stable_ids` | no rows rendered |
| 22 | same | `derived_values_over_lists_update_after_save` | not saveable |
| 23 | same | `list_text_is_escaped` | not rendered |
| 24 | existing native tests | MÖRK BORG view and save unchanged | guard |
| 25 | `publication_tests.rs:188` renamed | list definition publishes; oversized → `list-form-exceeds-limit` | `list-editing-unsupported` |
| 26 | `list_tests.rs` | `lantern_reach_list_sheet_end_to_end` (real publication of the translated pack) | refused by the gate |
| 27 | `tools/browser_smoke.py` | list stage negative control | — |
| 28 | `tools/test_smoke_release.py` | workspace form chosen by version ID, not `versions[-1]` | picks last |
| 29 | `list_tests.rs` | `non_editable_item_properties_render_read_only_and_reject_changes` (D11) | properties editable |

## 5. Commit-sized steps (each warning-clean; no `pub(crate)` item without a caller; no `#[allow]`)

1. **Writer invariant** (tests 1–4). Rewrite the `i-{n}` seeds to `r1-{n+1}`.
2. **List save and render** (tests 5–14, 29):
   - `ListDraft`;
   - `Pairs::parse_limited` and optional-key `admit`;
   - dynamic keys and the 5 MiB limit;
   - the template list branch and CSS;
   - remove the M2 refusal strings.
3. **Draft ops** (tests 15–20): op dispatch, top Save, `formaction`, messages, the 256 limit,
   the stale warning.
4. **History, derived values and escaping** (tests 21–24).
5. **Docs that do not claim publication:** contract (address grammar, ID format, optional
   state, non-editable properties), sheet presentation and campaign workspace.
6. **Gate lift last** (tests 25–28):
   - remove `list-editing-unsupported` and add `list-form-exceeds-limit`;
   - **delete** `publish_unshipped_for_tests` and move its callers to real publication;
   - add the list fixture `list-definition.json`, with a derived `load` sum, a formula reading
     `item.id` and the constraint `load <= 10`;
   - `smoke_release.py` publishes a third version and selects it by ID;
   - add a `browser_smoke.py` list stage at both viewports: add, fill, Enter saves, IDs,
     keyboard Move down, remove, a violation, read-only history, no script, `no_overflow`
     with a 3,000-character note;
   - flip the scope docs.
7. **Review record** with a red→green table.

Mutations for reviewers:
- writer invariant skipped;
- reconcile accepts unknown IDs;
- first button is not Save;
- the op path writes;
- the draft is evaluated;
- `|safe` on item text;
- the pair cap reverted to 516;
- the gate restored;
- `set-` ignored;
- a non-editable property accepted.

## 6. Container gates

The same `mimik-ci` gate set as M3 §8:
- fmt;
- strict clippy;
- the full test suite;
- release formula tests;
- the `formula_eval` build;
- `source_fixture`;
- ruff and unittest;
- actionlint;
- `cargo about`;
- a release build against a re-measured current baseline;
- `cargo audit`, which is the only networked run.

After step 6:
- `cargo package --offline` and verify;
- `cargo vendor` and the release package;
- checksums, unpack and binary equality;
- `smoke_release.py --auth`;
- the `mimik-browser` run with CI flags;
- the fresh offline rebuild with an empty `CARGO_HOME`;
- `lan_smoke.py`.

Also:
- record a 256-row render time (report-only);
- `--cpus 2` on weekdays 09:00–18:00 and while GPU benchmarks run;
- report every gate not run as not run.

## 7. Risks

1. Implicit submission fires a row op. Covered by D4, test 18 and the browser Enter check.
2. Form and page size. Bounded by the permit and measured.
3. Positional keys depend on the field index. Safe only because pins are immutable; document
   it.
4. A 200 response to an op makes a browser reload ask to resubmit. Harmless; document it.
5. The writer invariant constrains future import and tool-loop writers, which must re-ID
   through the host. Intended.
6. More POSTs contend for JOBS = 1, so more 503s.
7. The smoke `versions[-1]` selection. Test 28.
8. The test bypass must be removed at the lift.
9. Fieldset quirks on mobile. Checked by `no_overflow` at 390 px.
10. Scope creep into history diffs.
11. The third reviewer was unavailable in M3. Record it again if that is still true.
