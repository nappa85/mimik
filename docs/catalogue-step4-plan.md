# Catalogue step 4 plan: `catalogue_ref` and the pick flow

2026-10-19. Status: **draft, not reviewed.** It refines [catalogue-plan.md](catalogue-plan.md)
§3 item 4. The binding items there (D5, D8–D10, R3, R4, R6, R9, R14, R22–R27) stay binding.
This plan maps each one to a design (§3) and a test (§4). Where it proposes to simplify or
defer, §6 lists the change and its risk. Base: master `eece89b`, after step 3 (§17–§18 of the
catalogue plan).

## 1. Goal, user value, scope

**User value.** A player chooses a spell or item for a list row from the game's catalogue,
without JavaScript and without leaving the sheet. The saved sheet shows the entry's current
name, linked to its entry page in a new tab. A row cannot be saved with a key that is not in
the catalogue (Q4).

**Scope.**
- A `catalogue_ref` item-property control, which lifts the D8 gate.
- Its value (a key) and its formula lowering (`Text(64)`).
- The writer's membership check.
- The Browse, Search and Pick draft operations, and the picker panel.
- Cell rendering, the page budget, and the browser and process smoke checks.

**Non-goals** (unchanged from §0 of the catalogue plan):
- no copying of entry stats onto rows, and no `editable: false`;
- no formulas over entry data;
- no rule-filtered choices and no legality warnings;
- no LLM;
- no top-level `catalogue_ref` field;
- no full-text search of descriptions.

The membership check is structural, in the same class as enum options. It is not a legality
check: "assist, don't police" (definition-language-draft.md) still holds. No model is
involved.

**Out of scope for step 4.** Step 5 covers the documentation set, `cargo audit` and `cargo
about`, packaging, the unpacked-archive checks and the LAN smoke. Step 4 changes only the two
sentences that become false: `catalogue-publication.md:8` and
`runtime-definition-contract.md:265`.

**Prerequisite.** The `catalogue-decisions` branch (missing key → 404, refusals → uniform 403,
malformed → 400, all with meaningful messages) merges first. Step 4 branches from master
after that merge and reuses its message mechanism. At `eece89b` that branch has no commits
yet.

## 2. Facts verified against the code (master `eece89b`)

Spec line references that have drifted are marked **(was …)**.

**Values and ops.** `src/campaigns/values.rs`:
- `index` is at `:9`.
- `sheet_keys` is at `:24-85`. It tracks `op` only and builds the exact expected set.
- `Control` match sites:
  - `scalar_schema`, `:87-101` **(was :92)**;
  - `choices` (render units), `:199-205` **(was :201)**;
  - `scalar_value`, `:541-552` **(was :543)**.
- The schema cap is 256 KiB at `:153` **(was :155)**.
- `FORM_LIMITS` is at `:193-197`, with `nodes = MAX_NODES` at `:196`.
- `form_budget` is at `:224-259`.
- `Op` is at `:390-398` and derives `Copy` **(was :389-398)**.
- `Focus` is at `:410-420`. `Op::parse` (`:424-454`) splits on the first `-` only.
- `Op::apply` is at `:459-538`.
- `values::form` (`:614-656`) reads only named keys, so extra admitted keys are ignored.
- `ListDraft::write` (`:363-387`) keeps every key outside the list's own prefixes.

**Transport.** `src/campaigns/transport.rs`:
- `SHEET_FORM_PAIRS` = 65,536 (`:13`).
- `Pairs::admit` requires exact set equality (`:86-96`) **(was :86-94)**.
- The constant is asserted at `:127`; the 65,536-pair linear test is at `:153-161` (spec
  `:158`).
- `FORM_LIMITS.pairs == 65_536` is also asserted at `definitions/publication_tests.rs:507`.

**Web handlers.** `src/campaigns/web.rs`:
- `ECHO_BYTES` = 1 MiB (`:74`).
- Catalogue routes are at `:110-117`. `error()` (`:131-139`) maps `NotFound` to 403.
- `ListView` and `RowView`/`RowFocus` are at `:472-578`.
- `control_name` is at `:580-589` **(R14 said :572)**.
- `render_private` is at `:697-795`. `groups` is at `:797-873`.
- `Notice` is at `:877-896`. `rendered` (`:898-923`) marks 409/422 as `PrivateTrustedEditor`.
- `admit_form` is at `:1027-1047`. `draft_op` is at `:1053-1107`; it has no database phase.
- `apply_save` is at `:1110-1134`.
- In `save_private` (`:1136-1245`):
  - the editable check is at `:1158`;
  - `Op::parse` is at `:1162`;
  - the `ECHO_BYTES` check is at `:1173`;
  - draft ops branch at `:1190`.
  - A writer `Invalid` is re-rendered with a **generic** message (`:1223-1240`), so its
    reason is lost. `apply_save` failures keep their reason (`:1200-1215`).
