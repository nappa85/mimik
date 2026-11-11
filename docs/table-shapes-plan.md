# Table shapes plan

Goal: when the Owner creates a table from selected units, Mimik drafts the cells by recognising
the table's shape instead of the Owner typing them. Drafts are deterministic suggestions: the
table unit stays `unreviewed` (SRC-17), nothing is accepted automatically (SRC-6), and the save
path (`Store::material_create_table`) is unchanged except where stated.

Landed (slices 1-2): the `src/sources/shapes/` module (`ShapeInput`, `Draft`, `Problem`, `read`,
`best`; roll table, progression, stat line, label and value, plain columns), the form with "Read as"
and the header checkbox, `OwnerTable.header`, `MAX_TABLE_SOURCES = 200` and the union-box warning;
see `docs/sources-and-retrieval.md` "Owner-built tables" and SRC-19. Slices 3 and 4 extend that
module: `ShapeInput` gains optional word boxes (one more field next to the units' lines), `Shape`
gains `Column`, and `best` takes it into account.

## Slice 3: word boxes and Column grid

- The worker gains one operation `Words` for one page: MuPDF's text page (same flags as render),
  grouped into words by whitespace and font-size gaps, each word `{text, box}` in the same
  corrected page space as `TextLine` (PDF points, the unit polygons' space). Bounds: at most
  20,000 words per page, word text at most 256 bytes, boxes finite and inside the page (validated
  by the parent like `validate_lines`); a new `WORDS_LIMIT` on the frame. Same sandbox, deadline
  and cancel as render; a hostile-corpus test and record tests like `TextLine`'s.
- Words are stored at import (owner decision 2026-11-11: no release yet; no compatibility path
  beyond the one below). A migration (the next free number; `docs/source-limits-plan.md` claims
  029/030 and its pre-step's hard-coded "version >= 3 and < 30, just before 030" moves with it)
  adds `source_page_words(page_id INTEGER PRIMARY KEY REFERENCES source_pages, words BLOB NOT
  NULL)`, append-only through `BEFORE UPDATE`/`BEFORE DELETE` triggers like the other import
  artefacts; never evidence, never shown as text of its own, not indexed for search.
- Encoding: a version byte (1), then per word: four little-endian `f32` box values in PDF points
  and a `u16`-length-prefixed UTF-8 text of 1-256 bytes; at most 20,000 words and a hard cap of
  20,000 × (256 + 18) + 1 bytes per row. Validated again when read (version, lengths, UTF-8,
  finite boxes inside the page); an invalid row is treated as absent and reported, never trusted.
- `mimik import-source`: after `load_import` has the hash-verified PDF bytes (`Import.pdf`), the
  `spawn_blocking` stage writes them to a private 0600 file in the data directory's staging
  area (never `std::env::temp_dir()`: `/tmp` may be RAM), re-checks the hash, and runs `Words`
  for the bundle's PDF page indexes in batches of at most 8 pages (like render's batches), each
  with its own deadline; the file is removed afterwards. The worker never runs inside the write
  transaction; the validated rows are inserted in the existing import transaction. When the
  content-addressed PDF store lands (source-limits decision 9), the input comes from it instead.
- Existing sources: `import-source` on an existing, matching source (`verify_existing`) inserts
  rows only for its pages that have none (insert-once, idempotent, so re-running the same import
  fills in words without touching review history). This is the narrow backfill the owner's
  re-import needs; tested.
- When the worker is unavailable (platform, missing sandbox) the import still succeeds without
  words and says so; a page without words falls back to the text shapes, and the form says why.
  The upload path (route A, plan step 4) will carry the words from its own render run into the
  same table when uploads become sources; out of scope here.
- The table form reads the selected units' words (centre inside one of their polygons) from the
  stored row: no worker launch, cache, semaphore or busy state on the review screens.
- **Column grid**: rows from words grouped by baseline (y overlap > 50 %), columns from
  clusters that recur in at least half the rows (gap-based clustering, tolerance 0.6 × median
  character width; left edges for text, right edges for numeric cells, centres as the fallback); a cell is the words between two column starts. A first row with non-numeric
  cells over mostly numeric rows is the header. Without word boxes the shape falls back to
  splitting each line on runs of two or more spaces, scored lower.
- Coordinates: unit polygons and words share PDF points; bundle import refuses rotated pages and
  CropBoxes off the origin (`src/sources/import.rs:160`), so a fixture test checks that a word's
  centre falls inside its unit polygon on an imported synthetic page. Rotation is the upload
  path's concern (step 4).

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

- Slice 3: route tests for a page with no stored words and an import without a worker (the text
  shapes still work, the form says why); the existing-source import filling words once; the
  encoding's round trip and its refusal of malformed rows; the append-only triggers; worker:
  record validation, hostile corpus, one live words test like render's; a word-centre-in-polygon
  fixture test; unit
  tests for Column grid (clean fit, ragged rows, header, numeric right edges, the no-words
  fallback, a near-miss that loses to Plain columns).
- Slice 4: tests that proposals never start a worker job, are memoised per (page id, revision),
  never cross a page column and are capped at 8 per page.
- Fixtures are synthetic; no manual text in the repository.
- `docs/sources-and-retrieval.md` "Owner-built tables", L-SRC-TABLE and SRC-19 updated as each
  slice lands.
- Order: slices 3→4, each its own commit series on branch `table-shapes`, one review per slice,
  full gate before merge.
