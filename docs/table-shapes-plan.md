# Table shapes plan

Goal: when the Owner creates a table from selected units, Mimik drafts the cells by recognising
the table's shape instead of the Owner typing them. Drafts are deterministic suggestions: the
table unit stays `unreviewed` (SRC-17), nothing is accepted automatically (SRC-6), and the save
path (`Store::material_create_table`) is unchanged except where stated.

Landed (slices 1-3): the `src/sources/shapes/` module (`ShapeInput`, `Draft`, `Problem`, `read`,
`best`; roll table, progression, stat line, label and value, column grid, plain columns), the form
with "Read as" and the header checkbox, `OwnerTable.header`, `MAX_TABLE_SOURCES = 200`, the
union-box warning, and the stored word boxes (`source_page_words`, written by `import-source`); see
`docs/sources-and-retrieval.md` "Owner-built tables" and "Import", and SRC-19/SRC-20. Slice 4 only
reads what exists: the text shapes, and Column grid when a page's words are stored.

## Slice 4: proposals on the page

- On the page, Mimik runs the text shapes (and Column grid when words are cached; never starting
  a worker job from a page view) over each maximal run of consecutive unreviewed extracted units
  in reading order, bounded to 200 units per run and 64 KiB per page. A run is proposed when a
  non-plain shape scores at least 80 with at least 3 rows; a single multi-line or inline unit
  (the merged-paragraph case) is a run of its own. Runs never cross a page column (their union box
  must not overlap unselected units). Proposals are computed only when the page's unreviewed
  units are listed, memoised per (page id, page revision). Each proposal is a link to the table
  form with those units and the shape preselected ("Likely roll table: units 12–17"), at most 8
  per page. A proposal is never a decision and stores nothing.
- The same recognisers give batch accept (source-import plan step 8) its "looks like a table"
  exclusion later; not built here.

## Tests and docs

- Slice 4: tests that proposals never start a worker job, are memoised per (page id, revision),
  never cross a page column and are capped at 8 per page.
- Fixtures are synthetic; no manual text in the repository.
- `docs/sources-and-retrieval.md` "Owner-built tables", L-SRC-TABLE and SRC-19 updated as each
  slice lands.
- Order: slice 4 on branch `table-shapes` after slice 3's review; full gate before merge.