- `rejected_editor` is at `:1249-1278`.

**Auth layer.** `src/auth/mod.rs:375-405` keeps an error body only for 409/422 with
`PrivateTrustedEditor`. A 404 or 403 body is replaced by the static document. **A 404 cannot
carry a re-rendered draft.**

**Sheets.** `src/campaigns/sheets.rs`:
- `SheetView` is at `:28-39`.
- `private_read` is at `:129-176`: transaction A, then `spawn_blocking` validation, then
  `private_badge`.
- `private_create_context` is at `:204-248`.
- `private_save_keeping` (`:266-305`, `IMMEDIATE`) hands the permit back on `Invalid` and
  `Conflict`.
- `private_badge` is at `:307-358` **(was 309-345)**. It holds:
  - the create branch;
  - the reload with `corresponds` (409);
  - the editability recompute;
  - catalogue `links`, current pages only;
  - readiness;
  - the auth recheck.

**Writer.** `src/campaigns/snapshots.rs`:
- `authorize_read` is at `:271-286`.
- `write` is at `:306-406` **(was :306)**. Its order of checks:
  1. pin and value validation at `:319-326`;
  2. `previous_data` at `:335-359`;
  3. `host_item_ids` at `:363-372`, after the conflict check.
- Freeze callers: `domain.rs:264`.

**Catalogue reads.** In `src/campaigns/catalogue.rs`, `private_catalogue` (`:65-152`)
implements R6/R25 for the GET routes, and `links` is at `:156-177`. `src/catalogues/read.rs`
provides:
- `latest_sealed` (`:42-63`);
- `Badge` with `of_units`, which includes `Malformed` (`:65-136`);
- `BrowseQuery`, `BrowsePage`, `BrowseFailure { Query, Other }` (`:139-213`);
- `browse` and `entry_rows` (`:217-375`), which run under `Budget` and quote searches with
  `fts_query`;
- `unit_states` (`:389-411`).

The bounded key lookup pattern is `key IN (SELECT value FROM json_each(?))`: one bound
parameter, whatever the key count.

**Indexes.** `catalogue_entries` has primary key `(version_id, key)` and the index
`catalogue_entries_browse(version_id, type_id, name, key)` (`migrations/019_catalogues.sql:89`).

**Definitions.**
- `src/definitions/mod.rs`: `Control` (`:157-166`) has no `CatalogueRef`, and `ItemProperty`
  (`:195-202`) has no member.
- `schemas/definition-v1.json`: item properties allow `text|multiline|integer|boolean|enum`
  (`:77`), and fields add `list` (`:41`).
- **The D8 gate today is the schema itself.** No `catalogue_ref` exists anywhere in the code.
- `decode.rs` `limits()` (`:21-38`) caps `catalogues` at (8, 64).
- Item-property checks are in `formula_admission.rs::items` (`:42-82`), with the match at
  `:69-77` (spec `:70-75`).
- `value_type` is at `formula_admission.rs:215-227` (spec `:219`). Top-level `(control,
  options)` is at `admission.rs:119-145` (spec `:120-140`).
- The three D8 sites share `store.rs::linked_catalogues` (`:89-107`):
  - `references` (`:38-86`), at draft save and in the approval snapshot;
  - `linked_catalogues_available`, which feeds `private_evidence_available` at
    `publication.rs:576`.
- `publishable` is at `publication.rs:127-133`.
- `private_pin.rs`: `validate_values` (`:123-148`) matches `field.control` with a wildcard at
  `:139-145` (spec `:139`). `list_values` is at `:97-121`.

**Other sites.**
- `history_diff.rs:304` filters `!= Control::List`, which covers top-level fields only.
  `list_diff` (`:190`) compares property values and names labels only.
- The page-markup assertions are unchanged at `list_tests.rs:2691`, `:2887` and `:3290`. The
  constants `FIXED_PAGE_BYTES` and `UNIT_MARKUP_BYTES` are at `:2389-2390` (spec `:2384-2390`).
- `templates/private_sheet.html:8`: with lists, the **top Save button is the first submit
  control** (`op=save`). `private_list_field.html` renders the row cells.
- `assets/app.css` has no visually-hidden utility yet.

## 3. Design

### 3.1 Definition member, D8 gate, readiness (D8, R14, R26, R27, D5)

- **Types.** Add `Control::CatalogueRef` (serde `catalogue_ref`). Add
  `ItemProperty.catalogue_ref: Option<CatalogueRef { catalogue, entry_type }>`. The member is
  last, with `#[serde(default, skip_serializing_if = "Option::is_none")]`, so frozen vectors
  don't change. Both structs use `deny_unknown_fields`.
