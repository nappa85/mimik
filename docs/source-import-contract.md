# Prepared source-review bundle: first Rust vertical slice

## Trust and scope

Directory-only, operator-invoked CLI import. No browser file uploads, archives, arbitrary download URLs, provider execution, or automatic approvals. Container tooling prepares selected original PDF pages; the Rust runtime consumes fixed-name artifacts and stores immutable bytes in SQLite.

Hashes bind identities and detect changes; they do not authenticate that an owner-supplied extraction or render was produced honestly. Reviews compare original images with extraction and remain explicit local-owner decisions, not a universal correctness guarantee.

## Prepared contract v1

`bundle.json` declares contract_version=1, source_id, title, system_id, edition, language, source_sha256, extraction_sha256, extractor/version/settings, renderer/version/settings, and pages. Each page declares the one-based original PDF index, point dimensions, `page-<index>.png` SHA-256, and nonoverlapping review units (original block ID, kind, polygon, normalized raw HTML). The full original Marker `source.json` remains separately hash-bound and immutable.

Preparation verifies the existing extraction manifest/PDF/artifact hashes, checks actual PDF page bounds, renders those original pages using MuPDF, and records dimensions/settings. No reduced-PDF renumbering. Flat units are leaf text/section/list/formula blocks or complete tables/forms; parent groups and table children are not independently indexed. The full raw tree retains ignored graphical/header/footer content, which remains visible in page images and part of the completeness review.

## Prepared contract v2: registered identity

Version2 adds a required string `edition_id`; `system_id` is a registered game ID.
The `edition` label must equal its registered edition label or an exact same-game
alias agreeing with that stable pair. Both IDs are validated slugs; missing/null/
wrong-type IDs and unsupported versions fail. Version1 rejects the extension
field even when null. Version2 label limit is100 Unicode scalar values; v1 retains
its historical100-byte bound. All artifact/geometry/provenance checks are shared.
See `runtime-registry.md` for registration, aliases and transition policy.

Known-family v1 sources can remain unresolved for compatibility; unknown-family
v1 needs an explicit registered alias. v2 requires a registered pair. Import checks
identity and any prior association before duplicate-hash idempotency/backfill;
registration never approves evidence. Source UI shows resolved/unresolved identity.

## Rust validation/storage

- CLI accepts explicit PDF and bundle directory; bounded file reads with final symlinks/special files rejected. Required names are fixed; image names derive from checked original page indexes, not external path strings. No nested imports.
- Linux opens the root directory descriptor and fixed-name children through maintained rustix `open/openat` APIs with no-follow/nonblocking flags, then validates regular-file type on the opened descriptor before reading. Operator-selected ancestor symlinks are permitted; final PDF/root/artifact symlinks are rejected. The root descriptor stays pinned; FIFO/device files are rejected without blocking opens.
- Read/hash/parse the same snapshot bytes. PDF parser checks actual page count; JSON structure and block identifiers must match declared original pages. Enforce unique page/unit IDs, finite four-corner polygons within page bounds, finite point dimensions, and required hashes.
- First version accepts zero-rotation, zero-origin page crop/media boxes only. Point bounds originate at the upper-left of Marker's normalized page; prepared page dimensions must match raw page bounds and actual PDF bounds (up to 2 points for extractor rounding). PNG aspect ratio must match point dimensions. Units must exactly match the eligible raw leaf IDs, kinds, polygons and HTML; nested table/form groups or content references are rejected for now rather than flattened ambiguously. A table/form leaf is one review unit; its children are never independently indexed.
- Limits: PDF 128 MiB, raw extraction 32 MiB, prepared JSON 8 MiB, 64 selected pages, 20,000 units, 64 KiB HTML/unit, PNG 12 MiB/page with bounded decoded dimensions/memory. Reject rather than truncate.
- Persist source PDF/raw extraction/prepared manifest and original PNG bytes as private database BLOBs; one transaction avoids split filesystem/database commits. Artifact routes use database IDs only.
- Existing bundle hash may identify an idempotent import; different runs are separate sources and start entirely unreviewed. Never consume incoming review approvals.

## Review and lookup semantics

Sanitize displayed extraction using maintained HTML sanitization, limited to text/table/list structure with no executable markup, remote fetches, or styles. Corrections are plain text, stored separately from raw HTML. Preserve block type/polygon/page references.

Explicit unit states: unreviewed, accepted, corrected, rejected. Decision actor/rationale required; every change creates an append-only event and increments unit and page revisions transactionally. A unit change invalidates page completeness. Printed labels are owner-confirmed separately from PDF indexes.

Every unit decision/correction requires an expected unit revision and performs its state/correction/event/page invalidation in the same transaction. A stale decision cannot overwrite newer work. Page-completeness decisions likewise require the expected page revision.

Page completeness is a separate explicit decision after every unit is resolved and the owner checks omissions against the original render. It carries actor/rationale and expected page revision. Search uses only accepted/corrected units on complete pages, mandatory exact system/edition filters (optionally an explicit source set), applied in the same statement as ranking. Queries are bounded (1–200 characters, at most 16 words/quoted phrases, no control characters) and user text is only ever passed to FTS5 as quoted strings: words are ANDed and match exactly or as a prefix (3+ letters/digits), `"quoted phrases"` match exactly, case and diacritics are ignored (`unicode61 remove_diacritics 2`), results are bm25-ranked, capped at 50 with an explicit truncation flag, and carry unit/block/polygon/revision identities. The indexed and displayed text is the reviewer correction or an unwrapped normalised projection of the raw HTML (`source_search_text`); immutable `raw_text` and `raw_html` are unchanged (raw HTML updates are rejected by a trigger). A unit whose raw HTML cannot be normalised (oversized, unsanitizable) is recorded in `source_search_failures`, excluded from the index, counted as an index gap on the lookup page and in a startup warning, and cannot be accepted; repair it by reviewing it as `corrected` with a transcription (indexed from the correction) or `rejected`, or by importing a corrected bundle. Failed units are retried automatically when the normaliser version changes. Each lookup has a 1 s SQLite time budget and fails with "query too broad" past it. Table cells are flattened, so a phrase can match across adjacent cells; mid-word substrings do not match. Registered selection uses stable game/edition associations for coverage and matches; exact labels/aliases resolve that pair, while unresolved legacy fallback excludes associated sources. Counts/matches use a coherent read transaction. Unsupported source, incomplete coverage, and no reviewed match are visible states. Search evidence does not automatically establish executable rules or sheet legality.

Tests use synthetic PDFs/bundles/images, covering tampering, page isolation/bounds, geometry/size limits, symlinks, unreviewed imports, XSS, revision conflicts, corrections/history, page completeness, and exact-edition reviewed-only search. Actual Marker batch compatibility and visual correctness are verified separately on local manual pages.
