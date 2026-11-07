# Source size limits (SRC-15) applied to the upload and staging pipeline (step 3)

Goal: a large, high-quality manual can be uploaded, inspected, selected and rendered in the
browser. The PDF is bounded by disk only; no per-upload total remains; every kept limit has a
technical or security reason stated in `docs/sources-and-retrieval.md`. Decision: SRC-15
(`docs/decisions.md`). Remaining work only; the CLI `import-source` path is unchanged.

## Current state

How the PDF reaches the worker (not stdin, not a record): the parent opens the staged
`data/staging/<id>/document.pdf` (`launch.rs:283` `open`, `launch.rs:492` `prepare`), records
its device/inode and bind-mounts the file read-only at `/in/document.pdf` with that identity
(`profile.rs:62`, checked inside the sandbox after the mounts, `sandbox/mod.rs:539`, `:958`).
Landlock grants Read on `/in` (`profile.rs:85`). The spec (`/run/spec.json`, `entry.rs:28`)
carries only the operation, page numbers and the in-sandbox path. The worker then copies the
whole file into a `Vec` capped at 128 MiB (`entry.rs:61-71`), closes it, installs the second
seccomp filter (`entry.rs:72`; it kills clone/exec/socket/memfd/userfaultfd/ptrace/seccomp, not
`openat`, `filter.rs:6-17`) and passes `Input::Bytes` to `operations::open`
(`operations.rs:32-45`), which checks `%PDF-` in the first 1,024 bytes and `%%EOF` in the last
1,024, then `PdfDocument::from_bytes` (`vendor/mupdf/src/pdf/document.rs:585`: a second copy
into a MuPDF `Buffer`; DEP-29 notes both copies inside `RLIMIT_AS` 2 GiB, `runner.rs:450`).

The parent verifies size, SHA-256 and magic once, at creation (`uploads.rs:1355` `verify_spool`).
Jobs run `inspect(&path)` once and `render(&path, batch)` per 8-page batch (`uploads.rs:1020-1085`),
each a fresh worker process; nothing re-verifies the file between launches.

Places that encode the limits to remove or change:

| Limit | Where |
| --- | --- |
| 128 MiB PDF | `auth/uploads.rs:35` (`FILE_LIMIT`, `BODY_LIMIT`, per-chunk `:231`, multer `:311-316`, Content-Length `:349`); `staging.rs:19` `UPLOAD_RESERVATION`; `uploads.rs:1356` `verify_spool`; `entry.rs:61-71`; `operations.rs:34`; `migrations/026_source_uploads.sql:5` `pdf_bytes <= 134217728`; `templates/upload_form.html:4,11` |
| 64 selected pages | `uploads.rs:1478` `check_selection`; `templates/upload_page.html:13`; migration 026 `:15` (`selected_pages` <= 385 bytes, <= 64 entries), `:120-121` (`pages_done/total <= 64`); `StagedContract.limits.pages` `uploads.rs:901` |
| 128 MiB PNG total | `uploads.rs:875` (`contract`), `:1222-1226` (`publish_current`, reason `source_png_limit` at `:1404`), `templates/upload_progress.html:3`; contract `png_bytes` `:902` |
| 512 MiB text | `uploads.rs:1228` |
| 20,000 units | contract `units` `uploads.rs:903` (no enforcement in staging) |
| 2 GiB staging total | `staging.rs:17` `TOTAL`, checked in `Ledger::reserve` `:59` |
| Tests | `upload_tests.rs:655,1835` and the cumulative-PNG/text/total cases, `upload_route_tests.rs:715` (128 MiB + 65,537 stream), `worker/tests.rs` and `live_tests` cases naming 128 MiB |
| Docs | `sources-and-retrieval.md:89-93,110,157,182-186,217,286-287`; `release-scope.md:33`; `limitations.md` C5, L-SRC-3, L-SRC-8 (CLI numbers, rewording only); `source-upload-plan.md` (historical sections that still state the totals) |

Surprising: migration 026 hard-codes the same caps as CHECK constraints (PDF size, `selected_pages`
<= 64 entries and 385 bytes, exclusion `pages` <= 64, `pages_done`/`pages_total` <= 64). SQLite
cannot alter a CHECK, so lifting the page cap needs a table rebuild migration.