- **Schema.**
  - `catalogue_ref` goes into the `item_property` control enum only, never into the
    top-level field enum (`:41`).
  - `$defs/catalogue_ref` is `{catalogue: id, entry_type: id}`, closed.
  - `decode.rs` limits: `"catalogue" | "entry_type" => (256, 64)`.
- **Domain checks** in `formula_admission::items`. Explicit arms replace the wildcard (R26):
  - `(CatalogueRef, no options, Some(member))` is accepted only when:
    - `member.catalogue` is one of `definition.catalogues`;
    - both ids pass `valid_id`.
  - `catalogue-ref-member-required`, when the member is missing;
  - `catalogue-ref-member-only-for-catalogue-ref`, when the member is on another control;
  - `options-only-supported-for-enum`, when a `catalogue_ref` has options;
  - `catalogue-ref-not-linked`, when the catalogue is not one of the definition's links.
  - Per definition, more than 16 `catalogue_ref` properties fails with
    `too-many-catalogue-refs` (D5).
  - `admission.rs:119-145` gets an explicit `CatalogueRef` arm (`catalogue-ref-only-on-list-items`).
    The schema already refuses it there; this is defence in depth.
- **Three-site check (D8, R27).** Extend `linked_catalogues`: for each `catalogue_ref`, the
  latest sealed version of its catalogue must have the entry type in `catalogue_entry_types`.
  - The check is one indexed `EXISTS` per distinct (catalogue, type), at most 16. It verifies
    existence only (R5, R10).
  - Refusal code: `/fields/{f}/items`, `catalogue-ref-entry-type-not-published`.
  - The same function feeds draft save, the approval snapshot and readiness, so all three
    sites follow (§17's red pattern applies).
- **Formulas.** `value_type` gets `CatalogueRef => Text(64)`. `formula_profile` stays 1, and
  the type is never an enum of keys.
- **The D8 gate is lifted** by the schema change. §6 explains why there's no interim gate.

### 3.2 Value schema and lowering (D9)

- In `scalar_schema`, `CatalogueRef` becomes `{"type":"string","minLength":1,"maxLength":64}`.
  There is no key list, so the 256 KiB schema cap is unaffected.
- In `scalar_value`, `CatalogueRef` returns `Value::String(raw)` with explicit messages:
  - an empty value: "Choose an entry for every catalogue property that is set.";
  - a key that fails `valid_id`: "A chosen catalogue entry is not a catalogue key."
- `private_pin::list_values` also checks `valid_id` on `catalogue_ref` values. The check is
  structural, with no database access. It is safe on read because no stored data exists
  before step 4.
- No re-check runs on read, history or freeze (D9: membership is monotonic).

### 3.3 Writer membership check (D9, R9, R27)

- **Location.** `snapshots::write`, immediately after `host_item_ids` (`:363-372`): after the
  revision conflict, for `create` and `edit`, and skipped for `freeze`. It runs for custodian
  and Master writes alike, inside the caller's `IMMEDIATE` transaction.
- **Collection.** It walks the pinned definition's lists. For each `catalogue_ref` property,
  it collects the keys present in `resulting.data`, grouped by (catalogue, entry_type). It
  also records which (list id, property id) held each key in `previous_data`, for the retired
  rule.
- **Bound.** At most 16 properties × 256 rows = 4,096 keys exist in total. Distinct keys are
  deduplicated. More than 4,096 is a defensive `Invalid`, unreachable after schema validation.
- **One shared reader, `catalogues::read::members`.** The cell names (§3.6) and the Pick check
  use it too:

  ```sql
  SELECT key, <guarded name>, retired FROM catalogue_entries
   WHERE version_id=?1 AND type_id=?2 AND key IN (SELECT value FROM json_each(?3))
  ```

  - It runs once per (catalogue, type) group, on the primary key `(version_id, key)`. The
    version comes from `read::latest_sealed(game, edition, catalogue)`.
  - A missing sealed version is `Invalid("A linked catalogue is unavailable.")`. Only damage
    can cause it.
- **Rule.**
  - Every key must be returned for its group. A missing key covers both "not in the
    catalogue" and "wrong entry type".
  - A returned key with `retired = 1` is accepted only when the same (list, property) held
    that key in some row of the previous revision.
  - On create, any retired key is refused.
  - This is not a quantity rule.
- **Error.** `AppError::Invalid` carries a static, meaningful message: "A chosen catalogue
  entry is not in the catalogue, is of another type, or is retired. Choose another entry;
  nothing was saved."
  - `save_private`'s writer-rejection branch (`web.rs:1223-1240`) is changed to show
    `Invalid` reasons, as `apply_save` already does. The draft is re-rendered with 422 by
    `rejected_editor`, so the Q4 422 preserves the draft.
  - The cell markers (§3.6) show which rows are affected, so no formatted message is
    needed.

### 3.4 Ops grammar and form keys (R3, R4, R22, R23, R24)

- **`Op`** drops `Copy` and keeps `Clone + PartialEq`. New variants:
  - `Browse(f, r, p)`;
  - `Search(f, r, p)`;
  - `Pick(f, r, p, String)`.
- **Parsing `Op::parse`:**
  1. Refuse an op longer than 100 bytes.
  2. Split on the first `:`.
  3. Parse the left side with the existing index grammar, plus a third index for the three
     new verbs.
  4. Only `pick` takes a suffix, which must pass `valid_id`. A `:` on any other op, a missing
     suffix on `pick`, or a second `:` answers 400.
  5. The target must be a `catalogue_ref` property `p` of an existing row `r` of list `f`.
     The row count uses the existing `id-f-r` scan. Anything else is `None` (400).
- **Picker state.** Two new keys:
  - `catalogue-q`, at most 100 scalars, trimmed;
  - `catalogue-open`, a canonical `f-r-p` address of a `catalogue_ref` cell of an existing
    row.

  How they are admitted and validated:
  - `sheet_keys` admits them only when the definition has a `catalogue_ref` **and both** are
    submitted. Exactly one of them answers 400 (R23).
  - They are validated after admission, next to `Op::parse`.
  - `Search` and `Pick` require `catalogue-open` to equal the op's address; a mismatch
    answers 400. `Browse` accepts any valid open address, or none, because it opens or
    switches the panel.
  - The two values are never read by `values::form`, so they are never stored. They count
    toward `Pairs`, the pair budget and the `ECHO_BYTES` sum (the existing check at
    `web.rs:1173` already sums every value).
- **`SHEET_FORM_PAIRS`** becomes 65,538 (R22). The assertions at `transport.rs:127` and
  `publication_tests.rs:507` are updated.
- **Closing (R23).** The panel's state is an explicit `Picker` value passed to the template,
  never read back from the raw form. Add, Remove, Up, Down, Pick and every Save re-render
  pass no `Picker`, so the panel is closed and its two keys are gone.
- **Presence (R24).** Pick writes `row-f-r-p = key` and, for an optional property,
  `set-f-r-p = present`.

### 3.5 Draft-op request phases (R6, R25)

The flow is unchanged up to `Op::parse`:
1. parse;
2. CSRF;
3. `request_auth`;
4. `load_editor` (transaction A, `validate`, `private_badge`);
5. `editable`;
6. `admit_form`;
7. `ECHO_BYTES`.

New, only for definitions that have a `catalogue_ref`:
1. **Pure step** (`spawn_blocking`):
   - apply Add, Remove, Up or Down if requested (unchanged code);
   - collect the draft's keys per (catalogue, type);
   - build the lookup request: names, plus a picker query for Browse or Search, plus the
     Pick key.
2. **Transaction B**, `Store::private_catalogue_op` in `campaigns/catalogue.rs`, under the
   same permit:
   - refactor `private_badge`'s body into a shared `boundary(tx, auth, &mut view, create,
     history)`: the create branch, or reload with `corresponds` (409); recompute
     editability; readiness. The draft op uses it as is, then:
   - require `view.editable` (403);
   - for each referenced catalogue, take `latest_sealed`;
   - run `members` for names;
   - run the picker query (§3.7) for Browse or Search;
   - for Pick, check the key with `members` (it must exist, have the type and not be
     retired);
   - recheck auth before commit.
   - A test hook sits between step 1 and transaction B, like `AFTER_CATALOGUE_VALIDATION`.
3. **Pure step**: apply Pick if the check passed.
4. **Render** with the permit, as today.

Status codes of the pick flow, following the `catalogue-decisions` pattern:

| Case | Answer |
|---|---|
| Malformed op, index or address; `:` misuse; op over 100 bytes; bad key grammar; one of the two picker keys alone; `catalogue-open` mismatch; `q` over 100 scalars | **400** with a meaningful message, before any lookup |
| No session or CSRF; no edit rights; revocation between phases | **403**, uniform |
| A concurrent save between transaction A and transaction B | **409**: the draft is re-rendered, with names left out and a "reload" message |
| The query can't run (`BrowseFailure::Query`: no words, control characters, more than 16 pieces, time budget) | **422**: the panel stays open with the static message |
| Pick of a key that isn't offered (unknown, other type, retired, or forged); a catalogue with no sealed version | **422**: the draft is re-rendered with the panel closed, the message "That entry can't be chosen now. Choose another. Not saved yet.", and focus on the cell's Choose button |
| `BrowseFailure::Other` | its `AppError` mapping |

Pick deliberately does **not** answer 404 for a missing key. The auth layer would replace the
body, so the draft would be lost (§2). Retirement between Browse and Pick is a legitimate
race. See open question Q-A.

Save-rejection re-renders (`web.rs:1200-1215` and `:1223-1240`) also call transaction B
(names only), so a 422 cell shows which keys failed. If that call fails, the existing
`rejected_editor` loss paths apply (403 or 503).

### 3.6 Cell rendering (D10)

- **The cell** of a `catalogue_ref` property holds:
  - a hidden `row-f-r-p` input (the key; its only full echo);
  - the shown value:
    - "Nothing chosen" when the key is empty;
    - otherwise `<bdi>`name`</bdi>`, plus a "Retired" badge when the entry is retired;
    - "Not in the catalogue: `<key excerpt>`" when the lookup didn't return the key. The
      excerpt is the existing 64-character helper.
  - the existing optional Set/Not set select;
  - when editable, the button `op=browse-f-r-p`: visible text "Choose…", accessible name
    "Choose `<property label>` for row N of `<list label>`", with `formnovalidate`.
- **Link.** A key equal to the saved value of the same row id, on a current non-create page,
  links to `/private-sheets/{id}/catalogue/{catalogue}/{key}` with `target="_blank"
  rel="noopener"`. Unsaved choices and history pages show the name without a link. History
  pages keep §18's no-links rule.
- **Name lookup.** One bounded `members` lookup over the keys on the page:
  - for saved pages (view and history), in `private_badge`'s transaction, from
    `snapshot.data`;
  - for re-renders, in transaction B, from the form.

  The result is stored in `SheetView.names`, keyed by (catalogue, type, key).
- **Cell badges are deferred** (§6, D-2). The picker and the entry page show the description
  badge, and the cell shows the "Retired" marker only.

### 3.7 Picker panel (D10, R4, R22, R23)

- **Results.** `read::browse` gains two fields in `BrowseQuery`:
  - `limit` (50 for browse, 20 for the picker);
  - `offered_only`, which adds `AND e.retired=0`.

  The picker passes `entry_type = Some(property type)` and no cursor. It reuses:
  - the `(name, key)` order;
  - `fts_query` quoting;
  - `Budget`;
  - the rowid-range join (R20);
  - `Badge::of_units`, which includes `Malformed`;
  - the `BrowseFailure` split.

  There is no second query implementation. Retired entries stay listed and readable in the
  browse route (§17 decision 4); only the picker hides them.
- **Placement.** The panel sits inside the open cell, after its Choose button. It contains:
  - a `<label>`, then the `catalogue-q` field, then a visible Search button
    (`op=search-f-r-p`, `formnovalidate`);
  - a hidden `catalogue-open`;
  - up to 20 results, each a `<button op=pick-f-r-p:key formnovalidate>` labelled
    "Choose `<bdi>`name`</bdi>`", followed by its badge;
  - when a 21st result exists, "More entries match: add words to narrow the search". On saved
    sheets this text also links to the full browse page, in a new tab.
- **Hidden default button.** While a panel is open, the first submit control in tree order is
  a hidden `op=search-f-r-p` button. It is placed right after the `revision` input, before the
  top Save button (`private_sheet.html:8`), and it is:
  - `formnovalidate`, enabled and form-associated;
  - `tabindex="-1"`;
  - hidden with a new clip-based `.visually-hidden` rule in `assets/app.css`, never with
    `aria-hidden`.

  Its text is "Search the catalogue". Enter in any single-line field searches while a panel
  is open; this is documented (R22). With no panel open, the markup is unchanged, so Enter
  saves as today.
- **Focus.** `Focus` gains `Search` (the `catalogue-q` field, after Browse, Search and a query
  problem) and `Choose(f, r, p)` (after Pick and a refused Pick).
- **Status.** `Notice` gains an optional `subject`, rendered `<p role="status"><bdi>subject</bdi>
  suffix</p>`, so a right-to-left name stays isolated. Messages:
  - after Browse: "Choosing `<property label>` for row N of `<list>`: K entries shown. Not
    saved yet."
  - after Search: "K entries match. Not saved yet." or "No entries match. Not saved yet."
  - after Pick: "`<name>` chosen. Not saved yet." (D10).

### 3.8 Budget and markup assertions (D10, R23)

- **`form_budget`:**
  - `choices(CatalogueRef)` is 2, so a cell is 1 (value) + 2 (button and link), plus 2 when
    optional.
  - When a definition has any `catalogue_ref`, the page adds a fixed
    `PICKER_UNITS = 64`: 20 × (button, label, badge), plus the field, label, Search button,
    hidden default, `catalogue-open`, the "more" text and the status line.
  - `pairs` gains 2 in that case (R23).
  - The per-unit byte bound (`UNIT_MARKUP_BYTES` = 2,048) covers a name of 100 `"` escaped to
    500 bytes, so the D10 "byte term" is expressed as `PICKER_UNITS` units. §6 D-3 explains
    this.
