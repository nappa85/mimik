# Assisted sheet entry and adaptive sheets

How runtime (definition-driven) private sheets accept, record and present what the player
types. Decision IDs are in `docs/decisions.md`; remaining work in `docs/assisted-sheet-plan.md`.
None of this applies to native MÖRK BORG sheets: authenticated native private sheets are plain
full-form POST pages without scripts; only the `--no-auth` prototype's native editor uses htmx.

| Concern | Code |
|---|---|
| Number grammar | `src/campaigns/entry.rs` |
| Reading cells, recording notes | `src/campaigns/values.rs` (`Recorder`, `scalar`, `reconcile`) |
| Note values, ops, addresses, storage | `src/campaigns/notes.rs`, `migrations/020_sheet_notes.sql`, `021_row_notes.sql`; list units `src/campaigns/rows.rs` |
| Switch and Undo | `src/campaigns/assist.rs`, `src/campaigns/assist_web.rs` |
| Per-unit save | `src/campaigns/unit.rs`, `src/campaigns/unit_web.rs` |
| Writer, rechecks | `src/campaigns/sheets.rs` (`private_save_keeping`, `private_unchanged`, `private_read_base`) |
| Rendering | `src/campaigns/web.rs`, `templates/private_*.html`, `assets/app.css` |
| Unknowns in formulas | `src/definitions/formula/convert.rs` |
| Demo | `tools/assist_demo.py`, `assist_definition()` in `tests/support/source_fixture.rs` |

## Number entry

Integer cells (scalar fields and list properties, on create and edit) read input with
`entry::read`. Protocol integers (revisions, `base`, note keys) and native sheets keep the
strict `values::integer`.

```text
int   = DIGIT+                 ; ASCII digits only; leading zeros allowed ("007" = 7)
sign  = "+" | "-" | "−"        ; U+2212 accepted
term  = ws* sign ws* int
entry = [sign] int *term       ; a leading sign must touch its digits
ws    = space | tab | U+00A0 | U+202F
```

- Checks: over `MAX_CHARS` (200) characters → `TooLong`; a control character other than tab
  → `Control`; after trimming `ws`, empty → `Blank`; grammar → `Syntax`; over `MAX_SCALARS`
  (64) numbers → `TooManyScalars`.
- Terms are added left to right in checked `i128`; a term or running sum that overflows
  `i128`, or a result outside `i64`, is `Range`. Intermediate values may leave `i64`.
- Three readings: a **literal** ("84"), an **absolute sum** ("84 -15" = 69, "84-15+3" = 72),
  and a **sign-led** entry ("-15", "+3", or the sign-led sum "-5+3" = −2, V3). A sign-led
  entry is always applied as its own value (D-Q).
- Unreadable: "84--15", "84+-2", "- 5", "5 -", a lone sign, "1 2", "5.", "1e2", full-width
  digits, other Unicode spaces (U+2003, U+2009), words, `*` and `/`.
- `Blank` → unknown; `Syntax`, `TooManyScalars`, `Range` → a **scribble** (`keeps_value`);
  `TooLong`, `Control` → 422 with the draft kept (a note could not hold them faithfully).
- Runtime integer inputs have no `inputmode`, so phone keyboards keep minus and space. An
  empty integer control has `placeholder="?"`.

**Expression display** (`entry::sum_text`, `entry::offer_text`): magnitudes without leading
zeros, minus as U+2212 ("84 − 15"). Terms are kept while their text fits `SHOWN_CHARS` (64);
the rest are counted, never cut inside a number: "84 − 11 − 11 … (+52 more terms)". The first
piece and the result are always shown in full.

## Change notes and scribbles

Table `private_sheet_notes(sheet_id, revision, address, op, raw, before, after, bundle,
target)`, primary key `(sheet_id, revision, address)`, foreign key to the history row, index
`(sheet_id, address, revision)` for the current note (migration 020, rebuilt by migration
021 to admit row addresses; `SCHEMA_VERSION` 21).

- **Scope:** integer, enum and boolean cells only. Text, multiline, Name and General notes get
  no notes; history holds every revision of them.
