# Split and merge units on the review screen

Goal: the Owner fixes text the extractor joined or cut wrongly without rejecting units and
drawing new areas by hand. **Merge** turns 2 or more units of a page into one unit; **split**
turns one unit into 2 or more. Both run before agreement slice 3b
(`docs/source-agreement-plan.md`): the page then has fewer unreviewed fragments when 3b runs.
Merged and split units are decided, so 3b excludes them (`Owner`); a merged unit's area can also
cover neighbours' words, which 3b then excludes as `SharedWord`. Remaining work only.

Decided by the Owner (2026-11-12, to be recorded as SRC-22):

- The new units are decided in the same operation: each is recorded `corrected` with the Owner's
  text and rationale, as applying a merge suggestion already does. This differs from owner-built
  tables (SRC-17), which stay `unreviewed` because their cells are a generated draft; merge and
  split text is written or confirmed by the Owner against the page image in the same form.
- A split part gets its area automatically, shown on the unit's crop, and the Owner can adjust
  each part's box before saving.

What stays: every change is an Owner decision with actor, rationale, revision, one operation id
and the audit sidecar (SRC-4, SRC-5); the replaced units are `rejected` in that operation, never
deleted; raw extraction text and stored word rows are unchanged (append-only); suggestions and
drafts never decide anything (SRC-6).

## What exists

- Applying a model `merge` suggestion creates one owner `Text` unit (axis-aligned union
  rectangle of the targets), records its correction, and rejects the targets, under one
  operation (`add_owner_operation`, `record_source_suggestion_application`, migration 025).
- Owner-built tables (`add_owner_table`, migration 028): select 1–200 units with the card ticks,
  edit a draft, save; the units are rejected and one owner `Table` unit with the `UnionBox` area
  is added; `source_owner_tables` links it to its batch. The table form shows the crop of the
  selected area and the whole page with outlines.
- Owner units enter `unreviewed` at revision 1 (trigger `source_unit_origin_insert`); a decision
  is a separate review event. Events of one operation must be consecutive with one actor and
  rationale (024); nothing limits owner units or corrections per operation.
- Reading order (`ORDERED_UNITS`, `unit_sort_key!`) places owner tables at their first unit and
  merge-suggestion units at their first target (below).

## Design

### Reading order (done, migration 030)

`source_owner_replacements` links each table, merge or split unit to its anchor (the first
replaced unit in reading order, or the split unit) with a stored TEXT `order_key`: the anchor's
key plus `'.'||printf('%04d', part)`. `record_replacement` writes it (kind `merge` with part 1,
`split` with parts 1..n) after the owner unit's `corrected` event and the anchor's rejection in
the same operation; `first_in_reading_order` gives the merge anchor. The trigger refuses
anything else. See `docs/sources-and-retrieval.md` for the order.

After a merge or split, later units' positions shift: `?units=` cursors opened earlier, overlay
cursors and table proposal runs change, like after any reordering; each read recomputes them.

### Shared rules for both writes

- One IMMEDIATE transaction: Owner and CSRF rechecked; the page revision and every source
  unit's revision checked **once, before any write**, against the revisions the Owner saw (the
  form and the preview carry them through and never re-read them); each unit on the page and not
  `rejected`. The owner inserts and decisions then run without re-checking the page revision
  per write (each write bumps it); `insert_owner` gets a variant that takes the checked state.
- Text per new unit at most 20,000 characters; total text of one write at most 64 KiB, below
  the 320 KiB POST bound, refused with a 422 that keeps the draft. Rationale at most 5,000.
- 409/422 re-render keeping the draft, as the table form does.
- Save redirects to the first new unit's card (cursor computed after commit) with a notice;
  `material_is_owner_table` becomes a check over the replacement rows.
- History lines: "Units 12, 13 merged into 40" and "Unit 12 split into 40, 41, 42", from the
  replacement rows.
- The card tick shared by table and merge is labelled "Select".

### Merge

- **Entry:** "Merge selected units" beside "Create table from selected units", in the page and
  in the bottom bar (same `table-select` form, `formaction` to
  `/admin/materials/pages/{id}/merges/new`; works without script).
- **Form:** 2–200 distinct units on the page, none `rejected` (owner units allowed, so split
  parts can be merged back). Prefilled with their text in reading order (correction where
  corrected, else extracted text) joined by a newline, bounded at 64 KiB like the table prefill
  (the form says when it was cut). A warning when the selection includes a `Table` or a kind
  without text (structure is lost: the result is `Text`), and the table form's covered-units
  warning when the union box covers unselected units (a run across columns). The crop and the
  whole page with outlines as on the table form, through the same route renamed
  `/admin/materials/pages/{id}/selection/image`. Rationale required.