- **Markup tests.** `render_units` in `list_tests.rs` mirrors the change. The three
  assertions (`:2691`, `:2887`, `:3290`) get `catalogue_ref` variants:
  - worst labels with 16 `catalogue_ref` properties × 256 rows, and catalogue names and
    labels of 100 `"`;
  - the exact-budget definition with a `catalogue_ref` list, through a Browse re-render with
    the panel open and 20 results;
  - the all-optional variant, with the panel open.
- **Publication.** The budget test is extended: a definition at the unit limit with
  `catalogue_ref` publishes, and one unit more fails with `list-form-exceeds-limit`.

### 3.9 Unchanged

- `history_diff`: values-free. A changed key gives an "edited" line naming the property label,
  never the key or the name.
- Freeze: `validate_for_write` and 32-sheet freezes are not re-checked.
- Readiness: existence only; catalogue state never enters the wiring hash (R10).

## 4. Sub-steps (branch `catalogue-step4`, one commit each, each green)

Every test is written first and shown red: against the pre-change tree, or a narrow mutation
of the final code, recorded in the step-4 notes as in §17. Test names are indicative.

**4a. Definition member and checks.**
- Code: types, schema, decode limits, domain checks, the three-site type check, `Text(64)`,
  and every `Control` arm (`values.rs` three sites, `web.rs:580`, `formula_admission.rs`
  `items` and `value_type`, `admission.rs:119-145`, `private_pin.rs:139`). Value schema and
  `scalar_value` come in 4b; until then their arms return `Invalid`, and tests don't publish.
