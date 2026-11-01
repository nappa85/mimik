# Route A: the PDF's own text (source import step 4)

Goal: the Owner creates an unreviewed reading from a complete `mimik-staged-v1` upload,
then reviews it against the original renders in the existing material screen.
Umbrella: `docs/source-import-plan.md`, step 4. Remaining work only.

**Before execution:** obtain independent Sol and fresh Opus plan reviews through the
coordinator's `external-agents` setup. Resolve Blocking/Major findings. The pinned-crate
API check and local reference check below are prerequisites, not established capabilities.

## 4a. Worker reading and diagnostics

- Inspect the installed `mupdf =0.8.0` Rust wrappers, `mupdf-sys =0.8.0` bindings/shim and
  bundled MuPDF 1.27.2 headers before implementing. Check block-kind discrimination, line
  direction/writing mode, character Unicode/flags/quads/font/size, structure collection flags,
  structure traversal, role mapping and association of structure nodes to text. A catalog
  `tagged` boolean is not a structure extraction. Record exact available APIs in the source
  reference; prove them with one tagged table/list fixture. Do not assume C capabilities have
  safe Rust wrappers. Any wrapper gap goes through the owner decision below before work proceeds.
- Reuse DEP-29's maintained renderer and existing bounded transport/geometry/PNG validators;
  no second PDF parser. Keep PDF resource/font traversal, tag/MCID resolution, MuPDF objects,
  extraction flags, coordinate correction and glyph decoding entirely in `sources/worker/`.
  The server validates inert records and derives units; it never opens an upload with `lopdf`
  or calls MuPDF. `import-source` retains its separate prepared-bundle checks.
- Add an internal `read_a` operation, requested for 1–8 pages per confined run. Its spec names
  the reading schema/algorithm version and fixed extraction flags. Reopen the hash-verified
  staged PDF **in the worker**; return page facts and require equality with staged inspection
  geometry before accepting text. Reuse the no-network/no-host-write profile, 2 GiB address
  space, 25 s CPU and 30 s wall time per page, cancellation and tree-reap/exit checks.
- Extract ordered text blocks, their ordered lines, and glyphs: stable page-local ordinals,
  block/line bounds, line direction and writing mode, Unicode scalar or explicit undecodable
  marker, original CID/GID diagnostic status, quad/bounds, font reference, size and available
  character flags. Keep image rectangles for coverage, not embedded image bytes. Transform
  all geometry once to the staged displayed space. Preserve ligatures/whitespace explicitly;
  no spelling fixes, dehyphenation, digit/sign normalization or fabricated Unicode from CIDs.
- Preserve page-relevant tag nodes, parent/ordered-child links, standard and original role,
  and resolved content references when tagged; retain text not attached to tags too. Bound
  document-tree traversal even for one selected page. Mark absent, usable, malformed or
  incomplete structure distinctly. Never silently discard text or trust a tag's claimed order
  as approval. Unresolvable/broken tags use the flagged block fallback below; extraction-limit
  exhaustion fails the reading rather than publishing a partial raw reading.
  Convert malformed PDF tag cycles/references into bounded status diagnostics in the worker;
  never serialize a cyclic or dangling graph for the parent to repair.
- Extend 3a framing with `reading_page` (page number, schema version, length) and a matching
  `done` count/operation. Per page: exactly `page`, `stats`, `reading_page`, in request order;
  then `done`, EOF and exit zero. Unknown versions/fields, duplicates, missing or unsolicited
  records, inconsistent counts, terminal-error/exit disagreement and trailing bytes fail closed.
  Keep 4 KiB headers, 2 KiB inspection-page frames, 4 KiB stats and 4 KiB complete `done`.
  Reading payload ≤8 MiB/page; total wire budget `(8 MiB + 16 KiB) × pages + 4 KiB`, headers
  included. Test maximum legal frames together and one byte over, not just isolated records.
- Use closed, flat bounded arrays, not recursive deserialization: ≤40,000 total block/line/tag
  nodes, ≤20,000 lines, ≤1,000,000 glyphs and ≤20,000 image rectangles per page; ≤256 font
  entries (name ≤64 UTF-8 bytes); tag role ≤100 bytes; depth ≤32; references are page-local
  indices, each glyph has exactly one line/block owner. Walk ≤40,000 structure/resource
  objects per run, with visited indirect identities and depth bound. Reject cycles, duplicate
  identities, dangling/out-of-page references, impossible parentage and inconsistent text/counts.
  The 8 MiB byte cap also applies when count caps allow more. Reuse bounded collection/string
  visitors and limited serialization; all floats finite, sizes positive, directions valid,
  boxes ordered and inside page, quads valid or explicitly clipped. Keep clipped-away/zero-area
  glyph diagnostics; do not invent a positive-area unit for them. Check containment and
  aggregate counts on both sides of the pipe. No source text in diagnostics or logs.
