# Sources and retrieval

A **source** is one prepared extraction run of a locally owned manual: the original PDF, the Marker structured extraction (`source.json`), the prepared manifest (`bundle.json`) and one original-page PNG per selected page. Sources are stored privately in SQLite and reviewed by the Owner. Reviewed units feed lexical lookup. Nothing in this workflow lets a model approve evidence or change sheets or rules.

Code: `src/sources/`, `src/auth/materials.rs`, migrations `003`, `004`, `005`, `007`, `009`, `015`, `017`, `024`–`026`, `assets/source-regions.js`, `assets/source-review.js`. Tests: `tests/source_*.rs`, `src/sources/upload_tests.rs`, `src/campaigns/material_tests.rs`, `tools/test_source_tools.py`.

## Internal PDF worker (Linux)

`sources::worker::inspect(path)` and `inspect_cancellable(path, &Cancel)` run a blocking,
confined inspection. `render(path, pages, &Cancel, callback)` delivers one validated page
at a time on a dedicated record-reader thread; the callback must return promptly. A caller
can retain delivered pages after a later batch failure. `Cancel` is clonable and permanent.
These internal APIs are separate from the CLI import and are not browser upload routes.

`availability()` exposes a process-wide `Available` or `Unavailable { reason, how_to_fix }`.
The first inspect/render call lazily runs a fixed synthetic PDF through the confined inspect
operation and retains the result. This keeps the probe within the worker API rather than
coupling it to assistant startup; callers on async runtimes must use a blocking task for it
as for the other worker calls. Namespace/AppArmor remediation text is shared with the
assistant in `sandbox::host`; failure never selects an unconfined parser. Restart Mimik
after fixing the host setting to repeat the probe.

The shared long-lived spawner starts `assistant-runner`, whose `PdfWorker` operation builds
the Hakoniwa sandbox and executes `mimik pdf-worker --spec /run/spec.json`. The profile binds
the executable and PDF read-only with device/inode checks, shared system-library discovery,
the ELF interpreter at `/opt/mimik/loader` (execute granted only on it and Mimik), and read-only `/dev/null` for
stdin. It has an empty network namespace, no proc, sysfs, GPU or shared-memory mount, a
16 MiB `noexec,nosuid,nodev` scratch tmpfs and a private 128 KiB `/run` tmpfs for the bounded
64 KiB JSON spec. The scratch Landlock grant permits regular-file operations only.

The sandbox closure, after mount setup, sets address space to 2 GiB, file size to 16 MiB, descriptors to 64, core size to
zero, nice to 10, and equal soft/hard CPU limits (20 seconds for inspection, 25 seconds per
rendered page). The parent enforces 30 seconds for inspection or 30 seconds per render page.
Cancellation/deadlines kill through pinned pidfds, and results wait for tree exit. Stdout
uses the framed record reader; a separate stderr drain matches at most 64 KiB and discards
the remainder without logging raw output. The closure executes the explicitly bound loader
with Mimik as its program argument; no loader file bind overlays a system-library bind.
Cleanup kills all verified pins before a bounded five-second wait. A cleanup timeout
interrupts transport readers and reports upload unavailable, never a successful tree exit.

After closing the bounded input and before any MuPDF call, the worker installs a second
seccomp filter with the raw seccomp syscall and `SECCOMP_FILTER_FLAG_TSYNC`. It kills process/thread creation, exec, socket
creation, memfd, userfaultfd, ptrace and further seccomp calls; installation failure exits
26 before parsing. A missing closure marker is `upload unavailable`, with a fixed setup-step
name or runner exit code; a parent cancellation/deadline takes precedence. Successful framing
alone cannot establish success without exit zero. Terminal error frames must agree with
statuses 20–25; exit-only document reasons are accepted only before any output byte.
Local reader/callback errors retain their reason. Only the parent's deadline may establish
`took too long`.

The lazy availability probe caches success and confirmed sandbox-setup failures. Other
failures retry with a process-wide exponential backoff of one to thirty seconds. Upload
remedies name `user.max_user_namespaces` only after observing zero; otherwise they use the
observed namespace/AppArmor cause or neutral configuration/resource advice. Render request
validation precedes the probe.

The no-proc profile, generated hostile/valid corpus and peak-RSS acceptance check have
opt-in host confinement tests in `sources::worker::live_tests`. Every case declares its
exact inspect refusal or render outcome; render-stage fixtures must pass inspect. Signal
deaths fail the corpus, separately from explicitly reported parent kills. RSS comes from
wait4 on the reaped runner and its waited descendants: a conservative worker upper bound
only for naturally exiting runs, checked below 75% of the 2 GiB address-space limit. A
test-only 256 MiB touch control checks this propagation. Parent-killed cases instead use
host `/proc/<inner pid>/status` VmHWM, sampled through the verified worker pin during the
run and just before killing. The pidfd must be live before and after the read; an absent
measurement fails the gate. This figure covers execution through the pre-kill sample,
not the subsequent kill/reap interval. A separate 256 MiB touch-and-wait control tests it;
both figures are reported independently. Both the input Vec and MuPDF's input buffer are
present during measurement. The default MuPDF store is used because mupdf 0.8.0 exposes no
store-size setter. Fresh generator
subprocesses enforce 64 MiB peak RSS per case, with 8 MiB output and five-second generation
bounds; full-flush block repetition avoids compressing expanded bombs on the test host.
The responsiveness test overlaps a confined inspect with a hostile render forced to hit
its deadline by a test-only post-image wait. HTTP-level responsiveness belongs to slice 3b.
The ignored `local_pdfs::live_local_pdfs_inspect_and_render_every_page` check uses
`MIMIK_LIVE_LOCAL_PDFS` (colon-separated paths) and the real `MIMIK_LIVE_RUNNER`. It renders
every page in batches of eight, keeping text/images only in memory and printing only hashed
path identifiers, page counts, timings, both peak RSS figures and per-batch outcomes.

