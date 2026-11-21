# Route A: the PDF's own text (source import step 4)

Goal: the Owner creates an unreviewed reading from a complete `mimik-staged-v1` upload,
then reviews it against the original renders in the existing material screen.
Umbrella: `docs/source-import-plan.md`, step 4. Remaining work only.

## 4a0. Small preflight; freeze before route-A measurements

Settled (2026-11-07; synthetic-PDF tests `src/sources/worker/preflight_tests.rs`, results in
`docs/results.md`; local bar facts in the owner's notes, aggregate only here):

- **API fit.** The vendored fork (DEP-29) exposes per glyph quad, origin, size, argb, flags,
  font name and `bidi`/`is_rtl`, per line `wmode` and `dir`. With the 4a flags (no
  `COLLECT_STRUCTURE`, `SEGMENT` or `TABLE_HUNT`) top-level `blocks()` holds the text. With
  `COLLECT_STRUCTURE` it holds only Struct blocks and the text is reachable only through
  `structure().blocks()`; `SEGMENT` alone also creates Struct blocks (`TABLE_HUNT` or
  `COLLECT_VECTORS` alone do not). `structured()` drops Struct-block text (stays banned). The
  pinned MuPDF has exactly five block kinds (0–4); `TextBlock::r#type()` would panic on another
  raw value, which the safe API cannot pre-check, so such a panic is a contained worker failure,
  never a successful or partial reading.
  Right-to-left and vertical text are untested (no such base-14 fixture); 4a adds them.
- **Coordinates.** MuPDF page space is already the displayed page: y-down, `/Rotate`
  applied, scaled by `UserUnit`, origin at the top-left of CropBox∩MediaBox. 4a applies no
  second rotation; it checks this space against staged inspection geometry before accepting
  text.
- **Clipping (owner: `CLIP` off).** `CLIPPED` marks text that reaches the device as a clip:
  render mode 7 (`FILLED|CLIPPED`) and also visible text filled or stroked with a pattern or
  shading in any mode (`FILLED|CLIPPED` or `STROKED|CLIPPED`, argb 0). So `CLIPPED` cannot tell
  visible shaded text from invisible clip-only text: 4a records it as "possibly invisible",
  never as invisible. Modes 4–6 are visible and unflagged; mode 3 (invisible) has neither
  `FILLED` nor `STROKED`, which 4a marks invisible. Glyphs outside the page box and glyphs
  removed by an in-page clip carry no flag: 4a computes off-page/partly-off-page from quads,
  and in-page clipping stays the documented limit caught by review against the render. With
  `CLIP` on, glyphs wholly outside the clip's bounding box or the page disappear silently;
  partly clipped glyphs and glyphs inside the bounding box of a non-rectangular clip stay.
- **Tags.** IT 2025 is untagged; Bare Bones EN is tagged (catalogue level, including tables).
  The later versioned tag slice stays (owner rule); its cost estimate is part of 4a.
- **Encoding (frozen for `mimik-reading-v1`).** Columnar parallel arrays with a shared font
  table and `i32` coordinates in 1/1024 displayed pt, rounded to nearest with halves away from
  zero (maximum round-trip error 1/2048 pt); all parallel arrays of one table have equal
  lengths; a non-finite value or one whose rounded tick does not fit `i32` (about ±2,097,152
  pt) refuses the page. Off-page statuses are computed from the rounded quads against the page
  box rounded to the same grid. The test fixture (one font, uniform flags, no images) gives
  about 98 bytes per glyph as JSON (verbose records about 280): a dense 9,000-glyph page is 0.89
  MB. That is a lower bound; 4a measures its full records (ordinals, statuses, line and block
  records, images) and plans on about 130 bytes per glyph until then.
- **IT reference and scorer.** The reference is `{"note", "pages": {page or range: [row]}}`,
  one fact per row, identity page key plus index; the scorer (recovered, kept local with the
  reference) normalizes by NFC, dash/quote/× mapping, soft-hyphen and "a- b" joining, removal
  of all whitespace and lowercasing, so it merges adjacent numbers ("10 5" equals "105"): the
  separate numeric/dice/sign inspection is required. The EN reference keeps this format.

Remaining before any route-A measurement:

- Claude drafts the Bare Bones reference from the original page images only (owner decision);
  the owner checks every row and the page list. Freeze the PDF hash, page list, reference
  hash, row identities and N.
- Freeze `derive_units_v1`, the probe thresholds, the bar input hashes and the scorer (by
  hash) before the first route-A measurement. The thresholds below are proposals, not a blind
  preregistration: render statistics in `docs/results.md` were seen while writing them.

## 4a. Worker reading and diagnostics

- Reuse DEP-29's maintained renderer and existing bounded transport/geometry/PNG validators;
  no second PDF parser. Keep PDF resource/font traversal, structure diagnostics, MuPDF objects,
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
  all geometry once to the staged displayed space. Record origin, argb, `FILLED`, `STROKED`,
  `CLIPPED`, `SYNTHETIC` and CID/GID status per glyph, including undecodable glyphs.
  A glyph with `UNICODE_IS_CID`, even if its numeric value is a valid Unicode scalar, or with
  `char()` returning `None`, is always undecodable: emit U+FFFD as text and retain the CID only
  diagnostically. Retain line direction and character bidi (fork accessors, 4a0).
  Interpret `CLIPPED` as clip-mode text, possibly invisible independently of `FILLED`/`STROKED`.
  Compute `off_page`/`partly_off_page` from glyph quads against the displayed page box, separately
  from MuPDF flags; retain the original quads and these explicit statuses.
  Initial 4a uses `tag_status: not_collected` even when the catalog says tagged.
  Preserve ligatures/whitespace explicitly; no spelling fixes, dehyphenation, digit/sign
  normalization or fabricated Unicode from CIDs.
- Pin 4a's exact flags to `PRESERVE_LIGATURES | PRESERVE_WHITESPACE | PRESERVE_IMAGES |
  USE_CID_FOR_UNKNOWN_UNICODE`. `USE_GID_FOR_UNKNOWN_UNICODE` is off: CID or GID may serve as
   diagnostics, never both, and neither is emitted as text. Proposed `CLIP` is off, subject to
   the owner's 4a0 choice: out-of-clip/page glyphs survive without a MuPDF clipping flag;
   Mimik flags page-boundary status and clip-mode text, but cannot detect in-page scissor
   clipping. With `CLIP` on, scissored glyphs are silently dropped. `INHIBIT_SPACES` is off:
   inferred spaces are allowed and flagged synthetic.
   `DEHYPHENATE`, `SEGMENT`, `TABLE_HUNT`, `COLLECT_STRUCTURE` and all other flags
  are off. Preserve raw ligature scalars; search may expand them using the existing normalizer,
  but scoring uses only frozen IT scorer normalization. Search never changes raw/bar text.
- Extend 3a framing with `reading_page` (page number, schema version, length) and a matching
  `done` count/operation. Per page: exactly `page`, `stats`, `reading_page`, in request order;
  then `done`, EOF and exit zero. Unknown versions/fields, duplicates, missing or unsolicited
  records, inconsistent counts, terminal-error/exit disagreement and trailing bytes fail closed.
  Keep 4 KiB headers, 2 KiB inspection-page frames, 4 KiB stats and 4 KiB complete `done`.
  Reading payload ≤8 MiB/page; total wire budget `(8 MiB + 16 KiB) × pages + 4 KiB`, headers
  included. Test maximum legal frames together and one byte over, not just isolated records.
- Use closed, flat bounded arrays, not recursive deserialization: ≤40,000 total block/line/tag
  nodes, ≤20,000 lines, a glyph cap derived from 4a's final record layout (8 MiB divided by
  the minimal encoded glyph, rounded down, so a maximal page is constructible and testable) and
  ≤20,000 image rectangles per page; ≤256 font
  entries (name ≤64 UTF-8 bytes); tag role ≤100 bytes; depth ≤32; references are page-local
  indices into one node index space shared by block/line/tag kinds; each glyph has exactly one
  line/block owner. Resource/diagnostic walks visit ≤40,000 distinct objects per run with a
  depth bound. Repeated shared resource references and legitimate cross-page subtrees are
  not corruption: distinguish visited identities from the active ancestor stack. Reject cycles,
  dangling references, conflicting ownership, impossible parentage and inconsistent counts in
  serialized page-local graphs. Omit external-page content with explicit page scope, never a
  dangling reference.
  The 8 MiB byte cap also applies when count caps allow more. Reuse bounded collection/string
  visitors and limited serialization; all floats finite, sizes positive, directions valid,
   boxes ordered and quads valid. Raw glyph quads/bounds may extend outside the displayed page
   only with the matching Mimik-computed off-page/partly-off-page status; raw line/block bounds
   may enclose these glyphs outside the page too. Derived unit bounds remain inside the page.
   Keep off-page/zero-area glyph diagnostics; do not invent a positive-area unit for them.
   Validate boundary statuses from canonical quads after conversion. Check containment and
  aggregate counts on both sides of the pipe. No source text in diagnostics or logs.
- Keep render's existing `text` line-array contract for 3c proposals. Add bounded numeric
  diagnostics to render/probe stats: words, font count, fonts without `/ToUnicode`, undecodable
  glyphs, coverage and completeness. Use the existing bounded resource scan (Forms, patterns,
  appearances); distinguish absent `/ToUnicode` from failed decoding, including standard
  encodings and Type3/scan gaps. Missing-map counts come from the resource scan, not just
   fonts encountered in extracted text. Steps 3b/3c are unmerged: missing stats mean
   “not measured, re-render”, never zero; no old-render probe job.
- Require a bounded diagnostic-only `StructTreeRoot`/MCID walk in 4a's probe, even while
  `tag_status` is `not_collected`. Use MuPDF in the worker to inspect structure references and
  page marked content, never to supply glyph membership or resolve text associations.
  “Unresolved” means an MCID with no matching marked content on its referenced page, a dangling
  `/Pg` or `/K` reference, or an ancestor cycle. Compare page-scoped MCIDs (including their
  content-stream scope); a missing `/Pg` may inherit a valid ancestor page. Legal cross-page
  links and shared subtrees are not unresolved. Use distinct-object visitation plus an active
  ancestor stack, ≤40,000 distinct objects per run and depth ≤32; bound marked-content scanning
  by the same worker CPU/wall/memory limits. Count unresolved links/cycles without logging text.
  Report diagnostic completeness separately from association `tag_status`: complete only when
  all relevant structure references and page marked content have been examined, or verified
  absent. On a walk/count/depth limit, stop safely and report incomplete with the limit reason,
  never zero problems or clean tags; unavailable/unsupported scans are likewise incomplete.
  Incomplete diagnostics trigger B/C; worker deadline/resource failure fails the run without
  publishing partial raw. Recoverable unresolved/cyclic fixtures must return bounded diagnostics
  and valid block text, making fixtures 6 and 7 deliverable in 4a independently of the tag slice.

## 4b. One raw-reading → units contract

- Define prepared contract **v3**, requiring registered edition identity as v2 does, and raw
  **`mimik-reading-v1`**, stored per page so that no limit applies to a whole source (SRC-15):
  - **Envelope** (one immutable row per source): contract and reading versions, route, PDF
    hash, ordered one-based page list, pinned extractor/flags/`derive_units_v1` version, tag
    status and registered edition. Its hash is SHA-256 over a domain-separated canonical
    encoding.
  - **Page** (one immutable row per page): displayed page facts and image hashes, validated
    blocks/lines/glyphs/(later) tags and diagnostics as raw bytes with their hash, and the
    page's derived units as prepared bytes with their hash.
  - **v3 source hash**: SHA-256 over a domain-separated encoding of the envelope hash followed
    by the ordered (page number, raw hash, units hash) list. It is distinct from the v1/v2
    manifest hash, which keeps its meaning for existing sources. There is no whole-source
    prepared manifest in v3; v1/v2 keep theirs.
  - Verification runs per page (raw → `derive_units_v1` → exact units equality) plus one check
    of the envelope, the page order and the source hash.
  Bounds are per page (4a's reading payload and count caps against hostile worker output; at
  most one unit per block, so ≤40,000 units per page by the node cap) and per unit (≤64 KiB
  HTML); 4a measures whether the review page and search projection handle a maximal page, and
  sets a per-page unit-bytes bound from that. The number of pages and total size follow the
  upload; the only total check is disk space. Whole-upload totals inherited from step 3 are
  under the owner's cap review (TODO); route A adds none.
- Implement one pure `derive_units_v1(validated_reading) -> Result<ordered pages/units>` shared
  by creation, v3 verification and tests. Inputs contain no review state, exclusion decision,
  clock, database ID or model output. Outputs have block identity, kind, four-point polygon,
  escaped HTML and raw-node membership; reuse existing sanitiser and text/search projections.
- `derive_units_v1` is block-only: one nonempty MuPDF text block is one `Text` unit, lines
  joined by LF in recorded order, `tag_status: not_collected`. Do not merge columns or split
  every glyph/line. Images/empty nodes are metadata; furniture containing text is never hidden.
  Every glyph in a nonempty block belongs exactly once to derived membership, including
  undecodable, synthetic, invisible and clipped glyphs. Tag partitioning belongs only to the
  optional later versioned slice below.
- Text is glyph Unicode in recorded order, with U+FFFD for explicit undecodable glyphs; escape
  it before fixed HTML wrapping. Preserve line breaks with fixed markup.
   `ACCURATE_BBOXES` is off: glyph quads are font-metric boxes, not ink bounds.
   Geometry is the page-clipped bounding rectangle of member glyphs, in fixed
  top-left clockwise point order. Refuse a nonempty unit with no positive-area geometry.
  Stable IDs use one-based PDF page and the block's index in the single page-local node space
  (`/page/<one-based-pdf-index>/a/<node-index>`), never `owner:` or a separate per-kind counter.
  All page lists/indices here are one-based PDF indices, not printed page labels. Ascending
  pages and recorded block order determine output; no unordered-map iteration. Pin exact
  float/JSON round trips: integer coordinate ticks become exactly representable binary
  fractions, JSON must round-trip their f64 values unchanged. Compare inclusive containment
  in that same canonical space, without epsilon or a second rounding pass; test exact edges
  and adjacent ticks. Same raw bytes/version produce identical units.
- For v3, replace literal “units equal raw leaves” with exact equality to `derive_units_v1`:
  IDs, order, kinds, geometry, HTML and membership, no omitted or extra extracted units.
  Glyphs/lines are raw leaves but are not individually review units. Keep v1/v2's Marker
  `collect_units`/SRC-10 rule unchanged; never reinterpret existing immutable sources.
  Owner-added units remain outside extracted equality. Review/corrections never mutate raw.
  Dispatch raw-reading consumers by contract, including suggestion-report node citations and
  reimport verification; the per-page units keep the existing `units[]` element shape.
  Keep CLI v1/v2 import supported; exposing v3 prepared files is a separate interface decision,
  not permission to route browser uploads through `load_import` or its `lopdf` checks.
- Validate v3 pages against staged displayed dimensions, crop, rotation and transform, rather
  than inheriting v1/v2's zero-rotation rule. Test 90/180/270° pages through crops, overlays,
  owner units, suggestion citations and exclusion masks, including serialized round trips.
- Pin synthetic golden cases for block fallback, columns, ligatures,
  RTL/vertical/rotated text, missing Unicode, duplicate text, clipping and zero-area geometry.
  Check repeated runs and serialized round trips; tamper each output field, omit/add/reorder
  units and require refusal. Test source/raw/prepared byte and count boundaries. Retain legacy
  v1/v2 and childless `ListGroup` fixtures through the real importer.

## Later tag slice, only if retained by the owner in step 4

- Use a new reading version and `derive_units_v2`; add only `COLLECT_STRUCTURE` to frozen
  flags. MuPDF's stext struct-block tree is the **only** association source: Struct blocks
  expose `standard`, `raw`, `parent`, `first_block`, `last_block`, with nested text blocks.
  There is no MCID or partial-association signal. A Mimik `StructTreeRoot` walk is diagnostics
  only, never a second resolver. MuPDF's own collection/walk is bounded only by worker rlimits;
  our node/depth limits apply after it returns. Limit exhaustion fails, never publishes partial raw.
- Flatten page-relevant struct/text blocks with parent/ordered-child links, raw/standard roles
  and one node index space. Depth-first child order gives text-block fallback order even under
  nested Struct blocks; never use top-level `blocks()` alone. Shared PDF resources and legal
  cross-page subtrees are allowed; ancestor cycles, dangling local links and multiple structural
  parents are malformed. Bound Mimik's diagnostic/flattening walks separately.
  Malformed edges become bounded diagnostics, never cyclic/dangling serialized references;
  preserve text in the validated block fallback, or fail if complete text cannot be retained.
- Status is `absent` when collection yields no structure, `malformed` for invalid exposed graph,
  `partial` for observable coverage/ownership conflicts or unavailable required table metadata,
  and `usable` only when exposed associations satisfy the invariant below. `usable` certifies
  that invariant, not semantic completeness of PDF tags; hidden partial association is unknowable.
  Catalog-tagged with no usable struct content is `partial`, not evidence of complete tags.
- Eligible glyphs are all glyph records belonging to nonempty text blocks, including undecodable,
  synthetic, invisible and clipped glyphs. Every eligible glyph occurs **exactly once** in derived
  membership. Select outermost mapped ancestors; Table precedes its cells, LI its paragraphs.
  Absent/not-collected structure uses block-only output without a tag-error flag. A wholly
  unclaimed block is allowed and emitted as one fallback unit. Flagged page-wide block fallback
  is mandatory iff graph status is malformed, any block is partly claimed, any glyph/block has
  competing claims after ancestor selection, any glyph is missing/duplicated in tentative output,
  or a selected table lacks valid rows/cells/spans. Never combine tag output with a partially
  claimed block. Refuse if even fallback cannot satisfy ownership or geometry validation.
- Map `Table` → `Table`, `LI` → `ListItem`, `H`/`H1`–`H6` → `SectionHeader`, `P`, `Figure`,
  `Caption`, `Note`, `TOCI`, `Span`, `Artifact` → `Text` when containing text. Other roles are
  transparent containers; wholly unclaimed blocks use `Text`. Empty nodes/images remain metadata;
  Artifact never suppresses text. Units follow selected ancestors' depth-first order, followed
  by wholly unclaimed blocks in depth-first order; IDs use the selected node's single-space index.
- Raw table cells must carry `RowSpan`/`ColSpan`, each integer 1–100 plus provenance/status.
  Obtain them only through an association-linked MuPDF attribute accessor proven in preflight
  (a worker wrapper if approved), not a diagnostic tag walk guessed onto text. Default to 1
  only for verified absent attributes; unknown, noninteger, zero, negative, >100 or conflicting
  values force page fallback. No proven accessor means tables fall back. Preserve row/cell
  order and reject overlapping/inconsistent grids. Add a real span fixture (2×1 and 1×2),
  absent/invalid attributes, partially claimed block, shared block, conflicting claims,
  legitimate shared resources/cross-page subtree, cycle and nested fallback-order fixtures.
- Review and remeasure the versioned derivation and bars; never reinterpret v1 raw sources.

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
  inspection, not a server PDF parse. Derive each page's raw and units outside the Store mutex
  into a staged, hash-verified per-page file, one page in memory at a time. Before admission,
  reserve disk from the per-page upper bound (pages × (reading payload + unit bytes) plus PDF
  and PNG copies) for the staged per-page files and the database copies together (both exist
  until cleanup), plus one batch's rollback-journal growth, in addition to staging's ledger and
  512 MiB margin; after derivation, re-check against the actual staged sizes before writing.
- Write in page batches, never one unbounded transaction (C6).
  - **First** IMMEDIATE transaction: recheck Owner/session-bound CSRF, both revisions, current
    selection/render identities, the registered active job and the exclusion staging row with
    its attribution (`StagedContract.exclusion` alone lacks actor/time); resolve the registered
    edition; assert that no hidden source exists for the upload (unique constraint, at most one
    `finalizing` source per upload ID); insert the source as `finalizing`. A retry first
    removes a leftover hidden source of the same upload through the cleanup path below, before
    this transaction and only once no job owns it.
  - **Batch** transactions: each inserts one bounded batch of pages (the PDF is `put` before the first transaction that inserts the `sources` row, the guard kept until it commits; original
    PNGs, page facts/probe provenance, raw and units rows, units initially `unreviewed`, search
    projections, and the batch's exclusion rejections below), after checking that its own
    source still exists and is `finalizing`.
  - **Last** transaction: recheck everything the first one checked (Owner/session validity,
    both revisions, selection/render identities, the job still registered and not cancelled,
    the upload row and the exclusion staging row unchanged); verify the source hash over the
    stored rows; call `documents::register_import`; delete the upload row, exclusions and
    fingerprint key; record the receipt; flip the source to visible. Any mismatch fails the
    job and leaves the hidden source to cleanup.
  - **Hidden means hidden everywhere:** every query over sources, units, pages, documents and
    search projections filters to visible sources (one shared predicate): review screens,
    search/FTS, assistant retrieval, suggestion reports, edition coverage, exports and backups,
    source lists and counts, duplicate-PDF-hash checks and receipt lookups. Two uploads of
    the same PDF may therefore finalize in parallel; the last transaction's `register_import`
    applies the owner-choice and overlap rules against the sources visible at that moment, so
    no duplicate check belongs only in the first transaction.
  - **Cleanup:** startup and hourly cleanup delete a `finalizing` source only after confirming,
    under the lock the job holds when it registers and writes a batch, that no job owns it;
    deletion runs in bounded batches too. Migration guards make raw, units, page facts and
    exclusion provenance immutable without weakening 007/015/017, except that cleanup may delete
    the rows of a `finalizing` source.
  - 4a/4c measure batch and cleanup durations so other requests are not held.
  Add permanent exclusion provenance (pages, geometry, rationale, revisions, confirming
  actor/time and opaque fingerprint), crossing flags and explicit rejection-event links.
- Apply the confirmed geometry **after** derivation: wholly contained unit rectangles (inclusive
  edges) receive attributed `rejected` decisions citing the exclusion; positive-area overlap
  without containment stays `unreviewed` and flagged. Mere edge contact is not crossing. Raw
  output stays complete. At creation confirmation the current Owner authorizes these rejections;
  retain the original confirmer separately. Check all expected revision-1 units first, then
  update/event/authenticated sidecar per unit, sharing operation ID/rationale. Reuse review
  primitives; apply the rejections per page batch (one operation across batches), so >1,024
  excluded units never make one transaction unbounded, preserving the public batch limit; the
  last transaction's recheck keeps them from going stale. Any failure leaves the source
  `finalizing` for cleanup. List decisions and allow normal revision-checked undo; never
  auto-complete a page.
- Reuse `documents::register_import` (in the last transaction only): hash-keyed
  document/member, first uncovered page primary, overlaps subject to the existing owner-choice
  rules. Do not replace a primary reading or
  merge runs. PDFs live in the content-addressed store (migration 037), so identical PDFs share one file.
- The flip transaction holds the upload-row deletion (with exclusions and fingerprint key) and
  the receipt; retain an upload-ID → source-ID completion receipt without private staging data
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
   Flag every unit containing invisible (neither `FILLED` nor `STROKED`), possibly invisible
   clip-mode (`CLIPPED`, including mode 7's `FILLED|CLIPPED`), off-page or partly-off-page glyphs;
   include per-glyph diagnostic visibility/boundary status. Test render modes 3–7 and mixed
   visible/invisible content against original renders; possible invisibility is not silently
   treated as evidence. With `CLIP` off, in-page scissored text cannot be identified by these flags.

## Probe: freeze before manual measurements

- Define `probe-v1` on each selected page: words are maximal runs of Unicode letters/numbers
  in decoded line text (combining marks continue a run), U+FFFD/CID placeholders do not count;
  report count and words per 10,000 displayed pt², image rectangle union/page area (reuse
  `image_coverage`), missing `/ToUnicode` font count and undecodable/total glyph count. No
  inferred “correct words”; no model. Preserve scan-incomplete and dropped-text warnings.
- Proposed frozen rule: recommend **B or C** if words <120, image coverage ≥0.20, any undecodable
  glyph, missing-map font, incomplete diagnostics or malformed/partial collected tags; otherwise A.
  Show all triggered reasons and measurements, including words versus image fraction, rather
  than a confidence score. Missing maps can still decode via standard encodings: label the
  conservative warning accurately. No-text and unknown diagnostics never recommend A.
  B is local/Linux and C requires explicit online consent; unavailable routes stay disabled.
  `not_collected` is explicit, not a malformed-tag claim or automatic B/C trigger. A bounded
   diagnostic-only tag scan is mandatory in 4a and reports unresolved links/cycles and completeness
   independently of association status; these warnings also trigger B/C. It never supplies glyph
   membership.
- Show per-page recommendations and a selection summary on the rendered upload **before route
  choice**; persist the exact rule/version/result with the created reading and show it on its
  page list/review. Extend render stats; missing measurements require re-rendering.
  Test threshold equality, mixed selections, standard-font mapping and incomplete scans.

## Bars, fixed before measuring (umbrella step 4)

- **“MÖRK BORG Bare Bones EN (local, licence and hash recorded in the corpus table): a
  hand-checked reference of the weapons, armour, equipment and character-creation pages (rows
  counted as in the IT reference); at least 98% of rows intact in one unit, no digit, dice or
  sign error.”** Claude drafts the reference from the original page images (never from route A
  output), then the Owner checks every row before extraction is scored (owner decision).
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
- Freeze `derive_units_v1` and exact flags with both bars' input hashes. If the 98% row bar
  or zero numeric/dice/sign-error requirement fails, step 4 stays blocked. The Owner may
  retain that blocker, or approve a new versioned derivation through review, fresh measurements
  of both bars and independent re-review; no tuning relabelled as the frozen v1 pass.
- **“MÖRK BORG IT pages 22–39: the probe recommends route B or C on every art-heavy page (all
  18), and route A's scores are reported, with no bar.”** Denominator 18 for recommendations;
  report each page's result/reasons and require 18/18. Report intact, present and fuzzy on the
  existing 161-row reference (PDF pages 23–30), explicitly distinguishing the scored subset
  from the full probe range; inspect/report numeric/dice/sign errors. Never compare EN facts
  to IT facts as corrections. If frozen thresholds fail, report failure; any new rule gets
  reviewed/versioned and remeasured, not retrospectively declared pre-registered.
- On real text-only pages, report the frozen probe's A versus B/C counts, denominator and
  reasons as a descriptive specificity check (not another acceptance bar). Freeze the local
  page set in 4a0 and distinguish it from synthetic clean pages and the 18 art-heavy pages.
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

- **4a0:** remaining: the Bare Bones reference (drafted, owner-checked) and the freeze of
  `derive_units_v1`, thresholds, bar input hashes and scorer.
- **4a:** block/line/glyph records with `tag_status: not_collected` and diagnostic probe;
  the cost estimate of the later tag slice (owner rule, Bare Bones EN is tagged).
  The bounded StructTreeRoot/MCID diagnostic walk and fixtures 6/7 are required here.
  Independent review before completion; fake-reader max/over-limit/cyclic-reference tests,
  live sandbox and ten PDFs.
- **4b:** v3/reading schema and one derivation function. Independent review; synthetic goldens,
  mutation/bounds tests, raw equality, legacy import/reimport and sanitized projections.
- **Later tags:** only under the owner's retained scope; new reading/derivation version,
  independent review, ownership/span fixtures and remeasured bars before completion.
- **4c:** manager, batched finalization, permanent facts/exclusion migration and cleanup
  receipts; tests for a failure between batches, a retry with a leftover hidden source, the
  sweep racing an active job, revision/exclusion/discard changes between the first and last
  transactions, every reader filtering hidden sources (a search hitting a hidden source's
  projection, documents, assistant) and cleanup deleting a hidden source's immutable rows.
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

## Owner decisions (2026-11-05)

- **Wrapper gaps:** no maintained crate exposes them, so the owner chose a local fork of
  mupdf-rs, vendored until upstream releases it (DEP-29).
- **Tag partitioning:** decided after the 4a0 preflight from the bar PDFs' tag status (defer
  if both are untagged; otherwise keep the later versioned tag slice and estimate its cost).
- **Flags/ligatures/spaces:** the 4a flag set as proposed: raw ligature scalars, inferred
  spaces allowed and flagged, CID diagnostics only, no dehyphenation.
- **Scissor clipping:** `CLIP` off; clip-mode and off-page glyphs flagged, in-page scissoring a
  documented limit caught by review against the render.
- **Bare Bones reference:** Claude drafts it from the original page images only (never from
  route A text, raw readings or a remote model) in the IT reference's format; the owner checks
  every row and the page list before any scoring. The draft and the reference stay local.
- **Source limits (2026-11-07, SRC-15):** limits only where technically or security-justified;
  no whole-source totals in route A, storage and processing per page.
- **Probe policy:** `probe-v1` as proposed, advice only with visible reasons and an Owner override.
- **Bar failure:** step 4 stays blocked; a new versioned derivation is allowed only with a
  rationale, review and fresh measurements of both bars, never by waiving the bar.

## Risks

- Tags can be wrong, incomplete or expensive to resolve; block fallback preserves text but
  may scatter a fact across units. The 98% bar is not established by wrapper availability.
- Glyph-rich pages make the stored reading large (about 0.9 MB per dense page at the
  fixture's lower bound): disk space is reserved before admission; 4a measures its real
  per-glyph size and the worker's memory per page.
- Image coverage misses art drawn as vector paths; missing maps over-warn standard fonts.
   The frozen IT probe check can fail and require a separately reviewed algorithm revision.
- With proposed `CLIP` off, in-page scissor clipping is undetectable: retained text may not
  appear in the render and has no MuPDF clipping flag. Page-boundary and clip-mode flags cannot
  close this gap; review against original renders remains necessary. `CLIP` on instead loses
  scissored glyphs silently and cannot provide complete dropped-text diagnostics.
- SQLite BLOB copies/journaling block the single Store connection (C6); measure each page
  batch's duration and the disk headroom on a 500-page synthetic source of dense pages. Do not
  promise filesystem/SQL atomicity: the receipt, post-commit deletion and restart
  reconciliation bridge it.