The worker also caps the page count at 2,000 (`operations.rs:52-58`, `records.rs:411`; migration
`:22,30,60`: `pdf_page` 1-2,000). Inspection output is capped at 9 MiB (`records.rs:13`, migration
`:14`).

## Design decisions

1. **Open from the bound file, by path.** The bind already is a verified, read-only,
   single-file, identity-checked mount; the worker opens `/in/document.pdf` with
   `PdfDocument::open` (`vendor/mupdf/src/pdf/document.rs:580` -> `Document::open` `document.rs:67`
   -> `mupdf_open_document`, a path open through MuPDF's stdio file stream, which seeks and reads on
   demand and does not load the file; the cost is the xref, objects touched, and MuPDF's default
   store). No new mount, grant or syscall is needed: Landlock already allows reading `/in`, seccomp
   does not block `openat`, `RLIMIT_FSIZE` limits writes only, the file is not mmapped so `RLIMIT_AS`
   does not scale with the PDF. A single decompressed object, glyph cache and page raster still
   count against the 2 GiB; they are the per-object/page memory bound that stays. Rejected: passing
   an fd (mupdf-rs has no stream-from-fd constructor, no `/proc` in the sandbox, and a bind of
   `/proc/self/fd/N` resolves in the runner, not the parent).
2. **Format contract without a byte copy.** The `%PDF-` and tail `%%EOF` checks (`operations.rs:36-43`)
   move to a `std::fs::File` read of the first and last 1,024 bytes (seek from end) before the
   filter; MuPDF then reopens the same read-only bind. Both opens see the same inode; a
   swap is excluded by the identity check, in-place change by decision 3. The contract is a format
   rule, not a confinement boundary.
3. **Immutability and verification, cheap per launch.** In-place modification is outside the
   sandbox's model (same user); the aim is detection of accidental or hostile change between hash
   and parse so that `pdf_sha256` in the contract always describes what was parsed.
   - After `create` fsyncs the file, `fchmod` it 0400. The job opens it with `O_NOFOLLOW`, checks
     regular file, link count 1, owner, and pins (dev, ino, size, mtime_ns, ctime_ns).
   - Once per job, before the first launch, re-hash the file in 1 MiB reads on the job thread
     (cancel-aware, bounded by disk throughput; about 5 s for 1 GiB on the magnetic disk) and compare
     with the stored hash; a mismatch fails the job with a new fixed reason `document_changed`.
   - Each 8-page batch launch compares the pins (fstat on the held fd), and the job re-checks them
     after the last batch before settling success. Hashing per batch is rejected: 63 batches of a
     500-page book would re-read the file 63 times.
   - The parent keeps its opened `File` for the life of the job (`launch.rs` already holds
     `input_file`), so the inode cannot be replaced and reused by the bind.
4. **Admission by free space, not a size.** `Ledger::reserve` loses the `TOTAL` condition; it keeps
   free space >= outstanding reservations + 512 MiB margin (`staging.rs:60`). The upload reserves
   `Content-Length` (when present, plus the 64 KiB multipart overhead) at `begin_upload`; without a
   `Content-Length` it reserves a floor (the metadata and one hash buffer, 1 MiB) and the spool write
   path tops up the reservation in 64 MiB steps before each step is written, so a chunked client
   can never outgrow the ledger. A refusal answers 507 `space` and unlinks the spool (existing
   RAII path). The route's `FILE_LIMIT`/`BODY_LIMIT` become "free space at the time"; the
   per-request bounds (4 KiB before CSRF, small field limits, 30 s idle, read-ahead) stay.
5. **Upload deadline.** The 15-minute total deadline (`auth/uploads.rs:38`) would cap the PDF
   at what a link moves in 15 minutes. Replace it by the idle deadline plus a minimum average rate
   (at least 256 KiB/s after the first 60 s) so a stalled slow-loris is still cut and a slow but
   steady multi-gigabyte upload is not. Reason: a held slot and a held spool.
