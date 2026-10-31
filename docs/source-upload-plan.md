# Source upload plan: sandboxed PDF work, upload and jobs (import plan step 3)

Goal: the Owner uploads a PDF from `/admin/materials`, picks the pages and the game, sees them
rendered, and confirms a watermark exclusion. Every parse of the uploaded file happens in a
sandboxed worker process. Step 3 ends with a **staged upload**: the PDF, its inspection, the
rendered pages and the exclusion, waiting for a reading route. Route A (step 4) turns it into
the first source. Until then the routes are shown disabled with "arrives with route A".

Umbrella: `docs/source-import-plan.md` (step 3). Three parts, each its own reviewed change:
3a the worker, 3b upload, staging and jobs, 3c the watermark exclusion.

## Decisions this step lands

- **SRC-1** amended: the Owner may upload one PDF at a time from `/admin/materials`;
  `import-source` stays for prepared bundles. Still no archives, URLs or nested imports.
- **SRC-3** amended: Mimik renders and reads uploaded PDFs itself, only in the sandboxed PDF
  worker. Stored page images stay immutable once a source is made from them.
- **DEP-29** (renderer): `mupdf =0.8.0` and `mupdf-sys =0.8.0` (messense/mupdf-rs, AGPL-3.0,
  released 2026-06-22). The crate vendors MuPDF's C source and its third-party libraries and
  builds them with `cc`. Bindings are generated with `bindgen`, so the CI and release images
  gain `libclang`. All `sys-lib-*` features are off (nothing is linked from the system). Format
  features are off except PDF: no `js`, `html`, `epub`, `svg`, `cbz`, `img`, `docx-output` or
  `libarchive`. Fonts: `base14-fonts`, with missing embedded fonts substituted (recorded on the
  page). 3a records in DEP-29:
  - the bundled MuPDF version and its open CVEs;
  - the clean build time at 2 cores in `mimik-ci` and in the release container;
  - the release binary's size before and after;
  - whether a feature set this small builds at all.
  If it does not build, or the build adds more than 10 minutes, 3a stops and asks.
- **REL-2 kept:** `cargo vendor` puts the mupdf-sys crate, with its full C source, into the
  dependency-sources archive. 3a checks that the archive holds every C file the build compiles
  (the `cc` file list against the archive). The notices gain the licences of the bundled C
  libraries: MuPDF and extract (AGPL-3.0), jbig2dec (AGPL-3.0), FreeType (FTL), HarfBuzz (MIT),
  lcms2mt (MIT), libjpeg (IJG), OpenJPEG (BSD-2-Clause), zlib, gumbo (Apache-2.0) and brotli
  (MIT), each verified against the vendored tree. `about.toml` accepts AGPL-3.0 for these two
  crates only.
- **DEP-30** (upload parsing): axum's `multipart` feature (`multer`, MIT, already
  maintained with axum), with streaming and per-field limits. A plain no-JS form posts the
  file.
- Considered and not taken:
  - `hayro` 0.8.0: pure Rust and memory-safe, but it has no structured-text API, and route A
    would need a second parser.
  - `pdfium-render`: needs a prebuilt PDFium, a new REL-2 exception.
  - Running `lopdf` on the upload: the worker reports page bounds from MuPDF.

## 3a. The PDF worker

**Launch.** A new runner kind beside llama-server: the runner re-executes `mimik pdf-worker
--spec-fd 3` inside a Hakoniwa sandbox. It is spawned from the same long-lived spawning
thread, with the same parent-death signal and parent check; `hako::Plan` gains a profile
kind (llama-server or PDF worker). The worker profile (`hako::Plan` for the worker) differs from
llama-server's:

- **Binds:**
  - The Mimik executable read-only, with an identity check, as the probe does today.
  - The spooled PDF read-only at `/in/document.pdf`, with an identity check.
  - A fresh 0700 per-job output directory read-write at `/out`.
  - The read-only system library folders and the loader cache.
  - No `/dev/shm`, no GPU, no `/sys`.
  - `/tmp` as a 16 MiB tmpfs.
  - `/proc` only if Hakoniwa or glibc needs it (3a finds out and records it).