## Browser PDF uploads (Linux)

`/admin/materials/uploads` is the Owner's upload entry point. It redirects to the retained
upload, or shows system, edition, language and PDF controls in that order. An unavailable
sandbox replaces the form with the probe's fixed reason and remediation text. Other platforms
show that upload is unavailable. There is no unconfined fallback in production.

`POST /admin/materials/uploads` accepts multipart fields in exactly this order: `csrf`
(256 bytes), `system` (100 bytes), `edition` (400 bytes, at most 100 Unicode scalars),
`language` (30 bytes), `file` (128 MiB). Unknown, duplicate, out-of-order and post-file
fields are refused. Owner, Host/Origin, declared length, worker availability and the staging reservation/slot are
checked before the body is read. CSRF admission precedes all PDF writes; metadata uses the
import identity rules. The counting stream limits the complete request, including epilogue,
to 128 MiB + 64 KiB. The server is HTTP/1 only. Before CSRF admission the parser receives at
most 4 KiB; the process holds at most the pending frame plus hyper's channel slot and read
buffer, a small constant multiple of hyper's approximately 408 KiB maximum buffer
(SRC-UPLOAD-POLL in `limitations.md`). No file byte is spooled before admission; only one
upload streams at a time. After admission explicit Pending boundaries every 64 KiB bound
parser read-ahead to two 64 KiB batches plus delimiter lookbehind, apart from the pending
transport frame. Uploads have 30-second input-wait idle and 15-minute total deadlines;
metadata validation and file writes do not consume the client-idle budget.
An unavailable worker returns 503 with the probe's fixed reason and remedy before body
polling. A second upload returns 409: an active stream is in progress, or a retained upload
is already staged with a link to open or discard it.
An asynchronous-safe RAII wrapper schedules partial-file deletion and slot release on a
blocking thread on disconnect, refusal or future cancellation. Hashing is incremental;
publication rechecks Owner/CSRF and the PDF's size, hash and first-1,024-byte magic.
Original filenames are neither application arguments nor stored/logged identifiers.

`/admin/materials/uploads/{id}` shows size, hash prefix, matching permanent document,
inspection page count/sizes/rotations/encryption, and a selection of at most 64 pages.
POST actions at `/{id}/set`, `/inspect`, `/render`, `/cancel`, `/discard`, `/propose`,
`/exclude` and `/remove-exclusion` require Owner,
Host/Origin and session-bound CSRF; manager calls run on blocking threads. Render retries
keep current pages and render the remainder. Failed/restarted uploads offer inspection
again and rendering the remaining pages. Progress at `/{id}/progress` is an HTML fragment.
The external script polls it as an inert network document every two seconds and copies only
fixed text nodes, without an HTML injection sink; without JavaScript a two-second meta refresh applies while a job
is active. Rendered pages show text-layer diagnostics and zoom/pan; no-JS users can open the
full-size PNG. `/{id}/pages/{page}/image` serves only stored page IDs through Owner read
admission and descriptor-relative no-follow reads, with `no-store` and `nosniff`.
These screens/assets/images carry the material review screen's strict CSP.
The optional **Repeated text** section proposes matching text on the selected, current renders.
It shows the text only to the Owner, affected pages, absent pages, and highlighted per-page
boxes. The Owner confirms, adjusts numeric displayed-point inputs, or draws a box with the
same editor as the source review screen. Pointer corner grips and keyboard arrow-key handles
enhance the numeric form. Every write requires explicit page choices and a rationale.
Absent pages start unchecked and unhighlighted; their median boxes require explicit extension
after the missing-text warning. Confirmation replaces the whole current exclusion. Removal
requires a fresh rationale. Confirmed regions have opaque black SVG masks on staged renders;
the stored PNG and the original-image endpoint remain unchanged (SRC-13).
Reading routes are disabled with their requirements: step 4 creates sources; staged pages
are not approved evidence.

`dev-endpoint` builds alone expose `--upload-dev-worker-ms N`, an explicit browser-harness
worker with cancellable per-page delay. It accepts exactly the fixed three-page synthetic
PDF in `sources::uploads::development::PDF`, returns deterministic page facts/images, and
never parses uploaded PDFs. Default/release builds contain neither this worker nor its
flag. The required CI browser-smoke step runs the standalone `tools/upload_browser.py`
against the `dev-endpoint` binary artifact in `mimik-browser`; the image includes the script
and, among fixtures, only its shared synthetic PDF fixture. It covers 1280/390 px JavaScript/no-JavaScript
scenarios, including numeric zoom before image load, proposal confirmation, opaque masks,
numeric adjustment, removal, revision history and private-data log/database checks;
`tools/smoke_release.py` checks that release artifacts lack and refuse the flag.

## Internal upload staging (Linux)

`sources::uploads::Uploads` is the blocking storage/job API for browser upload integration.
The authenticated server opens one manager with the store and data directory before
accepting uploads; callers clone its handle and call it outside the async runtime.
Opening reconciles interrupted jobs, expires seven-day-idle uploads, removes orphan files and
folders, and measures retained files. A weak-reference maintenance thread sweeps hourly.
Cleanup failures use fixed diagnostics and leave surviving bytes charged for a later retry;
unknown disk usage blocks new reservations until a successful reconciliation. Restart drops
page rows unless both PNG and text sidecars are no-follow regular files with mode 0600,
settling the upload state and retained job progress in the same transaction.

`begin_upload` reserves 128 MiB + 64 KiB and returns an RAII stream slot with a private spool
file. The route must enforce its stream/body limits, hash the stream, and finish authentication
and CSRF admission before writing PDF bytes. Dropping the phase unlinks its spool and releases
the slot/reservation. `create` rechecks the supplied size, hash and `%PDF-` magic without parsing
the PDF, fsyncs and renames the spool, then inserts the upload and queued inspection in one
Owner/CSRF-rechecked IMMEDIATE transaction. The original filename is never an argument.
After that commit, `create` returns the upload ID even if worker launch fails; the Owner can
read the failed job and retry or discard it.

