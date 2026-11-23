# Sources and retrieval

A **source** is one prepared extraction run of a locally owned manual: the original PDF, the Marker structured extraction (`source.json`), the prepared manifest (`bundle.json`) and one original-page PNG per selected page. Sources are stored privately in SQLite and reviewed by the Owner. Reviewed units feed lexical lookup. Nothing in this workflow lets a model approve evidence or change sheets or rules.

Code: `src/sources/`, `src/auth/materials.rs`, migrations `003`, `004`, `005`, `007`, `009`, `015`, `017`, `024`–`026`, `029`, `036`, `assets/source-regions.js`, `assets/source-review.js`. Tests: `tests/source_*.rs`, `src/sources/upload_tests.rs`, `src/campaigns/material_tests.rs`, `tools/test_source_tools.py`.

## Internal PDF worker (Linux)

`sources::worker::inspect(path)` and `inspect_cancellable(path, &Cancel)` run a blocking,
confined inspection. `words(path, pages, &Cancel, callback)` delivers the validated words of one to eight pages the same way (the import's word boxes). `render(path, pages, &Cancel, callback)` delivers one validated page
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
zero, nice to 10, and equal soft/hard CPU limits (inspection: 20 seconds plus one second per
32 MiB of PDF, at most 200 seconds, because the page count is not yet known; 25 seconds per
rendered page). The parent enforces, for inspection, a base wall of 30 seconds plus 1 second per 8 MiB of PDF
(at most 600 s; MuPDF may read the whole file, for instance to repair it, before the first page
record) plus 0.1 second for each page record received and validated (not for a claimed count),
or 30 seconds per render page. The inspection
constants were checked by the host large-input gate (small synthetic files: at most 1.6% of the CPU limit and 0.1% of the wall deadline up to 20,000 pages; `docs/results.md`) and stay as ceilings for hostile input. The worker opens the bound `/in/document.pdf` by path with MuPDF's stdio stream, which seeks and reads on
demand: the PDF is never copied into memory, so its size adds no address space and no limit
of its own (a PDF can be larger than 128 MiB, or have its xref beyond 4 GiB). The pre-open
check (`%PDF-` in the first 1,024 bytes, `%%EOF` in the last 1,024) reads those two windows
by seek. A failure to open is `Unreadable`; `TooLarge` is only for allocation and limit errors
after the open. The worker accepts at most 20,000 pages: the inspection record is one 9 MiB frame
and a page record is at most about 450 bytes. The upload tables store the same 20,000 pages (a
longer document fails as `too_many_pages` after inspection). A damaged large file makes MuPDF
repair it by scanning the whole file; the inspection deadline, which grows per page, bounds that
scan, and a miss is reported as `took too long`.
Cancellation/deadlines kill through pinned pidfds, and results wait for tree exit. Stdout
uses the framed record reader; a separate stderr drain matches at most 64 KiB and discards
the remainder without logging raw output. The closure executes the explicitly bound loader
with Mimik as its program argument; no loader file bind overlays a system-library bind.
Cleanup kills all verified pins before a bounded five-second wait. A cleanup timeout
interrupts transport readers and reports upload unavailable, never a successful tree exit.

Before any MuPDF call, the worker installs a second
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
both figures are reported independently. The input is opened by path and never buffered; sparse
inputs over 128 MiB and with an xref beyond 4 GiB must stay far below the 75% gate. The default MuPDF store is used because mupdf 0.8.0 exposes no
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
`language` (30 bytes), `file` (no size limit of its own). Unknown, duplicate, out-of-order and
post-file fields are refused. Owner, Host/Origin, declared length syntax, worker availability and
the upload slot are checked before the body is read. CSRF admission precedes all PDF writes;
metadata uses the import identity rules. The route has no static body limit (the PDF is bounded by
disk only): once admission and metadata validation pass, the upload reserves its declared
`Content-Length` (the 512 MiB margin covers the multipart overhead), or 1 MiB when none is
declared, and grows the reservation in 64 MiB steps before a write would pass it. A reservation
or top-up that free space cannot cover, or a write that fails with ENOSPC, answers 507, unlinks
the spool and releases the reservation and the slot. The server is HTTP/1 only. Before CSRF admission the parser receives at
most 4 KiB; the process holds at most the pending frame plus hyper's channel slot and read
buffer, a small constant multiple of hyper's approximately 408 KiB maximum buffer
(SRC-UPLOAD-POLL in `limitations.md`). No file byte is spooled before admission; only one
upload streams at a time. After admission explicit Pending boundaries every 64 KiB bound
parser read-ahead to two 64 KiB batches plus delimiter lookbehind, apart from the pending
transport frame. The only upload deadline is 30 seconds without input (idle): a stalled client
holds the single slot and a spool. There is no total deadline and no minimum rate, so a slow
uplink that keeps sending works; metadata validation, reservations and file writes do not
consume the idle budget. The 4 KiB pre-CSRF read, the form-field sizes and the 64 KiB read-ahead
boundaries stay for unauthenticated-memory and parser-memory reasons, and after admission the
bytes that are not file data (headers, form fields, closing boundary, trailer) are bounded at
512 KiB (413), because the parser buffers a part header until its blank line.
An unavailable worker returns 503 with the probe's fixed reason and remedy before body
polling. A second upload returns 409: an active stream is in progress, or a retained upload
is already staged with a link to open or discard it.
An asynchronous-safe RAII wrapper schedules partial-file deletion and slot release on a
blocking thread on disconnect, refusal or future cancellation. Hashing is incremental;
publication rechecks Owner/CSRF and the PDF's size and first-1,024-byte magic; the stream's hash is
trusted there and the first job verifies the stored bytes (see Job pins below).
Original filenames are neither application arguments nor stored/logged identifiers.

`/admin/materials/uploads/{id}` shows size, hash prefix, matching permanent document,
inspection page count/sizes/rotations/encryption, and the page selection: any ascending,
distinct subset of the inspected pages (at most 20,000). The screen lists 100 pages at a time
(`?p=N`; the selection list, the rendered pages and the manual-box editors share that window,
so a page never carries 20,000 entries). The Owner types a range list (`1-500, 520`; unordered
and overlapping ranges are merged) or ticks "select all"; the list is prefilled in the compact
form `1-500,520`. The longest list, alternate pages of a 20,000-page document, is about 75 KB
form-encoded, inside the 131,072-byte action body limit.
POST actions at `/{id}/set`, `/inspect`, `/render`, `/cancel`, `/discard`, `/propose`,
`/exclude`, `/exclude-range` and `/remove-exclusion` require Owner,
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
It shows the text only to the Owner, affected pages and absent pages (as ranges), and the
proposals' boxes on the pages of the screen as a combined preview. Each proposal has its own
confirm control (unchecked until the Owner ticks it); several can be confirmed in one exclusion.
The form posts the confirmed fingerprints (never indices), per proposal the affected pages to
leave out and the absent pages to extend to, and the manual boxes of the screen's pages; the
server re-runs the proposals and rebuilds the boxes, so no coordinates of proposals travel.
Manual boxes use the same editor as the source review screen (numeric displayed-point inputs;
pointer corner grips and keyboard arrow-key handles enhance them) for the first box of a page,
and a textarea (`left top right bottom` per line) for up to seven more: eight boxes a page at
most (header, footer, page number, side tabs). The inputs' `max` is the page size rounded up
to 0.01 pt (the same function as the stored boxes' limit), so a stored edge box passes the
browser's own validation. Unticking a page drops it together with the further boxes the form
prefilled for it; further boxes of an unticked page that differ from the saved ones are refused,
not dropped. A repeated form field is refused whole (the last one never wins). Proposals with
the same text (one text at two places) share one fingerprint and so one confirm, skip and absent
control; the view lists their pages together and the preview shows every box. The boxes of
pages outside the screen are kept, with their proposal fingerprints, which join the newly
confirmed ones. Boxes carry no record of which proposal produced them, so a windowed edit can
never tell that a stored fingerprint no longer has a box behind it: fingerprints stay until the
exclusion is removed or replaced whole (a confirmation without a window). A run offers at most
three proposals and an exclusion holds at most eight distinct fingerprints, so a ninth
confirmation is refused until then. A confirmation replaces the boxes of the screen's pages.
Every write requires a rationale.
Absent pages' median boxes require explicit extension after the missing-text warning. Removal
requires a fresh rationale. Confirmed regions have opaque black SVG masks on staged renders;
the stored PNG and the original-image endpoint remain unchanged (SRC-13).

Boxes on a page range: a separate form on the exclusion screen (never nested in the `exclude`
form), posted to `/{id}/exclude-range`, edits the saved exclusion only (unsaved edits of the
screen form are discarded). It takes a page range (`parse_page_ranges`) or all selected pages, a
mode (`add`, `remove` or `clear`, a strict enum; `clear` needs an acknowledgement that proposal
boxes go too), one to eight boxes (*left top right bottom*, displayed points, origin top-left)
with the page they were drawn on, and a rationale (an exclusion has one; a range edit replaces
it; it is never prefilled, like every saved rationale). The boxes are checked on the source page
(`check_box`), divided by its displayed size, multiplied by each target page's, clamped to the
target's extent and rounded outward like every stored box; equal sizes keep their points exactly.
`add` merges exactly equal boxes; `remove` deletes exactly matching boxes and counts the pages
without a match; `clear` empties the pages. Proposal fingerprints are kept in every mode. A
result with no box left appends a removal event; an empty set is never stored. The whole request
is refused, naming the first page (422, the screen rendered again with the posted values), when a
listed page is not a selected rendered page, a page would exceed eight boxes, or a box collapses
on a page; a result equal to the saved exclusion and `clear` or `remove` without an exclusion are
refused too. Stale upload or exclusion revisions are a conflict. Success redirects to the screen
with the number of pages changed and of distinct page sizes among them (and, for `remove`, the
pages without a match). With JavaScript each page editor has a button that copies its current
boxes to the range form; each editor's legend shows the page size.

Stored boxes are rounded outward to 0.01 pt (minimum floored, maximum ceiled, so coverage is
never lost) and made bit-identical when numerically equal, because the migration compares boxes
as text (`[1,1,2,2]` and `[1.0,1,2,2]` differ); equal boxes of several confirmed proposals are
de-duplicated, not refused. A box must lie inside its page rounded up to 0.01 pt (a box ending at
the edge of a 595.2756 pt page stores 595.28), which keeps every coordinate at most seven
characters, so eight boxes stay under the 384 bytes a page the migration view allows. Each
revision stores the whole set, so the history grows by up to 6.4 MB per revision at 20,000 pages
(`limitations.md`). The masking rule that derivation applies (`exclusions::masking`, route A):
a unit rectangle wholly contained, edges inclusive, in the union of its page's boxes is
rejected (a header drawn as adjacent left and right boxes covers it); positive-area overlap
without containment stays unreviewed and flagged; edge contact alone is not crossing. Staging
only validates and carries the boxes.
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
unknown disk usage blocks new reservations until a successful reconciliation. A progress poll
reads only the job row, never the inspection (up to 9 MiB) or the page rows, and publishing a
page does not rewrite the upload row that holds the inspection. Restart drops
page rows unless both PNG and text sidecars are no-follow regular files with mode 0600,
settling the upload state and retained job progress in the same transaction.

`begin_upload` claims the single slot and a private spool file and returns them as an RAII
phase; it reserves nothing. The route finishes authentication, CSRF admission and metadata
validation, then `reserve_body` reserves the body (see above) before the first PDF byte is
written. Each write goes through the phase, which tops the reservation up when needed and
charges the bytes after the write succeeds. Dropping the phase unlinks its spool and releases
the slot/reservation. `create` checks the declared size by `fstat`, the hash's form and `%PDF-`
magic without parsing the PDF and without re-reading the spool (the route's streaming hash is
trusted for the stream; no multi-GB read happens under the control and gate mutexes), fsyncs and
renames the spool, then inserts the upload and queued inspection in one
Owner/CSRF-rechecked IMMEDIATE transaction. The original filename is never an argument.
After that commit, `create` returns the upload ID even if worker launch fails; the Owner can
read the failed job and retry or discard it.

Migrations 026 and 036 define `source_uploads`, `source_upload_pages`, `source_upload_exclusions` and
`source_jobs`: mutable staging tables with cascading deletion, separate from permanent
source immutability. Migration 036 (SRC-15) dropped and recreated them (no release had shipped,
so staged uploads were not copied; the startup reconciliation removes the staged files of dropped
rows) with the stored bounds that follow the inspection frame: `pdf_page`, `pages_done`/`pages_total`
and the inspected page count up to 20,000, a selection or exclusion page list of at most 20,000
entries, and no size bound on `pdf_bytes`. They store metadata/hashes, not PDFs, PNGs or extracted text.

**Job pins (SRC-15).** The worker reads the staged PDF lazily for the whole run, so every job
(inspect or render) first opens `document.pdf` relative to its staging directory descriptor with
`O_NOFOLLOW`, requires a regular file with one link owned by the effective user, takes **pins**
(device, inode, size, mtime and ctime in nanoseconds; the change time cannot be set by an
unprivileged user) and only then hashes the whole file once on the job thread (1 MiB reads,
cancel-aware, outside every lock), comparing with the stored `pdf_sha256`. Pins are per job and
never stored: an upload staged before this check (0600) simply gets its pins at its next job. The
launch (`prepare`) opens the file again without following links, requires its `fstat` to equal the
pins before the profile is built, and hands the pinned device/inode to the sandbox's bind check,
so a name that resolves to another object fails the mount check; after the worker exits (whatever
its result) the launch descriptor is compared with the pins again. The job repeats the comparison
on its own descriptor after every worker exit, before the next batch is reserved, and at the job
end, where it also `fchmod`s a still-0600 file to 0400 (hardening, not a boundary). Any mismatch,
a symbolic link, a second link, a non-regular file or a wrong hash is `document_changed`: the
upload fails permanently, **every** page row and file is removed (not only the batch in flight),
`source_uploads.document_changed` is set, the job reason is `document_changed` (shown as "the
staged file changed after it was uploaded; discard this upload and upload the PDF again"), and
inspect, render, retry and selection changes are refused; only discard remains. `contract` also
refuses a changed upload and requires the last job to be a render job that `succeeded`.
Limits: the pins see a write only to the clock's granularity, so a same-size write that lands in
the same coarse timestamp tick as the file's last change time, after the hash has read that
region, is not seen (kernels from 6.13 with multigrain timestamps may narrow this; not verified
here). A process of the same user that already holds a writable descriptor or a `MAP_SHARED`
writable mapping from before the job can change bytes without any timestamp update, and
`fchmod 0400` does not revoke it; only the application's user and root can reach `data/staging`
(0700). A read lease is a possible follow-up (`docs/limitations.md`). A restart or a panic during
a render drops that upload's pages, since their pins are gone. Files are
descriptor-relative and no-follow under `data/staging/<32-hex-id>/`: directories 0700,
`document.pdf` (0400 after its first job), `page-N.png` and `text-N.json` 0600. Page writes use exclusive temporary names,
fsync and atomic rename. Newly created files/directories are fchmod-normalized to their private
modes even under a restrictive umask. The exclusion table holds actor attribution, rationale,
pages, displayed-space boxes (a list of 1-8 per page, at most 384 bytes a page), a list of at most
8 proposal fingerprints, removal flag and revision. Set, replacement and
removal append rows; upload deletion/expiry cascades to all exclusion history. It stores no
proposal text. The current exclusion and its independent revision (zero before the first row)
are part of the upload model. A removal retains its historical boxes but exposes no current
exclusion. Writes recheck Owner/CSRF in the transaction and require both the upload revision
and exclusion revision. Selection changes that would deselect an excluded page are refused
until the Owner replaces or removes the exclusion; unrelated selection/metadata changes retain it.
The schema uses `group_concat(... ORDER BY ...)` (migration 036) and `->>`, so external tools that read the database need SQLite 3.44 or later.

The manager owns a process-wide ledger of used bytes and reservations. Admission is by free
space only (there is no staging total): a reservation is admitted while the filesystem's free
space is at least the outstanding reservations plus a 512 MiB margin that keeps SQLite and its
WAL writable. Each eight-page render batch reserves 160 MiB. Written bytes leave the
reservation and join the used figure, so a byte is never counted by both the reservation and
the filesystem's free space; deletes reduce usage and failures release the unwritten portion.
Usage that a scan could not measure is an explicit unknown state that refuses every
reservation until a scan succeeds. Per upload there is no PDF size limit and no total: no
selected-page count, no PNG or text sum, no unit count (publishing a page reads only that page's
facts). Capacity failures use `space`. `already_document` identifies a matching
permanent document without merging readings.

Limits that remain, each with its reason: form field sizes and the 4 KiB read before CSRF
admission (unauthenticated memory); per PNG 4,096 x 8,192 px, 12 Mpx, 12 MiB and a 64 MiB decode
(decompression bombs); per page text 8 MiB, depth 32, 40,000 nodes, 64 KiB HTML per unit and
512-byte block ids (parser and row size); the worker's 2 GiB address space, 16 MiB scratch and
CPU/wall limits (hostile PDFs); 160 MiB per eight-page render reservation (8 x (12 + 8 MiB));
one upload in progress and one retained upload; 20,000 pages (the inspection record is one 9 MiB
frame); eight boxes a page and eight proposal fingerprints (row size); 50,000 proposal groups,
a 64 MiB proposal memory budget, 50,000,000 proposal comparisons and a 500,000,000-unit proposal
work budget (server memory, and the time the gate is held); the 30-second idle upload deadline and the 512 KiB of non-file multipart bytes (a stalled client holds the single slot; the parser buffers a part header); the 512 MiB free-space margin (keeps SQLite and its WAL writable) and 64 MiB reservation top-ups (the declared length may be absent or wrong). No limit applies to a whole upload or source: not the PDF size, the number of selected pages, the sum of PNGs or text, or the unit count. The CLI import keeps its whole-file bounds, because it holds each file in memory (see Import).

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
substituted fonts/text statistics and metadata; it carries no limits and no PNG sum (the
consumer reads it per page: about 12 MB at 20,000 pages).
It refuses an incomplete/outdated rendering and includes the current exclusion and its revision.
This is staging
only: no permanent source, review approval or evidence is created. The later source-creation
step must consume this contract and copy permanent page facts/images/exclusion geometry (the
boxes of each page, as lists)/rationale and the exclusion's list of opaque proposal
fingerprints (empty for manual boxes),
then discard staging; it must not parse the PDF again or retain staging text/proposals.
The fingerprints' upload-local secret is not in the contract and is deleted with staging;
after that deletion the copied fingerprints are provenance only, not recomputable text hashes.
The contract refuses exclusions naming pages without a current selected rendering, including
page rows removed by missing-file sweeps or render replacement.

### Repeated-text proposal

`Uploads::propose_exclusions` reads staged text under the publication/deletion gate, one page
at a time: it checks the stored byte count and SHA-256, validates the text-line schema and page
bounds again, and drops it after use. It does so twice, and the gate is held for both passes
(measured figures below). Pass 1 counts, for each distinct normalized text of 4 to 200 characters, the
pages it appears on (a text once a page, keyed by its 128-bit digest, about 48 bytes a text).
A text on fewer pages than the threshold can never reach it, so after pass 1 it is forgotten
and never seeds a group; pass 2 is the streaming proposer. Only candidate groups
outlive a page.
It requires a complete current selection, no queued/running job, and Owner read admission
before and after the read. Propose responses never include an automatic refresh.
The pure proposal function normalizes NFKC, collapses whitespace, case-folds with
`unicase`, and replaces each Unicode decimal-digit run with `#`. No text is written to SQLite,
logs or error messages. Confirmation accepts only current proposal fingerprints (up to eight
distinct) or none for a manually drawn/adjusted exclusion. Fingerprints are SHA-256 over a random
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
fraction conversion. Ordered group indices skip the groups already matched on the current page
and the ones created on it, until the next page. At most 2,000,000 IoU comparisons
per normalized text across the selection are allowed: an exhausted text is dropped entirely,
while other texts retain exact grouping and remain eligible. After each page, tracking also
stops for a text if neither any existing group nor a future seed can reach the page threshold.
The proposer is bounded in memory and in work (live accounting: groups, their texts and every
stored box at about 64 bytes): 64 MiB, at most 50,000 live groups, 50,000,000 comparisons per
run on top of the per-text budget, and a deterministic work budget of `WORK_BUDGET` =
500,000,000 units. A work unit is one comparison, one index step of a scan (consumed,
same-page and compared indices alike) or of a prune (each tracked text and each of its
indices), 8 units for each text line (normalising, digesting, sorting), one unit per 8 bytes of
a page's JSON when it is read and hashed, one per 8 bytes of its line text when it is fed, and
one per 8 bytes of each distinct line's normalized text (NFKC can expand text about elevenfold,
so lines whose normalized text is not 4 to 200 characters are dropped right after, before
sorting). Scans do not copy the per-text index sets: a
page skips the groups it already matched and the ones it created. The run holds the upload
gate, as does `set_exclusion` when it confirms proposals, so the budget is what bounds the
hold. Pass 1 spends the same kinds of units as pass 2 and shares the budget and the memory
budget; each pass charges the read of every page (one unit per 8 bytes of JSON), so the bytes
are charged twice. Lines over 200 characters are charged (read, line and normalized-byte
units) but not counted or grouped. Measured with the release benchmark `proposer_calibration`
(12 cores; it serializes, hashes, parses and validates each page with the production function,
no disk reads): ordinary manuals of 20,000 pages fit the default bounds with at most 3 live
groups. At 47 lines of 80 characters a page (4 KB): 8.3 s, 90 million units (18%), 93 ns a
unit, 940,003 counted texts (45 MB). At 47 lines of 400 characters (about 20 KB): 27.7 s,
315 million units (63%), 88 ns a unit; a page of about 30 KB of such lines would no longer fit
`WORK_BUDGET`. At 47 lines of 200 characters (10 KB of groupable text): 16.3 s, 174 million
units (35%). A manual of 20,000 pages with 97 distinct lines of 200 characters a page (about 20
KB of groupable text) exceeds the count map's share of the 64 MiB budget (1.4 million texts) and
is refused after 12 s. A unit costs 74 ns (tombstone scans) to 144 ns (NFKC expansion) here, so
the longest the gate is held by one run is `WORK_BUDGET` times the highest cost, about 72 s of
CPU (37 s measured for the tombstone worst case, which is refused at the budget), plus the
disk reads of two passes over the staged text (not measured; the host gate reports them for the
render). Reaching any bound refuses with the same "comparison limit" error. A group that alone can no longer reach the
threshold becomes a tombstone: its boxes are freed but its seed and index stay, so later
scans and budgets behave as before and a match on it is consumed without a box; a text none of
whose groups can reach it is freed whole and only its digest is remembered. The output equals
that of the previous whole-corpus function (kept as the test oracle) with one intended
difference in the result: that function refused when nothing qualified and some text had
exhausted its per-text budget; a text on too few pages now never spends that budget, so a run
can return no proposal where the old function refused. The work also differs: the staged text
is read, hashed and parsed twice, and the budget is charged for both.
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

Only the operator imports prepared sources, through the CLI. Browser PDF uploads stage pages separately; there are no archives, URLs or nested imports. `Store::import_source` reads everything first (`load_import`) and then writes it all in one transaction. The CLI uses `import_source_with_words`, which also reads the word boxes (below) before that transaction.

**File access.** The directory is opened `O_DIRECTORY|O_NOFOLLOW`. Its children are opened with `openat` under fixed names: `bundle.json`, `source.json`, `page-<pdf index>.png`. `local_file::read` rejects final symlinks and special files and reads with a byte bound. Symlinks in ancestor directories are allowed. Each file is hashed and parsed from the same bytes.

**Bundle schema** (`deny_unknown_fields`): `contract_version`, `source_id` (≤100 B), `title` (≤200 B), `system_id` (a valid registry slug), `edition`, `language` (≤30 B), `source_sha256`, `extraction_sha256`, `extractor` and `renderer` provenance (name ≤100 B, version ≤200 B, `settings` an object), and `pages[]` (`pdf_page`, `width_points`, `height_points`, `image_sha256`, `units[]` with `id`, `kind`, `polygon`, `html`).
- **v1**: no `edition_id` (not even null); label ≤100 bytes; `system_id` is a registry slug; the source stays unresolved until a registered alias attaches it.
- **v2**: `edition_id` slug of a registered pair; label equal to its registered label or an exact same-pair alias, ≤100 scalars (`registry::source_identity`). Identity and any existing association are checked before the duplicate-hash path.

**Checks** (all fail closed with one generic "inconsistent bundle" error):
- The PDF and `source.json` hashes match the bundle. The PDF parses with `lopdf`; its page count becomes `total_pages`.
- Each page: a unique one-based index present in the PDF; one raw `Page` node; zero rotation; CropBox or MediaBox origin 0,0; 1–4,000 points per side, within 2 points of the declared size; raw page polygon equal to the declared rectangle.
- Units must match exactly the eligible raw leaf nodes: same id, kind, polygon and HTML, with no extras and none missing. Eligible kinds are in `schemas/source-kinds.json` (text, list, heading, table, form, caption, code, footnote, equation and similar). Kinds in `schemas/source-leaf-kinds.json` (`ListGroup`) are units only when they have no children: Marker's OCR gives a whole list as one `ListGroup`, its text-layer path a `ListGroup` of `ListItem` units. A bundle prepared before this rule that left such a group out is refused (SRC-10). An eligible node with children or a `<content-ref` is rejected, so a table or form is one unit and nested groups are never flattened. Other kinds (pictures, page headers) stay only in the raw tree and the page image.
- Polygons have exactly 4 finite points inside the page. Block ids are at most 512 bytes. The raw tree depth is at most 32, with at most 40,000 nodes per page.
- PNG: hash matches; no APNG; at most 4,096×8,192 px and 12 Mpx; aspect within 0.01 of the page; full decode within 64 MiB.
- Size limits, which apply to the CLI import only (it reads each file whole into memory and parses the JSON and the PDF with `lopdf` in one process, so these bound its memory; the browser upload path has none of them): PDF 128 MiB, `source.json` 32 MiB, `bundle.json` 8 MiB, 64 pages, 20,000 units, 64 KiB HTML per unit, 12 MiB per PNG and 128 MiB of PNGs in total. Over-limit input is rejected, never truncated. The PNG, decode and tree bounds above are per page or per object and stay for the same reason as in the upload path.
- Every unit's HTML must pass the sanitiser and both text projections, or the import fails.

**Word boxes** (migration 029, SRC-20). After `load_import` has the hash-verified PDF, the CLI asks the confined worker for the words of every bundle page that has none: it writes the PDF to a private 0600 file (`O_EXCL|O_NOFOLLOW`) in the data directory's own 0700 `import-spool` folder (never the system temporary directory, and not `staging`, whose upload sweep deletes unknown files; stale `import-*.part` files over an hour old are removed at the start of an import), pins the descriptor (device, inode, size, mtime, ctime), hashes it through the same descriptor and hands the worker that file by path through the pinned launch (it compares the file with the pins before the sandbox starts and after each worker exits; the same residual limits as for staged uploads apply), runs `inspect` (page sizes must be within 2 points of the bundle's, else the page is skipped) and then `Words` in batches of at most eight pages, each with the render deadline (30 s a page) and the same sandbox; a batch that fails is retried page by page so one hostile page costs only itself, a word whose box collapses when narrowed to `f32` is dropped, and the file is removed afterwards. Word text with a control character is refused by the parent and the reader (the worker splits words at control characters like whitespace). Size-mismatched pages are counted apart in the report (running again cannot help), and a worker that goes away after some pages were stored is reported as partial. The worker never runs inside the write transaction: the validated, encoded rows are inserted in it. A `Words` record is a JSON array of `{text, box}` per page in the same corrected page space as `TextLine` (PDF points, y down), at most 20,000 words of 1 to 256 bytes with finite boxes inside the page (a 8 MiB frame, parent-validated). `source_page_words(page_id, words)` is append-only (insert once, update/delete triggers, a one-row-per-page insert trigger because `OR REPLACE` skips delete triggers): byte `1`, then per word four little-endian `f32` (`x0 y0 x1 y1`) and a little-endian `u16` text length with 1 to 256 bytes of UTF-8, at most 5,480,001 bytes; a page with no words is the version byte alone. A stored row is validated again when read (version, lengths, UTF-8, finite boxes, no box past 4,000 points) and an invalid row is treated as absent. The rows are never evidence, never shown as text of their own and not indexed for search. Re-running `import-source` on an existing matching source inserts rows only for its pages that have none and touches no review history. Without a usable worker (no sandbox, a platform without one) the import succeeds without words and prints why; run it again where the worker works. The table form reads the stored row (no worker, cache or busy state on the review screens).

**Storage.** A `sources` row holds the raw JSON and prepared JSON as BLOBs under a unique `bundle_hash` (SHA-256 of `bundle.json`) and the PDF's `pdf_hash`; the PDF itself is in the PDF store (next section). Pages hold their PNG; units hold raw HTML, `raw_text` (html2text, width 120) and a `source_search_text` row. All units start `unreviewed`.

**PDF store** (`src/sources/pdf_store.rs`, migration 037, SRC-15). PDFs are not database values: a private `sources/` directory beside the database file (created 0700; one owned by another user or open to group or others is refused) holds one read-only (0400) file per PDF, named `<pdf_hash>.pdf`, so sources with the same PDF share one file. The name is always the 64-lowercase-hex hash from a row or a verified import, never user input; every access goes through the directory descriptor (`openat`, `O_NOFOLLOW`), so no path is built and no link is followed.
- `put(reader, sha256)` writes a verified copy (never a hard link: a staged file keeps its link count of 1) to a temporary name (`O_CREAT|O_EXCL`, 0400), checks the hash while copying, fsyncs the file, renames it into place and fsyncs the directory. It is idempotent: an intact stored file is kept (the directory is fsynced again), a damaged or replaced one is rewritten. It returns a `Pending` guard.
- `open(hash)` opens without following links, requires a regular file, hashes the opened descriptor (not a path) and only then returns it. A missing, linked, non-regular, replaced or tampered file (also one that cannot be opened for permission reasons) is the fixed error `pdf_corrupt`, shown as "The stored PDF of this source is missing or damaged; import the source again." with HTTP 500 (no path). The check happens at open: a consumer that needs the identity of what it reads (route A) hashes while it reads. `Store::open_source_pdf(source)` does this for a row.
- `import-source` stores through `put` after its hash check and compares an existing source by `pdf_hash`, not by blob. Importing a source again whose file is missing stores the file again.
- **Orphans.** `put` holds a shared `flock` on `sources/` from before the file is created until its guard is dropped, which the caller does only after the database row naming the hash has committed (order: temporary write, fsync, rename, directory fsync, database commit). The orphan sweep takes the lock exclusively without waiting and skips its run when a writer holds it; it then reads the referenced hashes and removes unreferenced `<hash>.pdf` and stale `.tmp-*` files older than one hour. Other names are left alone. It runs at server start and with the uploads sweep (hourly; only with authentication on Linux, and whether or not that sweep's earlier steps succeeded). Route A's finalization uses the same interface: `put` before the first transaction that inserts the `sources` row, and keep the guard until that transaction commits.
- **Store identity.** The database holds a random id (`pdf_store_identity`, migration 037) and `sources/.store-id` holds the same, written atomically (temporary file, fsync, link that never replaces) when it is missing. Two databases in one folder share `sources/`, and each one's rows name only its own files, so a sweep would delete the other's. When the marker holds another id, the opener also writes `.store-shared`; while either exists, no sweep of any of those databases deletes anything (a fixed warning is printed at startup and by the sweep, no paths). `put` and `open` keep working, since content addressing makes sharing safe for reads and writes. Give each database its own folder, then delete `sources/.store-shared` (and `.store-id` if it belongs to the database that moved) to sweep again. A copy of a database carries the same id, so it is not noticed: never run a copy in the same folder (its sweep would delete the PDFs only the original names); put a copy in its own folder.
- **Missing files.** Sources imported before migration 037 kept the PDF inside the database, which the migration dropped without copying it out (no release had shipped, so local data was disposable); import such a source again to have its PDF. Startup prints how many sources have no stored file (a count, no paths). A use that needs the PDF fails as `pdf_corrupt`; sheets and evidence never read it and keep working.
- **Backup and restore.** The database file alone no longer holds the PDFs: back up the database file and `sources/` together, and restore both. Run `VACUUM` once after upgrading to reclaim the space the dropped column held.

**Re-import and finalisation.** Re-importing the same `bundle_hash` is idempotent: `verify_existing` compares imported artefacts and extracted units (including derived text), fills a legacy `total_pages=0` and keeps review state and owner-added units. Every import writes `source_finalizations` and `source_page_hashes`; triggers make artefacts, extracted membership and the ruleset association immutable and undeletable. A different bundle hash is a separate, fully unreviewed source; runs are never merged. Raw HTML is immutable for all sources. Imported units have `origin='extracted'`; their equality to raw leaf nodes is unchanged.

## Documents and primary pages

`source_documents` identifies a PDF by its unique hash, with immutable `source_document_members`
linking each reading to its document. The PDF itself is in the PDF store, named by `sources.pdf_hash`.
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
- **Owner-built tables** (`Store::material_create_table`, migration 028): SQL accepts owner units of kind `Text` or `Table`, with every other identity, state, revision and geometry guard unchanged. On a page the Owner ticks "Select" on 1 to 200 units among the cards shown (`MAX_TABLE_SOURCES`, 50 cards per window; a checkbox per card submitting to a GET form, no script; a roll table is one unit per row, so a d20 table is 20 units) and opens `/admin/materials/pages/{id}/tables/new`. The form is prefilled with a draft of the selected units' text in reading order (corrected text where corrected, the extraction text otherwise), read by the best-scoring table shape (`src/sources/shapes/`, SRC-19). Shapes are pure linear scans over the units' lines (Column grid also over the stored word boxes), after removing html2text's decoration (`**`, `*text*`, `* `/`- ` bullets, `#`); the score is the share of lines read minus 20 for each problem, never an acceptance. **Roll table**: rows start with a marker (`3`, `3.`, `3)`, `3:`, `3-4`, `96-00`, `5+`, en/em dashes allowed) that together cover a die: `1..N` for N in 2, 3, 4, 5, 6, 8, 10, 12, 20, 30, 100 (`00` is 100), d66 (`11`..`66`), or an `ndX` sum (`2-12` for 2d6), at least four rows without a header (three with ranges); a `dN`/`ndX`/`d66` line (`d6 Result`, `1d20`) is the header row and fixes the die; an `<ol>` list (`1. `) is a weak marker that needs a header or a range; a line without a marker continues the last cell; a short first line or any line that fits no row stays in the cells as a single-cell row at its place (a leading one, such as a caption, before the header row, which then is not a `<thead>`; the problem list excerpts it); drafts lose no text of the selected units, because the units are rejected on save; when every row's outcome has the same number of tab or 2+-space parts it becomes several columns; a second scan over the joined lines handles a table merged into one paragraph (`1 Rats 2 Goblins ...`), splitting at the next expected number, capped at 90; in a merged paragraph only ranges and `+` count as strong markers, so bare numbers (with or without `)`) need six rows or a header. Gaps, duplicates, overlaps, interleaved sequences (two tables side by side) and incomplete covers are listed on the form as problems and lower the score; they are never fixed. **Progression**: lines starting with `1st`/`2nd`/`3rd`/`4th`, `1°`, `Level 1` or `Livello 1`, increasing by one (three rows at least), the rest split on 2+ spaces or tabs, or by numeric tokens (`+2`, `1d8`, a dash); a line before the first level that splits into as many cells is the header; ragged rows, gaps and duplicates are problems. **Stat line**: lines of `Key value` pairs (keys of 1-12 letters starting with a capital, values numbers, fractions, dice or a dash) or `Key: value` sequences separated by `,` or `;`, two pairs per line at least; one row per pair, or a header of keys plus one row per line when every line has the same keys. **Label and value**: a label of at most 60 characters and six words, then a dot leader, ellipsis, tab, spaced dash or `: `; two columns, at least two rows, continuation lines join the value (scored at most 85). **Column grid**: read from the page's stored word boxes (below) whose centres lie inside the selected units' polygons, and only when they carry exactly the selected units' text (the alphanumeric tokens of the words equal those of the units as a multiset) and no selected unit was corrected; otherwise the form says why and the line reading below is used: words are grouped into rows by baseline (vertical overlap over half the smaller height), columns are the left edges (right edges for numbers, centres as the last resort) that recur in at least half the rows, with edges within 0.6 of the median character width taken as one; a cell is the words between two column starts, a first row without numbers over mostly numeric rows is the header, rows with fewer cells are padded and listed as ragged (scored at most 80, less for ragged rows). Without words (none stored, an invalid row, or none inside the units) it splits each line on tabs or runs of two or more spaces (scored at most 60), and the form says why (`#table-words-note`). A grid needs two rows and two columns, otherwise it does not fit. **Plain columns**: one column per unit, one row per line, fixed score 10; above 16 units it is one column, one row per line, and is only chosen when nothing else fits. Over 200 rows, 16 columns, 1,000 characters per cell or the table text bounds, a draft is refused with a problem instead of being offered. The "Read as" form is a separate GET form carrying only the unit ids and `shape`, with each fitting shape and its score; re-drafting replaces the cells being edited. The problems list (the first 20, then "and N more") sits above the cells and is recomputed when a save is refused; the "First row is the header" checkbox defaults from the draft. When the table's union box would cover unselected units that lie mostly inside it (a run across page columns), the form names them (or says the check was not done on a page of more than 2,000 units). The cells are one textarea: one row per line, cells separated by `|` (edge bars optional), `\|` for a literal bar and `\\` for a literal backslash, cells trimmed, all-empty lines dropped, short rows padded. Bounds: 1 to 200 rows, 1 to 16 columns (a selection may be up to 200 units; only Plain columns is limited to 16), 1,000 characters per cell, 20,000 characters of text, 64 KiB of generated HTML, no NUL, a rationale of 1 to 5,000 characters. They keep the form, the HTML and the lookup text bounded like any unit. Saving runs one IMMEDIATE transaction that rechecks the Owner, CSRF, the page revision and each selected unit's revision, inserts one owner `Table` unit (`owner:<n>`, `unreviewed`, revision 1) and rejects the selected units in one batch (one operation id, the one rationale, an audit sidecar per rejection). The unit's polygon is the axis-aligned box over the selected units' points, clamped to the page, four convex points (a box without area is refused). Its HTML is generated by the server as `<table><tbody><tr><td>` with escaped text, or, with the header box ticked, `<table><thead><tr><th>` for the first row followed by `<tbody>` (the 64 KiB bound is measured on this output; no `<caption>`); the same sanitiser and search index as Marker tables apply. The stored plain text (`raw_text`, shown again if the table is re-selected) is one line per row with cells joined by ` | `, not a bordered projection; the search text is one line per row with the same ` | ` separator. Each creation is recorded in the append-only `source_owner_tables` link (unit, operation id, first rejection event); the history shows "Table unit N created from units …" on that event, and the page shows a confirmation. A pasted Markdown table works: edge bars and the `---` row are dropped, and the prefill is written in the same `| a | b |` form. The table card sorts at its anchor, the first selected unit in reading order (migration 030, `source_owner_replacements`; see the page order in the lookup panel description). Over 64 KiB of selected text is refused with its own message (a draft that cannot be saved shows its problem instead); units of another page are refused. The new table stays unreviewed until the Owner checks it against the original page, so it is not evidence yet; the originals and the raw extraction are untouched. A refused save shows the error and keeps the cells and rationale. **Proposals** (`src/sources/proposals.rs`, `Store::material_table_proposals`): computed on every page view (unless memoised, below), Mimik reads the page's units in reading order and looks at each maximal run of consecutive unreviewed extracted units with text (split at 200 units). The text shapes (roll table, progression, stat line, label and value) read clusters of units whose lines start rows of that shape (`shapes::starts_row`; at most 2 other units between two starters), inside runs split at layout breaks (a unit whose box does not overlap the previous one horizontally, or starts above it, i.e. another page column), and each cluster is trimmed from its first starting unit (with one short single-line caption before it) to its last, so prose before or after a table is left out. Column grid reads the whole run (units side by side are its columns) from the stored words under the same lossless rule and never starts a worker. Each unit alone is also read by every shape (a table merged into one paragraph). A candidate is proposed when a non-plain shape scores at least 80 with at least 3 body rows (not counting a header or lead lines), it does not overlap a better-scoring candidate, and its union box covers no unselected unit (the form's overlap logic, so a proposal never swallows the units of another column). At most 64 KiB of text is read per page, so the work is linear in it; a page of more than 2,000 units gets none. The store returns all proposals in reading order; the page lists the first 8 that touch a card shown, each a link \"Likely roll table: units 12-17\" to the table form with the units and `shape` preselected, in `#table-proposals`. Nothing is stored and it is never a decision. Results are memoised in a bounded in-memory cache (32 pages) keyed by page id, page revision and stored-words size; a timed-out read shows no proposals and is not remembered, and any other read failure is logged to stderr and shows none.
- **Owner merge** (`Store::material_create_merge`, `add_owner_merge`; SRC-22): "Merge selected units" on the same selection opens `/admin/materials/pages/{id}/merges/new` for 2 to 200 distinct, non-rejected units of the page (owner units included). The form is prefilled with their text in reading order (correction where corrected, the extraction text otherwise), one unit per line, cut at 64 KiB with a note; it warns when a selected unit is a table (its structure becomes plain text) or has no text, when the prefill is over the 20,000 characters a save takes, and when the union box covers unselected units; it shows the crop of the selected area and the whole page with outlines (`/admin/materials/pages/{id}/selection/image`, shared with the table form). Saving (`POST .../merges`) runs one IMMEDIATE transaction that rechecks the Owner and CSRF, checks every unit (on the page, at the revision the form carried, not rejected) before the first write, inserts one owner `Text` unit over the union box (clamped to the page) recorded `corrected` with the Owner's text, rejects the selected units, all in one operation with one rationale and an audit sidecar per event, and writes a `merge` replacement row anchored at the first selected unit in reading order. Bounds: 20,000 characters, 64 KiB raw and once escaped as HTML, no NUL, a rationale of 1 to 5,000 characters. The page then opens at the new unit with a notice; the history shows "Units … merged into N." on the operation's first event. A refused save keeps the text and rationale. Table saves and the table form refuse already-rejected units the same way.
- **Owner split** (`Store::material_create_split`, `add_owner_split`, `src/sources/split_areas.rs`; SRC-22): "Split…" on each non-rejected `Text` card opens `/admin/materials/pages/{id}/units/{unit}/split` with the unit's text (correction where corrected). A line that is exactly `---` once trimmed (CRLF accepted) starts a new part; parts are trimmed, 2 to 20, none empty (refused by number), each at most 20,000 characters and 64 KiB raw and escaped, 64 KiB in all. The form warns when the original text already has such a line; a unit holding more than 64 KiB of text is shown cut and cannot be split. "Show the areas" is a POST that writes nothing (CSRF checked; the text never travels in a URL): it computes each part's box and shows the unit's crop with every part outlined and numbered, then one box editor per part (in a `<details>`, the first open) with four number fields in page points (left, top, right, bottom) that work without script and the region editor's drawing and keyboard handles with script; crops use the region retry pattern. Boxes are rounded outward to two decimals and never past the page; a new preview keeps submitted boxes while the number of parts is unchanged and says so, otherwise it computes them again. Areas come from the stored words when the whole unit's tokens equal its text-layer tokens under agreement-v1 (`layered_tokens` and `consumed` give the words behind each token; a hyphenation the unit wrote as two words on a part boundary goes to the earlier part; a part's box is the bounding box of its words, and a part that ends mid-line shares that line's height with the next part); otherwise (no words, any difference, a corrected unit) the unit's box is cut into stacked horizontal bands in proportion to each part's token count, each at least one line high. The form says which way the areas were made. Saving (`POST .../units/{unit}/split`, with or without a preview; without one the areas are computed in the transaction) rechecks the Owner and CSRF, checks the page revision and the unit's revision once before any write against those the form carried, reads the page bounds once and validates every box (inside the page, not empty; a box may reach outside the unit), inserts one owner `Text` unit per part recorded `corrected` with its text, rejects the unit, all in one operation with one rationale and an audit sidecar per event, and writes `split` replacement rows (parts 1..n, anchored at the unit). The page opens at the first part with a notice; the history shows "Unit N split into …." on the operation's first event. A refused save keeps the text, boxes and rationale.
- **Agreement with the PDF text layer** (`src/sources/agreement.rs`, rule `agreement-v1`; `src/sources/agreement_view.rs`; SRC-21): each unreviewed extracted unit's text is compared with the page's stored word boxes (SRC-20), which are the only second reading so far. The rule is pure and deterministic. It drops shadow duplicates (the same text redrawn within 3 points and 0.3 of the smaller height, overlapping by half). A unit's words are those whose centre lies in its polygon, ordered by line (vertical overlap over half the smaller height), then left to right. Both sides are tokenised after removing html2text's own markup from the unit side (heading prefix, `* ` list marker, paired emphasis; a bullet glyph is dropped at most as often). Then comes NFC, never NFKC. Line-end hyphenation is joined on the text-layer side only: a letter before the hyphen, a lowercase letter after, no digit. A fixed fold table covers ligatures, quotes, dashes (not the minus sign), the ellipsis and space characters. The unit **agrees** when the token sequences are equal and non-empty and no orphan word (one inside no unit) touches the unit's box grown by one line height. It **differs** otherwise, and a bounded token difference is computed for display only. It has **no words** when nothing falls inside it. There is no similarity threshold. These units are **excluded**, with the reason shown:
  - owner units, reviewed units, and the kinds Table, TableGroup, Figure, FigureGroup, Picture, PictureGroup, Equation and Form;
  - units in a table proposal, or whose text alone a non-plain table shape scores 80 or more on;
  - units past the 64 KiB proposal text budget;
  - units targeted by an open suggestion, and every unit of a page with an open page-level `note` or `missing_text`;
  - units holding a word that lies in two units' polygons.
  A page of more than 2,000 units or 64 KiB of unit text is not compared. What a match proves is the comparison kind, taken from the bundle's self-declared extractor provenance: `marker-pdf` 2.0.0 with `force_ocr` true, `disable_ocr` not true and `use_llm` absent or false is **independent**; the same version otherwise is **same layer**; the route-A name is **derived** (no comparison is shown); anything else is **unknown**. The pass without the per-view exclusions (suggestions, proposals) is memoised beside the table proposals, keyed by page, page revision, stored-words size, rule version and exclusion revision. The exclusion revision is always 0, because imported sources have no exclusion boxes. The table-shape score is memoised in the pass, and suggestions and proposals are applied on every view. On the review screen:
  - an **agreement bar** gives the summary (match, differ, no second reading, excluded, text-layer words in no unit) and the comparison kind, or why there is none (no stored words, a words row over its bound, over the page bound, the read time limit, a read failure);
  - **tabs** (`show=attention|matching|reviewed|all`, with counts except in the reduced screen) filter the card window through the same position cursor. The default is `attention` on an independent page with a result, otherwise `all`. `units=` without `show`, an unknown value and a missing result give `all`. A requested unit (`unit=`, or a save's `to`) that the tab would hide shows `all` around it. Every unit link carries `unit=`. `show` travels through the unit decision forms, and a save continues to the tab's next unit;
  - each unreviewed card shows its verdict or exclusion reason, and for a difference the escaped, bounded changes (unit text against "PDF text layer", at most 160 characters a side, 600 a card, 4,096 and 200 changes a window), or that the difference is too long to display;
  - orphan words are outlined on the page image (one path, at most 2,000 boxes, the rest counted);
  - on a page that offers the batch form, `matching` is a compact list (unit number, kind, first line, a link to the card in `all`) under the form.
  The display is charged to the screen budget as a fixed 224 KiB allowance (`AGREEMENT_ALLOWANCE`).
- **Batch accept** (`Store::material_batch_accept_agreeing`, `POST /admin/materials/pages/{id}/batch`; SRC-14, SRC-21): offered in the agreement bar only on an independent page with at least one matching unit. The form lists the first 100 unit numbers and carries the page revision, the stored words row's length and up to 1,024 `id:revision` pairs in one hidden field. Larger sets are accepted in parts. The rationale is the server-written prefix `agreement-v1; independent:` followed by the Owner's text (required, prefilled with "matches the PDF text layer word for word; checked against the page image"). Submitted text cannot replace the prefix. The handler rechecks the Owner and CSRF, recomputes the verdicts from the stored rows (never from the memo; the table proposals come from their cache, which the revision check below makes sound) and refuses unless every unit agrees. One IMMEDIATE transaction then:
  - rechecks the Owner and CSRF;
  - checks the page revision, the words row's length, each unit's page, revision, `unreviewed` state and `extracted` origin, and that no suggestion is open on those units or the page (opening one does not change the page revision);
  - records one `accepted` decision per unit with the full rationale under one operation id, each with its audit sidecar.
  Any change refuses the whole batch with the reason, and the refused form keeps at most 5,000 characters of the Owner's text. Raw text and word rows are never changed, and page completion stays a separate action. The page then shows "Accepted N units that match the PDF text layer." (only when the latest batch on the page decided N units).
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
lists the current 50-card page in reading order: extracted units by id; a table, merge or split
unit at its anchor (the first replaced unit in reading order, or the split unit), its parts in
order (`source_owner_replacements.order_key`, the anchor's key plus the part); a merge-suggestion
unit at its first target; any other owner unit at the end by id, with the replacements of such a
unit right after it. Incomplete pages and non-primary readings are
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
Each unit card has one decision form. Its decision is one dropdown (accepted, corrected, rejected) on the card's heading row, tied to the form by the `form` attribute; the fields the choice needs are accepted none, corrected the corrected text and the rationale (required, SRC-4), rejected the rationale. Without script every field shows with a label saying which decisions use it; `source-review.js` hides the others on change. The server ignores the fields a decision does not use. A refused unit save (422 or 409) shows a red error with `role=alert` inside that unit's card, marks the missing field `aria-invalid`, and adds a page summary (“A save failed — see unit N”) linking to it; `source-review.js` moves focus to the error.
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

Navigation links lead to the next unreviewed unit, the next later page with units to review, the
next later page to confirm complete (not complete and with no unreviewed unit, so it needs only a
completeness confirmation; later complete pages and pages of other sources are skipped) and the
source's review list. The review list (`GET /admin/materials/sources/{id}/review`) groups the
source's unreviewed units by PDF page, with the counts per page and a link to each unit's card
(`#unit-N`, with the card-window cursor when the unit is beyond the first 50); it is keyset-paged
by PDF page, 10 pages per request and at most 100 unit links per page (so at most 1,000 links,
kinds and 80-character snippets escaped, which keeps the worst case far under the 2 MiB HTML
budget; these are technical bounds in the sense of SRC-15). "N more" opens the page at the first
unlisted unit. It runs under the read deadline; after a deadline it shows only the counts and the
next link. It is linked from the source and page screens. On wide screens the page image with the
unit boxes is sticky beside the cards (`.sticky-original`); on narrow screens it stays above
them. Ticking "Select" records no decision: with script a bar fixed to the
bottom of the viewport shows the count with "Create table", "Merge" and "Clear"; without script the
"Create table from selected units" and "Merge selected units" buttons appear before and after the
card list (same GET form, the merge button through `formaction`).
Re-deciding a unit that was already reviewed stays on that unit (`then=stay`), with the same
true-link check; first decisions advance as above. With the script, `j`/`k` focus adjacent cards
and `a` focuses an acceptance
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
- **Index**: `source_search_text` holds unwrapped text from `search_text()` (sanitiser, then tables rewritten as one line per row with cells joined by ` | `, then html2text raw mode, width 2^20, ≤256 KiB; `SEARCH_NORMALISER = 2`, golden-tested). Table flattening: every `tr` is one line (header and body rows alike), cells in order, nested markup reduced to text, whitespace collapsed, empty cells kept as empty positions (`a |  | c`) so columns stay aligned, `colspan`/`rowspan` not expanded, a table nested in a cell flattened into that cell; HTML without a table is unchanged. The separator matches the owner-table lookup text (`tables::plain`). `source_search_fts` is FTS5 `unicode61 remove_diacritics 2`, `prefix='3'`, rowid = unit id, holding only accepted search text or corrected corrections. Triggers maintain it on every review; reviewed units cannot be inserted, units without a search row cannot be accepted, and accepted search text cannot be deleted.
- **Open-time maintenance** (`search::ensure` in `Store::open`'s migration transaction): missing or outdated rows (a different `SEARCH_NORMALISER`, so a bump re-derives every unit on the next open, in the same transaction, which refreshes the FTS rows of accepted units through the update trigger) are derived in batches of 64 (64 KiB raw-HTML guard). The re-derivation runs inside the single migration transaction before the server listens, so startup blocks until it ends; stale rows print one line with their count, and the FTS index is optimised afterwards. Its duration on a large database is not measured. Unnormalisable units go to `source_search_failures`, stay unindexed and produce a startup warning (`src/main.rs`); the open succeeds. The index is rebuilt (plus FTS5 integrity check) only when indexed and eligible counts differ. `Store::rebuild_search_index` has no CLI. Repair a failed unit by reviewing it `corrected` or `rejected`, or import a corrected bundle.
- **Query** (`fts_query`): 1–200 characters, no control characters; words and balanced `"phrases"`, at most 16; pieces without letters or digits are dropped (none left: `Invalid`). Every piece is a quoted FTS5 string, so no operators can be injected. Words of 3+ letters/digits become `("w" OR "w"*)` so exact matches outrank prefixes; pieces are ANDed. Case and diacritics are ignored; mid-word substrings do not match; flattened table text lets a phrase span adjacent cells and rows.
- **Filtering and ranking** (`match_sql`, FTS-driven `CROSS JOIN`): the same statement as MATCH requires the page's primary reading, the exact edition (registered `game_id`/`edition_id`, or an unresolved legacy `system_id`/`edition` excluding associated sources), `p.complete=1`, an accepted or corrected unit, and the optional source set. Order: `bm25`, source, PDF page, unit; 51 fetched, 50 returned, `truncated` set. bm25 statistics are table-wide, so cite `unit_id` + `unit_revision`, never rank.
- **Result** (`EvidenceSearch`): primary-reading counts (`source_count`, `incomplete_pages`, `partial_runs`, `unindexed_units`), `needs_primary_choice`, `truncated`, and byte-guarded `Evidence` (unit, block, kind, polygon, revisions, state, page, source, labels, effective text). `source_count` counts readings of the requested edition (the unresolved-choice state is separate); incomplete and unindexed counts exclude non-primary pages. `partial_runs` counts distinct documents with unknown total or incomplete primary-page coverage for the requested edition, including coverage split across readings. Unresolved-primary counts include matching readings without selecting one. Coverage counts describe the edition as a whole; the optional source set further restricts matches. Unsupported source, unresolved choice, incomplete coverage, index gap and no reviewed match are distinct result states.
- **Budget**: a progress handler stops all statements of one lookup after 1 s (checked every 10,000 VM ops): "query too broad". The progress handler is installed once per `Store` connection (`search::install_budget_handler`) and a `Budget` arms a per-thread deadline; a connection opened outside `Store` ignores budgets. It cannot interrupt one long FTS5 `xFilter` call.
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

## Licensing of sources and assets

Mimik's AGPL-3.0-only grant covers only Mimik's own code. Manuals, official sheet artwork, model weights and extractions keep their own terms. The instance owner must lawfully obtain the materials they import. Marker's code is Apache-2.0, and its weights are under modified OpenRAIL-M terms, read from the upstream README on 2026-10-11; the installed version's terms must be preserved. The root `logo.png` is owner-provided artwork embedded in the binary. Its authorship and redistribution rights are unconfirmed (`assets/README.md`).
