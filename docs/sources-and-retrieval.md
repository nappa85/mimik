# Sources and retrieval

A **source** is one prepared extraction run of a locally owned manual: the original PDF, the Marker structured extraction (`source.json`), the prepared manifest (`bundle.json`) and one original-page PNG per selected page. Sources are stored privately in SQLite and reviewed by the Owner. Reviewed units feed lexical lookup. Nothing in this workflow lets a model approve evidence or change sheets or rules.

Code: `src/sources/`, `src/auth/materials.rs`, migrations `003`, `004`, `005`, `007`, `009`, `015`, `017`, `024`, `assets/source-regions.js`. Tests: `tests/source_*.rs`, `src/campaigns/material_tests.rs`, `tools/test_source_tools.py`.

## Import (`mimik import-source --bundle DIR --pdf FILE`)

Only the operator imports sources, through the CLI. The browser cannot upload, and there are no archives, URLs or nested imports. `Store::import_source` reads everything first (`load_import`) and then writes it all in one transaction.

**File access.** The directory is opened `O_DIRECTORY|O_NOFOLLOW`. Its children are opened with `openat` under fixed names: `bundle.json`, `source.json`, `page-<pdf index>.png`. `local_file::read` rejects final symlinks and special files and reads with a byte bound. Symlinks in ancestor directories are allowed. Each file is hashed and parsed from the same bytes.

**Bundle schema** (`deny_unknown_fields`): `contract_version`, `source_id` (≤100 B), `title` (≤200 B), `system_id` (a valid registry slug), `edition`, `language` (≤30 B), `source_sha256`, `extraction_sha256`, `extractor` and `renderer` provenance (name ≤100 B, version ≤200 B, `settings` an object), and `pages[]` (`pdf_page`, `width_points`, `height_points`, `image_sha256`, `units[]` with `id`, `kind`, `polygon`, `html`).
- **v1**: no `edition_id` (not even null); label ≤100 bytes; a known family (`crate::SYSTEMS`) may stay unresolved, an unknown one needs a registered alias.
- **v2**: `edition_id` slug of a registered pair; label equal to its registered label or an exact same-pair alias, ≤100 scalars (`registry::source_identity`). Identity and any existing association are checked before the duplicate-hash path.

**Checks** (all fail closed with one generic "inconsistent bundle" error):
- The PDF and `source.json` hashes match the bundle. The PDF parses with `lopdf`; its page count becomes `total_pages`.
- Each page: a unique one-based index present in the PDF; one raw `Page` node; zero rotation; CropBox or MediaBox origin 0,0; 1–4,000 points per side, within 2 points of the declared size; raw page polygon equal to the declared rectangle.
- Units must match exactly the eligible raw leaf nodes: same id, kind, polygon and HTML, with no extras and none missing. Eligible kinds are in `schemas/source-kinds.json` (text, list, heading, table, form, caption, code, footnote, equation and similar). Kinds in `schemas/source-leaf-kinds.json` (`ListGroup`) are units only when they have no children: Marker's OCR gives a whole list as one `ListGroup`, its text-layer path a `ListGroup` of `ListItem` units. A bundle prepared before this rule that left such a group out is refused (SRC-10). An eligible node with children or a `<content-ref` is rejected, so a table or form is one unit and nested groups are never flattened. Other kinds (pictures, page headers) stay only in the raw tree and the page image.
- Polygons have exactly 4 finite points inside the page. Block ids are at most 512 bytes. The raw tree depth is at most 32, with at most 40,000 nodes per page.
- PNG: hash matches; no APNG; at most 4,096×8,192 px and 12 Mpx; aspect within 0.01 of the page; full decode within 64 MiB.
- Size limits: PDF 128 MiB, `source.json` 32 MiB, `bundle.json` 8 MiB, 64 pages, 20,000 units, 64 KiB HTML per unit, 12 MiB per PNG and 128 MiB of PNGs in total. Over-limit input is rejected, never truncated.
- Every unit's HTML must pass the sanitiser and both text projections, or the import fails.

**Storage.** A `sources` row holds the PDF, raw JSON and prepared JSON as BLOBs under a unique `bundle_hash` (SHA-256 of `bundle.json`). Pages hold their PNG; units hold raw HTML, `raw_text` (html2text, width 120) and a `source_search_text` row. All units start `unreviewed`.

