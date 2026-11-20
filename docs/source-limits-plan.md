# Source size limits (SRC-15) applied to upload, staging and source storage

Goal: a large, high-quality manual can be uploaded, inspected, selected, rendered, reviewed and
stored. The PDF is bounded by disk only; no per-upload or per-source total remains; every kept
limit has a technical or security reason stated in `docs/sources-and-retrieval.md`. Decision:
SRC-15 (`docs/decisions.md`). Remaining work only. The CLI `import-source` path keeps its
whole-file JSON bounds (memory bounds, documented as such).

## Current state

**How the PDF reaches the worker** (not stdin, not a record): the parent opens the staged
`data/staging/<id>/document.pdf` (`launch.rs:283` `open`, `:492` `prepare`: by path, no
`O_NOFOLLOW`, then `canonicalize`), records device/inode, and bind-mounts it read-only at
`/in/document.pdf` (`profile.rs:62`); the sandbox compares the mounted identity with that
launch's fd after the mounts (`sandbox/mod.rs:539,958`). Landlock grants Read on `/in`
(`profile.rs:85`). The spec (`entry.rs:28`) carries operation, pages and the in-sandbox path. The
worker copies the file into a `Vec` capped at 128 MiB (`entry.rs:61-71`), installs the second
seccomp filter (kills clone/exec/socket/memfd/userfaultfd/ptrace/seccomp, not `openat`,
`filter.rs:6-17`) and calls `operations::open` (`operations.rs:32-45`): `%PDF-` in the first 1,024
bytes, `%%EOF` in the last 1,024, `PdfDocument::from_bytes` (a second copy in a MuPDF buffer, both
inside `RLIMIT_AS` 2 GiB, `runner.rs:450`; DEP-29). Inspection budget is fixed: 30 s wall, 20 s
CPU (`launch.rs:611-618`); render 30 s / 25 s per page.

The parent hashes the file once, at creation (`uploads.rs:1355` `verify_spool`, called in `create`
at `:477-491` while the control and gate mutexes are held). Jobs call `inspect(&path)` once and
`render(&path, batch)` per 8-page batch (`uploads.rs:1020-1085`), each launch re-opening by path;
nothing re-verifies the file. Pages are published inside the render callback
(`uploads.rs:1060-1085`, `publish_current` `:1183`). `contract()` (`:871-879`) ignores job state.

**Places that encode the limits:**

| Limit | Where |
| --- | --- |
| 128 MiB PDF | `auth/uploads.rs:35-36` (`FILE_LIMIT`, `BODY_LIMIT`), `:46` (`DefaultBodyLimit::max`), `:163`, `:174` (`Counting`), `:311-316` (multer `whole_stream`, `for_field("file")`), `:231` (per chunk), `:349` (Content-Length); `staging.rs:19` `UPLOAD_RESERVATION`; `uploads.rs:1356`; `entry.rs:61-71`; `operations.rs:34`; `templates/upload_form.html:4,11` |
| 64 selected pages | `uploads.rs:1478`; `upload_page.html:13`; `upload_exclusions.rs:276` (`e.pages.len() > 64`); contract `limits.pages` `uploads.rs:901` |
| 128 MiB PNG total | `uploads.rs:875`, `:1222-1226` (reason `source_png_limit`, `:1404`), `upload_progress.html:3`, contract `:902` |
| 512 MiB text | `uploads.rs:1228` |
| 20,000 units | contract `uploads.rs:903` (not enforced in staging) |
| 2 GiB staging total | `staging.rs:17` `TOTAL`, `Ledger::reserve` `:59`; also the "unknown usage" sentinel `staging.rs:467,679` (blocks reserve only through `used + reserved > TOTAL`) |
| Whole-source PDF BLOB | `003_sources.sql:10` `sources.pdf BLOB NOT NULL`; written only by `import.rs:584`, read only by the `import.rs:375` equality check; no other reader |
| Tests | `upload_tests.rs:655,1835,1974`, `upload_route_tests.rs:715`, `worker/tests.rs` (about 10 `Input::Bytes`/`from_bytes` sites), `preflight_tests.rs`, `live_tests` 128 MiB cases |
| Docs | `sources-and-retrieval.md` (worker/RSS sections near `:28-70`, upload `:89-93`, `:110`, staging `:154-186`, contract `:217`, import `:286-287`); `release-scope.md:33` and its backup note (`:119`); `limitations.md` C5, L-SRC-3, L-SRC-8; `source-upload-plan.md` historical totals; `source-route-a-plan.md` ("PDF BLOB once"); the TODO entry |