- **Addresses:** `field:<id>`, `cell:<list>/<host-id>/<property>` and `row:<list>/<host-id>`
  (a removed row; the data part is `history_diff::address`). The table also admits `meta:…`,
  which nothing writes yet. CHECK: 6–256 bytes, no NUL, one of the four prefixes, each with a
  non-empty rest.
- **Ops:** `literal`, `sum`, `sign_literal`, `switched`, `unresolved`, `erased`, `reverted`
  are written; `confirmed` is reserved for model proposals. `bundle` is reserved; `target` is
  the revision an Undo reverted.
- **Typed values** (`NoteValue`), canonical JSON ≤ 80 bytes (`MAX_VALUE_BYTES`):
  `{"absent":true}` (optional property not set, distinct from unknown), `{"unknown":true}`,
  `{"int":"-15"}` (decimal string), `{"enum":"<id>"}`, `{"bool":true}`, `{"row":"present"|
  "absent"}` (a row's removal and its Undo). The decoder accepts only canonical text. `raw` is
  what was typed: ≤ 200 characters, ≤ 800 bytes, no NUL.
- **When a note is written** (`Recorder`): every value change of a saved in-scope cell;
  Known ↔ Unknown and Set ↔ Not set as `literal` with empty `raw`; an `unresolved` scribble;
  an `erased` blank. New cells (create, new row) get a note only for an interpreted sum or an
  unreadable entry; a new-row form writes at most one per property (`MAX_ROW_NOTES`, 32).
  Removing a row writes one `literal` row note (`row present` → `row absent`). The current
  note of an address is its latest note.
- **Scribbles:** an unreadable entry keeps the saved value (number or unknown); in a new row,
  a newly present property or on create the cell becomes unknown. The `unresolved` note holds
  the text. The control echoes the scribble in full, so re-posting the page is a no-op; the
  same scribble again writes nothing. Entering a value equal to the kept one writes a
  `literal` note and a data-unchanged revision, which clears the scribble.
- **Integrity (D-T):** inserted after `snapshots::write` in the save's `IMMEDIATE`
  transaction; any failure rolls back the revision too. Triggers abort UPDATE, DELETE and a
  replacing insert; an insert must name the sheet's live, active `create`/`edit` revision and
  run under the request auth of that revision's actor (`campaign_request_auth`); at most 256
  notes per revision (`MAX_NOTES`; a save above it is 422). Notes are not sealed (L1).
- **Reading:** `notes::current` does one indexed probe per address (`CROSS JOIN` over
  `json_each`, so the planner keeps that loop order). `notes::latest_rows` reads, per list,
  the range of its `row:` addresses on the same index and keeps the latest note.
- **Display:** a sum or switched cell shows "84 − 15 = 69"; a scribble "kept 84 · ‘lots’ not
  understood" or "? (unknown) · ‘lots’ not understood", quoted to `SCRIBBLE_EXCERPT_CHARS`
  (32) characters (other excerpts: `EXCERPT_CHARS`, 64). Raw text is escaped inside `<bdi>`;
  the input is `aria-describedby` its note and offer. History pages list each revision's notes
  (address, op, raw, before → after, "undid revision N") to the custodian and Masters (D-E).

## No-op saves

A save whose parsed values all equal the saved ones (integer by value, so `007` equals 7;
Name after trim; text exactly; enum by ID; nullable pair and optional property by state
plus value) and that records no note writes no revision (D-U). `Store::private_unchanged`
still rechecks request auth, edit rights, the active state and the revision: lost rights
are 403 and a stale page is 409. The answer is 200 with the saved page and a `role="status"`
"No change" message.

## Switch and Undo

On a saved page the reader may edit, the current note of a cell may offer:
- **Switch** (D-Q): for a `sign_literal` note over a known number b whose cell still holds
  n and whose raw text spells n: "Did you mean <bdi>84 − 15</bdi> = 69? [Yes] Hide". b + n is
  checked `i64`; no offer on overflow, when b = 0, or with no known value before. Hide is a
  CSS-only checkbox ("Hide the suggestion"); nothing is stored, so the offer returns on
  reload while its note is current.
- **Undo** (D-N, D-X1): "Last change: was 84 [Undo]" for a current note with op `literal`,
  `sum`, `sign_literal`, `switched`, `confirmed`, `reverted` or `erased`, a recorded `before`
  that differs from `after`, and the cell still holding `after`. Not for scribbles, new
  cells, row notes, text cells or pages without edit rights. Undo of an Undo works. Enum
  values show their option IDs. When the note's `before` is `{"absent":true}` (an optional
  property that was not set), Undo makes the property absent again, not `null`
  (`assist.rs`, `NoteValue::Absent`).