- **Save** (`POST .../merges`): one owner `Text` unit with the `UnionBox` area, `corrected` with
  the Owner's text; every selected unit `rejected`; one operation, one rationale, sidecars; a
  `merge` replacement row anchored at the first unit in reading order.

### Split

- **Which units:** `Text` units (extracted or owner), not `rejected`. Other kinds have no "Split…".
- **Entry:** "Split…" in each eligible card's actions, a link to
  `GET /admin/materials/pages/{id}/units/{unit}/split`.
- **Form:** the unit's text (correction where corrected, else extracted text) in a textarea. A
  part starts after a line that is exactly `---` once trimmed (CRLF accepted). If the original
  text already has such a line, the form warns that it will split there. 2–20 parts; a part with
  no text is refused by name. Rationale required. "Show the areas" is a POST that writes nothing
  (CSRF checked, text out of URLs and history); it computes the parts' areas and shows the
  unit's crop with every part outlined and numbered, and one box editor per part on that crop
  (four corner fields in page points; the existing region editor adapted to edit inside a crop
  without the page-wide `#owner-polygon` id; axis-aligned boxes only).
- **Areas, computed** (pure function in `src/sources/`, reusing agreement's `without_shadows`,
  `lines_of`, `unit_tokens` and `layer_tokens`, made `pub(super)`; `layer_tokens` gains the word
  indices behind each token, and the agreement tests must show every agreement-v1 verdict
  unchanged):
  1. **Words path**, only when the whole unit's tokens equal its layer tokens under
     agreement-v1 (the same comparison as `Agrees`, ignoring the orphan check): part boundaries
     in the unit's tokens map to the words each layer token consumed (a hyphenated token on a
     boundary goes to the earlier part); a part's area is the bounding box of its words. A part
     that ends mid-line shares that line's height with the next part.
  2. **Bands** otherwise (no words, any difference, a two-column unit whose lines interleave):
     the unit's box cut into stacked horizontal bands in proportion to each part's token count,
     each at least one line high (the unit's height over its line count, or 1/20 of it).
  The form says which way the areas were made. Every submitted box is validated as an owner
  polygon (`validate_polygon`: inside the page, non-empty); it need not stay inside the unit.
- **Save** (`POST .../units/{unit}/split`): the parts' text and boxes and the rationale; one owner
  `Text` unit per part, each `corrected` with its text; the unit `rejected`; one operation;
  `split` rows with `part` 1..n anchored at the unit.

### Other interactions

- Open suggestions targeting a replaced unit become stale as for any reviewed target; the
  existing stale handling shows them so.
- Page completeness is unchanged: these writes bump the page revision like other review writes,
  and the existing completion guards apply.

## Slices

2. **Merge.** Form, route rename, save, buttons, tick label, history line, redirect. Tests: happy
   path (events, operation, states, union area, order, history), stale page or unit revision, a
   rejected or foreign unit, one unit, 201 units, text over the bounds, prefill over 64 KiB, a
   selection with a `Table`, the covered-units warning, missing rationale, CSRF, non-owner,
   draft kept on refusal, no-script form.
3. **Split.** Area function (unit tests: words path exact on an agreeing unit, a hyphenated
   boundary, a bullet unit, a two-column unit and a corrected unit falling back to bands, equal
   token counts with different text falling back to bands; a proptest: one area per part, each
   inside the page and non-empty), form, preview POST, save. Tests as for merge, plus: split of
   a corrected unit and of an owner unit, edited boxes saved as submitted, a box outside the
   page or empty refused, an original `---` line warned, CRLF markers, empty parts, 21 parts, a
   part over 20,000 characters, a preview whose revisions went stale before saving, history
   line.
4. **Docs.** `docs/sources-and-retrieval.md` (merge, split), SRC-22 in
   `docs/decisions.md`, limitations (bands are approximate when the words path does not apply; a
   part boundary mid-line shares the line; a merge across columns covers neighbours' words),
   owner browser check of one merge and one split.

## Risks

- **Wrong area on a split part:** shown before saving and editable; a wrong area makes later
  crops and agreement less useful, never decides text.
- **Ordering change:** `ORDERED_UNITS` feeds card cursors, overlays, next-unit links, proposals,
  agreement and its measurement; slice 1 tests the order on a page with every owner kind.
- **Two decisions in one step:** the Owner both restructures and decides; the form shows the
  page crop and requires a rationale, as applying a merge suggestion does today.
