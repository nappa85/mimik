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
| 128 MiB PDF | `auth/uploads.rs:35-36` (`FILE_LIMIT`, `BODY_LIMIT`), `:46` (`DefaultBodyLimit::max`), `:163`, `:174` (`Counting`), `:311-316` (multer `whole_stream`, `for_field("file")`), `:231` (per chunk), `:349` (Content-Length); `staging.rs:19` `UPLOAD_RESERVATION`; `uploads.rs:1356`; `entry.rs:61-71`; `operations.rs:34`; migration 026 `:5`; `templates/upload_form.html:4,11` |
| 64 selected pages | `uploads.rs:1478`; `upload_page.html:13`; migration 026 `:15` (`selected_pages` <= 385 B / 64 entries), `:107` (exclusion `pages` <= 385 B / 64), `:120-121` (`pages_done`/`total` <= 64); `upload_exclusions.rs:276` (`e.pages.len() > 64`); contract `limits.pages` `uploads.rs:901` |
| 32 KiB exclusion boxes | migration 026 `:108` |
| 128 MiB PNG total | `uploads.rs:875`, `:1222-1226` (reason `source_png_limit`, `:1404`), `upload_progress.html:3`, contract `:902` |
| 512 MiB text | `uploads.rs:1228` |
| 20,000 units | contract `uploads.rs:903` (not enforced in staging) |
| 2 GiB staging total | `staging.rs:17` `TOTAL`, `Ledger::reserve` `:59`; also the "unknown usage" sentinel `staging.rs:467,679` (blocks reserve only through `used + reserved > TOTAL`) |
| 2,000 pages | `operations.rs:52-58`, `records.rs:411`, `geometry.rs:72`, migration 026 `:22,30,55,60,129`; inspection frame 9 MiB `records.rs:13`, migration `:14` |
| Whole-source PDF BLOB | `003_sources.sql:10` `sources.pdf BLOB NOT NULL`; written only by `import.rs:584`, read only by the `import.rs:375` equality check; no other reader |
| Tests | `upload_tests.rs:655,1835,1974`, `upload_route_tests.rs:715`, `worker/tests.rs` (about 10 `Input::Bytes`/`from_bytes` sites), `preflight_tests.rs`, `live_tests` 128 MiB cases |
| Docs | `sources-and-retrieval.md` (worker/RSS sections near `:28-70`, upload `:89-93`, `:110`, staging `:154-186`, contract `:217`, import `:286-287`); `release-scope.md:33` and its backup note (`:119`); `limitations.md` C5, L-SRC-3, L-SRC-8; `source-upload-plan.md` historical totals; the TODO entry |

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
     remains, and `contract()` refuses a changed or failed upload. Pins are checked after each
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
   once it reads it; the CPU ceiling follows file size and `MAX_PAGES`. Constants come from the
   cold-disk measurements of slice 6.
6. **Selection and UI without a count.** Any ascending, distinct subset of the inspected pages.
   The page screen lists 100 pages per screen with a range input ("1-500, 520") and "select all";
   `set` takes ranges. The action body limit (131,072 B) must cover ranges, not 20,000
   checkboxes. The contract (`mimik-staged-v1`) drops `limits` and the PNG sum; step 4 reads it
   per page (about 12 MB at 20,000 pages); no consumer exists yet.
7. **Exclusions: bound per page, not per page count.** One exclusion keeps one box per page (four
   numbers within 0-4000). `pages <= 64`, 385-byte and 32 KiB totals are replaced by: one box per
   page, at most 96 bytes of JSON per page; any number of pages within the selection
   (`upload_exclusions.rs:276`, migration view `:127`). Each revision stores the whole row, so
   history grows with pages x revisions (about 2 MB per revision at 20,000 pages): noted in
   `limitations.md`. Today's proposal rule (`propose_keyed`, `upload_exclusions.rs:437`: a line
   must recur on at least max(3, ceil(3n/5)) pages) makes proposals over 64 pages routine for
   selections of 107 pages or more, so the old cap would have broken them.