## Design decisions

1. **Worker opens the bound file by path.** `PdfDocument::open("/in/document.pdf")`
   (`vendor/mupdf/src/pdf/document.rs:580` -> `Document::open` -> `mupdf_open_document`): MuPDF's
   stdio file stream seeks and reads on demand, never loads or mmaps the file, so the PDF adds
   no address space; per-object decompression, glyph cache and raster stay inside `RLIMIT_AS`
   (the per-object/page memory bound). No new mount, grant or syscall. Passing an fd is rejected
   (no constructor in mupdf-rs, no proc in the sandbox).
   - *Recognition reliance.* `Document::open` runs content recognition over every handler; the gz
     handler decompresses into memory (`gz-doc.c:30`). The PDF handler wins only because `%PDF-`
     lies in the first 4 KiB, which the 1,024-byte pre-check guarantees. The reliance is
     documented in `operations.rs` and tested with a gzip stream whose FNAME header holds `%PDF-`
     and whose tail holds `%%EOF` (must be refused, not decompressed). PDF code opens no other
     file (Landlock confines reads to `/in` anyway).
   - *Magic checks* (`%PDF-` in the first 1,024 bytes, `%%EOF` in the last 1,024) read the file
     with `std::fs::File` seek+read before MuPDF opens it; both opens are the same bind.
   - *Errors.* A failed open (`FZ_ERROR_SYSTEM`) must not map to `TooLarge` (`operations.rs:20`):
     open-stage failures map to `Unreadable`; `TooLarge` stays for allocation/limit errors after
     the open.