- Keep render's existing `text` line-array contract for 3c proposals. Add bounded numeric
  diagnostics to render/probe stats: words, font count, fonts without `/ToUnicode`, undecodable
  glyphs, coverage and completeness. Use the existing bounded resource scan (Forms, patterns,
  appearances); distinguish absent `/ToUnicode` from failed decoding, including standard
  encodings and Type3/scan gaps. Old staged stats mean “not measured”, never zero. Add a
  stats-only confined probe job for old renders; retain their current PNGs and line sidecars.

## 4b. One raw-reading → units contract

- Define prepared contract **v3**, requiring registered edition identity as v2 does, and raw
  **`mimik-reading-v1`**: PDF hash, selected pages, displayed page facts/image hashes, route,
  pinned extractor/settings, tag status, validated blocks/lines/glyphs/tags and diagnostics.
  Persist immutable raw reading bytes separately from the prepared manifest; hashes bind both.
  Keep raw ≤32 MiB/source, prepared ≤8 MiB, ≤64 pages, ≤20,000 units, ≤64 KiB HTML/unit,
  ≤128 MiB PNG total and existing per-PNG/PDF bounds. Fail with “select fewer pages”, never
  truncate. Process pages incrementally and bound retained raw/derived allocations before SQL.
- Implement one pure `derive_units_v1(validated_reading) -> Result<ordered pages/units>` shared
  by creation, v3 verification and tests. Inputs contain no review state, exclusion decision,
  clock, database ID or model output. Outputs have block identity, kind, four-point polygon,
  escaped HTML and raw-node membership; reuse existing sanitiser and text/search projections.
- Pin the partition: with usable tags, a complete `Table` subtree is one `Table` unit, a
  complete `LI` subtree one `ListItem`, a heading one `SectionHeader`, and a paragraph one
  `Text`. Select outermost eligible ancestors (table before its cells, list item before its
  paragraphs), never emit their descendants again. Use explicit content order and emit every
  unclaimed nonempty text block as one `Text` fallback. A partial/ambiguous association disables
  tag partitioning for that page and flags it; no guessing associations from role names alone.
  Without usable tags, one nonempty MuPDF text block is one `Text` unit, its lines joined by LF
  in recorded order. Do not merge nearby columns or split at every glyph/line. Treat images,
  empty tags and page furniture as raw metadata unless they contain extracted text; do not
  hide a watermark by classifying it as furniture.
- Text is glyph Unicode in recorded order, with U+FFFD for explicit undecodable glyphs; escape
  it before fixed HTML wrapping. Preserve line breaks with fixed markup. For tagged tables,
  retain explicit row/cell order and bounded spans (1–100); incomplete table associations use
  the page fallback. Geometry is the clipped bounding rectangle of member glyphs, in fixed
  top-left clockwise point order. Refuse a nonempty unit with no positive-area geometry.
  Stable IDs use PDF page and selected raw node ordinal (`/page/<zero-based>/a/<ordinal>`),
  never `owner:`. Page order is ascending PDF index; units follow recorded tag/block order,
  with unclaimed blocks following in original block order. Pin tie-breaking and float/JSON
  serialization; no unordered-map iteration. Same raw bytes/version produce identical units.
- For v3, replace literal “units equal raw leaves” with exact equality to `derive_units_v1`:
  IDs, order, kinds, geometry, HTML and membership, no omitted or extra extracted units.
  Glyphs/lines are raw leaves but are not individually review units. Keep v1/v2's Marker
  `collect_units`/SRC-10 rule unchanged; never reinterpret existing immutable sources.
  Owner-added units remain outside extracted equality. Review/corrections never mutate raw.
  Dispatch raw-reading consumers by contract, including suggestion-report node citations and
  reimport verification; retain the existing prepared `pages[].units[]` shape for review bounds.
  Keep CLI v1/v2 import supported; exposing v3 prepared files is a separate interface decision,
  not permission to route browser uploads through `load_import` or its `lopdf` checks.