Migration 026's `source_uploads`, `source_upload_pages`, `source_upload_exclusions` and
`source_jobs` are mutable staging tables with cascading deletion, separate from permanent
source immutability. They store metadata/hashes, not PDFs, PNGs or extracted text. Files are
descriptor-relative and no-follow under `data/staging/<32-hex-id>/`: directories 0700,
`document.pdf`, `page-N.png` and `text-N.json` 0600. Page writes use exclusive temporary names,
fsync and atomic rename. Newly created files/directories are fchmod-normalized to their private
modes even under a restrictive umask. The exclusion table holds actor attribution, rationale,
pages, displayed-space boxes, fingerprint, removal flag and revision. Set, replacement and
removal append rows; upload deletion/expiry cascades to all exclusion history. It stores no
proposal text. The current exclusion and its independent revision (zero before the first row)
are part of the upload model. A removal retains its historical boxes but exposes no current
exclusion. Writes recheck Owner/CSRF in the transaction and require both the upload revision
and exclusion revision. Selection changes that would deselect an excluded page are refused
until the Owner replaces or removes the exclusion; unrelated selection/metadata changes retain it.

The manager owns a process-wide ledger of used bytes and reservations. Total staging is
2 GiB; free space must cover outstanding reservations plus 512 MiB for SQLite/WAL. Each
eight-page render batch reserves 160 MiB. Written bytes replace reservation bytes; deletes
reduce usage and failures release the unwritten portion. Per upload: PDF 128 MiB, PNG
128 MiB, text 512 MiB, at most 64 selected pages, and one retained upload/one active stream.
PNG overflow has its own `source_png_limit` reason (“too large for one source: select fewer
pages”). Other capacity failures use `space`. `already_document` identifies a matching
permanent document without merging readings.

One dedicated worker thread runs inspection/rendering. The production `Worker` adapter uses
the confined 3a API; synthetic unit tests inject a fake. Jobs transition queued → running →
succeeded/failed/cancelled; queued cancellation never starts a worker. Launch is serialized
with queueing under the control lock; spawn failure and worker panic end the job failed with
`unavailable`. Running cancellation signals `Cancel` and joins the worker before returning.
Selection/metadata changes require the expected revision and advance it. Selection changes
cancel/reap active work and delete deselected page rows/files; metadata-only edits preserve
active work and advance its publication revision under the publication lock. Render settings
are currently fixed. Selected renders remain current across revisions when renderer versions and
settings match. Retrying renders only missing/outdated selected pages, preserving successfully
published pages from failed batches.

Page publication and deletion share a lock; the publication transaction also requires a running
job and matching upload/job revision. Completion settles a revision mismatch as cancelled.
The data-free publication gate and consistent control slot recover poisoned mutexes.
Failed/stale publication removes its new files. Discard
holds the control lock across cancel/reap, row deletion and folder deletion. Sweeps preserve
active workers under these locks, fail stranded queued/running jobs with `unavailable` when
the control slot has no live worker, reconcile orphan folders even after a failed discard, and
reset measured ledger usage while preserving reservations. Creation and spool cleanup share
the publication lock; reconciliation excludes the exact active spool. Directory deletion uses
descriptor-relative no-follow traversal and `AT_REMOVEDIR`. On opening, queued/running jobs
become failed with `interrupted`. Page publication reads geometry from the batch's inspected
pages and loads only current job/selection/page facts, without reloading the inspection JSON.

`contract(id)` builds `mimik-staged-v1` from a consistent row snapshot: PDF hash/size, pinned
renderer versions/settings, selected pages/revision, per-page geometry, PNG hash/pixels/dpi,
substituted fonts/text statistics, metadata, and limits (64 pages, 128 MiB PNG, 20,000 units).
It refuses an incomplete/outdated rendering and includes the current exclusion and its revision.
This is staging
only: no permanent source, review approval or evidence is created. The later source-creation
step must consume this contract and copy permanent page facts/images/exclusion geometry/rationale
and the exclusion's opaque proposal fingerprint (when present),
then discard staging; it must not parse the PDF again or retain staging text/proposals.
The fingerprint's upload-local secret is not in the contract and is deleted with staging;
after that deletion the copied fingerprint is provenance only, not a recomputable text hash.
The contract refuses exclusions naming pages without a current selected rendering, including
page rows removed by missing-file sweeps or render replacement.

### Repeated-text proposal

`Uploads::propose_exclusions` reads staged text under the publication/deletion gate, checks
its stored byte count and SHA-256, and validates the text-line schema and page bounds again.
It requires a complete current selection, no queued/running job, and Owner read admission
before and after the read. Propose responses never include an automatic refresh.
The pure proposal function normalizes NFKC, collapses whitespace, case-folds with
`unicase`, and replaces each Unicode decimal-digit run with `#`. No text is written to SQLite,
logs or error messages. Confirmation accepts only a current proposal fingerprint or no
fingerprint for a manually drawn/adjusted exclusion. Fingerprints are SHA-256 over a random
32-byte per-upload secret followed by normalized UTF-8 text. `getrandom` supplies the secret;
only the staging upload row stores it (schema 27), and upload deletion removes it. Migration
27 clears legacy unkeyed proposal fingerprints while preserving confirmed geometry/history.
Identical
text in different uploads has different fingerprints, preventing offline text guessing from
exclusion history. Confirmation computes proposals under the upload gate before opening
the write transaction, leaving the shared Store available to other users during file reads,
validation and grouping; the short transaction rechecks revisions/admission and appends.