6. **Selection and rows without a total.** Selection any subset of the inspected pages (ascending,
   distinct, within `document.pages`). Storage is per page rows already; the new migration rebuilds
   `source_uploads` and `source_jobs` without the 64/385/128 MiB checks, with the 2,000-page range
   check (decision 7) and a `selected_pages` bound that follows from 2,000 integers (<= 10,000
   bytes). Progress counters follow the selection size.
7. **Keep 2,000 pages per document**, reason technical: the inspection record is one JSON frame
   (<= 9 MiB, 2,000 pages x ~0.4 KiB geometry) held by parent and database, the page-selection form
   posts one field per page under a 128 KiB body, and MuPDF's page-tree walk is bounded by the
   inspect CPU/wall budget. This is an input bound per document structure, not a size or selection
   total. Raising it needs per-page inspection records (open question 1).
8. **Contract `mimik-staged-v1`.** Drop `limits` (nothing consumes it yet; step 4 is unbuilt) and the
   `png_bytes` sum check; keep per-page facts. At 2,000 pages the contract is about 1.2 MB of JSON,
   built in memory from rows; step 4 reads it per page (consumers must not assume a small list;
   `docs/source-route-a-plan.md` already stores readings per page). Keep the name `mimik-staged-v1`
   (no consumer exists); record the removal in the reference doc.