- Pin synthetic golden cases for tagged tables/lists/paragraphs, fallback, columns, ligatures,
  RTL/vertical/rotated text, missing Unicode, duplicate text, clipping and zero-area geometry.
  Check repeated runs and serialized round trips; tamper each output field, omit/add/reorder
  units and require refusal. Test source/raw/prepared byte and count boundaries. Retain legacy
  v1/v2 and childless `ListGroup` fixtures through the real importer.

## 4c. Staged upload → finalized unreviewed reading

- Add Owner/Host/Origin/CSRF-protected “Read with PDF text” on the rendered upload, carrying
  upload ID, expected upload **and exclusion** revisions, title (≤200 bytes), and explicit
  confirmation of the exclusion's listed effects/rationale. Generate the run/source identity;
  never use the original filename. A route recommendation is advisory; A remains selectable.
  Run the CPU reading as a cancellable staging-manager job, one worker/job at a time. Extend
  job kind/state validation and restart failure handling; no long worker work on HTTP/SQL locks.
- Under the existing control → publication/deletion lock order, claim a complete current
  contract and immutable input handles. A saved `contract()` alone is not a lease. Capture
  both revisions, selected pages, PDF/image hashes/settings and admission. Register active work
  so sweep/discard cannot delete its inputs. Selection cancels/reaps it; metadata or exclusion
  edits make its result stale (do not silently rebase it). Probe results also carry revisions.
- Read bounded no-follow regular files and verify hashes from the very bytes consumed. Copy
  PNGs unchanged; validate them again. Take total page count/rotation/transform/size from staged
  inspection, not a server PDF parse. Derive raw/manifest/units outside the Store mutex; reserve
  space for permanent SQLite BLOBs and rollback-journal growth in addition to staging's ledger
  and 512 MiB margin. Budget the PDF, raw, manifest and PNG copies before admission.
- In one IMMEDIATE transaction, recheck Owner/session-bound CSRF, both revisions, current
  selection/render identities and active job; resolve registered edition. Insert source/raw/
  manifest/PDF BLOBs, pages with original PNGs, immutable page facts/probe provenance, extracted
  units initially `unreviewed`, search projections, page hashes and finalization. Add permanent
  exclusion provenance (pages, geometry, rationale, revisions, confirming actor/time and opaque
  fingerprint), crossing flags and explicit rejection-event links. Migration guards make these
  facts immutable without weakening 007/015/017. Obtain exclusion attribution from its staging
  row: `StagedContract.exclusion` alone lacks actor/time. Recheck it in the transaction.
- Apply the confirmed geometry **after** derivation: wholly contained unit rectangles (inclusive
  edges) receive attributed `rejected` decisions citing the exclusion; positive-area overlap
  without containment stays `unreviewed` and flagged. Mere edge contact is not crossing. Raw
  output stays complete. At creation confirmation the current Owner authorizes these rejections;
  retain the original confirmer separately. Check all expected revision-1 units first, then
  update/event/authenticated sidecar per unit, sharing operation ID/rationale. Reuse review
  primitives; explicitly handle >1,024 excluded units by bounded internal chunks within this
  same transaction/operation, preserving the public batch limit. Any failure rolls everything
  back. List decisions and allow normal revision-checked undo; never auto-complete a page.
- Reuse `documents::register_import`: hash-keyed document/member, first uncovered page primary,
  overlaps subject to the existing owner-choice rules. Do not replace a primary reading or
  merge runs. Schema 24 still stores PDF bytes per source; PDF deduplication (C5) is not an
  incidental migration in this slice.
- Commit source creation and upload-row deletion (including exclusions and fingerprint key)
  together; retain an upload-ID → source-ID completion receipt without private staging data
  for retry/lost-response idempotency. Only after commit remove the staging folder and charge
  surviving bytes until cleanup succeeds; startup/hourly orphan cleanup retries. Failure before
  commit retains retryable staging; crash after commit cannot produce another reading. Copy
  **no** proposal, fingerprint secret or `text-N.json` file. The new immutable raw reading is
  freshly serialized extraction, not a retained staging sidecar. Fingerprint is provenance
  only after its key is deleted.