Lines are visited by PDF page, then top/left/bottom/right, normalized text bytes and original
line ordinal. Each joins the first eligible group in creation order with the same normalized
text and seed-box IoU at least 0.8 in page fractions, with at most one line per page per group.
The IoU comparison tolerates four floating-point epsilons at the boundary to account for
fraction conversion. Ordered eligible-group indices remove already matched groups from the
current page's search and omit newly created groups until the next page. Per-page eligible
sets are copied lazily only for texts touched on that page. At most 2,000,000 IoU comparisons
per normalized text across the selection are allowed: an exhausted text is dropped entirely,
while other texts retain exact grouping and remain eligible. After each page, tracking also
stops for a text if neither any existing group nor a future seed can reach the page threshold.
If budget exhaustion leaves no qualifying proposal, an HTML error explains the remedy:
"draw an exclusion by hand". IoU comparisons are therefore bounded per text, and a colliding
text cannot suppress another text's valid footer proposal (SRC-REPEAT-COST).
Ranking is descending page count and normalized Unicode character count,
then ascending seed top/left/bottom/right, normalized bytes and group creation order. Only
4–200-character groups on at least three pages and at least 60% of the entire selection qualify;
the top three are offered. Matched boxes expand by 2 points and clip to the displayed page.
Absent-page offers use the coordinate-wise median unpadded box in page fractions (the mean of
the middle two coordinates for an even count), converted to that page's points, padded by
2 points on each side like matched boxes, and clipped.
They are never selected automatically. Pages without text still count in the denominator.
Path/image marks cannot generate proposals; manual boxes remain available.

## Import (`mimik import-source --bundle DIR --pdf FILE`)

Only the operator imports prepared sources, through the CLI. Browser PDF uploads stage pages separately; there are no archives, URLs or nested imports. `Store::import_source` reads everything first (`load_import`) and then writes it all in one transaction.

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
their own state, apart from an unsupported source, with direct links to the first 50 affected
source page lists (`/admin/materials/sources/{id}`). The operator recovers them with
`mimik set-primary-source --document-pdf-sha256 HASH --pages 3,5-7 --source ID --rationale TEXT`
(pages ascending and distinct, at most 1,024; actor `operator:cli`; the source must cover every
page and be finalized).

`Store::source_primary_state` returns page/source/latest-event tokens. `set_primary_source`
checks all expected tokens before changing any page; the event token detects intervening
A → B → A changes. The chosen source must cover each requested page in the document. A
transaction appends the actor, rationale, previous source and new source, then updates the
selection through a trigger. Primary history and authenticated `source_primary_web_events`
attribution are retained and immutable. The Owner-bound store wrapper rechecks Owner and
CSRF before and after writing each decision and its sidecar. The Owner can select this reading
for checked pages in `/admin/materials/sources/{id}`, or for one page in its review screen.
Both forms carry the expected source and latest-event tokens and require a rationale.
After a failed page-list submission, status labels describe the stored primary state;
submitted source/event tokens remain separately in the checked form controls.

## Review

States: `unreviewed`, `accepted`, `corrected` and `rejected` (a CHECK constraint).
- **Unit decision** (`store::review_unit`): accepted, corrected or rejected. Only `corrected` carries a correction (plain text, ≤20,000 scalars, no NUL, separate from raw HTML). Actor (≤100) is required; rationale (≤5,000) is required except for `accepted` (SRC-4), and the review screen's accepted choice has no rationale field, so it records none. A stale expected revision gives `Conflict`. The update and an append-only `source_review_events` row share one transaction; triggers advance the revision by one and reset the page to `complete=0`, `revision+1`.
- **Page completeness** (`store::complete_page`): expected page revision, no unreviewed units, a printed label (1–50 scalars; `unnumbered` allowed), actor and rationale; writes a `page-complete` event. Triggers (007, 009) enforce the same in SQL, and history is append-only.
- **Owner-added text** (`Store::add_owner_unit`): expected page revision, four finite, strictly convex polygon points inside the original page, plain text, actor and rationale. A finalized source accepts this insert only as `origin='owner'`, `kind='Text'` (or `'Table'`, below), reserved `owner:<positive integer>` identity, revision 1 and `unreviewed` state. Extracted ids cannot use that namespace; origin never changes. The transaction inserts escaped text inside `<p>`, derives both text projections, then records a normal `corrected` decision with the supplied text. Insertion and correction each reset completeness and advance the page revision once. An unreviewed owner unit prevents completion. The insert-then-correct pairing is enforced by the store: SQL also accepts the unreviewed owner insert (it resets the page, and the unit is then reviewable like any other), so a direct insert without the correction leaves an unreviewed unit rather than a decision. Owner block ids are ASCII digits only (no NUL), at most 25 bytes; an ordinal beyond the integer range is refused. The authenticated store wrapper writes the normal review sidecar in the same transaction.
- **Owner-built tables** (`Store::material_create_table`, migration 028): SQL accepts owner units of kind `Text` or `Table`, with every other identity, state, revision and geometry guard unchanged. On a page the Owner ticks "Select unit N for a table" on 1 to 16 cards (a checkbox per card submitting to a GET form, no script) and opens `/admin/materials/pages/{id}/tables/new`. The form shows one column per selected unit in reading order (corrected text where corrected, the extraction text otherwise), one row per nonempty line, short columns padded. The cells are one textarea: one row per line, cells separated by `|` (edge bars optional), `\|` for a literal bar and `\\` for a literal backslash, cells trimmed, all-empty lines dropped, short rows padded. Bounds: 1 to 200 rows, 1 to 16 columns (the 16 selected units cap the prefill the same way), 1,000 characters per cell, 20,000 characters of text, 64 KiB of generated HTML, no NUL, a rationale of 1 to 5,000 characters. They keep the form, the HTML and the lookup text bounded like any unit. Saving runs one IMMEDIATE transaction that rechecks the Owner, CSRF, the page revision and each selected unit's revision, inserts one owner `Table` unit (`owner:<n>`, `unreviewed`, revision 1) and rejects the selected units in one batch (one operation id, the one rationale, an audit sidecar per rejection). The unit's polygon is the axis-aligned box over the selected units' points, clamped to the page, four convex points (a box without area is refused). Its HTML is generated by the server as `<table><tbody><tr><td>` with escaped text and no header row; the same sanitiser and search index as Marker tables apply. The stored plain text (`raw_text`, shown again if the table is re-selected) is one line per row with cells joined by ` | `, not a bordered projection; the search text is the usual flattening, one cell per line. Each creation is recorded in the append-only `source_owner_tables` link (unit, operation id, first rejection event); the history shows "Table unit N created from units …" on that event, and the page shows a confirmation. A pasted Markdown table works: edge bars and the `---` row are dropped, and the prefill is written in the same `| a | b |` form. The table card sorts after the extracted units (owner units come last). Over 64 KiB of selected text, or a unit of more than 200 lines, is refused with its own message; units of another page are refused. The new table stays unreviewed until the Owner checks it against the original page, so it is not evidence yet; the originals and the raw extraction are untouched. A refused save shows the error and keeps the cells and rationale.
- **Atomic multi-unit decisions** (`Store::review_units_atomic`): 1–1,024 distinct units and one rationale, under a store-generated operation id (128-bit random, one per batch, exposed as `ReviewEvent.operation_id`). A trigger lets an operation id be reused only by the latest event with the same actor and rationale; it does not seal a finished batch against a later direct insert that copies all three, so batch integrity rests on the store generating the ids. All expected unit revisions are checked first. Each update, review event (with nullable `operation_id`) and sidecar hook runs in order before the next unit changes, preserving migration 015's latest-event attribution. A stale revision or any hook failure rolls back the entire batch. The authenticated wrapper forces the account actor and rechecks Owner and CSRF at both transaction boundaries.
- **Authenticated mode** (default; Owner only; `/admin/materials[/sources/{id}|/pages/{id}]`): actor forced to `account:<id>`, plus an immutable `source_web_events` sidecar (account, epoch, session digest; migration 015). Owner and CSRF are rechecked in the same IMMEDIATE transaction. Lists page at 50 rows (independent unit and history cursors); text reads are byte-guarded; images ≤16 MiB; POST ≤320 KiB. 409/422 re-render with the draft kept.
- **`--no-auth` loopback prototype** (`/sources`, `/sources/pages/{id}`, `/sources/search`, `/sources/setup`): free-text reviewer label, latest 100 events. Denied (403) in authenticated mode.
- Display HTML is sanitised with `ammonia` (text, list, heading, table tags; only `colspan`/`rowspan` 1–100, other span values reject the unit; no URLs, styles or scripts).
- Directories show selected pages against the PDF total, resolved or unresolved identity, and finalisation, which does not mean reviewed.