**Re-import and finalisation.** Re-importing the same `bundle_hash` is idempotent: `verify_existing` compares imported artefacts and extracted units (including derived text), fills a legacy `total_pages=0` and keeps review state and owner-added units. Every import writes `source_finalizations` and `source_page_hashes`; triggers make artefacts, extracted membership and the ruleset association immutable and undeletable. A different bundle hash is a separate, fully unreviewed source; runs are never merged. Raw HTML is immutable for all sources. Imported units have `origin='extracted'`; their equality to raw leaf nodes is unchanged.

## Documents and primary pages

`source_documents` identifies a PDF by its unique hash, with immutable `source_document_members`
linking each reading to its document. The PDF bytes remain on `sources` in schema 24.
`source_primary_pages` selects at most one source per document/PDF-page pair. Selection does
not review units or change their state. Lookup excludes every non-primary reading, even if
the primary reading is incomplete or outside the requested ruleset/source set.

A new import selects itself for a page only when no other reading of that document covers it,
recording `source_primary_events` with actor `import` in the import transaction. A page with
overlapping readings stays unresolved, including when a later reading is imported onto a legacy
overlap. Re-import does not change selection. A registered source cannot change or lose its
`pdf_hash` or id (triggers on `sources`), so membership stays consistent with its document. A
primary decision needs a member source with the document's hash and a finalization row;
migration 024 still backfills the unique reading of an unfinalized legacy source, which keeps
that selection but cannot be selected again until it is finalized. Migration
024 groups legacy readings by PDF hash, selects pages covered by exactly one reading, and
leaves overlapping pages unresolved. It preserves existing source, page, unit and review ids,
artefacts and hashes. `EvidenceSearch.needs_primary_choice` reports distinct unresolved document
pages for the requested edition: these need the owner's choice. The search page shows them as
their own state, apart from an unsupported source. The operator recovers them with
`mimik set-primary-source --document-pdf-sha256 HASH --pages 3,5-7 --source ID --rationale TEXT`
(pages ascending and distinct, at most 1,024; actor `operator:cli`; the source must cover every
page and be finalized).

`Store::source_primary_state` returns page/source/latest-event tokens. `set_primary_source`
checks all expected tokens before changing any page; the event token detects intervening
A → B → A changes. The chosen source must cover each requested page in the document. A
transaction appends the actor, rationale, previous source and new source, then updates the
selection through a trigger. Primary history and authenticated `source_primary_web_events`
attribution are retained and immutable. The Owner-bound store wrapper rechecks Owner and
CSRF before and after writing each decision and its sidecar; browser controls are a separate
review-screen step.

## Review

States: `unreviewed`, `accepted`, `corrected` and `rejected` (a CHECK constraint).
- **Unit decision** (`store::review_unit`): accepted, corrected or rejected. Only `corrected` carries a correction (plain text, ≤20,000 scalars, no NUL, separate from raw HTML). Actor (≤100) and rationale (≤5,000) are required. A stale expected revision gives `Conflict`. The update and an append-only `source_review_events` row share one transaction; triggers advance the revision by one and reset the page to `complete=0`, `revision+1`.
- **Page completeness** (`store::complete_page`): expected page revision, no unreviewed units, a printed label (1–50 scalars; `unnumbered` allowed), actor and rationale; writes a `page-complete` event. Triggers (007, 009) enforce the same in SQL, and history is append-only.
- **Owner-added text** (`Store::add_owner_unit`): expected page revision, four finite, strictly convex polygon points inside the original page, plain text, actor and rationale. A finalized source accepts this insert only as `origin='owner'`, `kind='Text'`, reserved `owner:<positive integer>` identity, revision 1 and `unreviewed` state. Extracted ids cannot use that namespace; origin never changes. The transaction inserts escaped text inside `<p>`, derives both text projections, then records a normal `corrected` decision with the supplied text. Insertion and correction each reset completeness and advance the page revision once. An unreviewed owner unit prevents completion. The insert-then-correct pairing is enforced by the store: SQL also accepts the unreviewed owner insert (it resets the page, and the unit is then reviewable like any other), so a direct insert without the correction leaves an unreviewed unit rather than a decision. Owner block ids are ASCII digits only (no NUL), at most 25 bytes; an ordinal beyond the integer range is refused. The authenticated store wrapper writes the normal review sidecar in the same transaction.
- **Atomic multi-unit decisions** (`Store::review_units_atomic`): 1–1,024 distinct units and one rationale, under a store-generated operation id (128-bit random, one per batch, exposed as `ReviewEvent.operation_id`). A trigger lets an operation id be reused only by the latest event with the same actor and rationale. All expected unit revisions are checked first. Each update, review event (with nullable `operation_id`) and sidecar hook runs in order before the next unit changes, preserving migration 015's latest-event attribution. A stale revision or any hook failure rolls back the entire batch. The authenticated wrapper forces the account actor and rechecks Owner and CSRF at both transaction boundaries.
- **Authenticated mode** (default; Owner only; `/admin/materials[/sources/{id}|/pages/{id}]`): actor forced to `account:<id>`, plus an immutable `source_web_events` sidecar (account, epoch, session digest; migration 015). Owner and CSRF are rechecked in the same IMMEDIATE transaction. Lists page at 50 rows (independent unit and history cursors); text reads are byte-guarded; images ≤16 MiB; POST ≤320 KiB. 409/422 re-render with the draft kept.
- **`--no-auth` loopback prototype** (`/sources`, `/sources/pages/{id}`, `/sources/search`, `/sources/setup`): free-text reviewer label, latest 100 events. Denied (403) in authenticated mode.
- Display HTML is sanitised with `ammonia` (text, list, heading, table tags; only `colspan`/`rowspan` 1–100, other span values reject the unit; no URLs, styles or scripts).
- Directories show selected pages against the PDF total, resolved or unresolved identity, and finalisation, which does not mean reviewed.