- **Row removed · Undo** (D-V): a list whose latest row note is a removal whose row is still
  away shows "Row removed · Undo" (one per list; "Undo Row removed Equipment"). Undo reads
  the row from the revision before the removal (read with L), re-inserts it at its old index
  clamped to the current length with the new host ID `r<L+1>-1` (removed IDs never return),
  and writes `reverted` with `target`. A full list (256 rows) refuses it with a 422 message;
  the route accepts only the list's latest row note. The restored row is a new row: its cells
  have no Undo until edited again; the Undo itself is not undoable (Remove removes the row).

Routes `POST /private-sheets/{id}/switch` and `/undo` take an 8 KiB body
(`ASSIST_POST_BYTES`) with exactly `csrf`, `base`, `note` (`<revision>:<address>`, or for a
row cell `<revision>:<f>.<r>.<p>` with the row index read at `base`, which keeps each
button's markup short), posted by
the page's hidden forms `assist-switch` and `assist-undo`; each button sits by its cell, joins
its form through `form` and is named by `aria-labelledby` (button, cell label, list and row
legends). They take the `JOBS` permit at once (plain 503 when busy). Order: parse, CSRF,
request auth, edit rights (403), keys, then a malformed or zero `base`, a note revision above
`base` or an address the pin lacks (400). A `base` above the live revision, a removed row, a
note no longer current or a cell no longer holding its `after` is 409 with the saved page
("Not switched: … changed since, or it was already switched."), so double submits and stale
tabs write nothing. `private_save_keeping` rechecks the bound note inside its `IMMEDIATE`
transaction (`bound_notes_hold`). A switch writes `switched` (before = the literal, after =
b + n), Undo writes `reverted` with `target`; success is a 303.

## Unknown values

`null` is the unknown value of every integer cell (D-M): the value schema makes integer cells
and integer list properties `["integer","null"]` (the nullable wrapper is idempotent), and
`private_pin::list_values` accepts `null` for integer properties only.
- Blank input makes the cell unknown, noted `erased` when it replaces a recorded value
  (Undo-eligible); blank over unknown changes nothing. On a nullable field "Known" with a
  blank value is also unknown (U4).
- Unreadable input in a new cell gives unknown plus a scribble (see Change notes).
- Create still refuses an empty Name and unselected required enum or boolean controls.
- Formulas (formula reference for details): a formula that reads an unknown integer field,
  nullable or not (D-Z), or an integer property unknown in any item, is not run (static read
  set, U2) and reports "a value it uses is unknown (null)"; dependents say "it uses X, which
  could not be computed". `==`, `!=`, `in` and ternaries never pass or fail on an unknown; a
  `Null` result is `null_value`.
- An empty integer control is `aria-describedby` the page's one hidden "Unknown value" hint
  (`unknown-hint`). Freeze copies nulls; the history diff counts null versus a number as a
  change.

## Saving a saved sheet

A saved runtime page the reader may edit has no full-page form. Each **unit** is its own form
with hidden `csrf` and `base` (the revision its value was read at) and one Save button after
the control, named "Save <label>" (`aria-labelledby`). Enter in a single-line input submits
it; selects, Known/Unknown pairs and multiline fields save with their button. Scalar DOM ids
use the field index (`v0`, `label-0`, `note-0`); metadata uses `meta-name`, `meta-notes`.

| Unit | Route | Keys |
|---|---|---|
| Name, General notes | `POST /private-sheets/{id}/meta/{name\|notes}` | `csrf`, `base`, `value` |
| Scalar field | `POST /private-sheets/{id}/fields/{field_id}` | `csrf`, `base`, `value`, plus `null` if nullable |
| Saved-row cell | `POST /private-sheets/{id}/cells/{f}/{r}/{p}` | `csrf`, `base`, `value`, plus `set` if optional |
| New row | `POST /private-sheets/{id}/lists/{f}/rows` | `csrf`, `base`, `row-<p>` per property, `set-<p>` per optional property |
| Remove, Move up, Move down | `POST /private-sheets/{id}/lists/{f}/rows/{r}/{remove\|up\|down}` | `csrf`, `base` |

List routes name the list field, the row and the property by canonical index; the row index
is read at `base` and mapped to its host ID there, so a row moved since is still found. Each
row cell is its own form (DOM ids `row-<f>-<r>-<p>`, `label-…`, `save-…`; Save named "Save
Quantity Equipment Row 1"); each row's Move up, Move down and Remove are buttons of one
page-level form (`form="row-actions"`, `formaction`), never inside a cell form, so Enter in a
cell cannot trigger them ("Remove Row 2 Equipment"); the first row's Move up and the last
row's Move down are disabled. A "New row" form ends the list (one Add button, "Add row
Equipment"), replaced by "This list holds 256 rows" when full.

Unit routes (`UNIT_POST_BYTES` 256 KiB body for metadata, fields and cells; 5 MiB like the
full form for a new row; 8 KiB for a row action; a principal required before the body is
read): parse (400), CSRF and request auth (403), the bounded permit wait, read and edit rights
(403; an unsupported pin is read-only, so 403), then 400 for a native pin, an ID outside the
slug grammar, not in the pin, derived or a list, a list index outside the grammar or not a
list of the pin, a row missing at `base`, a forged edge move, or a key set that is not exact.

**Value-based conflicts:** the live revision L and revision `base` are read in one
transaction. A malformed or zero `base` is 400. A `base` above L is 409. Otherwise the value
is read against L: equal to L's value is "No change" (200, even from a stale page). Then, if
the unit's protected value (Name or Notes text, or the field's stored value) differs between
`base` and L, 409; then a refused value is 422. An A → B → A change is not a conflict (L3).
The patch is applied to L; the writer rechecks rights, the active state and that L is still
live. A 409 or 422 re-renders the live page with the draft in the unit's control (focused),
its note and actions hidden, and every unit's `base` at L, so a second Enter is a conscious
overwrite. Success is a 303 to `/private-sheets/{id}#label-<f>` (`#meta-name-label`,
`#meta-notes-label`, `#label-<f>-<r>-<p>`).

List units protect, between `base` and L: a cell its value (a cell whose row is gone from L is
a 409 that shows the draft read-only under "its row was removed", never in the row now at
that index; the same when the row goes before the writer's transaction); a new row nothing
(append only; values over `ECHO_BYTES` are 422 and not echoed; a list that filled up keeps the
refused row read-only to copy); Remove the row's existence and every value (an edited row is
409, a row already gone "No change"); Move the order of host IDs (409). A new row gets
`r<L+1>-1` and a blank number is unknown. Remove redirects to `#removed-<f>`; a Move to
`?focus=<up|down>-<f>-<r>`, which autofocuses the moved row's Move button in the same
direction, or the other one at an edge.

Create, native, read-only, frozen and history pages and the re-render of a refused full-form
POST keep the one full form (`value-<id>`, `row-<f>-<r>-<p>` names, with draft Add, Remove and
Move); the full-form edit POST is still accepted, with whole-sheet 409, and so is a slice-1.5a
list form (`csrf`, `revision`, `list`, `op`, that list's rows) from a page loaded before
per-cell save; its draft operations re-render in the full form.

**Busy server (D-X3):** after parsing, CSRF and session checks, unit and list-unit saves and
the full-form or list-form edit wait up to `PERMIT_WAIT` (2 s) for the `JOBS` permit
(`definitions::admit_within`); reads, create, switch and Undo do not wait. Without the permit
the answer is a styled 503 with `Retry-After: 1` and a form that posts the request again:
a unit or cell entry in a "Your entry" control (named by field ID, "Name", "General notes" or
the cell's indices), a row action's retry button ("Remove row 2 again"), or a full, list or
new-row form's pairs as hidden inputs when keys, values and 48 bytes per pair total
≤ `ECHO_BYTES` (1 MiB; else the page says they were too large to keep). Nothing is read or
written. The `SheetBusy` marker keeps the auth layer's generic error page from replacing it.

## Page-size bound

Rendered sheet pages stay within `FIXED_PAGE_BYTES` (64 KiB) + render units ×
`UNIT_MARKUP_BYTES` (2,048), plus 5 × `ECHO_BYTES` on a re-render that echoes a draft
(test constants in `src/campaigns/list_tests.rs`; units from `values::form_budget`, at most
`RENDER_UNITS` = 32,768); definition text adds at most 2,621,440 bytes (D7). Assisted markup
fits by elision, not a larger budget: term-boundary cuts, 32-character scribble quotes,
`aria-labelledby` names, index DOM ids, index list routes and note keys, one shared
row-action form. The worst-case fixtures there assert it; the tightest is a list of 256 rows ×
32 integer properties with 64-character IDs, labels of 100 `"` and a 200-`"` scribble kept
on each cell, every cell its own form: about 2.6% headroom (ASSIST-L2). Rerun those fixtures
whenever per-cell markup grows.

## Adaptive sheet presentation

Official sheets are recreated in HTML/CSS for structural fidelity with adaptive sizing, not
pixel-identical pages. Data is canonical and versioned, never mapped to PDF coordinates.
- Semantic headings, fieldsets and labels; Grid and Flexbox keep desktop grouping and reflow
  to one column on narrow screens. Content grows vertically and is never clipped; textareas
  use `field-sizing: content` where supported. Rule explanations sit by their fields or in
  collapsed disclosures.
- Runtime sheets save with POST/redirect/GET and load no script (one inline `style` remains on
  the full form's fieldset). Derived values are read-only text with a "Computed" label and a
  collapsed "How this is computed"; they and the advisory checks are computed from the saved
  snapshot and change on save and reload, never while typing.
- Lists: a fieldset of row fieldsets ("Row 2 of 5 · ID r1-2"), every control labelled, an
  explicit Set/Not set select for optional properties. On a saved sheet each cell, the new
  row and each row action save at once (see Saving a saved sheet); on the create page and in a
  full-form re-render the per-row buttons ("Move row 2 of Equipment up", "Remove row 2 of
  Equipment") and "Add Equipment row" (disabled at 256 rows) re-render the draft until Save.
  Targets are at least 24 × 24 CSS px; rows stack without tables at 390 px. History shows
  rows read-only.
- The native MÖRK BORG layout follows the owner-supplied blank sheet (htmx only in the
  `--no-auth` prototype's editor)
  (kept locally; exact edition and fidelity unverified). Logos, art and fonts keep their rights.
- Print styling exists only for the native sheet; exporting the publisher's PDF is not a goal.
- Automated Chromium runs at 1280 × 720 and 390 × 844 cover editing, long text, reflow and
  kept drafts. No approved official layout, print or cross-browser result is claimed.

## Demo

`tools/assist_demo.py` (dev tool, synthetic data only) builds a fresh data directory, publishes
the `assist-sheet` demo definition, creates a demo Owner and player with generated dummy
passwords and a campaign with one sheet (HP 84, Max HP 84, Armour −1), then serves loopback
auth mode on `http://127.0.0.1:3000` until interrupted:

```sh
cargo build --release --locked
cargo run --locked --example source_fixture -- target/demo-source
python3 tools/assist_demo.py target/release/mimik --fixture target/demo-source --data target/assist-demo
```

`--exit-after-setup` stops once the sheet exists. The definition
(`tests/support/source_fixture.rs`, `assist_definition`): integers Current HP, Max HP,
Armour modifier and nullable Luck; an Equipment list (Item name, Quantity, Weight, optional
Broken boolean); derived Load = `sum(equipment.map(i, i.quantity * i.weight))`; the check
`hp <= max_hp`.