Every authenticated material write binds its Owner and CSRF admissions to the same session
id, exact expiry, account id and epoch, before and after the write. Independently valid
admissions from different sessions are refused. The authenticated single-unit path retains
`operation_id=NULL`, including a one-decision call to its atomic wrapper; multi-unit calls
retain one shared operation id. The standalone `review_units_atomic` API remains an explicit
batch API. Primary writes read only document/page identity before writing; they load the
editor only for a failed form. Suggestion writes compute the matching return link before
commit and redirect directly afterwards, without a post-commit detail read or difference.

## Stored suggestions

`Store::insert_source_suggestion` records advice separately in `source_suggestions`, bound to
the exact source and its bundle hash. Origin (tool/reviewer name) and kind are each 1–100
characters; rationale is 1–5,000; suggested text is nullable and at most 20,000; text fields
reject NUL. `kind` is one of `correction` (one unit, text), `merge` (2–1,024 distinct units on one page, text),
`missing_text` (page region, text), `reject` (one unit, no text), or `note` (one unit or page,
nullable quoted text, no acceptance action); SQL enforces the pairing of kind,
target shape and text, with at most 64 KiB of target JSON. Units/pages must belong to the source; every unit target records
a positive revision no newer than its current revision. Older revisions remain identifiable
as stale. A page note can retain a raw-node `block_id` citation (≤512 UTF-8 bytes, no NUL).
Region geometry has the same bounds as owner units, checked in SQL and Rust (page dimensions
that are not numbers are `Invalid`). SQL uses `source_suggestion_page_bounds`, keyed by
`(source_id, pdf_page)` from the prepared source, rather than by the mutable database page id.
Migration backfills all prepared pages of existing sources; source insertion and a change to
`prepared_json` while the source has no bounds fill the lookup. Once any bounds exist, prepared
JSON is immutable. Region checks join the cited page through its source and PDF page number,
so replacing or renumbering an unfinalized database page cannot transfer another page's bounds.
Suggestion insertion does not parse the entire prepared bundle. Advice and its nullable
same-source `report_id` are immutable.

`dismiss_source_suggestion` uses an IMMEDIATE transaction to append actor, time and an optional
reason (≤5,000 characters; blank/whitespace-only stored as NULL) to `source_suggestion_dismissals`. Repeat dismissal conflicts.
`source_suggestion_web_events` is the authenticated sidecar, tied to the latest dismissal.
`record_source_suggestion_application` records the latest matching review event, operation id
for a merge, owner unit when created, and whether the text was edited. Its SQL guards check
the decision kind, target unit/revision, page, and merge operation's rejections and initial owner
correction on that same page. A merge owner polygon must be exactly the axis-aligned bounding
rectangle of the targets, and its raw text must equal its first correction in the operation.
Consequently merge-created owner units retain that exact approved plain `raw_text`, rather
than the width-120 html2text projection used for other owner units: migration 025 requires
byte equality, including whitespace and wrapping. Their escaped HTML and search projection
are derived normally, and effective lookup uses their correction. Unifying the historical
raw projection would require changing that schema contract; no migration 026 is needed for
the review-screen fixes.
One decision event applies at most one suggestion; an owner unit can be named by at most one
application. `edited` equals whether the applied correction differs from the suggested text
(always false for a rejection). Applications and dismissals are insert-only and mutually exclusive. A unit revision
change alone never records an application. Neither suggestion insertion nor dismissal changes
units, their history, the search index or page completeness.