## Stored suggestions

`Store::insert_source_suggestion` records advice separately in `source_suggestions`, bound to
the exact source and its bundle hash. Origin (tool/reviewer name) and kind are each 1–100
characters; rationale is 1–5,000; suggested text is nullable and at most 20,000; text fields
reject NUL. `kind` is one of `correction` (one unit, text), `merge` (2–1,024 distinct units on one page, text),
`missing_text` (page region, text) or `reject` (one unit, no text); SQL enforces the pairing of kind,
target shape and text, with at most 64 KiB of target JSON. Units/pages must belong to the source; every unit target records
a positive revision no newer than its current revision. Older revisions remain identifiable
as stale. Region geometry has the same bounds as owner units, checked in SQL and Rust (page dimensions that are not numbers are `Invalid`). Advice is immutable; the first
`dismiss_source_suggestion` records actor and time, and repeat dismissal conflicts. Neither
insertion nor dismissal changes units, their history, the search index or page completeness.
Report-file import and browser suggestion controls are not exposed.

## Page images and region previews

`/…/pages/{id}/image` serves the original PNG (`no-store`, `nosniff`). `/…/pages/{page}/units/{unit}/image` serves a **derived bounding-rectangle crop** (`region::crop`): not a polygon mask, not evidence, no state change.
- One process-wide crop job (`Semaphore(1)`), no queue; busy gives 503 `Retry-After: 1`. `assets/source-regions.js` retries a lazy preview up to 4 times, then points to the full page.
- Limits: PNG and prepared JSON ≤24 MiB (SQL guard), polygon ≤1,024 B, ≤10,000 page geometries (unit data skipped), decoder 64 MiB, decoded/crop buffers ≤48 MiB, output ≤4 MiB (error, not truncation). Zero-area rectangles are rejected.
- Outward rounding; indexed/alpha expanded; 16-bit becomes 8-bit; gamma, chromaticities, sRGB and ICC kept. cICP/mDCV/cLLI give 422 (png 0.18 cannot encode them).

## Lookup