- **Network:** an empty network namespace, and Landlock refuses TCP bind and connect.
- **Landlock rules:** read on `/in` and on the libraries; read and write on `/out` and `/tmp`;
  execute only on the Mimik executable and the loader.
- **Limits:** `RLIMIT_AS` 2 GiB, `RLIMIT_CPU` set per job, `RLIMIT_FSIZE` 16 MiB, `RLIMIT_NOFILE`
  64 and `RLIMIT_CORE` 0. The worker runs at nice 10.
- **Wall clock:** the parent kills the worker tree on its deadline (below).
- **Seccomp:** the existing default-allow program, with additions. Once the worker has opened
  `/in` and `/out`, it stacks a second filter of its own. That filter refuses:
  - every `clone`, `clone3`, `fork` and `vfork` (MuPDF runs single-threaded, with no lock
    callbacks);
  - `execve` and `execveat`;
  - `socket` and `socketpair`;
  - `openat` with `O_CREAT` outside the fixed output names (checked by Landlock; seccomp cannot
    see paths);
  - `memfd_create` and `userfaultfd`.
  Filters stack, so the second can only tighten the first.
- **No fallback:** if the sandbox is unavailable (no unprivileged user namespaces, AppArmor),
  upload is unavailable, with the same "how to fix" text as the assistant. There is no
  unconfined fallback (ASSIST-44). On macOS and Windows upload is unavailable (limitation).

**Protocol.** The request is a JSON spec in the spec memory file:
- the operation;
- the page list;
- the render scale;
- the limits.

The worker writes three things:
- **Status lines:** JSON lines on the status pipe, at most 64 KiB each and at most 4 per page
  plus 4. Every line is parsed with `deny_unknown_fields` and range-checked.
- **Page images:** `/out/page-<n>.png` only, opened by the parent with `openat2` beneath the
  output directory's descriptor (`RESOLVE_BENEATH`, no links). Each must be a regular file under
  12 MiB. Any other entry in `/out` fails the job.
- **Text:** `/out/text-<n>.json`, at most 8 MiB per page.

The parent validates every PNG with the import rules: `validate_image_header` first, then a
bounded decode, with the dimensions matching the reported page size. The server never opens the
PDF with a parser. It only streams the file, hashes it and checks the `%PDF-` magic in the first
1,024 bytes.

**Operations** (MuPDF only):
- **`inspect`:**
  - Whether the PDF is encrypted: a document that needs a password is refused as
    "password-protected"; owner-password-only PDFs open.
  - The page count, at most 2,000.
  - Per page:
    - width and height in points after the CropBox and rotation are applied, at most 4,000;
    - the rotation;
    - whether the page has a text layer: characters, characters without a Unicode mapping,
      and the area covered by images;
    - whether the page is tagged.
  - Deadline: 60 s.
- **`render`:** the selected pages at 150 dpi, scaled down to fit the import limits (4,096 ×
  8,192, 12 MP). One status line per page as it finishes. Deadline: 30 s per page, 20 minutes
  in all.
- **`text`:** MuPDF structured-text lines with their boxes for the selected pages. The limits
  are 20,000 lines per page and 4 KiB per line. Step 3 uses it for the watermark proposal, and
  step 4 extends it to route A's reading. Deadline: 10 s per page.

Geometry everywhere is in the displayed page's space: points, origin top-left, with the CropBox
and rotation applied. That is the space Marker's polygons and the review screen already use. A
test compares MuPDF's bounds with `lopdf`'s on the unrotated synthetic fixtures, so the upload
path and `import-source` agree.

**Hostile corpus** (synthetic, committed as a generator in the tests, not as binaries). Cases:
- deep nesting of arrays and dictionaries (100,000 levels);
- a FlateDecode bomb: 1 GiB of zeros, and nested filters;
- an image bomb: a declared 100,000 × 100,000 image;
- an xref `Prev` loop, and object streams that reference each other;
- a page tree with a cycle, and a `Count` of 2³¹;
- a content stream of 50 million operators (the CPU limit);
- a huge MediaBox;
- garbage JBIG2, JPX and font streams;
- a truncated file, a file that is not a PDF, and a password-protected one;
- rotated pages and a non-zero CropBox origin, which must work.