### Owner review screen

Saving a unit decision redirects (no JavaScript needed) to the next unit still unreviewed on the
same page (after the saved one, else from the start), or, when the page has none, to the next
later page of the source with unreviewed units (the same rule as the “Next page with units to
review” link), or to the page-completeness form when nothing is left from here on; the target page
shows a confirmation (“Saved unit N: accepted as extracted.”). If the next-target lookup fails
(deadline, busy pool, storage error) the save still stands and the redirect returns to the saved
unit with a neutral notice (“The next unit could not be found in time”); it never claims the
review is finished. The notice is shown only when the link is true (the unit is on this page, or
another page of the source for the next-page jump, and is in the stated state; for the
finished case nothing unreviewed remains from this page on); words come from a fixed set, never
from free text. An accepted-as-extracted decision needs no rationale (stored as an empty string,
shown as “No rationale given.”, with its normal audit event); corrected and rejected decisions,
owner-added units, page completeness and primary-reading choices still require one. The legacy
`/sources` screen shares this rule. The
selected-pages list explains that the checkbox only selects pages for the primary-reading choice.

`/admin/materials/pages/{id}` is an Askama-rendered page usable without JavaScript. It shows
the original image, sanitised rich extraction, separate lookup text and attributed review
history. Accepted lookup text is the search projection; corrected text is the stored correction;
rejected units have none, and unreviewed units are labelled “not evidence yet”. The lookup panel
lists the current 50-card page in bundle order. A merge-created owner unit appears at its first
target's position; other owner units come last. Incomplete pages and non-primary readings are
explicitly labelled as excluded from lookup.
Suggestion comparisons for unreviewed targets use the extraction's search projection and
are labelled as extraction text, not approved evidence.

The SVG uses the original page dimensions in PDF points. Every unit has a named, focusable link
to its card, including cards beyond the current cursor. Each unit box is
coloured by its review state and also differs by outline pattern (unreviewed dashed blue,
accepted solid green, corrected dotted orange, rejected dash-dot red); a legend with those words
sits above the image and each box has a text name with its state. Above 2,000 units the unit overlay is
omitted with a note. The full-size image and independent region crops remain available.
Missing-text overlays include the first 2,000 regions of the entire page, independently of
the 20-row notes cursor, with a note when more exist. Off-cursor regions link to their detail
panel. Missing or invalid page geometry omits overlays and drawing with an explicit note;
storage errors propagate instead of substituting fictitious dimensions.
Each unit card has one decision form: radios for accepted, corrected and rejected, and only the fields the choice needs, revealed by CSS `:checked` (no script): accepted none, corrected the corrected text and the rationale (required, SRC-4), rejected the rationale. The server ignores the fields a decision does not use. A refused unit save (422 or 409) shows a red error with `role=alert` inside that unit's card, marks the missing field `aria-invalid`, and adds a page summary (“A save failed — see unit N”) linking to it; `source-review.js` moves focus to the error.
`source-review.js` adds fit-width/100%/200% zoom, merge-region highlighting, rectangular
pointer drawing and arrow-key corner adjustments. Four polygon corners can also be entered
directly in PDF points in the Add missing text form.
Zoom buttons expose their pressed state. A keyboard-only box-creation button exposes the
corner handles without a pointer drag; it uses entered corners or a starter rectangle. Its
button explicitly warns that creating the rectangle replaces an entered polygon.

Cards show up to 20 suggestions, including merges on every target card. Each shows its state,
claimed origin, report id and a labelled rationale preview of at most 200 characters.
Repeated multi-target advice is full on the first rendered card (including owner-unit position
ordering), with a compact detail link on additional cards. Labels and the
compact comma-separated highlighting id list include at most eight targets, with the total
remaining target count in the label. Additional suggestions have a cursor-paged list at
`/admin/materials/pages/{page}/units/{unit}/suggestions`; page notes and missing-text suggestions
have an independent 20-row cursor. Notes cannot be accepted. Stale suggestions cannot be
accepted or edited, but may be dismissed; ordinary unit review stays available.

Accept loads the stored suggestion inside an IMMEDIATE transaction, checks every stored target
revision, and applies a correction, rejection, merge or missing-text insertion. The browser
submits only the required Owner rationale and optionally an explicit edit, never the proposed
text as authority. A merge creates one owner unit, rejects all targets, and attributes all events
and sidecars to one operation and rationale. Its rectangle is computed from SQLite's JSON
coordinates so application guards compare the exact same binary values. Zero-area rectangles
are refused. Applications record whether the resulting text differs from the stored proposal.
Dismissal records only advice state and optional reason. Every browser write uses an
authenticated store wrapper, rechecking Owner and CSRF at both transaction boundaries.
The detail form has separate **Accept as suggested** and **Accept my edit** submit buttons.
A present edit field submitted without an explicit choice is refused with 422 and retained;
**Accept as suggested** also refuses and retains any present edit, including an empty one,
that differs from the stored suggestion after CRLF/CR-to-LF newline normalisation,
requiring the Owner to choose **Accept my edit** or restore the proposal;
card acceptance without an edit field still applies the stored suggestion.
Selecting a primary reading for a missing page returns the authenticated route's uniform 403.