Lexical FTS5 retrieval is in master (migration 017, `src/sources/search.rs`). There is no vector index and no reranking.
- **Index**: `source_search_text` holds unwrapped text from `search_text()` (sanitiser, then html2text raw mode without table borders, width 2^20, ≤256 KiB; `SEARCH_NORMALISER = 1`, golden-tested). `source_search_fts` is FTS5 `unicode61 remove_diacritics 2`, `prefix='3'`, rowid = unit id, holding only accepted search text or corrected corrections. Triggers maintain it on every review; reviewed units cannot be inserted, units without a search row cannot be accepted, and accepted search text cannot be deleted.
- **Open-time maintenance** (`search::ensure` in `Store::open`'s migration transaction): missing or outdated rows are derived in batches of 64 (64 KiB raw-HTML guard). Unnormalisable units go to `source_search_failures`, stay unindexed and produce a startup warning (`src/main.rs`); the open succeeds. The index is rebuilt (plus FTS5 integrity check) only when indexed and eligible counts differ. `Store::rebuild_search_index` has no CLI. Repair a failed unit by reviewing it `corrected` or `rejected`, or import a corrected bundle.
- **Query** (`fts_query`): 1–200 characters, no control characters; words and balanced `"phrases"`, at most 16; pieces without letters or digits are dropped (none left: `Invalid`). Every piece is a quoted FTS5 string, so no operators can be injected. Words of 3+ letters/digits become `("w" OR "w"*)` so exact matches outrank prefixes; pieces are ANDed. Case and diacritics are ignored; mid-word substrings do not match; flattened table cells let a phrase span adjacent cells.
- **Filtering and ranking** (`match_sql`, FTS-driven `CROSS JOIN`): the same statement as MATCH requires the page's primary reading, the exact edition (registered `game_id`/`edition_id`, or an unresolved legacy `system_id`/`edition` excluding associated sources), `p.complete=1`, an accepted or corrected unit, and the optional source set. Order: `bm25`, source, PDF page, unit; 51 fetched, 50 returned, `truncated` set. bm25 statistics are table-wide, so cite `unit_id` + `unit_revision`, never rank.
- **Result** (`EvidenceSearch`): primary-reading counts (`source_count`, `incomplete_pages`, `partial_runs`, `unindexed_units`), `needs_primary_choice`, `truncated`, and byte-guarded `Evidence` (unit, block, kind, polygon, revisions, state, page, source, labels, effective text). `source_count` counts readings of the requested edition (the unresolved-choice state is separate); incomplete and unindexed counts exclude non-primary pages. `partial_runs` counts distinct documents with unknown total or incomplete primary-page coverage for the requested edition, including coverage split across readings. Unresolved-primary counts include matching readings without selecting one. Coverage counts describe the edition as a whole; the optional source set further restricts matches. Unsupported source, unresolved choice, incomplete coverage, index gap and no reviewed match are distinct result states.
- **Budget**: a progress handler stops all statements of one lookup after 1 s (checked every 10,000 VM ops): "query too broad". It cannot interrupt one long FTS5 `xFilter` call.
- **Entry points**: `Store::search_evidence` (label resolved through the registry first), `search_ruleset_evidence`, `search_ruleset_sources` (1–32 source ids; tests only). The only UI is `/sources/search` under `--no-auth` (either `edition_id` or `edition`); authenticated mode has no lookup page.

## Extraction and preparation tooling (prototype, containers only)

Optional Python; never shipped. `containers/extraction.Dockerfile`: digest-pinned python:3.12-slim, mupdf-tools, CPU PyTorch 2.8.0, marker-pdf 2.0.0, surya-ocr 0.22.1, jsonschema 4.25.1 (`tools/requirements-extraction.txt`; resolved `pip freeze` kept in the image). Run as your user with dropped capabilities, `no-new-privileges`, CPU/memory/PID limits, read-only PDF, writable output and a cache volume.

- `tools/ocr_server.sh start|stop` serves surya's OCR model (`datalab-to/surya-ocr-2-gguf` at a pinned revision, hash-checked, modified OpenRAIL-M) with the pinned llama.cpp b11609 in a container on the internal network `mimik-ocr-net`; It refuses an existing `mimik-ocr-net` that is not internal, runs with CPU, memory and PID limits (2 cores on weekdays 09:00–18:00, `CPUS` otherwise) and refuses `DEVICE=gpu` (the Vulkan build, image encoder on the CPU) on weekdays 09:00–18:00. It does not stop itself: run `stop` after the extraction.
- `tools/extract_manual.py PDF --source-id ID --pages 28,30-33 [--mode fast|balanced] [--ocr auto|force|disabled] [--ocr-server URL]`, run on `mimik-ocr-net` when OCR is on. One-based pages, converted for Marker; `use_llm=False`; headers and footers kept; JSON and Markdown from one build. OCR needs `--ocr-server` (surya attaches to it, never spawns or downloads); the manifest records what the server reports (served model, which must be surya-ocr-2 alone, model path and llama.cpp build) and the surya version, not a hash of the weights (those are checked by `ocr_server.sh` at start), and any "OCR failed" warning fails the run, since Marker would keep the block without its text. Forced OCR is the route for art-heavy manuals (`docs/results.md`, extractor screen). Writes `data/extractions/<id>-<uuid>/` (`source.json`, `reading.md`, metadata, `manifest.json` with hashes/config/versions/status, `review.json` all unreviewed). Mismatched coverage marks the run `failed`.
- `tools/prepare_review.py EXTRACTION --pdf --title --system --edition --language` (can run with `--network=none`). Verifies the snapshots, renders the verified temporary PDF copy with `mutool draw -r 110` at original indexes, checks rotation, origin and size, and emits `review-ready-<uuid>/` (`bundle.json`, `source.json`, `page-N.png`). It writes **contract v1 only**.
- `tools/crosscheck_text_layer.py BUNDLE --pdf FILE` aligns each unit's text with the PDF text-layer words inside its box and writes `text-layer-check-<uuid>/report.json` in the reviewer report's shape: spelling suggestions (an OCR word with a near-identical text-layer twin, same digits and non-letter characters), differences without a correction (a twin differing in digits, dice, signs or markers, where either may be the misprint) and unread-word hints (three or more text-layer words the unit's text lacks, each occurrence counted). Suggestions only; nothing is applied.
- `tools/review_extraction.py`: an explicit, owner-billed call sending page images and blocks to an HTTPS OpenAI-compatible endpoint. Key from `MIMIK_REVIEW_API_KEY`/`OPENAI_API_KEY` (environment, then dotenv). Redirects refused, keys not stored, unknown citations rejected; findings are suggestions, never applied.
- Helpers: `tools/source_bundle.py`; tests `tools/test_source_tools.py` (no Marker or provider run).

## Local corpus

Manuals, extractions, renders and databases are local runtime data under ignored paths. They are never bundled. The candidate primary manuals were identified on 2026-10-11 from metadata and front matter. These identifiers are not adapter ids.

| Source ID | PDF pages | Identification |
| --- | ---: | --- |
| `pf2-core-it-2022` | 646 | Pathfinder 2e core, Italian, Sept 2022 printing, pre-remaster |
| `dnd5-phb-it-legacy` | 321 | D&D 5e PHB, Italian, 2014-era; noisy OCR layer |
| `vtm5-core-it-2019` | 432 | Vampire V5, Italian NEED GAMES, 2019 |
| `mork-borg-core-it-2025` | 96 | MÖRK BORG, Italian NEED GAMES, Oct 2025 |
| `mork-borg-core-en-2019` | 96 | MÖRK BORG, English, 2019 |

```text
pf2-core-it-2022         1be771e5c9b1e01260fd547b9e4f6358f2b7c84e2adc75f3e3805be7d4199901
dnd5-phb-it-legacy       84a1967b032ff825219dfba244889802b95dbcb0479c293bc6651d5b8664ee7d
vtm5-core-it-2019        1fc2ab494581067aa92d3760fbe595772d3228a17336557f9ee9e9fbc87ee213
mork-borg-core-it-2025   83bcd0f7354332faa0a2824ce87ff4175c97ce67a3ca17be5c0ae884ce5286fa
mork-borg-core-en-2019   3d2873f68f4cc83598724c28ab820353566c23728d0f34550417c7033e460ac2
```

Known hazards: Pathfinder reference-sheet Markdown loses action-cost glyphs and interleaves sidebars; the D&D text layer has OCR defects; IT and EN MÖRK BORG differ (e.g. occult treasure 9) and are never merged or corrected against each other.

**MÖRK BORG native adapter.** It is pinned to `MÖRK BORG — IT 2025`. Its citations come from a manual visual audit of original PDF pages 28, 30, 31, 32, 33, 34 and 38 (printed 24, 26–29 and 34), recorded in `src/mork_borg.rs` and `src/mork_advice.rs`. These pages were checked by eye. They are not reviewed source units.

## Licensing of sources and assets

Mimik's AGPL-3.0-only grant covers only Mimik's own code. Manuals, official sheet artwork, model weights and extractions keep their own terms. The instance owner must lawfully obtain the materials they import. Marker's code is Apache-2.0, and its weights are under modified OpenRAIL-M terms, read from the upstream README on 2026-10-11; the installed version's terms must be preserved. The root `logo.png` is owner-provided artwork embedded in the binary. Its authorship and redistribution rights are unconfirmed (`assets/README.md`).