9. **Kept limits and reasons** (stated in `sources-and-retrieval.md`): form field sizes and the
   4 KiB pre-CSRF read (unauthenticated memory); per-PNG 4,096 x 8,192 px, 12 Mpx, 12 MiB and the
   64 MiB decode bound (decompression bombs); per-page text 8 MiB, depth 32, 40,000 nodes, 64 KiB
   HTML per unit, 512-byte block ids (parser and database row size); worker `RLIMIT_AS` 2 GiB,
   16 MiB scratch, CPU 20 s inspect / 25 s per page and wall 30 s (hostile PDFs); 160 MiB per
   8-page render reservation (worst case 8 x (12 MiB + 8 MiB)); one upload in progress and one
   retained upload (single-owner staging); 2,000 pages; exclusion records of at most 64 pages and
   32 KiB of boxes (one proposal's comparison and row size; selection is not limited by it);
   CLI import: `bundle.json` 8 MiB, `source.json` 32 MiB, PDF 128 MiB read into memory, 64 pages and
   20,000 units per bundle, 128 MiB of PNGs (all in-memory JSON/`lopdf` bounds; document them as
   memory bounds).

## Large page counts (500 pages, checked in the code)

- Render batches: 63 launches of 8 pages, each reserving 160 MiB and releasing the unwritten
  part. Free-space exhaustion mid-job fails the job with `space`; published pages stay and a retry
  renders the rest. Cancel signals pinned pidfds and is checked between batches; progress is
  `done/total` polled as an HTML fragment (`progress` re-reads the upload each poll).
- Per-page publication loads all page rows (`load_parts`, `uploads.rs:1195`) per page: O(n^2)
  row reads, fine at 500, to be measured at 2,000 and narrowed to the needed facts if slow.
- Each launch reopens the PDF; a huge xref or a damaged file (MuPDF repair scans the whole file)
  can exceed the fixed 30 s inspect wall and fail as `took too long`; this is the intended
  bound, shown to the Owner and documented in `limitations.md`.
- UI: `upload_page.html` renders one checkbox per inspected page (up to 2,000) and `set`
  posts `page_N` fields (`auth/uploads.rs:660`) under the 131,072-byte body limit (2,000 fields
  about 20 KB: fits). Add "select all" / range input so a book is one click, with the 64 text removed.
  The material screens read selected pages only.
- Staging bytes at 500 pages: PNG ~1-3 MiB plus text per page, charged by the ledger per batch.

## Slices

Each slice is independently reviewable; each ends with a regression test that fails first.

1. **Worker opens the file.** `Input::Path`, remove the `Vec` and `from_bytes`, move the magic checks to
   file reads, drop the 128 MiB check. Tests: unit (magic checks on a truncated/no-EOF file);
   live sandbox corpus incl. password, damaged, repair-needed files; a PDF over 128 MiB (sparse
   padded stream; peak RSS from wait4 stays far below 75% of `RLIMIT_AS`; asserts the old code
   returned `too_large`); a PDF whose xref lies beyond 4 GiB (sparse file, 64-bit seeks); cancel
   mid-render on the big file; `profile.rs` pinned-profile test unchanged (no new grant). Update
   DEP-29 text ("held in both a Vec and MuPDF's buffer") and the RSS sections.
2. **Free-space admission and upload route.** Remove `TOTAL` and the 128 MiB constants, reserve by
   `Content-Length` with top-up for chunked bodies, rate-based deadline, 507 on refusal. Tests:
   route tests for a body over 128 MiB (declared and chunked without `Content-Length`, sparse
   streamed generator, hash matches); a ledger fake with free space exhausted mid-stream (partial
   spool unlinked, reservation released, slot freed); the existing disk-full ENOSPC test
   (`upload_tests.rs:1974`, coordinator-supplied small filesystem) extended to a mid-spool and
   a mid-render ENOSPC; reservation top-up race under the gate; 2 GiB total no longer refuses.
3. **Verification and immutability.** 0400 after create, no-follow open with nlink/owner checks,
   per-job hash, per-launch pin compare, `document_changed` reason (and template text). Tests:
   mutate one byte between inspect and render, truncate, append, replace by rename, symlink at the
   final component, hard link; each fails the job and publishes no page.
4. **Selection and per-upload totals.** Migration 028 (table rebuild without the 64/385/128 MiB
   checks; data-preserving copy; trigger recreation; a migration test with existing rows),
   `check_selection` (any count up to `document.pages`), delete the PNG/text/total checks and
   `source_png_limit`, contract without `limits`, template and fragment text, select-all/range
   control. Tests: select 500 and 2,000 pages of a synthetic PDF end to end with the fake and a live
   worker; contract at 2,000 pages is built from rows; old per-upload cases now succeed; the cases
   that remain (per-PNG, per-page text, 12 Mpx) still fail; a stale 64-page DB check would fail the
   test (migration is applied in tests).
5. **Large-input gate and docs.** 500-page and 2,000-page synthetic PDFs through inspect, all
   render batches, cancel, restart reconciliation, exclusion on a subset of a large selection;
   time `load_parts` growth and the progress poll. Rewrite `sources-and-retrieval.md` (upload,
   staging, limits sections: each kept limit with its reason), `release-scope.md:33`,
   `limitations.md` (add the 2,000-page, exclusion 64-page, CLI memory bounds and the damaged-file
   wall limit; reword C5/L-SRC-3/L-SRC-8 as CLI-only), `decisions.md` DEP-29 sentence, remove the
   totals from `source-upload-plan.md`, delete this plan.

Review: this plan and each slice get a fresh Opus reviewer (AGENTS.md). Slices 1 and 3 are
security-relevant (sandbox input, TOCTOU) and need the adversarial pass; builds and live tests of
agent-written code in containers, weekday builds at 2 cores.

## Risks

- A MuPDF bug reachable now on a very large or deliberately deep file is contained by the same
  sandbox; memory is bounded by `RLIMIT_AS`, time by CPU/wall, so a big PDF fails as
  `too_large`/`took too long`, never exhausts the host.
- Disk: an unbounded PDF plus PNGs can fill the disk; the margin keeps SQLite/WAL writable, but
  other processes can race free space between checks. A write failure mid-stream must release the
  slot and unlink the spool; tested in slice 2.
- Page cache and hashing a multi-gigabyte file once per job is I/O heavy on the magnetic disk.
- The migration rebuilds the upload tables; staging rows are transient, but a failed migration
  must roll back without touching permanent sources.
- Removing the totals moves failures later (during rendering) and the Owner can stage a very
  large selection; step 4 must create the source page by page and cannot assume the old caps
  (C5, PDF stored once per hash).

## Open questions for the owner

1. Keep the 2,000-page document cap (reason: one 9 MiB inspection frame, one selection form), or
   restructure inspection into per-page records so it can go (costs a records-protocol change)?
2. Exclusions cover at most 64 pages and 32 KiB of boxes (one proposal object). Acceptable while
   selection is unlimited, or should exclusions be per page range with several records?
3. Rate-based upload deadline (decision 5) instead of a fixed total: acceptable?