Unicode word differences use `similar` 3.2.0 (DEP-28), plain-text inputs and template-escaped
segments with fixed `ins`/`del` markup; adjacent changes of the same kind share one segment.
Screen reads resolve a suggestion's target page through the first target unit's primary-key
index before expanding its target list; other pages' lists are not expanded. Selected unique
suggestions are fetched in one query, and all card cursors in one ordered query. A screen may
read lookup text for at most **4,096 suggestion-target rows**. Subsequent suggestions compare
revisions against one page-scoped revision snapshot without accessing lookup text, and label
their lookup as omitted/truncated. Within the row budget, SQL limits aggregate lookup text
before it crosses into Rust.
Each review read transaction uses a **250 ms SQLite progress-handler deadline**, including
the page's ordered units and history, unit cursors, suggestion lists and detail target links.
An interrupted page, cursor, list or detail read returns HTTP **408 Request Timeout**;
retrying starts a new deadline. Interruption during the optional screen reads
discards the read result and returns a labelled reduced screen with links to unit suggestion
lists; suggestions, lookup text and overlays are omitted, and only the first unit decision
form is retained, including any refused POST's complete draft. Its lookup says “Lookup not
read (time limit)” without an empty evidence block. The reduced screen's primary-state read
has its own protected 250 ms deadline; if that also expires, the request returns 408.
The handler is cleared on drop, before
final authority checks and transaction completion. No writes run under this handler. This
is a VM-opcode deadline, not a hard real-time guarantee for an individual SQLite opcode.
Cards and suggestion lists read at most 2,000 characters per comparison side. A shortened
side forces a labelled preview, never a purported complete difference. Rationale reads stop
at their 200-character card preview. All comparisons run outside the store mutex in blocking
workers; screen, list and detail paths recheck Owner authority after that work. Limits remain 100 comparisons,
50 ms each and a shared 250 ms comparison window. Tests inject an expired deadline instead
of assuming a particular repetitive input always consumes 50 ms on every machine.

Lookup and suggestion display work shares a 2 MiB budget, with 512 KiB reserved for screen
structure (128 KiB on a suggestion list). Each rendered suggestion additionally reserves
**8 KiB** for its markup, controls, two CSRF copies, both origin copies, target labels and
identifiers, before optional text is allocated. With the first form reserved, at most **32**
suggestion rows fit on a screen, potentially fewer once their text is charged; omitted rows
remain reachable through the unit-list or notes cursor. Both lookup copies, correction controls, rationale
previews and comparison output count. Text charges conservatively include HTML escaping (six
bytes per input byte per rendered copy), and comparisons also reserve segment wrappers.
The first unit form reserves **1,280 KiB** before other text: HTML escaping takes at most twice
the 320 KiB form-urlencoded wire bound per rendered copy, and every submitted field is
conservatively charged at two copies (including an invalid state's option value and label,
and the submitted revision's control and truncated label). Rich extraction and historical review events have their own existing
row/byte bounds, so this is not a universal 2 MiB cap on the whole page editor. When a
correction control does not fit, a link opens that unit first on a card page with its full
form; an editable value is never silently shortened. Exhausted comparisons show labelled
previews of up to 2,000 characters, further shortened or omitted at byte-budget exhaustion.

Every suggestion links to `/admin/materials/suggestions/{id}`. Detail reads bound each text
and the aggregate merge lookup to 20,000 characters. A shortened lookup is explicitly
labelled separately from shortened suggested text; neither prefix can produce a complete
difference. The renderer reserves
the entire escaped evidence, edit/draft, rationale, controls and target links before adding
a difference and renders exactly once in its blocking worker. **256 KiB** is the detail
renderer’s planning threshold for adding differences and retaining full evidence, rather
than its enforced response bound. If necessary it omits the difference and shows labelled
2,000-character evidence/rationale previews and the first eight target links. The edit/draft
value remains intact. The enforced bound is **2 MiB of complete rendered UTF-8 HTML** on
GET as well as 409/422 responses, including oversized invalid drafts accepted under the
320 KiB POST limit. GET initializes the intact edit field from the stored proposal and uses
the same renderer and bound. Exceeding that bound returns a server error.

409 and 422 responses retain submitted values and expected tokens in the matching form or
suggestion detail panel, with an alert and a cursor-aware return link. Reloading is an explicit
choice; a failed form never silently adopts newer revisions. Textarea CR LF is normalised to
LF before validation, and leading newlines survive HTML parsing. Stable `data-unit`,
`data-suggestion`, `data-form`, `data-target`, `data-overlay-unit` and
`data-overlay-suggestion` attributes identify review elements.
In particular, a card Accept/Dismiss failure opens the suggestion detail panel, retaining
the submitted draft there, rather than re-rendering it on the original card. This supplies
one bounded edit/error form for multi-target advice and a matching-card return link.

Navigation links lead to the next unreviewed unit and the next later incomplete page of the
same reading. With the script, `j`/`k` focus adjacent cards and `a` focuses an acceptance
rationale without submitting. Shortcuts apply only inside the review list, outside form fields
and editable elements, and can be disabled per browser in the help panel. Focus indicators are
visible. Every Owner material page carries the sheet pages' strict policy (`SHEET_CSP`:
scripts and styles from this origin only, no inline script or style, Trusted Types with no
policy). This strict policy covers material pages (including suggestion details), both
`/assets/source-*.js` scripts, page and crop PNGs, successful POST redirects and retained
409/422 draft pages. Rebuilt refusals (including HTML browser error documents and route
misses) and unauthenticated navigation's login redirect keep only `frame-ancestors 'none'`.
Zoom sizes the page image through the CSSOM. Browser checks:
`tools/review_browser.py` (synthetic 60-unit review bundle and v2 report from
`examples/source_fixture.rs`, 1280×720 and 390×844, with and without JavaScript).
Overlay and next-unit links use fragments when the target card is already on the current
card page, preserving unsaved fields; off-cursor navigation still loads another card page.
The shortcut preference switch stays hidden until the script runs. The acceptance/dismissal
unit test named `material_suggestion_accept_dismiss_serialized_store_calls_have_one_winner`
checks competing calls through the single-connection store mutex, not SQLite concurrency
across independent connections; integration coverage separately races file-backed stores.

