# Sources and retrieval

A **source** is one prepared extraction run of a locally owned manual: the original PDF, the Marker structured extraction (`source.json`), the prepared manifest (`bundle.json`) and one original-page PNG per selected page. Sources are stored privately in SQLite and reviewed by the Owner. Reviewed units feed lexical lookup. Nothing in this workflow lets a model approve evidence or change sheets or rules.

Code: `src/sources/`, `src/auth/materials.rs`, migrations `003`, `004`, `005`, `007`, `009`, `015`, `017`, `assets/source-regions.js`. Tests: `tests/source_*.rs`, `src/campaigns/material_tests.rs`, `tools/test_source_tools.py`.

## Import (`mimik import-source --bundle DIR --pdf FILE`)

Only the operator imports sources, through the CLI. The browser cannot upload, and there are no archives, URLs or nested imports. `Store::import_source` reads everything first (`load_import`) and then writes it all in one transaction.

**File access.** The directory is opened `O_DIRECTORY|O_NOFOLLOW`. Its children are opened with `openat` under fixed names: `bundle.json`, `source.json`, `page-<pdf index>.png`. `local_file::read` rejects final symlinks and special files and reads with a byte bound. Symlinks in ancestor directories are allowed. Each file is hashed and parsed from the same bytes.

**Bundle schema** (`deny_unknown_fields`): `contract_version`, `source_id` (≤100 B), `title` (≤200 B), `system_id` (a valid registry slug), `edition`, `language` (≤30 B), `source_sha256`, `extraction_sha256`, `extractor` and `renderer` provenance (name ≤100 B, version ≤200 B, `settings` an object), and `pages[]` (`pdf_page`, `width_points`, `height_points`, `image_sha256`, `units[]` with `id`, `kind`, `polygon`, `html`).
- **v1**: no `edition_id` (not even null); label ≤100 bytes; a known family (`crate::SYSTEMS`) may stay unresolved, an unknown one needs a registered alias.
- **v2**: `edition_id` slug of a registered pair; label equal to its registered label or an exact same-pair alias, ≤100 scalars (`registry::source_identity`). Identity and any existing association are checked before the duplicate-hash path.

**Checks** (all fail closed with one generic "inconsistent bundle" error):
- The PDF and `source.json` hashes match the bundle. The PDF parses with `lopdf`; its page count becomes `total_pages`.
- Each page: a unique one-based index present in the PDF; one raw `Page` node; zero rotation; CropBox or MediaBox origin 0,0; 1–4,000 points per side, within 2 points of the declared size; raw page polygon equal to the declared rectangle.
- Units must match exactly the eligible raw leaf nodes: same id, kind, polygon and HTML, with no extras and none missing. Eligible kinds are in `schemas/source-kinds.json` (text, list, heading, table, form, caption, code, footnote, equation and similar). An eligible node with children or a `<content-ref` is rejected, so a table or form is one unit and nested groups are never flattened. Other kinds (pictures, page headers) stay only in the raw tree and the page image.
- Polygons have exactly 4 finite points inside the page. Block ids are at most 512 bytes. The raw tree depth is at most 32, with at most 40,000 nodes per page.
- PNG: hash matches; no APNG; at most 4,096×8,192 px and 12 Mpx; aspect within 0.01 of the page; full decode within 64 MiB.
- Size limits: PDF 128 MiB, `source.json` 32 MiB, `bundle.json` 8 MiB, 64 pages, 20,000 units, 64 KiB HTML per unit, 12 MiB per PNG and 128 MiB of PNGs in total. Over-limit input is rejected, never truncated.
- Every unit's HTML must pass the sanitiser and both text projections, or the import fails.

**Storage.** A `sources` row holds the PDF, raw JSON and prepared JSON as BLOBs under a unique `bundle_hash` (SHA-256 of `bundle.json`). Pages hold their PNG; units hold raw HTML, `raw_text` (html2text, width 120) and a `source_search_text` row. All units start `unreviewed`.

**Re-import and finalisation.** Re-importing the same `bundle_hash` is idempotent: `verify_existing` compares every stored byte and derived text, fills a legacy `total_pages=0` and keeps review state. Every import writes `source_finalizations` and `source_page_hashes`; migration 007 triggers then make artefacts, membership and the ruleset association immutable and undeletable, leaving only review columns mutable. A different bundle hash is a separate, fully unreviewed source; runs are never merged. Raw HTML is immutable for all sources.

## Review