- Redirect to the existing source page list/review screen. Show route/versions, probe and tag
  warnings, primary state, unreviewed/incomplete status, crossing flags and exclusion decisions.
  Mask confirmed regions on review images (including derived previews); keep original evidence
  PNG endpoints clearly labelled. Any hints built for A/B or later C suppress excluded glyphs
  before use, including crossing-line portions; never filter the stored raw reading. Source
  suggestions, owner additions and ordinary accept/correct/reject/completion continue unchanged.

## Probe: freeze before manual measurements

- Define `probe-v1` on each selected page: words are maximal runs of Unicode letters/numbers
  in decoded line text (combining marks continue a run), U+FFFD/CID placeholders do not count;
  report count and words per 10,000 displayed pt², image rectangle union/page area (reuse
  `image_coverage`), missing `/ToUnicode` font count and undecodable/total glyph count. No
  inferred “correct words”; no model. Preserve scan-incomplete and dropped-text warnings.
- Proposed frozen rule: recommend **B or C** if words <120, image coverage ≥0.20, any undecodable
  glyph, missing-map font, incomplete diagnostics or malformed/incomplete tags; otherwise A.
  Show all triggered reasons and measurements, including words versus image fraction, rather
  than a confidence score. Missing maps can still decode via standard encodings: label the
  conservative warning accurately. No-text and unknown diagnostics never recommend A.
  B is local/Linux and C requires explicit online consent; unavailable routes stay disabled.
- Show per-page recommendations and a selection summary on the rendered upload **before route
  choice**; persist the exact rule/version/result with the created reading and show it on its
  page list/review. Extend render stats for new uploads and offer the probe job for older ones.
  Test threshold equality, mixed selections, standard-font mapping and incomplete scans.

## Bars, fixed before measuring (umbrella step 4)

- **“MÖRK BORG Bare Bones EN (local, licence and hash recorded in the corpus table): a
  hand-checked reference of the weapons, armour, equipment and character-creation pages (rows
  counted as in the IT reference); at least 98% of rows intact in one unit, no digit, dice or
  sign error.”** The Owner, or an owner-named local human transcriber, makes the reference by
  eye from original page images, then the Owner checks every row before extraction is scored.
  Freeze PDF SHA-256/licence, exact one-based page list, reference hash, row IDs and denominator
  N before the first measurement. Use the IT reference's actual local format/scorer, one fact
  per row (including wrapped facts as one row; table headers are not extra facts). First inspect
  that local format and preserve its field names/normalization with a synthetic equivalent in
  CI; do not invent a substitute and call it equivalent. Required logical fields: PDF page,
  row identity and reference text; keep numeric/dice/sign tokens and hand-check attribution.
  Score uncorrected derived units: intact iff the full reference row occurs in order in one
  unit after only the IT scorer's frozen normalization. Pass iff intact ≥ceil(0.98×N); report
  numerator/denominator overall and by section. Independently inspect every reference numeric,
  dice and sign occurrence against raw units and images, including missing/split rows; zero
  substitutions/omissions/additions. A fuzzy match cannot excuse one. Record present-in-page
  and fuzzy scores descriptively with the IT scorer, not as acceptance substitutes.
- **“MÖRK BORG IT pages 22–39: the probe recommends route B or C on every art-heavy page (all
  18), and route A's scores are reported, with no bar.”** Denominator 18 for recommendations;
  report each page's result/reasons and require 18/18. Report intact, present and fuzzy on the
  existing 161-row reference (PDF pages 23–30), explicitly distinguishing the scored subset
  from the full probe range; inspect/report numeric/dice/sign errors. Never compare EN facts
  to IT facts as corrections. If frozen thresholds fail, report failure; any new rule gets
  reviewed/versioned and remeasured, not retrospectively declared pre-registered.
- **“Ten synthetic hostile or odd PDFs (rotated, no text layer, broken tags): no crash, correct
  probe result.”** Freeze ten generated inputs with expected outcomes: (1–3) clean ≥120-word
  pages rotated 90/180/270 → A; (4) full-page scan → B/C; (5) blank page → B/C; (6) unresolved
  tag content links → B/C; (7) cyclic structure → bounded broken-tag diagnostic and B/C;
  (8) unmapped CID glyphs → B/C; (9) missing `/ToUnicode` with valid standard encoding → B/C
  with absent-map, not undecodable, reason; (10) ≥20% image union plus dense text → B/C.
  The three clean fixtures include valid Unicode maps and no other warning triggers.
  Assert 10/10 outcomes, aligned glyph/render geometry, no signal crash, deadline overrun,
  leftover worker or published partial source. Keep broader 3a hostile-input refusal tests;
  a fixed refusal is not a successful probe result for these ten recoverable fixtures.
