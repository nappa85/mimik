# Table shapes plan

Goal: when the Owner creates a table from selected units, Mimik drafts the cells by recognising
the table's shape instead of the Owner typing them. Drafts are deterministic suggestions: the
table unit stays `unreviewed` (SRC-17), nothing is accepted automatically (SRC-6), and the save
path (`Store::material_create_table`) is unchanged except where stated.

## Shared design (slice 1)

- New module `src/sources/shapes/` (pure functions, no I/O): `fn read(input: &ShapeInput, shape:
  Shape) -> Draft` and `fn best(input) -> Vec<Draft>` (all shapes, sorted by score, then by a fixed
  shape order for ties). `ShapeInput` holds the selected units in reading order, each as its text
  lines (corrected text where corrected, extraction text otherwise, as the prefill reads today)
  and, from slice 3, optional word boxes. `Draft { shape, rows: Vec<Vec<String>>, header: bool,
  caption: Option<String>, score: u32 (0..=100), problems: Vec<Problem> }`. `Problem` is a fixed
  enum with the row index (gap in a sequence, duplicate, line that fits no row, ragged row), shown
  as text on the form; never auto-fixed.
- Lines: the units' text split on newlines, trimmed, empty lines dropped, then normalised:
  `raw_text` comes from html2text's decorating renderer (`src/sources/import.rs:357`), so `**`/`*`
  emphasis around a marker, `* `/`- ` bullets and heading `#` are stripped. A marker that
  html2text itself produced for an `<ol>` (`1. `, `2. ` on list lines) is a weak marker: alone it
  never proves a die (a 6-item ordered list is not a d6), it only scores when a `dN` header line or
  a range marker supports it. Fixtures are the actual html2text output for `<strong>`, `<ol>`,
  `<ul>` and wrapped `<p>`.
- Inline items: Marker often merges `1 Rats 2 Goblins 3 …` into one `<p>`, which html2text wraps
  at 120 columns at arbitrary points. Roll table therefore also reads the token stream of the
  joined lines: one linear pass looks for the next expected marker (the next number or the next
  range start) at a token boundary and splits there; the split is accepted only when the full
  cover is reached. No backtracking. A shape works on the
  concatenated lines of all selected units in reading order (except `Plain columns`, which keeps
  today's one-column-per-unit grid).
- Caption: when the first line fits no row of the chosen shape and is short (at most 80
  characters, no row marker), it is kept as a plain first row (not a header). The header box is
  ticked only when a real header line is recognised (`d6 Result`, a Column grid header).
- Continuation: a line with no row marker after a row joins that row's last cell with a space.
- Score: the share of lines the shape consumed as rows or continuations, minus penalties for
  problems; `Plain columns` has a fixed low score so any shape that fits wins.
- Shapes in slice 1:
  - **Roll table**: a leading marker, a number `1`–`100`, `00`, a range `a–b`/`a-b`/`a—b` or
    `a+` (en dash, hyphen, em dash), optionally followed by `.`, `)` or `:`; then the outcome.
    The markers must be increasing and form a complete cover of a die: `1..N` for
    N in {2,3,4,5,6,8,10,12,20,30,100} (d100 may use `01`…`00`, and `00` means 100 inside
    ranges such as `96–00`), d66 (`11`…`16`, `21`…`66`), or a sum cover `n..n·X` for `ndX`
    (2d6: 2–12, 3d6: 3–18) with ranges. A `dN`/`ndX`/`d66` header line (`d6 Result`, `1d20`) is
    the header row and fixes the expected cover. Gaps, overlaps, duplicates and interleaved
    sequences (two side-by-side tables read as 1,1,2,2…) are problems that lower the score, never
    a fit. Columns: roll, outcome; the outcome splits further when every row has the same number
    of tab or 2+-space separated parts.
  - **Label – value**: a label, then a separator (a dot leader of 3+ `.` or `…`, ` – `/` — `/` - `,
    `:` followed by a space, or a tab), then the value. Two columns. At least 2 rows.
  - **Plain columns**: today's prefill (one column per unit, one row per line).
- Form (`/admin/materials/pages/{id}/tables/new`): the best draft fills the textarea; the
  problems list sits above it. "Read as" is a separate small GET form carrying only the `unit`
  values and `shape` (never csrf, cells or rationale in a URL), with each shape and its score; it
  says that re-drafting replaces the cells being edited.
- Header: `OwnerTable` gains `header: bool` (form checkbox "First row is the header", default
  from the draft). `tables::html` emits that row as `<thead><tr><th>` and `check()` measures the
  header-aware output, so the 64 KiB bound is exact. The sanitiser allows `thead`/`th`
  (`src/sources/mod.rs:178`) and `flatten_tables` reads `th`; tests cover the round trip, the
  search text and the stored `raw_text`. No `<caption>` element (allowlist unchanged).
- Selection cap: shapes read units as rows, so a d20 table is 20 units. A new
  `MAX_TABLE_SOURCES = 200` replaces `MAX_COLUMNS` as the selection cap in `validate_table`
  (`src/sources/documents.rs:484`), the prefill (`src/sources/materials.rs:188`), the GET parser
  message (`src/auth/materials.rs:1282`) and the bar (`assets/source-review.js:51`);
  `MAX_COLUMNS` stays the grid's column cap, so `Plain columns` is not offered above 16 units. A
  route test saves a 20-unit table; the prefill's per-unit reads stay under its read deadline (a
  200-unit test). When the union box (`documents.rs:522`) overlaps units that were not selected
  (a run across two page columns), the form warns and names them.