States: `unreviewed`, `accepted`, `corrected` and `rejected` (a CHECK constraint).
- **Unit decision** (`store::review_unit`): accepted, corrected or rejected. Only `corrected` carries a correction (plain text, ≤20,000 scalars, no NUL, separate from raw HTML). Actor (≤100) and rationale (≤5,000) are required. A stale expected revision gives `Conflict`. The update and an append-only `source_review_events` row share one transaction; triggers advance the revision by one and reset the page to `complete=0`, `revision+1`.
- **Page completeness** (`store::complete_page`): expected page revision, no unreviewed units, a printed label (1–50 scalars; `unnumbered` allowed), actor and rationale; writes a `page-complete` event. Triggers (007, 009) enforce the same in SQL, and history is append-only.
- **Authenticated mode** (default; Owner only; `/admin/materials[/sources/{id}|/pages/{id}]`): actor forced to `account:<id>`, plus an immutable `source_web_events` sidecar (account, epoch, session digest; migration 015). Owner and CSRF are rechecked in the same IMMEDIATE transaction. Lists page at 50 rows (independent unit and history cursors); text reads are byte-guarded; images ≤16 MiB; POST ≤320 KiB. 409/422 re-render with the draft kept.
- **`--no-auth` loopback prototype** (`/sources`, `/sources/pages/{id}`, `/sources/search`, `/sources/setup`): free-text reviewer label, latest 100 events. Denied (403) in authenticated mode.
- Display HTML is sanitised with `ammonia` (text, list, heading, table tags; only `colspan`/`rowspan` 1–100, other span values reject the unit; no URLs, styles or scripts).
- Directories show selected pages against the PDF total, resolved or unresolved identity, and finalisation, which does not mean reviewed.

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
- **Filtering and ranking** (`match_sql`, FTS-driven `CROSS JOIN`): the same statement as MATCH requires the exact edition (registered `game_id`/`edition_id`, or an unresolved legacy `system_id`/`edition` excluding associated sources), `p.complete=1`, an accepted or corrected unit, and the optional source set. Order: `bm25`, source, PDF page, unit; 51 fetched, 50 returned, `truncated` set. bm25 statistics are table-wide, so cite `unit_id` + `unit_revision`, never rank.
- **Result** (`EvidenceSearch`): counts (`source_count`, `incomplete_pages`, `partial_runs`, `unindexed_units`), `truncated`, and byte-guarded `Evidence` (unit, block, kind, polygon, revisions, state, page, source, labels, effective text). Unsupported source, incomplete coverage, index gap and no reviewed match are distinct visible states.
- **Budget**: a progress handler stops all statements of one lookup after 1 s (checked every 10,000 VM ops): "query too broad". It cannot interrupt one long FTS5 `xFilter` call.
- **Entry points**: `Store::search_evidence` (label resolved through the registry first), `search_ruleset_evidence`, `search_ruleset_sources` (1–32 source ids; tests only). The only UI is `/sources/search` under `--no-auth` (either `edition_id` or `edition`); authenticated mode has no lookup page.

## Extraction and preparation tooling (prototype, containers only)

Optional Python; never shipped. `containers/extraction.Dockerfile`: digest-pinned python:3.12-slim, mupdf-tools, CPU PyTorch 2.8.0, marker-pdf 2.0.0, surya-ocr 0.22.1, jsonschema 4.25.1 (`tools/requirements-extraction.txt`; resolved `pip freeze` kept in the image). Run as your user with dropped capabilities, `no-new-privileges`, CPU/memory/PID limits, read-only PDF, writable output and a cache volume.

- `tools/extract_manual.py PDF --source-id ID --pages 28,30-33 [--mode fast|balanced] [--ocr auto|force|disabled]`. One-based pages, converted for Marker; `use_llm=False`; headers and footers kept; JSON and Markdown from one build. Writes `data/extractions/<id>-<uuid>/` (`source.json`, `reading.md`, metadata, `manifest.json` with hashes/config/versions/status, `review.json` all unreviewed). Mismatched coverage marks the run `failed`.
- `tools/prepare_review.py EXTRACTION --pdf --title --system --edition --language` (can run with `--network=none`). Verifies the snapshots, renders the verified temporary PDF copy with `mutool draw -r 110` at original indexes, checks rotation, origin and size, and emits `review-ready-<uuid>/` (`bundle.json`, `source.json`, `page-N.png`). It writes **contract v1 only**.
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

Mimik's AGPL-3.0-only grant covers only Mimik's own code. Manuals, official sheet artwork, model weights and extractions keep their own terms. The instance owner must lawfully obtain the materials they import. Marker's code is Apache-2.0, and its weights are under modified OpenRAIL-M terms, read from the upstream README on 2026-10-11; the installed version's terms must be preserved. `assets/logo.png` is owner-provided artwork embedded in the binary. Its authorship and redistribution rights are unconfirmed (`assets/README.md`).