- Keep manuals, references, row text, renders, detailed scores and derived fixtures local under
  ignored data paths; no material goes to reviewers/providers, CI, logs or release archives.
  Only aggregate counts, timings/RSS, version/hash/licence metadata and limitations enter
  `docs/results.md`/the corpus table. Owner reference creation and local scoring are release
  blockers until done, not CI skips counted as passes. Review aggregate conclusions independently.

## Slices, checks and landing

- **4a:** API fit/gap resolution, bounded worker records and probe. Independent review before
  completion; fake-reader max/over-limit/cyclic-reference tests, live sandbox and ten PDFs.
- **4b:** v3/reading schema and one derivation function. Independent review; synthetic goldens,
  mutation/bounds tests, raw equality, legacy import/reimport and sanitized projections.
- **4c:** manager/source transaction, permanent facts/exclusion migration and cleanup receipts.
  Independent review; migration/immutability tests, excluded/crossing/edge cases, >1,024
  decisions, stale revisions, attribution failure rollback, competing creation/discard/sweep,
  selection/metadata/exclusion changes during work, cancellation/restart and disk-full faults.
  Test file tampering, no-follow handling, lost response, and crash before/after SQL commit.
- **4d:** browser route/probe/review integration and local bars. Independent review; synthetic
  browser checks at 1280/390 px with/without JS, strict CSP, retained stale forms, masks,
  unreviewed lookup exclusion, explicit correction/undo/page completion and primary overlap.
  Unpacked release smoke uploads → creates → reviews → restarts and verifies persistence;
  prove release lacks development-worker hooks. Manual-derived fixtures never enter CI.
- Every slice runs the applicable fail-closed gate: fmt, strict Clippy, locked tests, helper
  lint/tests, audit, licensing/notices, release build and unpacked-artifact smoke. Retain
  failing-first regressions for reproducible findings. Separate synthetic/helper, real worker,
  browser and local-manual evidence; two-core cap during weekday working hours.
- When landing, update `sources-and-retrieval.md` (contracts, probe, creation, exclusions),
  schema references, `release-scope.md` (Linux route A), `limitations.md` (narrow only limits
  actually closed), `decisions.md` (approved contract/owner choices), `results.md` and the
  local-corpus licence/hash entry; update CLI reference only if prepared v3 import is exposed.
  Remove finished sections here and the umbrella's finished step; retain only remaining work.

## Decisions the owner must make

- **Pinned wrapper gaps:** options: use sufficient safe 0.8.0 APIs; approve a narrow
  exception-safe worker-only wrapper over its pinned C bindings; or change the renderer pin.
  Recommendation: safe APIs first, then the smallest reviewed wrapper if demonstrated necessary;
  stop before implementation if glyph/tag fidelity is unavailable. A second parser is extra
  attack surface. Exact tag/quad wrapper coverage is unverified in this sandbox.
- **Bare Bones gold:** options: Owner transcribes/checks it, or appoints a local human
  transcriber with Owner checking. Recommendation: delegate transcription if convenient, Owner
  signs off every row and the page set. Name that person and settle the exact IT-compatible
  local format/page list before measuring; do not derive gold from route A or a remote model.
- **Probe policy:** options: approve the conservative `probe-v1` thresholds above, or choose a
  different explicit rule before measurements. Recommendation: approve the conservative rule
  as advice, with visible reasons and Owner override; do not weaken the 18/18 acceptance bar.

## Risks

- Tags can be wrong, incomplete or expensive to resolve; block fallback preserves text but
  may scatter a fact across units. The 98% bar is not established by wrapper availability.
- Glyph-rich raw data can hit 32 MiB before the 64-page cap; refuse with a smaller-selection
  remedy. Measure memory under the existing worker limits before increasing any bound.
- Image coverage misses art drawn as vector paths; missing maps over-warn standard fonts.
  The frozen IT probe check can fail and require a separately reviewed algorithm revision.
- SQLite BLOB copies/journaling block the single Store connection (C6); measure transaction
  duration and disk headroom on maximum synthetic sources. Do not promise filesystem/SQL
  atomicity: the receipt, post-commit deletion and restart reconciliation bridge it.