8. **Proposals stream pages.** `upload_exclusions.rs:141-170` reads every `text-N.json` into server
   memory (the 512 MiB text total was its bound). Replace by: read, hash-verify, parse and validate
   one page at a time and keep only candidate groups (normalized lines of 4-200 characters with
   their boxes). Memory is bounded by a cap on live groups (50,000) and candidate text bytes
   (64 MiB; worst case 50,000 x 200 chars x 4 B = 40 MB plus boxes); reason: the proposal runs on
   the server under the upload gate, without cancellation. A group is dropped once it cannot reach
   the required page count (`seen + remaining pages < required`), which keeps real books far under
   the cap. Hitting a cap refuses with the existing `COMPARISON_LIMIT` ("too many distinct
   lines"). Output must be identical to the current function on corpora under the cap (test).
9. **Finished-source PDFs are content-addressed files.** A `sources/` directory beside the
   database: `<sha256>.pdf`, directory 0700, files 0400, written to a temp name with
   `O_CREAT|O_EXCL`, fsynced, `renameat`ed, directory fsynced; opened via a dirfd with
   `O_NOFOLLOW`; the name is the 64-hex `pdf_hash` from the database, never user input.
   `sources.pdf` is dropped; the row keeps `pdf_hash`; identical PDFs share one file (resolves C5).
   This removes SQLite's BLOB limit (about 1 GB) and the in-memory copy. Interface for route A /
   step 4 (replacing "insert the PDF BLOB once"): `put(file, sha256)` (hard link from the staging
   file when on the same filesystem, else verified copy; idempotent) and `open(hash) -> File`
   (no-follow, regular, streaming hash check before use; mismatch is a fixed `pdf_corrupt`
   error). The CLI import (`import.rs:375,584`) stores through the same call after its hash check
   and compares by hash.
   - *Migration.* A Rust pre-step at store open, before SQL migration 029 drops `sources.pdf`:
     write each blob to the store, verify SHA-256 equals `pdf_hash` (a mismatch aborts with a fixed
     message; nothing is dropped), then the SQL drops the column. Idempotent if interrupted; the
     file set is complete before the column goes.
   - *Backup/restore.* The database file alone no longer holds PDFs: `release-scope.md:119` and
     the reference doc say the backup is the database file plus `sources/`; a missing file is
     reported at startup (count only, no paths) and as `pdf_corrupt` on use; existing sheets and
     evidence do not read the PDF and keep working.
   - *Orphans.* Temp files and files whose hash no `sources` row names are removed by the startup
     and hourly sweep, never while an import holds the store (same lock as import).
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
2. **Migration 028 (upload tables).** New bounds, `document_changed` in `source_jobs.reason` (026
   `:122` would otherwise reject it) and `source_uploads.document_changed`, `error_code` mapping
   (`uploads.rs:1398`). Migrations run in an IMMEDIATE transaction with foreign keys on
   (`store.rs`): `DROP TABLE source_uploads` cascades into pages, exclusions (audit history) and
   jobs, and `RENAME` fails while views reference the table. Plan: drop the views
   (`source_upload_invalid_inspection`, `source_upload_invalid_exclusion`), all triggers and index
   `source_upload_single_slot`; rename the four old tables to `*_old`; create the four new tables
   under their final names (keeping `fingerprint_key` from 027); copy the parent, then pages,
   exclusions with full history, jobs (a leftover `source_png_limit` reason maps to `too_large`);
   drop the old children, then the old parent; recreate the views, every trigger and the index;
   `PRAGMA foreign_key_check` must be empty. The migration test seeds rows in all four tables
   (selection, pages, two exclusion revisions, a job in each state), asserts equality after, and
   that 20,000-page rows are accepted and the old caps are gone.
3. **Pins, verification, immutability** (decision 2): O_NOFOLLOW opens, pins into launch, per-job
   hash outside locks, `create` without the locked re-read, `document_changed` handling (delete
   all pages, retry refusal, `contract()` refusal). Tests: change one byte, truncate, append,
   replace by rename, symlink and hard link at the final component, change between batches and
   during the last batch (no page remains), a legacy 0600 upload, cancel during the hash.
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
   `limits`; per-page exclusion bounds; streaming proposal with caps. Tests: 500 and 20,000
   selected pages (fake worker); old per-upload cases now succeed while per-PNG/per-page cases
   still fail; an exclusion over 200 pages; proposal identity with the current function and
   `COMPARISON_LIMIT` on hostile many-distinct-lines input; server memory flat across 500 text
   pages.
6. **Large-input gate.** Synthetic 500-page and 5,000-page PDFs (and a 20,000-page minimal one for
   the frame) through inspect with a cold page cache (`posix_fadvise` DONTNEED on the magnetic
   disk, weekend window), all render batches, cancel, restart reconciliation; fix the
   per-page inspection budget constants from the measurements; time `progress` polls and the page
   screen at 20,000 pages.
7. **Content-addressed PDF store** (decision 9): store, interface, Rust pre-step, migration 029,
   CLI import through it, sweep, backup note. Tests: legacy blobs migrated and verified (hash
   mismatch aborts untouched), an interrupted migration resumes, two sources with one PDF share a
   file, a tampered or missing file is `pdf_corrupt`, a symlink in the store directory is refused,
   orphans are swept, an import racing the sweep keeps its file.
8. **Docs.** Rewrite the reference sections in the table (each kept limit with its reason),
   `release-scope.md`, `limitations.md` (add: damaged-file inspection bound, exclusion history
   growth, CLI memory bounds; reword C5, L-SRC-3, L-SRC-8 as CLI-only), `decisions.md` DEP-29,
   `source-upload-plan.md`, TODO; delete this plan.

Review: this plan and each slice get a fresh Opus reviewer (AGENTS.md); slices 1, 3, 4 and 7
(sandbox input, TOCTOU, accounting, store security) need the adversarial pass. Builds and tests
of agent work run in containers; weekday builds at 2 cores.

## Risks

- A MuPDF bug on a very large or hostile file is contained by the same sandbox; memory by
  `RLIMIT_AS`, time by the deadlines; a big PDF fails as `too_large`/`took too long`, never
  exhausts the host.
- Disk can fill; the margin keeps SQLite/WAL writable but other processes race free space. Every
  write failure releases slot, spool and reservation (slice 4 tests).
- Hashing a multi-GB file once per job is heavy on the magnetic disk; it runs outside locks.
- Migrations 028/029 touch live tables and the PDF column; both are tested on seeded data and sit
  under the backup-before-upgrade guidance (the backup now includes `sources/`).
- Unlimited selection moves failures to render time; step 4 must create the source page by page.

## Open questions for the owner

1. Migration 029 aborts startup if a legacy `sources.pdf` blob fails its hash (it should not:
   import verifies the hash). Default: abort with a fixed message and restore from backup; or keep
   that row's blob and skip dropping the column?