### Report import (`mimik import-suggestions --source ID --report FILE`)

`Store::import_suggestion_report` reads a regular local file through `local_file::read_path`
(final-component symlinks and special files refused), at most 4 MiB and 5,000 issues. The closed
formats are `schemas/suggestion-report.v1.json` and `.v2.json`. Unknown fields, malformed targets,
empty rationales and out-of-source pages refuse the whole report. Failed pages carry an error,
import no issues and are counted. Report and page states distinguish unverified model/tool advice
from failed review. Origin is the report's claim, `tool:<name>` or `reviewer:<requested_model>`,
at most 100 UTF-8 bytes, without control characters; it is not authenticated provider identity.
The report timestamp is nonempty, at most 64 UTF-8 bytes, without control characters.

The PDF hash must equal the source's hash. v2 requires the exact stored `bundle_sha256`; v1
also permits `artifact_sha256["source.json"]` equal to the hash of the stored raw extraction
only when no bundle binding is declared. A contradictory declared bundle hash is refused.
v1 actions come only from shape: an extracted
unit with nonblank correction text becomes a correction; absent, empty or whitespace-only
text makes it a unit note. A raw container/page
citation or null block id becomes a page note retaining any quoted text. An unknown block id
refuses import. Free-text `kind` never selects reject or merge. v2 requires `action` per issue;
only merges carry `merge_block_ids` (2–1,024 distinct extracted units on one page, nonzero
bounding rectangle), and only missing text carries `region` (four PDF-point corners).
All unit targets use revision 1, so advice about an already-decided unit is stale.

One IMMEDIATE transaction writes immutable `source_suggestion_reports` metadata (report hash,
source, origin, version, binding, report timestamp as given, import time, action and failed-page
counts) and its suggestions. Re-importing identical report bytes for the same source is a no-op.
The CLI prints a JSON summary with report id, counts by action, failed pages, binding and `repeat`.
Refusals distinguish PDF-hash mismatch, missing/mismatching binding, out-of-source page,
duplicate page and schema/parse errors (line and column, without report content). Issue errors
use one-based numbers. Storage failures retain their storage error classification. Unknown
source ids report `Source not found.` and print no summary; character routes retain their own message.

Both Python report producers validate the closed emitted v2 findings subset and importer
bounds before accepting each page: description 1–5,000 characters and nonblank, correction
at most 20,000 characters, block id at most 512 UTF-8 bytes, no NUL, at most 5,000 issues per
report and a 4 MiB serialized report. They use the v2 JSON Schema when `jsonschema` is
available and explicit equivalent checks for their emitted correction/note subset otherwise.
Invalid pages are marked failed with an actionable error; text is never silently truncated.
Empty or whitespace-only model text proposals map to notes.

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

- `tools/ocr_server.sh start|stop` serves surya's OCR model (`datalab-to/surya-ocr-2-gguf` at a pinned revision, hash-checked, modified OpenRAIL-M) with the pinned llama.cpp b12110 in a container on the internal network `mimik-ocr-net`; It refuses an existing `mimik-ocr-net` that is not internal, runs with CPU, memory and PID limits (2 cores on weekdays 09:00–18:00, `CPUS` otherwise) and refuses `DEVICE=gpu` (the Vulkan build, image encoder on the CPU) on weekdays 09:00–18:00. It does not stop itself: run `stop` after the extraction.
- `tools/extract_manual.py PDF --source-id ID --pages 28,30-33 [--mode fast|balanced] [--ocr auto|force|disabled] [--ocr-server URL]`, run on `mimik-ocr-net` when OCR is on. One-based pages, converted for Marker; `use_llm=False`; headers and footers kept; JSON and Markdown from one build. OCR needs `--ocr-server` (surya attaches to it, never spawns or downloads); the manifest records what the server reports (served model, which must be surya-ocr-2 alone, model path and llama.cpp build) and the surya version, not a hash of the weights (those are checked by `ocr_server.sh` at start), and any "OCR failed" warning fails the run, since Marker would keep the block without its text. Forced OCR is the route for art-heavy manuals (`docs/results.md`, extractor screen). Writes `data/extractions/<id>-<uuid>/` (`source.json`, `reading.md`, metadata, `manifest.json` with hashes/config/versions/status, `review.json` all unreviewed). Mismatched coverage marks the run `failed`.
- `tools/prepare_review.py EXTRACTION --pdf --title --system --edition --language` (can run with `--network=none`). Verifies the snapshots, renders the verified temporary PDF copy with `mutool draw -r 110` at original indexes, checks rotation, origin and size, and emits `review-ready-<uuid>/` (`bundle.json`, `source.json`, `page-N.png`). It writes **contract v1 only**.
- `tools/crosscheck_text_layer.py BUNDLE --pdf FILE` aligns each unit's text with the PDF text-layer words inside its box and writes `text-layer-check-<uuid>/report.json` as v2 with the bundle hash: spelling corrections (an OCR word with a near-identical text-layer twin, same digits and non-letter characters), difference notes (a twin differing in digits, dice, signs or markers, where either may be the misprint) and unread-word notes (three or more text-layer words the unit's text lacks, each occurrence counted). Suggestions only; nothing is applied.
- `tools/review_extraction.py BUNDLE --pdf FILE --pages LIST --api-url URL --model MODEL`: an explicit, owner-billed call sending page images and blocks from a prepared bundle to an HTTPS OpenAI-compatible endpoint. Writes v2 with the prepared bundle hash; extracted-unit text proposals become corrections, other findings notes. Key from `MIMIK_REVIEW_API_KEY`/`OPENAI_API_KEY` (environment, then dotenv). Redirects refused, keys not stored, unknown citations rejected; findings are suggestions, never applied.
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