- Tests:
  - Frozen definition vectors unchanged.
  - Member last, part of the identity.
  - R26 negatives:
    - top-level `catalogue_ref`;
    - missing member;
    - with `options`;
    - member on another control;
    - unlinked catalogue;
    - invalid ids;
    - 17 refs;
    - over-long `catalogue` or `entry_type` at decode.
  - Three sites, each red under its own mutation: the type absent at draft save, at the
    approval snapshot, and at readiness.
  - The type is checked in the latest sealed version only.
  - A formula over `item.spell == "x"` compiles as text.
  - Publishing a catalogue v2 doesn't stale the definition (R10 regression extended).

**4b. Values and the writer check.**
- Code: `scalar_schema`, `scalar_value`, `list_values`, `read::members`, the check in
  `snapshots::write`, and `Invalid` reasons shown on writer rejection.
- Tests:
  - Direct writer calls (R9): unknown key, wrong type, retired key on create, a retired key
    kept on edit (same property, another row) versus newly added, and versus held by a
    *different* property.
  - Master and custodian edits.
  - Conflict precedence: a stale revision with a bad key answers 409.
  - The largest group: 16 × 256 distinct keys in one write, accepted, and one forged key
    among them refused.
  - Colliding keys across two catalogues.
  - Freeze of 32 sheets holding references, with a key retired after the save, unaffected.
  - Forged POST values through the web handler: 422, draft preserved, meaningful message,
    nothing written.
  - Empty and non-`valid_id` values answer 422 with their messages.
  - `history_diff` names the property only.