- Bounds stay as today (64 KiB of selected text, 200 rows, 16 columns, 1,000 characters per cell);
  each recogniser is linear in the input and uses no regex backtracking (the `regex` crate is
  linear; plain char scanning is fine too).

## Slice 2: Stat line and Progression

- **Stat line**: lines made of repeated `KEY value` pairs where KEY is 1–12 letters (allowing
  `.`) and value a signed number, a dice expression (`2d6+1`) or a short token, at least 2 pairs
  per line; also `Key: value` sequences separated by `,` or `;`. Output: one row per pair (KEY,
  value), or, when every line carries the same keys in the same order, one header row of keys and
  one row per line.
- **Progression**: the first token of each line is an ordinal (`1st`, `2nd`, `3rd`, `4th`…,
  Italian `1°`, or `Level 1`/`Livello 1`), the levels increasing by one; the rest split on runs of
  two or more spaces or on single spaces between numeric tokens (`+2`, `1d8`, `—`). A preceding
  line splits the same way into the header row. Ragged rows are problems.

## Slice 3: word boxes and Column grid

- The worker gains one operation `Words` for one page: MuPDF's text page (same flags as render),
  grouped into words by whitespace and font-size gaps, each word `{text, box}` in the same
  corrected page space as `TextLine` (PDF points, the unit polygons' space). Bounds: at most
  20,000 words per page, word text at most 256 bytes, boxes finite and inside the page (validated
  by the parent like `validate_lines`); a new `WORDS_LIMIT` on the frame. Same sandbox, deadline
  and cancel as render; a hostile-corpus test and record tests like `TextLine`'s.
- Source PDFs live in `sources.pdf` (BLOB). On demand (chosen over storing words at import: one
  path for every source, no migration or backfill job, words never persisted): the server streams
  the BLOB with rusqlite `blob_open` to a private 0600 file in the data directory's staging area
  (never `std::env::temp_dir()`: `/tmp` may be RAM), with the uploads' disk reservation, removes
  it after the call, runs `Words` for the page's PDF page index, and keeps the words whose box
  centre lies inside one of the selected units' polygons. One words job at a time process-wide
  through a non-queuing semaphore like `src/sources/region.rs:16`; when busy, unavailable or
  failed, the form says so and the text shapes still work. An in-memory LRU of 8 pages keyed by
  (`pdf_hash`, PDF page index); never stored, never evidence.
- **Column grid**: rows from words grouped by baseline (y overlap > 50 %), columns from
  clusters that recur in at least half the rows (gap-based clustering, tolerance 0.6 × median
  character width; left edges for text, right edges for numeric cells, centres as the fallback); a cell is the words between two column starts. A first row with non-numeric
  cells over mostly numeric rows is the header. Without word boxes the shape falls back to
  splitting each line on runs of two or more spaces, scored lower.
- Coordinates: unit polygons and words share PDF points. Bundle import refuses rotated pages
  (`src/sources/import.rs:161`), but the worker handles `/Rotate`
  (`src/sources/worker/operations.rs:118`): `Words` uses the same corrected space as render's
  lines, and a rotated-page test checks the words land inside the matching boxes.

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

- Each shape: unit tests for a clean fit, continuation lines, a caption, each problem kind, and a
  near-miss that must lose to Plain columns; property test that every recogniser terminates and
  stays within bounds on arbitrary input (proptest is already a dev dependency if present; else
  a fixed adversarial set). `proptest` is already a dev dependency. Near-miss fixtures that must
  not become roll tables: prose starting with numbers ("2 hours later"), page numbers, footnote
  markers, a plain 6-item ordered list.
- The form POST stays within the 320 KiB body limit for the largest accepted grid of non-ASCII
  text, or the refusal is a clear message, not an opaque 413 (test it).
- Route tests: the form with `shape=`, the header checkbox producing `th`, the 200-unit selection,
  Plain columns refused above 16, busy/unavailable words. Worker: record validation, hostile
  corpus, one live words test like render's.
- Fixtures are synthetic; no manual text in the repository.
- `docs/sources-and-retrieval.md` "Owner-built tables" updated; L-SRC-TABLE updated; decision
  SRC-19 (deterministic table shapes are suggestions; words never evidence).
- Order: slices 1→2→3→4, each its own commit series on branch `table-shapes`, one review per
  slice, full gate before merge.
