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
| Quick add | `src/campaigns/quick.rs` (reading), `rows.rs` (`quick_add`), `definitions/formula_admission.rs` (admission) |
| Writer, rechecks | `src/campaigns/sheets.rs` (`private_save_keeping`, `private_unchanged`, `private_read_base`) |
| Rendering | `src/campaigns/web.rs`, `templates/private_*.html`, `assets/app.css` |
| Validity flag | `src/campaigns/formulas.rs` (`Validity`, `Doubt`), `templates/private_sheet.html` |
| Sheet module, JSON answer, CSP | `assets/sheet-assist.js`, `src/campaigns/sheet_json.rs`, `SheetPage` in `src/campaigns/web.rs`, `normalize` in `src/auth/mod.rs` |
| Unknowns in formulas | `src/definitions/formula/convert.rs` |
| Interpretation assistant | see "Interpretation assistant" and [model-runtime.md](model-runtime.md) |
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
  and `confirmed` (an applied assistant reading) are written. `bundle` numbers the notes of one
  applied proposal (per sheet, from 1); `target` is the revision an Undo reverted.
- **Typed values** (`NoteValue`), canonical JSON ≤ 80 bytes (`MAX_VALUE_BYTES`):
  `{"absent":true}` (optional property not set, distinct from unknown), `{"unknown":true}`,
  `{"int":"-15"}` (decimal string), `{"enum":"<id>"}`, `{"bool":true}`, `{"row":"present"|
  "absent"}` (a row's removal and its Undo). The decoder accepts only canonical text. `raw` is
  what was typed: ≤ 200 characters, ≤ 800 bytes, no NUL.
- **When a note is written** (`Recorder`): every value change of a saved in-scope cell;
  Known ↔ Unknown and Set ↔ Not set as `literal` with empty `raw`; an `unresolved` scribble;
  an `erased` blank. New cells (create, new row) get a note only for an interpreted sum or an
  unreadable entry; a new-row form writes at most one per property (`MAX_ROW_NOTES`, 32); a
  quick-add row writes none.
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
| Quick add | `POST /private-sheets/{id}/lists/{f}/quick` | `csrf`, `base`, `text` |

List routes name the list field, the row and the property by canonical index; the row index
is read at `base` and mapped to its host ID there, so a row moved since is still found. Each
row cell is its own form (DOM ids `row-<f>-<r>-<p>`, `label-…`, `save-…`; Save named "Save
Quantity Equipment Row 1"); each row's Move up, Move down and Remove are buttons of one
page-level form (`form="row-actions"`, `formaction`), never inside a cell form, so Enter in a
cell cannot trigger them ("Remove Row 2 Equipment"); the first row's Move up and the last
row's Move down are disabled. A "New row" form ends the list (one Add button, "Add row
Equipment"), replaced by "This list holds 256 rows" when full.

Unit routes (`UNIT_POST_BYTES` 256 KiB body for metadata, fields and cells; 5 MiB like the
full form for a new row; 8 KiB for a row action and a quick-add line; a principal required
before the body is read): parse (400), CSRF and request auth (403), the bounded permit wait, read and edit rights
(403; an unsupported pin is read-only, so 403), then 400 for a native pin, an ID outside the
slug grammar, not in the pin, derived or a list, a list index outside the grammar or not a
list of the pin, a row missing at `base`, a forged edge move, a quick-add on a list without
the line, or a key set that is not exact.

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
direction, or the other one at an edge. A quick-add redirects to `?focus=quick-<f>`. The
sheet view reads `focus` from the raw query: a repeated `focus`, an unknown value or any
other parameter is ignored, never refused.

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
the cell's indices, or a quick-add line's `text`, "the quick-add line of list field 4"), a
row action's retry button ("Remove row 2 again"), or a full, list or
new-row form's pairs as hidden inputs when keys, values and 48 bytes per pair total
≤ `ECHO_BYTES` (1 MiB; else the page says they were too large to keep). Nothing is read or
written. The `SheetBusy` marker keeps the auth layer's generic error page from replacing it.

## Quick add

A list whose definition declares `items.quick_add` (`{"name": <text property>,
"quantity": <integer property>?}`, runtime definition contract) gets a quick-add line on a
saved page the reader may edit, between its rows and its new-row form: a form with hidden
`csrf` and `base`, the input `quick-<f>` named `text` ("Quick add Equipment" through
`aria-labelledby`, described by the hint `quick-hint-<f>`: "A count and a name, like “3
torches” or “healing potion x2”", or "A name, added as one new row" when no quantity is
mapped) and one Add button ("Add Equipment"). A full list (256 rows) shows no line; create, native, read-only, frozen and
history pages have none.

**Reading** (`quick::read`, fixed rules, no model): over `MAX_CHARS` (200) characters or a
control character other than tab is 422 with the entry kept in the line (focused; an entry
holding a line break is shown in a textarea, here and on the busy page, so a retry posts
it unaltered); after
trimming, a blank line is 200 "Nothing to add" and writes nothing. With a mapped quantity:
`^([0-9]{1,6})\s+(.+)$` is count and name ("3 torches", "007 bolts" = 7), else
`^(.+?)\s+[x×]\s*([0-9]{1,6})$` is name and count ("healing potion x2", "potion x 2",
"lamp oil ×3"); otherwise the whole line is the name with count 1 ("1234567 torches",
"torch x1234567", "potionx2", "Box 2", "Hex 5", "2x4 plank", "Rope X2", non-ASCII digits):
the white space before `x` keeps words ending in x whole. `\s` is Unicode white space. Without a mapped
quantity the whole line is the name, so a typed count is never lost. The name is stored as
typed (trimmed), never recased or matched against a catalogue.

**Row:** appended to L with the host ID `r<L+1>-1`, like the new-row form (append only, never
stale; a `base` above L is 409 with the entry kept). The mapped name and count are always
set, even when they are optional properties; every other required property is an integer
(admission refuses anything else) and is unknown (`null`); every other optional property is
not set. One revision, no note. A full list is 422 before the line is read, and the entry is
kept read-only to copy (`kept-<f>`) with no form that could add it. Success is a 303 to
`?focus=quick-<f>`, which focuses the empty line for the next item. Order and rechecks are
those of the other unit routes (parse, CSRF, request auth, the D-X3 wait, rights 403, then
400 for routes and keys, then the writer's `IMMEDIATE` rechecks).

## Page-size bound

Rendered sheet pages stay within `FIXED_PAGE_BYTES` (64 KiB) + render units ×
`UNIT_MARKUP_BYTES` (2,048), plus 5 × `ECHO_BYTES` on a re-render that echoes a draft
(test constants in `src/campaigns/list_tests.rs`; units from `values::form_budget`, at most
`RENDER_UNITS` = 32,768); definition text adds at most 2,621,440 bytes (D7). Assisted markup
fits by elision, not a larger budget: term-boundary cuts, 32-character scribble quotes,
`aria-labelledby` names, index DOM ids, index list routes and note keys, one shared
row-action form, one quick-add line per list (charged to the list's own units). The worst-case fixtures there assert it; the tightest is a list of 256 rows ×
32 integer properties with 64-character IDs, labels of 100 `"` and a 200-`"` scribble kept
on each cell, every cell its own form: about 2.6% headroom (ASSIST-L2). Fixtures also cover,
with both the densest sign-led notes and kept 200-`"` scribbles, the quick-add line on the
widest list (255 and 256 rows, and a refused entry kept from a full list), a new row kept
read-only from a full list (the tightest re-render, about 2.0% within its bound plus echo)
and an orphaned cell's kept draft, each asserting its `readonly` textareas. Rerun those fixtures whenever per-cell markup grows.

## Validity flag

RR-3 with ASSIST-4 and ASSIST-17 to ASSIST-20. The header of a saved sheet shows
`Not validated (saved revision N): <reasons>. This flag never blocks saving.` (`#validity`,
plain text; each reason links to the section that explains it: checks to "Advisory checks",
derived values and refused input to "Derived values") unless every declared check of a
covered pin passed. When derived values were not computed, "Derived values" names the count
and links to the first one, whose own block gives its reason. Reasons, in this order, separated by semicolons:

- "its definition declares no checks": a supported runtime pin that is formula-free or has
  no constraints;
- "N checks violated";
- "N checks not evaluated": an unknown input (D-Z) or an evaluation error;
- "N derived values not computed";
- "derived values and checks unavailable": the engine refused the saved values as input;
- "no rule checks cover its interpretation pin": native MÖRK BORG and unsupported pins.

It is computed on view from the evaluation the page already does of the saved revision
(`Validity::of`), never stored and never computed from a draft: the create page shows none,
and a refused draft's re-render shows the saved revision's flag. Each history revision page
(`/history/<n>`) shows its own; the history list evaluates nothing (RR-L1). Saving, values and
controls are unaffected. One header paragraph per page, inside the fixed page budget; it needs
no script and wraps at 390 px. With the sheet module a save's answer carries the flag of the
revision it describes and the module replaces the paragraph from it (ASSIST-21).

## Script on sheet pages

Step 2a, D-G, D-AA, ASSIST-21 to ASSIST-25. A saved runtime page the reader may edit loads one
module, `<script type="module" src="/assets/sheet-assist.js">` (served by the auth router,
embedded in the binary); create, native, read-only, frozen and history pages and full-form
re-renders load none. Without it every save, Undo, switch, quick-add and row action works
through PRG as above.

**CSP:** every private sheet page (view, create, history revision, 409 and 422 re-renders)
and every sheet JSON answer carries the `SheetPage` extension, and `normalize` gives it
`default-src 'self'; script-src 'self'; style-src 'self'; object-src 'none'; base-uri 'none';
form-action 'self'; frame-ancestors 'none'; require-trusted-types-for 'script'; trusted-types
'none'`. Every other answer keeps `frame-ancestors 'none'`: redirects, the busy page, refusals
(400, 403, 415) and any error page rebuilt by `normalize` or by outer middleware (timeout,
503), which drop the extension. The module builds DOM with `createElement` and `textContent`
only, with attribute names from an allowlist; a Rust test refuses HTML sinks, any mention of
`style`, dynamic attribute names outside that allowlist and code from strings in its source.

**JSON answer** (`sheet_json.rs`, ASSIST-22): the module posts the unit form's pairs as a
URL-encoded body (`post_origin` still refuses anything else with 415) with exactly one
`Accept: application/json` header, to the Name, General notes, field and cell routes only.
The route order, rechecks and refusals are those of the PRG path; instead of the 303 or the
re-rendered page the answer is JSON of the live revision read after the write (or L for "No
change" and refusals), built by the same view model as the page:

```text
{status: saved | unchanged | conflict | invalid, message, revision,
 unit: {address, action, value, state, note: {kind, shown, value, unknown, key?} | null,
        actions: {note, offer: {text, value} | null, undo} | null},
 derived: [{id, text, code}], not_computed: {text, id, label} | null,
 checks: {violated, not_evaluated, passed, items: [{id, status, word, message, code,
          explanation, expr, rule_ids}]} | null,
 unavailable: {code, message} | null, validity: {validated, reasons: [{text, anchor}]}}
```

Every integer, count and revision is a string, so 9007199254740993 survives. An
`unresolved` note carries `key`, the note's own revision and typed address (a row cell by host
ID), which starts an interpretation (step 2c-1); the page markup has no key. `action` is the
unit's route at this revision (a cell's row index can change; `null` when its row is gone); a
refusal names only `address`, `action` and `current` (the saved value as the page reads it: an
excerpt in quotes, "(empty)" for empty text, "? (unknown)", "not set" or the value a scribble
kept). The status codes
are 200 (saved, "No change"), 409 and 422.

**Module behaviour:**
- Enter or Save always sends; leaving a changed text control (`focusout`) or changing a
  select (`change`) sends only when the entry differs from the one in flight, the value the
  server acknowledged and the control text it acknowledged (D-AA), so Tab out of an unchanged
  field and Enter followed by Tab send once, and retyping a saved "84 -15" sends again.
- Per unit one request is in flight; a later save waits, and only the latest entry is sent
  after the answer, with the new `base`. A draft generation counter increments on `input`; a
  saved answer replaces the control's text, note, switch offer and Undo only if the player has
  not typed since the request, but always records the acknowledged value. `base` (and the
  form's route) moves only for the saved unit and only to a newer revision.
- Derived values, the checks section, the "Advisory checks" link and the validity flag have
  their own projection revision and change only for an answer at least as new.
- A refusal keeps the draft without the unit's note, switch or Undo, shows the answer's
  message and the saved value in a `role="alert"` paragraph under the cell, moves that unit's
  `base` to L; after a conflict it drops a save queued before it and leaving the field sends
  nothing until Enter or Save (ASSIST-23). An answer without a route makes a cell copy-only
  (ASSIST-26). A non-JSON or malformed answer (busy 503, 403, a network failure, a save that
  hangs 30 s) shows a generic alert and keeps the draft; no save stays pending.
- A save's switch and Undo post through hidden forms of their own at the answer's revision;
  any page post waits until every save in flight or queued has its answer, then goes only if
  its button still stands for the same action on the same note (ASSIST-24).
- Accessibility: the page's `role="status"` paragraph announces each save; focus stays in the
  cell after Enter and returns to it after Save; buttons the module adds use the page's
  classes (24 × 24 px targets) and the 390 px layout.

What it does not refresh is listed in ASSIST-L15 and ASSIST-L16.

## Interpretation assistant

Steps 2c-1 and 2c-2a (D-D, D-F, D-H, D-J, D-P, ASSIST-27 to ASSIST-43). The assistant reads a
scribble a number field could not take (its current `unresolved` note) and offers a
confirmable reading; it never writes on its own. It is off by default. With `--assistant`,
Mimik starts the pinned official llama-server as its child confined by the Hakoniwa sandbox (and not at
all where it cannot run, ASSIST-43), checks every connection
to it and primes and calibrates it before it takes jobs ([model-runtime.md](model-runtime.md)).
Without it there is no client, no worker, no child and no job route (the three paths answer
404), and the sheet works as above.

| Concern | Code |
|---|---|
| Schema v2 (template), instructions, fixed prefix, data envelope, per-request schema | `schemas/interpretation-v2.json`, `schemas/interpretation-v2-instructions.txt`, `src/assistant/contract.rs` |
| Phrase numbers and fraction words | `src/assistant/lexer.rs` |
| Reading a reply | `src/assistant/output.rs` |
| Bounds of a request | `src/assistant/request.rs` |
| Client and transport | `src/assistant/interpreter.rs`, `src/assistant/transport.rs` |
| Jobs, admission, runtime events | `src/assistant/jobs.rs` |
| Circuit breaker | `src/assistant/breaker.rs` |
| Model spec, flags and `max_tokens` rules | `src/assistant/model.rs`, `src/main.rs` |
| Owner model setting (trial, Switch, stored choice) | `src/assistant/runtime/manager_model.rs`, `src/assistant/records.rs` ([model-runtime.md](model-runtime.md)) |
| Model runtime | `src/assistant/runtime/` ([model-runtime.md](model-runtime.md)) |
| Development endpoint (tests) | `src/assistant/development.rs` |
| Context of a job | `src/campaigns/interpret.rs`, read sets in `src/definitions/formula/mod.rs` (`Compiled::read_sets`) |
| Routes and apply | `src/campaigns/interpret_web.rs` |
| Module flow | `assets/sheet-assist.js` (assistant part) |

**Flags:** `--assistant`; `--assistant-model` (`local:<name>.gguf`; without it the default
depends on the device, ASSIST-55: `local:Qwen3.5-9B-Q4_K_M.gguf` on the GPU path when the file is
in the models folder, `local:granite-4.2-3b-Q4_K_M.gguf` on the CPU path and when the 9B file is
missing, with the reason on `/admin/assistant`), `--assistant-model-sha256` (required for any
file but the two defaults)
and `--assistant-model-alias` (the request's model name and the server's `--alias`; default
the file name without `.gguf`, lower case); the runtime's `--models-dir`,
`--assistant-runtime` and `--assistant-threads` ([model-runtime.md](model-runtime.md),
"Options"). The owner's stored model setting, when there is one, replaces the model flags
([model-runtime.md](model-runtime.md), "Owner model setting"). `max_tokens` is 352 for the file
whose SHA-256 is the compiled-in `e0406663…` (the 2b rule over the 2c-1 screen of the v2-short
prefix: largest `predicted_n` 263, × 1.25, rounded up to a multiple of 32, capped at 384;
re-deriving it from a screen of the shipped prefix is owed, ASSIST-L47; the Qwen3.5-9B default
has 352 too, from the 352-token field-reference screens, ASSIST-L51), 256 for any other file named by the flags, and for a
model the owner tried, what the trial derived (`min(384, max(256, 1.25 × the five phrases'
largest predicted_n, rounded up to a multiple of 32))`). The budgets (B, N(B), R,
D, Q) come from each generation's calibration and arrive in its `ready` event (generation
started, ended with its reason, ready with the budgets). Builds with the `dev-endpoint`
feature (never a release; `smoke_release.py` checks the release binary has none) also take
`--assistant-dev-endpoint` and `--assistant-dev-token-file`, a hand-started server for tests,
calibrated the same way.

**Request.** System message: the fixed prefix, the instructions, `\nJSON schema of the
reply:\n` and the schema's compact text with the `ref` enum replaced by a description ("t for
the target, or the id of a context item from the data"), byte-identical across requests
(SHA-256 pinned in Rust and `tools/test_assist_eval.py`), so llama-server reuses it from its
prompt cache. It is arm D53 of the field-reference experiments (ASSIST-56, `docs/results.md`):
context items are named by their ID, the request carries no context values (Mimik substitutes
them when it evaluates a reply), two reference examples, the ASSIST-53 literal sentence and
the request for compact single-line JSON (ASSIST-52). `tools/assist_eval.py` keeps the earlier
prefixes for the experiments (`v2-compact`, `v2-base`, ...). User message: the data envelope,
`<data>\n{"note", "target": {alias "t", id, label?, value}, "context": [{id, label, type}]}\n</data>`
with `<`, `>` and `&` escaped. Body members beside rig's own: `response_format` with the
pinned schema in which `ref` is limited to `t` and this request's context IDs
(`contract::schema_for`; `json_schema`, strict, the pinned bytes in their key order; serde_json
keeps document order, DEP-18), `temperature` 0, `top_k` 1, `seed` 0, `cache_prompt` and
`stream` true. Priming and calibration send no schema. The phrase is read from the stored note,
never from the request.

- **Context:** the other stored integer and enum fields in definition order; with B at most
  1,024 only the fields some read set contains with the target (for a row cell, with its
  property), none without a formula layer; for a row-cell target, on every path, the integer
  and enum cells of that row read in the same iteration as its property (a binder per
  comprehension, shared by a filter and the comprehension it feeds, as in
  `gear.filter(i, …).map(j, …)`; two unchained iterations over a list are not co-reads;
  `has()` counts). Never derived,
  text, multiline, boolean, notes or other rows' cells; an optional cell not set is not sent.
- **Names:** `t` is the target; every other item is called by its ID (a field ID, or the ID
  of a property of the target's row). An item is left out of the context when its ID fails
  `valid_field_id`, equals `t` or the target's ID, or is shared by two items (a field and a
  cell of the row can share one): nothing is picked as a winner, so a reference to such an ID
  is an unsupplied reference and drops that option only (ASSIST-L52). IDs equal to the schema's
  keywords (`lit`, `op`, `l`, `r`, `round`, `ref`) and look-alikes (`f1`, `c1`, `t1`) are
  ordinary IDs. At 4,096 bytes at most 108 minimal items fit (tested). The reply schema's `ref`
  enum lists `t` and the integer items' IDs; enum items are context only.
- **Bounds:** the envelope must hold B bytes and N(B) tokens (llama-server's `/tokenize`):
  whole items are dropped from the end, then the target's label; the phrase, the target's ID
  and its value never are. If they alone exceed a bound the job ends "too long for the
  assistant on this machine" with no completion request.

**Reply.** JSON, then schema v2 (`jsonschema`), then typed decoding: `options` (at most 3, an
expression tree of at most 3 operator levels and an explanation of at most 120 characters) or
`ambiguous` (1 to 3 readings of at most 120 characters, display only). Per option: a `ref`
must name a supplied integer item with a known value (an enum, unknown or unsupplied ID
drops it); every literal's magnitude must be a number of the phrase (the phrase-number lexer:
decimal digits folded to ASCII, a run not touching letters or other number characters, not
part of a decimal or grouped number, no leading zero, at most 18 digits), a number word of
the phrase (ASSIST-53: EN and IT, zero to twenty, the tens thirty to ninety and hundred/cento,
`uno`, `una` and `un` for 1, also elided as `un'`; whole words after case and diacritic
folding, so "someone" and "alone" give nothing; "a" and "an" are not numbers; a compound
such as "ventuno" gives nothing and "twenty-one" gives its parts 20 and 1), the target's
value, or 2, 3, 4 or 100 when a fraction or percent word is present (EN half, third,
quarter, percent, `%`; IT metà/meta, mezzo, mezza, terzo, terza, quarto, quarta, percento,
per cento; whole words after case and diacritic folding, ASSIST-38); a literal 1 that only
multiplies or divides (either side of `×`, the right of `÷`) needs no provenance, since it
changes nothing, unless its sibling operand is also a literal 1 (`1 × 1` and `1 ÷ 1` are checked
like any literals), and a 1 anywhere else does; a literal outside `i64` drops the
option; evaluation is checked `i128` with an `i64` result, `div` rounds floor, ceil or exact
(a remainder drops it), division by zero drops it. Shown: the current value, the expression
with `−`, `×`, `÷` (every reference, the target included, as its label with Mimik's value) and the effect, labelled "the
assistant's reading".

**Jobs.** One worker, one job at a time. A start is admitted only after the origin note was
found current: the in-memory note-write map (latest note-write revision per target, row
cells by host ID; entries kept while a start registered before its database check or a live
job needs them) refuses a start whose target got a newer note (409); a repeat for the origin
of a live job, or of a finished job with applicable options, returns that job; a start older
than the account's live job on the sheet is 409, a live job on another sheet 429; then the
runtime state (503 "starting"/"unavailable"), the breaker (503) and capacity (429: at most D
waiting, the running job's remaining run deadline plus R per waiting job within Q, fewer than
64 live jobs), all computed as if the superseded job were gone, so a refused start cancels
nothing; then the account's earlier job is superseded (its request dropped, which closes the
connection) and the new one queued. Every save that writes notes calls the post-commit hook
(`Store::notes_committed`, run by the committing task once the database mutex is released, also when the request was cancelled after the commit): jobs whose origin it makes non-current end ("changed since: ask
again") or lose their options; a `row:` note covers its row's cells. A job past its queue wait
or its run deadline ends "ask again"; a generation change ends every queued and running job
("the assistant restarted"). At most 64 records: a start evicts the oldest finished one, never
a live one; an evicted job answers "ask again". Options expire 10 minutes after the job ends.
Each option is stored with a SHA-256 content hash over the job, its index, the origin, the
value before and after, the expression, the explanation and every reference with its value.

**Breaker** (one per endpoint configuration, in memory, injectable clock): counts transport
errors, HTTP 4xx and 5xx, bodies over 1 MiB and malformed streams; three consecutive counted
failures open it for 2 minutes, then one half-open probe decides (a cancelled probe leaves it
half-open for the next job). A completed 2xx exchange is a success whatever its content;
supersession, cancellation, queue-wait and run-deadline expiry are neutral. A counted failure
whose generation exits within 2 s is withdrawn, and a breaker it opened goes back. Only Restart
and the app's start reset it. While open, a start answers 503 and a queued job ends
"unavailable"; entry, notes and Undo are unaffected.

**Client policy.** One HTTP/1 exchange per connection over a stream Mimik opened and, for the
runtime's child, checked before its first byte (`hyper`'s client connection: no proxy
whatever the environment says, no redirects, no retries, no decompression, no reuse), plain
HTTP only. A TCP endpoint must be `http://127.0.0.1:<port>` (or `[::1]` for the
development endpoint), checked on the raw authority and on the parsed URL (`localhost`,
`127.1`, `0x7f.1`, `2130706433`, `::ffff:127.0.0.1`, userinfo, zone IDs, `https` refused); a
request for any other destination is refused before it is sent. The runtime's connectors reach
only the current generation's socket or port (the endpoint check, [model-runtime.md](model-runtime.md)).
Bodies are refused past 1 MiB, plain and streamed, as they arrive. Nothing is logged: no
prompt, phrase, reply or token.

**Routes** (JSON, `Accept: application/json`, the sheet CSP; fixed messages survive the auth
layer's error page through `AssistantAnswer`):

| Route | Keys | Answers |
|---|---|---|
| `POST /private-sheets/{id}/interpretations` | exactly `csrf`, `note` | 202 `{job, poll}` (200 for a repeat); 400 malformed or not an integer field or cell of the pin; 403 no edit rights; 409 not the current `unresolved` note; 429 busy; 503 starting, unavailable or breaker open; 413 over 8 KiB |
| `GET /private-sheets/{id}/interpretations/{job}` | — | `thinking`, then `options` (`n`, `current`, `expression`, `value`, `effect`, `explanation`, `action`), `readings`, or a message (`nothing`, `too_long`, `unavailable`, `ask_again`, `superseded`, `stale`); 404 "ask again" for an unknown, evicted, another account's or another sheet's job; 403 without edit rights |
| `POST /private-sheets/{id}/proposals/{job}/{n}` | exactly `csrf` | 200 the unit's JSON save answer; 409 "Something this proposal used has changed: ask again."; 403; the styled busy 503 after the D-X3 wait |

The `note` key is the one the JSON save answer now carries in `unit.note.key` whenever the
note is `unresolved` (`<revision>:<address>`, a row cell as `cell:<list>/<host>/<property>`);
the page markup is unchanged. An indexed key (`<revision>:<f>.<r>.<p>`) is resolved at its own
revision. **Apply** reads the live sheet with the permit and requires the origin note still
current, the target still holding the value the job saw, every reference still holding its
value in a row that still exists, the content hash and the re-evaluated value to match; then
it writes the value with a `confirmed` note (raw = the phrase, before = the value) in a new
bundle number of the sheet, bound to the origin note inside the `IMMEDIATE` transaction, and
consumes the job after the commit. Any mismatch or expiry writes nothing and never falls back
to a literal. Undo of the applied value works like any Undo.

**Module.** After a save whose answer has an `unresolved` note with a key, the module starts a
job (once the start answers 404 it stops trying for the page) and shows "Thinking…" in the
cell's `role="status"` box (`assist-<key>`, described by the cell); it polls at 500 ms backing
off to 2 s; a newer save of the cell supersedes it. Options show "Now <value>:", the
expression in `<bdi>`, "= <value>" and the explanation in `<bdi>`, with a Use button named
"Use <expression> = <value>"; readings show in `<bdi>` without Use; other answers show their
message. Use posts the apply route as the cell's flight, like a save (a draft typed meanwhile is kept, a save asked for meanwhile waits), and draws the answer like a save. Everything is
`textContent`.

## Adaptive sheet presentation

Official sheets are recreated in HTML/CSS for structural fidelity with adaptive sizing, not
pixel-identical pages. Data is canonical and versioned, never mapped to PDF coordinates.
- Semantic headings, fieldsets and labels; Grid and Flexbox keep desktop grouping and reflow
  to one column on narrow screens. Content grows vertically and is never clipped; textareas
  use `field-sizing: content` where supported. Rule explanations sit by their fields or in
  collapsed disclosures.
- Runtime sheets save with POST/redirect/GET; a saved page the reader may edit also loads the
  sheet module (see Script on sheet pages). Sheet pages have no inline style. Derived values
  are read-only text with a "Computed" label and a collapsed "How this is computed"; they and
  the advisory checks are computed from the saved snapshot and change on save (with the
  module, from the save's answer) and reload, never while typing.
- Lists: a fieldset of row fieldsets ("Row 2 of 5 · ID r1-2"), every control labelled, an
  explicit Set/Not set select for optional properties. On a saved sheet each cell, the new
  row, the quick-add line and each row action save at once (see Saving a saved sheet); on the create page and in a
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

`--exit-after-setup` stops once the sheet exists. `--reset-passwords` (with `--data` naming an existing demo folder, no `--fixture`) gives both demo accounts new dummy passwords, prints them and signs both out. The definition
(`tests/support/source_fixture.rs`, `assist_definition`): integers Current HP, Max HP,
Armour modifier and nullable Luck; an Equipment list (Item name, Quantity, Weight, optional
Broken boolean) with a quick-add line (name and quantity; try "3 torches"); derived Load = `sum(equipment.map(i, i.quantity * i.weight))`; the check
`hp <= max_hp`: set Current HP above Max HP, or blank it, and the header says "Not validated".