**4c. Cell rendering on saved pages.**
- Code: `members` in `private_badge`, `SheetView.names`, and the cell template (no Choose
  button yet).
- Tests:
  - Name and link with `target="_blank" rel="noopener"` on the current page.
  - No link on history and create pages.
  - The retired marker.
  - The "Not in the catalogue" marker for a damaged row.
  - Hostile-markup and right-to-left names escaped and `<bdi>`-isolated.
  - A name renamed in v2 shows the current name.

**4d. Ops, picker, phases.**
- Code: `Op` grammar, picker keys, `SHEET_FORM_PAIRS` + 2, transaction B, `offered_only` and
  `limit`, Pick apply and presence, `Focus`, `Notice.subject`, hidden default button, CSS.
- Tests:
  - Op grammar edges (R3): keys with `-`; missing or extra `:`; `:` on Browse, Search, Save,
    Add, Remove, Up or Down (R24); out-of-range f, r and p; a non-`catalogue_ref` property;
    101 bytes.
  - Picker keys (R4, R23): one without the other answers 400; forged `catalogue-open`
    (invalid, not a `catalogue_ref`, nonexistent row, mismatching Search or Pick) answers
    400; `q` of 101 scalars answers 400; both keys absent with the panel closed; neither key
    admitted for definitions without `catalogue_ref`.
  - The query is never stored: Save with the panel open stores no q.
  - Add, Remove, Up and Down close and clear the panel.
  - Picker results: at most 20, non-retired, of the type, `(name, key)` order, badges
    including Malformed; "more" at 21; the query problem answers 422 with the panel open;
    the budget is interrupted.
  - Pick → Save round-trip; Pick into an absent optional sets presence, then Save (R24).
  - Pick of a retired, unknown or other-type key answers 422 with the draft kept.
  - Focus and status: `autofocus` on `catalogue-q` after Browse and Search, on the Choose
    button after Pick; the status text with `<bdi>`.
  - Hidden default: first submit control in tree order when open, `formnovalidate`,
    `tabindex=-1`, no `aria-hidden`; absent when closed.
  - Phases (R6, R25), for Browse, Search and Pick, each red when its guard is removed: the
    permit is held across every phase; revocation between the pure step and transaction B
    answers 403; a concurrent save answers 409 with the draft; a rewritten pin answers 422;
    corrupt pin, hash or pair, and unsupported pins, are refused.
  - The create page: Browse and Pick work before the first save (create branch), with no
    links.
  - A read-only sheet: no Choose button; a forged op answers 403.