2. **Immutability and TOCTOU: job pins, passed into every launch.**
   - A job opens the staged file relative to the staging directory fd with `O_NOFOLLOW`,
     requires a regular file, link count 1, owner = effective uid, then takes **pins** (dev, ino,
     size, mtime_ns, ctime_ns) *before* the job's one full SHA-256 pass (1 MiB reads on the job
     thread, cancel-aware) and compares the hash with the stored `pdf_sha256`. Pins are per job and
     not persisted; ctime cannot be set by an unprivileged user.
   - `inspect`/`render` take the pins. `prepare` (`launch.rs:492`) opens `O_NOFOLLOW`, requires
     the launch fd's fstat to equal the pins before building the profile, and passes that identity
     to the bind check (`sandbox/mod.rs:958`); it no longer trusts a re-resolved name.
   - Each worker exit and the job end re-check pins on the held fd. The job also `fchmod`s 0400 on
     its fd if still 0600 (hardening, not a boundary).
   - **On any mismatch (`document_changed`)** the upload fails permanently: all its page rows and
     files are removed (not only the current batch's, so no page from a changed file is kept),
     `source_uploads.document_changed` is set, inspect/render/retry are refused, only discard
     remains, and `contract()` refuses a changed upload and requires the last render job to be `succeeded`. Pins are checked after each
     worker exits and before the next batch is reserved; because pages are published inside the
     callback, the delete-all rule is the backstop for the batch in flight.
   - *Existing staged uploads after upgrade* (0600, no persisted pins) work unchanged: pins are
     taken at their next job, which hashes first; nothing depends on a prior chmod.
   - *`create`* no longer re-reads the spool under the control and gate mutexes
     (`uploads.rs:477-491`): the route's streaming hash is trusted for the stream; `create` checks
     size by fstat and magic, fsyncs, renames; the first job's hash (outside every lock) verifies
     the stored bytes. Cancel, discard, sweep and publish no longer block on a multi-GB read.
3. **Admission by free space; slot first, reserve after admission.** `begin_upload` claims the
   single slot and the spool only. After CSRF admission and metadata validation the phase reserves
   the declared `Content-Length` (no extra 64 KiB: overhead is inside the 512 MiB margin) or, with
   none, a 1 MiB floor, and tops up in 64 MiB steps before writing past the reservation. A refusal
   answers 507 `space`, unlinks the spool, frees the slot.
   - **Accounting (no double count):** the spool write charges `Reservation::written(n)` after
     each successful `write_all` (`auth/uploads.rs:102-111` does not today), so written bytes
     leave `reserved` while `statvfs` already excludes them. `Ledger::reserve` keeps
     `free >= outstanding reservations + 512 MiB`; `TOTAL` and its `used + reserved` clause go.
   - **Unknown usage:** the `TOTAL` sentinel (`staging.rs:467,679`) becomes an explicit
     `Accounting.unknown` flag that makes `reserve` fail until a successful reconcile.
   - Route limits: no static body limit (`DefaultBodyLimit::disable()` on the upload route at
     `:46`; `FILE_LIMIT`/`BODY_LIMIT`, multer `whole_stream` and `for_field("file")` removed,
     `Counting`'s admitted limit unbounded at `:163,:174`); the bound is the reservation. The 4 KiB
     pre-CSRF read, field sizes, 64 KiB read-ahead boundaries and 30 s idle deadline stay.
4. **Upload deadline is idle-only** (30 s without input); the 15-minute total goes. Reason for
   the idle deadline: a stalled client holds the single slot and a spool. No minimum rate (it
   would cut off slow uplinks that work today; only the Owner holds the slot).
5. **Page cap = what one inspection frame holds.** The 2,000-page limit goes. The inspection
   record is one 9 MiB frame (`records.rs:13`, migration `:14`); a page record is about 0.4 KiB,
   so `MAX_PAGES` = 20,000, derived from the frame and asserted by a test that fails if 20,000
   worst-case page records exceed it. Updates: `operations.rs:52-58`, `records.rs:411`,
   `geometry.rs:72`, migration CHECKs (`pdf_page` 1-20,000; `selected_pages` <= 20,000 entries,
   about 140 KB; `pages_done`/`total` <= 20,000). The inspection budget scales per page: the first
   frame (`Document`) carries the page count, so the parent extends its wall deadline per page
   once it reads it; the CPU limit must be fixed before the page count is known (`runner.rs:454` rejects > 200 s): it stays a ceiling of 200 s, sized from file size up to that, and the per-page wall deadline remains the enforcing bound; the guard is not changed. Constants come from the
   cold-disk measurements of slice 6.
6. **Selection and UI without a count.** Any ascending, distinct subset of the inspected pages.
   The page screen lists 100 pages per screen with a range input ("1-500, 520") and "select all";
   `set` takes ranges. The action body limit (131,072 B) must cover ranges, not 20,000
   checkboxes. The contract (`mimik-staged-v1`) drops `limits` and the PNG sum; step 4 reads it
   per page (about 12 MB at 20,000 pages); no consumer exists yet.
7. **Exclusions: several boxes per page, bounded per page.** Owner decision: up to 8 boxes per
   page (header, footer, page number, side tabs), any number of pages within the selection.
   Today `boxes` is `BTreeMap<u32, BoxPoints>` (one box, `upload_exclusions.rs:37-42`) and view
   `source_upload_invalid_exclusion` requires `json_array_length(boxes.$."p") = 4` (migration 026
   `:127-133`). New shape: `boxes: BTreeMap<u32, Vec<BoxPoints>>`, 1-8 boxes per listed page,
   distinct, each valid and inside the page as in `validate` (`:284-304`, applied per box); the
   view checks array-of-arrays, 1-8 entries, four numbers per entry in 0-4000, min < max.
   - *Bytes per page:* stored coordinates are rounded outward to 0.01 pt (min floor, max ceil, so
     coverage is never lost), making each number at most 7 characters ("4000.00" shortest form
     is "4000"); a box is at most 4 x 8 + 2 = 34 B, eight boxes plus key and separators at most
     320 B. The view enforces 384 B per page (`length(value)`), derived from this worst case
     rather than from shortest-form f64 text (up to 24 characters) which would overflow a naive
     96-byte bound. At 20,000 pages a revision is at most 6.4 MB by that worst case (the column bound is 6.5 MB); the history grows by that per
     revision (noted in `limitations.md`).
   - *Several proposals:* each proposal carries one box per page; the Owner can confirm more than
     one in one exclusion. The `exclusion` row keeps one rationale and replaces `proposal_fingerprint`
     by `proposal_fingerprints` (JSON array of at most 8 distinct 64-hex values, none for manual
     boxes); the eight-box page limit bounds the proposal count. The proposal screen
     (`upload_page.html:18-20`, `Editor` list in `auth/uploads.rs:473`) gets a confirm control per
     proposal and a combined preview; `set` carries the confirmed fingerprints (not indices; the absent-page confirmation applies per confirmed proposal) and per-page
     edits, not 20,000 pages of coordinates: `set_exclusion` already re-runs the proposal
     (`:184,194`) and rebuilds boxes server-side; manual boxes are limited by the action body.
   - *Masking/crossing (route A's rule, `source-route-a-plan.md:312-314`, kept):* a unit wholly
     contained (edges inclusive) in the **union** of its page's boxes is rejected (so a header
     drawn as adjacent left and right boxes is covered); positive-area overlap without
     containment stays unreviewed and flagged; mere edge contact is not crossing. Staging only
     validates and carries boxes. Route A's permanent exclusion provenance becomes a list of
     fingerprints plus per-page box lists. The contract carries `boxes` per page as lists; the
     revision/fingerprint rules of `set_exclusion` apply to the whole set.
   - *Rounding and the page edge:* after rounding outward each box is clamped to
     `[0, page width] x [0, page height]` (`padded()` clamps to the page edge,
     `upload_exclusions.rs:368-375`; `validate` refuses `b[2] > width_points`, `:296-300`; e.g.
     595.2756 would round to 595.28). Slice 5 rounds outward and clamps new boxes (migration 036 held no rows to convert).
     Confirmed proposals whose rounded boxes coincide are de-duplicated, not refused. Today's proposal rule (`propose_keyed`, `:437`: recur on at least
     max(3, ceil(3n/5)) pages) makes proposals over 64 pages routine from 107 selected pages.
8. **Proposals stream pages, with bounded memory and time.** `upload_exclusions.rs:141-170` reads
   every `text-N.json` into server memory (the 512 MiB text total was its bound). Replace by:
   read, hash-verify, parse and validate one page at a time; keep only candidate groups.
   - *Memory budget* (live accounting, 64 MiB): per group seed and text bytes plus **every live
     box entry** at about 64 B (`Group.boxes` grows one entry per matched page, `:496-509`, and
     recurring groups are never pruned: 20,000 pages x 50,000 recurring lines would be 10^9
     entries). Reaching the budget refuses with the existing `COMPARISON_LIMIT` ("too many
     distinct lines"); at most 50,000 groups (tombstones included).
   - *Same output as today (N1):* today's per-text rules stay exactly (first match in index order,
     per-text 2,000,000-comparison budget, whole texts dropped when no group can reach the
     threshold, `:470-490,:510-524`). A group that alone cannot reach the threshold is not
     removed: only its `boxes` are freed, and it stays as a tombstone (seed and index) in
     `by_text` so later scans and budgets behave as before; tombstones count against the group
     cap. A match on a tombstone is consumed (removed from `available`) but stores no box.
     Tests: identical output with the current function on duplicate-line, budget-exhaustion
     and ordinary corpora; the hostile recurring-line case (a few thousand pages x many recurring
     lines) refuses with `COMPARISON_LIMIT` at the budget, memory flat.
   - *CPU (N2):* proposals hold the upload gate (`:91-100`; `set_exclusion` repeats them) with no
     CPU bound (about 10^11 comparisons are possible). Add a global comparison budget of
     50,000,000 per proposal run (about one second; reason: gate hold time) checked with the
     per-text budget; exhaustion refuses with `COMPARISON_LIMIT`. Constant confirmed in slice 6.
9. **Finished-source PDFs are content-addressed files.** A `sources/` directory beside the
   database: `<sha256>.pdf`, directory 0700, files 0400, written to a temp name with
   `O_CREAT|O_EXCL`, fsynced, `renameat`ed, directory fsynced; opened via a dirfd with
   `O_NOFOLLOW`; the name is the 64-hex `pdf_hash` from the database, never user input.
   `sources.pdf` is dropped; the row keeps `pdf_hash`; identical PDFs share one file (resolves C5).
   This removes SQLite's BLOB limit (about 1 GB) and the in-memory copy. Interface for route A /
   step 4 (replacing "insert the PDF BLOB once"): `put(file, sha256)` (a **verified copy**, never a
   hard link: a link would make the staged file's link count 2 and a later job would report
   `document_changed`; idempotent) and `open(hash) -> File`
   (no-follow, regular, streaming hash check before use; mismatch is a fixed `pdf_corrupt`
   error). The CLI import (`import.rs:375,584`) stores through the same call after its hash check
   and compares by hash.
   - *Migration.* The migration loop (`store.rs:111-117`, one IMMEDIATE transaction) runs a Rust
     pre-step only when version >= 3 and < 37, just before 037: read blobs one at a time
     (`sqlite3_blob_open`, streaming), write each to the store, verify SHA-256 equals `pdf_hash`,
     then SQL drops the column. Idempotent if interrupted (files are complete before the column
     goes). **Disk (N5):** the pre-step doubles every PDF and `DROP COLUMN` rewrites every
     `sources` row inside the transaction (journal about the blob total), so before it starts
     the step checks free space >= 2 x total blob bytes + 512 MiB and refuses with a fixed message
     naming the needed size. `VACUUM` is not run by migration; the reference doc tells the Owner
     to run it (or a CLI `compact`) afterwards to reclaim the space.
   - *Hash failure.* Default: abort with a fixed message naming source ids (no paths). Ways out,
     documented: restore from backup, or a CLI command that moves the bad blob to
     `sources/quarantine/`. Before 037 `pdf` is NOT NULL, so the command writes an empty blob
     plus a quarantine marker for that row; the pre-step treats a marked row as a missing file
     (`pdf_corrupt` on use) instead of aborting again.
   - *Backup/restore.* The database file alone no longer holds PDFs: `release-scope.md:119` and
     the reference doc say the backup is the database file plus `sources/`; a missing file is
     reported at startup (count only, no paths) and as `pdf_corrupt` on use; existing sheets and
     evidence do not read the PDF and keep working.
   - *Orphans and ordering (N4).* `import-source` is another process (`main.rs:460`) and the
     rundir lock covers only the assistant, so the sweep needs its own protocol: a cross-process
     `flock` on `sources/`, shared by `put` from before the file is created until the database row
     commits, exclusive for the sweep, taken non-blocking (the sweep skips that run if busy); additionally the sweep skips files younger than one hour.
     Ordering: temp write, fsync, rename, directory fsync, then the DB commit. Route A's
     finalization follows the same rule (put before the finalizing row).
10. **Kept limits, each with its reason** (written into `sources-and-retrieval.md`): form field
    sizes and the 4 KiB pre-CSRF read (unauthenticated memory); per-PNG 4,096 x 8,192 px, 12 Mpx,
    12 MiB, 64 MiB decode (decompression bombs); per-page text 8 MiB, depth 32, 40,000 nodes,
    64 KiB HTML per unit, 512-byte block ids (parser and row size); worker `RLIMIT_AS` 2 GiB,
    16 MiB scratch, CPU/wall (hostile PDFs); 160 MiB per 8-page render reservation
    (8 x (12 + 8 MiB)); one upload in progress and one retained upload; 20,000 pages (frame);
    proposal group caps (server memory); the exclusion per-page box. CLI import: `bundle.json`
    8 MiB, `source.json` 32 MiB, PDF 128 MiB, 64 pages, 20,000 units, 128 MiB images (in-memory
    JSON/`lopdf`).

## Large page counts (code-checked)

- 500 pages = 63 launches of 8, each reserving 160 MiB; space exhaustion fails the job with
  `space`, published pages stay, a retry renders the rest. Cancel kills via pinned pidfds and is
  checked between batches; progress is `done/total` from an HTML poll that reloads the upload.
- `publish_current` loads every page row only to sum totals (`uploads.rs:1195-1226`, `load_parts`
  `:1491`): O(n^2). With the totals gone it reads the job/revision/selection facts and the one
  existing row for the page (slice 4, not deferred).
- A damaged large file triggers MuPDF repair (scans the whole file); the scaled inspection
  deadline bounds it, a miss is `took too long`.

## Slices (each ends with regression tests that fail first)

1. **Worker opens the file; page ceiling.** `Input::Path`; remove the `Vec`, `from_bytes` and
   128 MiB checks; file-based magic checks; open errors to `Unreadable`; `MAX_PAGES` with the
   worst-case-record test; unit tests in `worker/tests.rs`/`preflight_tests.rs` run through temp
   files so they exercise the production open path. Live sandbox corpus: a PDF over 128 MiB
   (sparse padding), xref beyond 4 GiB (sparse, 64-bit seeks), peak RSS far below 75% of
   `RLIMIT_AS`, the gzip-with-`%PDF-` fixture, a missing input. Pinned-profile test unchanged (no
   new grant). Update the DEP-29 sentence.
2. **Migration 036 (upload tables), built; remaining: review.** Owner decision 2026-11-11: no
   release has shipped, so staged uploads are disposable and migration 036 drops and recreates
   the four upload tables (views, triggers and the single-slot index with them) instead of
   copying rows. Built: `pdf_page`, `pages_done`/`pages_total`, the inspected count and the
   selection/exclusion page lists bounded at 20,000; `pdf_bytes` without a size bound (the route
   and staging limits go in slice 4); `source_jobs.reason` gains `document_changed` and
   `source_uploads` gains `document_changed` (slice 3 sets it; `error_code` maps it);
   `fingerprint_key` kept; exclusion `boxes` as 1-8 boxes per page (view: 384 B a page, box keys
   in `pages` order so the view stays linear) and `proposal_fingerprints` (at most 8 distinct
   64-hex values). Staged files of the dropped rows are orphan directories that the startup
   reconciliation already removes without following links (tested). Until slice 5 the exclusion
   code writes one box per page and at most one fingerprint and refuses more when reading.
   Migration test: seeded v35 rows in all four tables, tables empty after, foreign-key check
   empty, 20,000 accepted and 20,001 refused.
4. **Free-space admission and upload route** (decisions 3-4): slot first, reserve after
   admission, per-chunk `written`, top-up, explicit unknown state, no static limits, idle-only
   deadline, 507; `publish_current` narrowed. Tests: body over 128 MiB declared and chunked
   (sparse generator); free space exhausted mid-stream (spool unlinked, reservation and slot
   released); accounting near the free-space limit (a reserve right after a large write must not
   be refused by a double count); unknown usage refuses `reserve` and recovers after reconcile;
   the ENOSPC case (`upload_tests.rs:1974`) extended to mid-spool and mid-render; a slow upload
   past 15 minutes of fake time succeeds while 30 s idle fails.
5. **Totals, selection, exclusions, proposals** (decisions 5-8 except the budget): remove
   selection/PNG/text/total checks and `source_png_limit` code; ranges UI; contract without
   `limits`; several boxes per page, the confirm-several-proposals screen, the 384 B per-page bound; streaming proposal with the memory and comparison budgets. Tests also: 8 boxes per page accepted, 9 and a worst-case-digits box (rounding outward) checked against the view. Tests: 500 and 20,000
   selected pages (fake worker); old per-upload cases now succeed while per-PNG/per-page cases
   still fail; an exclusion over 200 pages; proposal identity with the current function and
   `COMPARISON_LIMIT` on hostile many-distinct-lines input; server memory flat across 500 text
   pages.
   Box de-duplication must be numeric: migration 036 compares boxes as text (`[1,1,2,2]` and `[1.0,1,2,2]` differ), so the rounding step makes equal boxes bit-identical before writing.
6. **Large-input gate.** Synthetic 500-page and 5,000-page PDFs (and a 20,000-page minimal one for
   the frame) through inspect with a cold page cache (`posix_fadvise` DONTNEED on the magnetic
   disk, weekend window), all render batches, cancel, restart reconciliation; fix the
   per-page inspection budget constants from the measurements; time `progress` polls and the page
   screen at 20,000 pages.
7. **Content-addressed PDF store** (decision 9): store, interface, Rust pre-step, migration 037,
   CLI import through it, sweep with the `flock` and age rule, backup note; test fixtures inserting `pdf` (`documents.rs:509`, `search.rs:455`, `campaigns/tests.rs`) updated. Tests: legacy blobs migrated and verified (hash
   mismatch aborts untouched), an interrupted migration resumes, two sources with one PDF share a
   file, a tampered or missing file is `pdf_corrupt`, a symlink in the store directory is refused,
   orphans are swept, an import (separate process) racing the sweep keeps its file; the hash-mismatch abort and the quarantine command; insufficient free space refuses before the pre-step; the staged file's link count stays 1 after `put`.
   Question for the coordinator (not changed here): under the 2026-11-11 owner decision (no release, disposable local data) the pre-step that migrates legacy `sources.pdf` blobs, its free-space check, quarantine command and resume logic may be unnecessary; 037 could drop the column and the `sources` rows (or the whole local data set) instead.
8. **Docs.** Rewrite the reference sections in the table (each kept limit with its reason),
   `release-scope.md`, `limitations.md` (add: damaged-file inspection bound, exclusion history
   growth, CLI memory bounds; reword C5, L-SRC-3, L-SRC-8 as CLI-only), `decisions.md` DEP-29,
   `source-upload-plan.md`, `source-route-a-plan.md` (containment rule over the union of boxes,
   fingerprint list and per-page box lists as provenance), TODO; delete this plan. Slice 5 also
   tests an edge-touching box (595.2756 wide page), union containment and edge contact.

Review: this plan and each slice get a fresh Opus reviewer (AGENTS.md); slices 1, 3, 4 and 7
(sandbox input, TOCTOU, accounting, store security) need the adversarial pass. Builds and tests
of agent work run in containers; weekday builds at 2 cores.

## Risks
- Forward work found in slice 2 review: every `load()`/progress poll parses the up-to-9 MiB inspection (slice 4: read only the facts a poll needs); each published page updates the row that holds the inspection (`uploads.rs` ~1280, slice 4); the Rust selection/exclusion checks are O(selected x pages) (`uploads.rs` ~1484, `upload_exclusions.rs` ~166 and ~304-315) and must be made linear when slice 5 lifts the 64 cap; the page screen renders up to 20,000 checkboxes until slice 5's ranges UI (slice 5); `upload_progress.html` prints `document_changed` raw until slice 3 gives it a label (slice 3).
- Tools that read the database outside Mimik need SQLite >= 3.44 (migration 036 views use `group_concat(... ORDER BY ...)`; an earlier catalogue migration already uses `->>`, 3.38).

- A MuPDF bug on a very large or hostile file is contained by the same sandbox; memory by
  `RLIMIT_AS`, time by the deadlines; a big PDF fails as `too_large`/`took too long`, never
  exhausts the host.
- Disk can fill; the margin keeps SQLite/WAL writable but other processes race free space. Every
  write failure releases slot, spool and reservation (slice 4 tests).
- Hashing a multi-GB file once per job is heavy on the magnetic disk; it runs outside locks.
- Migrations 036/037 touch live tables and the PDF column; both are tested on seeded data and sit
  under the backup-before-upgrade guidance (the backup now includes `sources/`).
- Unlimited selection moves failures to render time; step 4 must create the source page by page.

## Open questions for the owner

None open. Decided: pages cap by frame, several boxes per page, hash-failure default (abort, with
restore or quarantine command), idle-only deadline.