Each case must:
- end with a bounded reason ("could not be read", "too many pages", "took too long",
  "password-protected"), or succeed where it is valid;
- finish within its deadline plus 5 s;
- leave no file behind;
- leave the server answering requests while it runs (a request served during the job, a test).

**Sandbox tests (live, like the assistant's):**
- The worker cannot open a socket, read `$HOME` or the data folder, exec, or write outside `/out`.
- A write over 16 MiB kills it.
- The parent refuses a link, an extra file, an oversized PNG header and an over-long status
  line in `/out`.
- `SIGKILL` of Mimik takes the worker with it.
- A cancel mid-render leaves nothing running.
- The hidden subcommand `mimik pdf-worker` is an internal protocol, not a privilege boundary
  (as for `assistant-runner`).

## 3b. Upload, staging and jobs

**Route.** `POST /admin/materials/uploads` requires the Owner session, REL-5's Host and Origin
check and the CSRF admission bound to the session (`recheck_owner_write`). All of these are
checked before a byte of the body is read.
- The multipart `csrf` field must come first. A form whose first field is anything else is
  refused (browsers send fields in document order).
- `Content-Length` over 128 MiB answers 413 at once.
- The file is streamed to `data/staging/upload-<random>.part`: 0600, `O_EXCL`, in a 0700
  directory. It is hashed while it streams, and the stream is cut at 128 MiB plus one byte
  (413, the partial file deleted).
- Free disk under 512 MiB refuses before streaming.
- One upload streams at a time; a second gets 409 "an upload is in progress".
- On success the file is fsynced and renamed to `<id>.pdf`, and the row is inserted.
- The original file name is kept for display only: control characters dropped, at most 200
  characters. It is never logged, since it can carry the buyer's name.

**Staging (migration 026, outside 007's immutability).** Tables:
- **`source_uploads`:**
  - id;
  - sha256 and size;
  - display name;
  - actor (the Owner's attribution, as in review events);
  - created and last-activity times;
  - state: `uploaded`, `inspected`, `rendered` or `failed`;
  - the inspection JSON (bounded, validated);
  - the selected pages;
  - system id, edition and language, with the same rules as an import.
- **`source_upload_pages`:** upload, PDF page, PNG BLOB, hash, pixel size and point size.
- **`source_upload_exclusions`:** 3c.

A source made later (step 4) copies what it needs into its permanent tables in one
transaction. Staging is never evidence and never searched.

Other staging rules:
- **Open uploads:** one at a time. "Discard" deletes the row (cascade) and then the file.
- **Abandoned uploads:** no activity for 7 days. They are deleted at start-up and every hour.
  Files without a row and `.part` files are swept at start-up.
- **Duplicates:** a PDF whose hash matches an existing `source_documents` row says so ("already
  document N"). Going on gives a new reading of the same document (allowed by the umbrella
  plan).

**Jobs (same migration).** `source_jobs` records:
- the upload;
- the kind: `inspect`, `render` or `text`;
- the state: `queued`, `running`, `succeeded`, `failed` or `cancelled`;
- the pages done and the total;
- a bounded error code and message;
- the start and end times.

How jobs run:
- **Concurrency:** one job runs process-wide, and a new one queues behind it. This is separate
  from SRC-8's crop slot and from the model-work gate: the worker is CPU-only and runs at nice 10.
- **Start-up:** `queued` and `running` jobs become `failed` with "interrupted by a restart"
  (the worker died with Mimik). The owner can start them again.
- **Cancel:** kill the runner's tree through the pidfds the manager already uses. The job shows
  `cancelled` once the tree is reaped, and its partial pages are deleted.
- **Results:** a job writes its results in one transaction after validation. A render that
  fails on one page fails the job and keeps no page.

**Screens** (`/admin/materials/uploads`, strict CSP, desktop and tablet first):
1. **Upload form:** file, game, edition and language.
2. **Upload page:**
   - the inspection: page count, sizes, rotation, text layer per page, encryption note;
   - a page-range picker, at most 400 pages per upload;
   - a Render button;
   - job progress with Cancel, refreshed by `<meta http-equiv="refresh" content="2">` without
     JavaScript and by a fragment poll with it;
   - the reading routes, disabled with what each needs.
3. **Rendered pages:** shown as on the review screen (zoom, pan).

Browser checks extend `tools/review_browser.py`:
- upload;
- inspect;
- render;
- cancel;
- a restart mid-job;
- discard;
- 1280 and 390 px, with and without JavaScript;
- no overflow;
- the strict CSP.

## 3c. Watermark exclusion

After the render, a `text` job reads the selected pages. The proposal is a deterministic
function on the validated worker output, unit-tested on synthetic pages:
- **Normalisation:** NFKC, collapsed whitespace, case-folded.
- **Grouping:** lines with the same normalised text whose boxes, as fractions of the page's
  width and height, overlap the group's median box with IoU ≥ 0.8.
- **What is proposed:** groups of 4 to 200 characters present on at least 60% of the selected
  pages and on at least 3 pages. The top 3 are proposed, by page count. Each box is the union of
  the group's boxes plus 2 pt.
- **No text:** a page without a text layer gives no proposal; the owner draws the box.

The owner:
- sees the proposal highlighted on a page;
- confirms it, adjusts the box (handles with JavaScript, numeric inputs without), or draws one
  with the step-2 drawing tool;
- picks the pages (default: all selected) and gives a rationale.

The exclusion row records:
- the actor;
- the rationale;
- the pages;
- the geometry as page fractions plus points per page;
- the proposal it came from, if any.

It can be replaced or removed until a source is made, and each change is a new row (history
kept). The staged pages show the region masked with an opaque overlay. The stored PNG stays the
original render (SRC-13). Its effect on readings (units inside rejected, crossing ones flagged,
blacked-out hints) is applied by each route from step 4 on. The proposal text is personal data:
it is shown to the Owner only and never logged (a test scans the log).

## Checks and gate

- Every part:
  - the full gate (fmt, strict Clippy, locked tests, helper lint and tests, audit, licences,
    release build, unpacked-release smoke);
  - failing-first regression tests for each issue found.
- **3a:** the hostile corpus and the live sandbox tests run in CI, on Linux with user
  namespaces. Where CI lacks them, the job fails rather than skips.
- **Local live checks:**
  - inspect and render the MÖRK BORG IT PDF and Bare Bones EN;
  - record seconds per page and peak memory in `docs/results.md`.
  The PDFs stay local.
- **3c:** the proposal finds the IT PDF's buyer watermark (a local check: only the page count
  and the box are recorded, never the text) and proposes nothing on Bare Bones.

## Docs when it lands

- `docs/sources-and-retrieval.md`: upload, staging, jobs and exclusion.
- `docs/model-runtime.md`: the runner's two kinds; the worker profile moves into a shared
  sandbox section.
- `docs/release-scope.md`: upload is Linux-only, with the sandbox required.
- `docs/decisions.md`: SRC-1, SRC-3, DEP-29 and DEP-30.
- `docs/limitations.md`:
  - MuPDF CVE tracking is manual, a watch item like llama.cpp's;
  - no fuzzing of the worker yet;
  - substituted fonts;
  - macOS and Windows have no upload.
- The umbrella plan's step 3 is deleted.

## Risks

- MuPDF is C parsing hostile input. The sandbox, the limits and the stacked filter are the
  boundary, and the server holds the same library but never calls it on uploaded data
  (MuPDF calls live in one module, `src/sources/worker/`, used only by `mimik pdf-worker`; a
  CI check refuses `mupdf::` anywhere else).
- Build cost and binary size grow; 3a measures both before going on.
- A 128 MiB upload and its renders sit in staging; the 7-day sweep and Discard bound it.