**4e. Budget.**
- Code: `form_budget` terms, `PICKER_UNITS`, `render_units`.
- Tests:
  - `form_budget_counts_match_arithmetic` extended.
  - The publication limit ± 1 unit with `catalogue_ref`.
  - The three markup assertions extended (§3.8).
  - The `ECHO_BYTES` sum includes `catalogue-q`.

**4f. Browser, smoke, notes.**
- `tools/catalogue_browser.py` adds a keyboard-only pick at 1280×720 and 390×844:
  1. Tab to Choose, press Enter.
  2. Type into an **empty required Name**, press Enter: it searches and doesn't save (R22).
  3. Tab to a result, press Enter: check the status text and focus on Choose.
  4. Save: check the name link's `target` and `rel`.
  5. In a second sheet with no panel open, Enter in Name saves.

  The run also checks that the hidden default button stays hidden, that there is no overflow
  at either viewport, and that a right-to-left name stays isolated in the cell, the result
  and the status.
- `smoke_release.py --auth`:
  - adds a list with a `catalogue_ref` to the generic definition;
  - runs Browse, Search, Pick and Save over HTTP, then reloads;
  - submits a forged key and expects 422.
- The two doc sentences named in §1.
- Step-4 notes in the catalogue plan: red → green table, gate, and what was not run.

## 5. Risks and open questions

**Risks.**
- **More database work per catalogue op.** A catalogue op runs transaction A,
  `private_badge` and transaction B. The added time is one more short read transaction under
  the single permit. More picker traffic makes the existing `rejected_editor` 503 path (R25)
  more likely. It's recorded, not measured, here.
- **Badge probe cost.** It is bounded to 20 picker rows, which is less than browse's 50, whose
  worst case measured 0.42 s (§18 item 2).
- **Forms with two submit paths.** Enter-to-search applies to every single-line field while
  the panel is open. This is documented, and the browser test covers it.
- **Visible writer messages.** Showing writer `Invalid` reasons (`web.rs:1223-1240`) also makes
  the existing writer messages visible (capacity, identity). They are static strings, but
  existing notice assertions may need updating; each such change is listed in the notes.
- **Ordering dependency.** Step 4 depends on `catalogue-decisions` merging first, because of
  the shared message mechanism and error mapping.

**Open questions for the user:**
- **Q-A.** A Pick of a key that the picker doesn't offer (retired since Browse, unknown, or
  forged) answers **422 and keeps the draft**, not 404. A 404 body is replaced by the error
  document, so the draft would be lost. Confirm.
- **Q-B.** Defer description badges in sheet cells (D-2)? The picker and the entry page
  still show them.
- **Q-C.** Is having no "Close" button on the picker acceptable (D-4)? The panel closes on
  Pick, on any row operation and on Save. A Close button would add a `close-f-r-p` op to R3's
  grammar.

## 6. Requirement map and proposed deferrals

| Requirement | Section | Status |
|---|---|---|
| D5: 16 refs per definition, picker 20, query 100 scalars and 16 pieces | 3.1, 3.4, 3.7 | planned |
| D8: member, item properties only, three-site type check, `Text(64)`, gate | 3.1 | planned; gate per D-1 |
| D9: string 1–64, writer check on save only, 422 keeps the draft, no re-check elsewhere | 3.2, 3.3, 3.9 | planned |
| D10: cell name, link in a new tab, unsaved choice without link, one bounded lookup | 3.6 | planned; cell badge per D-2 |
| D10: Browse and Pick, key checked on Pick and again on Save, focus, status | 3.4, 3.5, 3.7 | planned |
| D10: budget, three markup assertions, no datalist or select | 3.8 | planned; byte term per D-3 |
| R3: property-addressed ops, `:` parse, 100 bytes, `Op` without `Copy`, 400 | 3.4 | planned |
| R4: `catalogue-q`, `catalogue-open`, admitted only with `catalogue_ref`, never stored, counted | 3.4 | planned |
| R6 / R25: phases, second boundary extends `private_badge`, create branch, permit threaded | 3.5 | planned (shared `boundary` fn) |
| R9 / R27: in `snapshots::write` after `host_item_ids` and conflict; grouping; ≤ 4,096; retired rule; freeze skipped; readiness = sealed and type exists | 3.1, 3.3 | planned |
| R10: catalogue state outside wiring | 3.1, 4a | regression extended |
| R14 / R26: every `Control` site explicit, negatives | 3.1, 4a | planned (current lines in §2) |
| R15: formula literals unchecked (contract note) | — | step 5 docs |
| R22: `formnovalidate`, hidden default first in tree order, clip style, pairs + 2, Chromium | 3.4, 3.7, 4f | planned |
| R23: both keys or neither, close and clear, budget pairs | 3.4, 3.8 | planned |
| R24: Pick sets presence; only Pick takes `:` | 3.4 | planned |
| R28: requests use `validate_for_write` where nothing is evaluated | 3.5 | planned (transaction B runs no formulas) |
| §17 / §18: reuse `read.rs`, Badge including Malformed, `BrowseFailure`, two-transaction pattern | 3.5, 3.7 | reused, not duplicated |
| User 2026-10-19: 404, 403, 400 with meaningful messages | 3.5 | planned; Q-A for Pick |

**Proposed deferrals and simplifications** (for the reviewers to judge):

| # | Proposal | Justification | Risk |
|---|---|---|---|
| D-1 | No interim D8 gate in `publishable`. The schema stays the gate on master, and the branch merges only after step 4's review | The gate's purpose is "a half-integrated control can't reach sheets". Branch isolation meets it, and an interim gate would block 4b–4e's own tests | If 4a merged alone, `catalogue_ref` would publish without the writer check. Mitigation: merge only the whole step |
| D-2 | No description badge in sheet cells, only the "Retired" marker. Badges stay in the picker and on the entry page | A badge needs the per-unit probe. Up to 4,096 keys × 16 units (each up to 256 KiB) can't fit the page budget. A cheaper badge that skips the malformed scan could show "Reviewed" wrongly, against §18's fail-closed rule | Users don't see "Unchecked" at a glance on the sheet. Low: it is one click away, and the picker shows it at choice time |
| D-3 | D10's per-page byte term is expressed as `PICKER_UNITS` within the existing unit bound. `FormCounts` gets no new byte field | Names are at most 100 scalars, so at most 500 escaped bytes, within `UNIT_MARKUP_BYTES`. The `catalogue-q` echo is already in `ECHO_BYTES` | The test assertions, not a type, carry the bound. They are extended in 4e |
| D-4 | No Close button | Keeps R3's grammar. Pick, row ops and Save close the panel | Keyboard users keep Enter-to-search until they pick or act (Q-C) |
| D-5 | No new profiling: the real-route p95 and picker-induced 503 rates are not measured | POC first (user, 2026-10-19). The step-3 open item stays open | Unknown latency at 8 catalogues × maximum lists. Bounded by `Budget` (1 s) and by indexed lookups |
| D-6 | The 422 save message is static; per-row feedback comes from the cell markers | `AppError::Invalid` is `&'static str` | None for correctness |

Not deferred: confinement through the pin's links and pair, the membership check on save,
the two-transaction boundary with rechecks, Askama escaping with `<bdi>`, no JavaScript,
`formnovalidate`, and the exact-key admission.

## 7. Verification and gate

- **Plan review** before code: Sol (`--chain main`) and a fresh Sonnet in parallel, plus the
  third chain. Blocking findings are adjudicated against the code. The record goes in
  `docs/reviews/2026-10-xx-catalogue-step4-plan.md`.
- **Machine schedule.**
  - Weekdays 09:00–18:00: containers with `--cpus 2` and `CARGO_BUILD_JOBS=2`.
  - Full-gate runs with `--cpus 6`: outside that window only.
- **Rust gate per sub-step:**
  - `cargo fmt --check`;
  - `cargo clippy --all-targets --all-features -- -D warnings` (pedantic);
  - `cargo test --locked --all-targets --all-features`.
- **Tools:** `ruff check tools`, `ruff format --check tools`, and the tools unittest suite.
  The known `test_fake_runner_script` error is reported, not hidden.
- **At 4f:**
  - the release build;
  - `browser_smoke.py --auth` in `mimik-browser` with the CI flags (`--network=none
    --cpus=2`), at both viewports, with the keyboard-only pick;
  - `smoke_release.py --auth` with pick, against that binary.
- **Implementation review** of the step-4 change set (Sol and Sonnet, plus the third chain),
  then a re-review of the fixes. The record goes in `docs/reviews/`, the summary in `TODO.md`.
- **Step 5 keeps:** `cargo audit`, `cargo about`, packaging and the unpacked-archive checks,
  the LAN smoke, the full docs set (R15, R16), and the real-route profiling (D-5).
- Unrun checks are reported as not run.
