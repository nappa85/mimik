# Source upload plan: sandboxed PDF work, upload and jobs (import plan step 3)

**Goal.** The Owner uploads a PDF from `/admin/materials`, picks the pages and the game, sees
them rendered, and confirms an exclusion of repeated text (a buyer watermark).

**The boundary.** Every parse of the uploaded file happens in a sandboxed worker process. The
worker writes nothing to the host: everything it returns travels over a pipe. The server
bounds, validates and stores it.

**Where step 3 ends.** With a **staged upload** that follows a versioned contract (below). Route
A (step 4) turns it into the first source. Until then:
- the reading routes are shown disabled, with "arrives with route A";
- `docs/release-scope.md` says that upload stages pages but does not yet make a source.

Umbrella: `docs/source-import-plan.md` (step 3). Three parts, each its own reviewed change:
- **3a:** the shared sandbox and the worker.
- **3b:** upload, staging and jobs.
- **3c:** the repeated-text exclusion, an optional step for the owner.

The umbrella's step 3 text is deleted only once all three parts and their integration checks
have landed.

## Decisions this step lands

- **SRC-1, amended.** The Owner may upload a PDF from `/admin/materials`, one upload at a time.
  `import-source` stays for prepared bundles. Still no archives, URLs or nested imports.
- **SRC-3, amended.** Mimik renders and reads uploaded PDFs only in the sandboxed PDF worker.
  The server never parses them: step 4 must not call `lopdf::Document::load_mem`
  (`import.rs:473`) on an upload, and takes the page facts from the staged contract.
  `import-source` keeps its own `lopdf` checks, and stays unrotated-only.
- **DEP-29 (renderer).** `mupdf =0.8.0` and `mupdf-sys =0.8.0` (messense/mupdf-rs, AGPL-3.0,
  released 2026-06-22).
  - The crate vendors MuPDF's C source and its third-party libraries. They are built with `cc`,
    and the bindings with `bindgen` (`libclang`).
  - It is compiled on Linux only (`cfg(target_os = "linux")`): other platforms have no upload.
  - Features:
    - every `sys-lib-*` feature is off, so nothing is linked from the system;
    - every format but PDF is off: no `js`, `html`, `epub`, `svg`, `cbz`, `img`, `docx-output` or
      `libarchive`;
    - fonts: `base14-fonts`, and a missing embedded font is substituted and noted on the page.
  - 3a's first gate checks the following before any other code. If any check fails, or the build
    adds more than 10 minutes to the slowest CI job, 3a stops and asks.
    1. **Tarballs:** the pinned crate tarballs.
    2. **API:** the crate's coverage of what the worker needs (open from memory, page bounds with
       rotation, render to a pixmap, structured text with boxes, the store size and the
       error mapping).
    3. **MuPDF:** the bundled version and its open CVEs.
    4. **Build:**
       - whether this feature set builds;
       - pregenerated bindings if the crate offers them, otherwise a `libclang` pinned by
         package version in `mimik-ci` and in the Ubuntu 22.04 release image (REL-4);
       - the build cost in every CI job (Clippy, tests, release, audit) at 2 cores, with how
         the mupdf-sys build is cached;
       - the size of the release binary, before and after.
    5. **Memory:** peak RSS and address space under the exact worker profile.
- **REL-2 kept.**
  - `cargo vendor` puts the whole mupdf-sys tarball into the dependency-sources archive: C
    sources, headers, generated files, fonts and the build script.
  - 3a extends the release check: a clean offline rebuild from the distributed archives builds
    the worker. `BUILD.txt` and `SOURCE.txt` gain the C compiler and the `libclang`
    prerequisites.
- **Notices.**
  - The notices cover the vendored tree: every licence file in it, whether its code is compiled
    or not (mujs, and anything else vendored but switched off, is listed too).
  - Each is verified against the vendored text: MuPDF and extract (AGPL-3.0), jbig2dec
    (AGPL-3.0), FreeType, HarfBuzz (MIT), lcms2mt (MIT), libjpeg (IJG), OpenJPEG
    (BSD-2-Clause), zlib, gumbo (Apache-2.0), brotli (MIT), and the base14 fonts' licence.
  - FreeType: Mimik takes the FTL option, provided the gate confirms on the FSF licence list
    that the FTL is compatible with GPLv3, so that it combines with AGPL-3.0. FreeType's GPL
    option is GPLv2 and would not do. If the FTL is not compatible, 3a stops and asks. Its credit
    clause is met in the notices. DEP-29 records this.
  - `about.toml` accepts AGPL-3.0 for exactly `mupdf` and `mupdf-sys`.
- **MuPDF security updates.** `cargo audit` does not cover the bundled C. So:
  - each release checks MuPDF's security advisories for the bundled version (a release-checklist
    item and a TODO watch item, like llama.cpp's);
  - a fix upstream means a crate bump, or, if the crate lags, a patched vendored tree recorded
    as a DEP decision.
- **DEP-30 (multipart).** `multer` directly (MIT, the parser under axum's `Multipart`).
  - Its `Constraints` give the allowed fields and per-field size limits, which axum's wrapper
    does not expose.
  - It is new to the tree: licence and maintenance are checked when it is added.
- **Considered and not taken:**
  - `hayro` 0.8.0: pure Rust, but no structured-text API, so route A would need a second
    parser.
  - `pdfium-render`: needs a prebuilt PDFium, which would be a new REL-2 exception.

## Staged-upload contract (`mimik-staged-v1`)

This is what step 4 reads, without parsing the PDF again:
- the PDF's SHA-256 and size;
- the renderer: the crate and MuPDF versions and the render settings (dpi, colourspace,
  anti-aliasing);
- per page:
  - the displayed size in points;
  - the transform from PDF space to displayed space;
  - the PNG's hash, pixel size and dpi;
  - the substituted fonts;
  - the text-layer statistics;
- the selected pages, with the upload revision; a page's rendering is current when it is
  still selected and its render settings match, whatever revision it was rendered for;
- the source limits it was built to: 64 pages, 128 MiB of PNG, 20,000 units (checked again by
  each route);
- the system id, edition and language;
- the current exclusion and its revision (3c), or none.

**Displayed space.** The visible box is the CropBox clipped to the MediaBox, scaled by
`UserUnit`. It is moved to origin (0, 0), rotated by `Rotate`, and its y axis points down
(points, origin top-left). That is the space of Marker's polygons and of the review screen.
- The text boxes use the same matrix.
- A page larger than 4,000 points in either dimension after `UserUnit` is refused.
- The pixel size comes from one shared function, so it always passes `validate_image_header`'s
  0.01 aspect tolerance.

## 3a. Shared sandbox and the PDF worker

**Refactor first.** Today `hako::Inputs` and `hako::Plan` require a model, a token and a socket
directory (`hako.rs:86-130`). `container()` always mounts `/proc` (`hako.rs:926`), and
`landlock_rules` always grants it (`hako.rs:247`). The refactor:
- moves the sandbox construction to a shared module (`src/sandbox/`), with a profile that
  declares its binds, its tmpfs, its Landlock rules and whether it mounts `/proc`;
- keeps llama-server's profile byte-for-byte as it is: its existing tests must pass unchanged;
- means `sources` does not depend on `assistant::runtime`.

**Launch.** The runner re-executes `mimik pdf-worker` inside the sandbox from the same
long-lived spawning thread, with the same parent-death signal and parent check.

**Descriptors and transport.** Hakoniwa closes descriptors above 2, and the closure keeps
descriptor 3 for the unforgeable marker and puts `/dev/null` on stdin. So the worker gets
exactly fds 0, 1 and 2:
- **Spec:** a JSON spec of at most 64 KiB that the runner has Hakoniwa write into the `/run`
  tmpfs at `/run/spec.json`, as the token is written today. It holds no secret.
- **Output:** framed records on stdout, read by a dedicated record reader (the line drain of
  `drain.rs` splits lines at 4 KiB and would corrupt binary records). Each record
  is a header line (JSON, at most 4 KiB, `deny_unknown_fields`) and then exactly the
  `length` bytes it declares. Limits:
  - **per record:** 12 MiB for a PNG, 8 MiB for one page's text, 4 KiB for a statistics or
    inspection-page record, 4 KiB for the final `done` record;
  - **per render run:** at most (12 MiB + 8 MiB + 16 KiB) × the pages in the run + 4 KiB, so a
    run of permitted maximum-size records always fits; anything past it kills the run;
  - **per inspect run:** one `document` record, one `page` record per page (at most 2,000; a
    page record's header and payload together at most 2 KiB) and `done`: at most 9 MiB in all.
  - Tests: a render run of maximum-size PNG and text records on every page, and an inspect
    report of 2,000 maximum-size page records, both accepted; one byte more, refused.
  - **Schemas** (`deny_unknown_fields`; every number finite):
    - `document`: `pages` (1–2,000), `encrypted` (bool), `mupdf` (version string, at most
      32 bytes). Password-required inputs produce a terminal error instead.
    - inspect `page`: `n` (1–`pages`, in order), `width` and `height` (1–4,000 points),
      `rotate` (0, 90, 180 or 270), `user_unit` (0.1–10), `transform` (6 numbers, each at most
      10⁵ in magnitude, determinant non-zero, mapping the visible box onto
      `[0,width]×[0,height]` within 0.5 pt), `tagged` (catalog `MarkInfo /Marked true`
      and a `StructTreeRoot`). Reversed box corners are normalized before clipping.
    - render `image`: `n` (a requested page, in order), `length` (1 byte–12 MiB), `width_px`
      and `height_px` (equal to the shared pixel-size function's for that page), `dpi`.
      PNGs are fully decoded through IEND with no trailing bytes; worker images require
      RGB, 8-bit samples. Import and worker readers share the validator.
    - render `stats`: `n` (same page), `chars` and `unmapped` (0–10⁶, `unmapped` ≤ `chars`),
      `image_area` (0–1, a fraction of the page), `fonts_substituted` (at most 8 names of 64
      bytes, so even worst-case JSON escapes fit the 4 KiB frame), `fonts_skipped`
      (0–10,000 malformed entries), `incomplete` (diagnostic scan budget exhausted or
      malformed resources), `dropped_lines` (0–10⁶ zero-area/clipped-away lines).
      Font diagnostics never reject a renderable page. Indirect objects are visited once;
      traversal includes Forms, tiling patterns and normal annotation/widget appearances.
      Names use deterministic PDF `#xx` escapes and `Unknown` for an absent name.
    - render `text`: `n` (same page), `length` (at most 8 MiB); its payload is a JSON array of
      at most 20,000 lines, each `text` (1–4,096 bytes of UTF-8) and `box` (finite, inside the
      page, x0 < x1 and y0 < y1).
    - `done`: the operation and the count of each record kind.
    - terminal `error`: one bounded reason enum, no diagnostic text; replaces `done` on
      operation failure. Readers return that reason and require EOF after it.
    Each requested page gives exactly `image`, `stats`, `text`, in that order; a run is complete
    only with every requested page in order, then `done` with matching counts; anything else
    fails the run.
- **Stdout protection:** before MuPDF runs, the worker moves fd 1 to a private descriptor and
  points fd 1 at stderr, so a C warning or stray `printf` cannot break the framing.
- **Stderr:** drained on its own thread with the existing line drain and a 64 KiB cap, so
  neither pipe can fill and deadlock. It is never logged verbatim: a failure is logged and
  shown by a fixed reason code.
- **Exit:** read only after the worker tree is reaped. Before framing, statuses 20–25 map
  respectively to unreadable, timeout, too large, too many pages, password protected and
  unsupported (`Error::from_exit_code`). After framing starts, errors also carry a terminal
  `error` record. The entry reads bounded input bytes and closes `/in` before MuPDF starts.

The worker writes nothing to the host. This removes the risks of a writable output directory:
FIFOs and links, files rewritten between validation and storage, filling the disk with many
files, and per-file versus total quotas.

**Worker profile.**
- **Binds:**
  - the Mimik executable read-only, identity-checked, as the probe is today;
  - the spooled PDF read-only at `/in/document.pdf`, identity-checked;
  - the read-only system library folders and the loader cache;
  - `/tmp` as a 16 MiB tmpfs with `noexec`;
  - `/proc` only if glibc or MuPDF needs it (3a finds out; the profile makes it optional);
  - no `/dev/shm`, no `/sys`, no GPU, no writable host path.
- **Network:** an empty network namespace, and Landlock denies TCP bind and connect.
- **Landlock:** best effort, as for llama-server (ASSIST-49):
  - read on `/in`, `/run/spec.json` and the libraries;
  - on `/tmp` only `MakeReg`, `WriteFile`, `ReadFile`, `Truncate`, `RemoveFile` and `ReadDir`;
  - execute only on the executable and the loader.
  Which protection holds without Landlock:
  - **Without Landlock (namespaces, mounts and pipe transport):** the network, the host's
    files, and every write to the host.
  - **Landlock only:** finer limits within `/tmp`.
- **Limits:**
  - `RLIMIT_AS`: measured in 3a's gate, starting from 2 GiB;
  - `RLIMIT_CPU`: soft equal to hard, set per run;
  - `RLIMIT_FSIZE`: 16 MiB;
  - `RLIMIT_NOFILE`: 64;
  - `RLIMIT_CORE`: 0;
  - nice 10.
  - MuPDF's store is capped at 256 MiB through its context. If the crate cannot set it, the
    gate accepts the default only when peak RSS on the valid difficult fixtures, the two local
    PDFs and the corpus stays below 75% of `RLIMIT_AS`; otherwise 3a stops and asks.
  - CPU: hard 20 s for `inspect`, 25 s per page for `render`.
  - The worker is PID 1 of its namespace, so default-action signals do not reach it: SIGXFSZ
    and soft SIGXCPU are ignored. A write over the limit fails with `EFBIG` and the worker exits
    non-zero, and the CPU hard limit's SIGKILL lands.
  - **Exit statuses:** any non-zero exit, a signal or a missing final record maps to a bounded
    reason: "could not be read", "took too long", "too large", "too many pages",
    "password-protected" or "unsupported".
- **Wall clock:** the parent kills the worker tree at its deadline, through the pidfds the
  manager already uses.
- **Seccomp:**
  1. The existing default-allow program runs as the first filter.
  2. The worker installs a second filter, as its first action after reading the spec and
     opening `/in` (before MuPDF sees a byte). It uses a raw
     `seccomp(SECCOMP_SET_MODE_FILTER, 0, &prog)`: `no_new_privs` is already set, and
     `seccompiler::apply_filter` would call the `prctl` the first filter refuses.
  3. The second filter kills the process (`KILL_PROCESS`) on:
     - `clone`, `clone3`, `fork` and `vfork` (MuPDF runs single-threaded, with no lock
       callbacks);
     - `execve` and `execveat`;
     - `socket` and `socketpair`;
     - `memfd_create`, `userfaultfd`, `ptrace` and `seccomp`.
  4. A failed installation exits before MuPDF runs.
- **Availability.** A worker probe at start-up runs a fixed synthetic PDF through `inspect`.
  - If the probe fails, upload is unavailable, with the assistant's "how to fix" text (user
    namespaces, AppArmor) and the probe's reason.
  - There is no unconfined fallback (ASSIST-44).
  - macOS and Windows have no upload (limitation).
- **Internal protocol.** The hidden subcommand `mimik pdf-worker` is an internal protocol, not
  a privilege boundary, as `assistant-runner` is.
- **MuPDF stays in one module:** `src/sources/worker/`, reached only through `mimik pdf-worker`.
  A CI check refuses `mupdf::` anywhere else.

**Operations.** Each run is one operation:
- **`inspect`** (document-level, cheap). It reports:
  - whether the PDF is encrypted. One that needs a password is refused as
    "password-protected"; one with an owner password only opens.
  - the page count (more than 2,000 is "too many pages");
  - per page: the displayed size, the transform, the rotation, `UserUnit` and whether the page
    is tagged.
  It does not interpret content streams. Deadline: 30 s.
- **`render`.** One run per batch of up to 8 pages. For each page:
  - the PNG at 150 dpi, scaled down to fit 4,096 × 8,192 and 12 MP, with fixed deflate
    settings. A PNG over 12 MiB fails that page as "too large".
  - the text-layer statistics: characters, characters without a Unicode mapping, the area
    covered by images, and the substituted fonts.
  - the page's structured-text lines with boxes: at most 20,000 lines and 4 KiB per line, with
    the 8 MiB per-page byte limit as the one that binds.
  Deadline: 30 s per page.

A crash or deadline kills only its batch.

The parent handles each record as it arrives:
1. It checks that the page is one requested, appears once, and comes in order.
2. It validates the in-memory copy it will store:
   - the PNG with the import rules (`validate_image_header` first, then a bounded decode, with
     the pixel size equal to the shared function's);
   - the text with every box finite and inside the page.
3. Only after that does it write the page into staging.

Two rules apply on the server side:
- a page that fails ends the job;
- the pages already stored stay, so a retry renders the rest.

**Tests.**
- **Fake worker:** a fake worker binary drives the parent with malformed and hostile output:
  - over-long headers, a length past its cap, unrequested or repeated pages, truncated records
    and trailing bytes;
  - a never-ending stdout and stderr spam;
  - valid frames mixed with text on stdout;
  - a worker that exits early.
- **Live sandbox tests,** like the assistant's:
  - The worker cannot open a socket, exec or clone; each is killed, and the job fails.
  - It cannot read `$HOME`, the data folder or `/proc/1/root` of the host.
  - It cannot write anywhere on the host.
  - A full `/tmp` (`ENOSPC`, 16 MiB tmpfs) and a file over `RLIMIT_FSIZE` (`EFBIG`, tested with
    a larger test-only tmpfs) each give a failed job, not a hang.
  - `SIGKILL` of Mimik takes the worker with it.
  - A cancel mid-batch leaves nothing running.
  - A failure to install the second filter is reported.
  - With Landlock unavailable (simulated as today), the boundaries the namespaces and the pipe
    give still hold.
- **Geometry fixtures** (synthetic), with an asymmetric landmark (a filled square and a word in
  one corner):
  - rotations 90, 180 and 270;
  - a non-zero CropBox origin;
  - a CropBox larger than the MediaBox;
  - `UserUnit` 2 and an extreme `UserUnit`;
  - `Rotate` and `CropBox` inherited from the page tree.
  An exclusion drawn on a rotated page round-trips into the same displayed box on the review
  screen (3c test).
  The landmark must sit in the same displayed corner in the PNG and in the text boxes. On the
  unrotated fixtures, MuPDF's sizes must equal `lopdf`'s.

**Hostile corpus.** It is synthetic, committed as a generator in the tests, not as binaries. The
generator streams its bombs through `flate2` with bounded memory: it never holds a decompressed
gigabyte (each case at most 64 MiB of memory, 8 MiB of output and 5 s to generate; the whole
corpus at most 10 minutes in CI's confinement job). Valid difficult PDFs are included, to check that the limits do not refuse real
manuals. The cases:
- deep nesting of arrays and dictionaries (100,000 levels);
- a FlateDecode bomb (1 GiB of zeros) and nested filters;
- an image bomb (a declared 100,000 × 100,000 image), and many tiny images;
- an xref `Prev` loop, object streams that reference each other, a large damaged file that
  needs xref repair, and a huge object count;
- a page tree with a cycle, and a `Count` of 2³¹;
- a content stream of 50 million operators, Type3 font recursion, and heavy shading and pattern
  pages;
- a huge MediaBox, and extreme `UserUnit` values;
- garbage JBIG2, JPX and font streams;
- embedded files, JavaScript actions and external references, which must stay inert;
- a truncated file, a file that is not a PDF, and a password-protected one;
- valid difficult pages: a 300-page text PDF, a large full-page image, many fonts and the
  rotated fixtures.

Each case must:
- end with a bounded reason, or succeed where it is valid;
- finish within its deadline plus 5 s;
- leave no process and no staging file behind;
- leave the server answering requests while it runs (a test).

The privileged sandbox checks run in CI's existing confinement job. Where CI cannot provide
user namespaces, the job fails; it never skips. The corpus's worst-case wall time is budgeted
in that job. Fuzzing (cargo-fuzz in a container, on the record parser and on the worker's
MuPDF entry points) is a TODO follow-up after 3a, not only a limitation.

## 3b. Upload, staging and jobs

**Route.** `POST /admin/materials/uploads`, with its own body limit: it overrides the global
`DefaultBodyLimit::max(8192)` (`src/auth/mod.rs:210`). It takes `multer` with `Constraints`.

*Before reading the body:*
- the Owner session;
- REL-5's Host and Origin check;
- `Content-Length` at most 128 MiB plus 64 KiB (multipart overhead), when present (413 at once);
- the staging ledger's reservation for the upload (below);
- the upload slot (one stream at a time; a second gets 409 "an upload is in progress").

*Body stream:* the body is wrapped in a counting stream that enforces the whole-request limit
(128 MiB + 64 KiB) whether or not `Content-Length` is sent, and lets at most 4 KiB (multipart
headers and the `csrf` field) be polled before the CSRF admission. Instrumented-body tests: a
request refused for authentication, Host or Origin polls no byte; a refused CSRF token
spools no file byte; a disconnect mid-file, and the handler's future dropped mid-file, each release the slot and
delete the partial file.

*Fields, in this exact order:*
1. `csrf`, at most 256 bytes. It goes through `csrf_admission` before the file field is read.
2. `system`, `edition` and `language`, each capped and validated with the import rules.
3. `file`, last, at most 128 MiB.

*Refused:*
- any other field;
- a duplicate field;
- a field out of order;
- a field after the file;
- a file over the limit (413).

In each case the partial file is deleted.

*Deadlines:*
- 30 s with no bytes;
- 15 minutes in all.

The slot and the partial file are released by a drop guard, so a client that disconnects or a
cancelled future cleans up too.

*Spool:* `data/staging/upload-<random>.part`, 0600, `O_EXCL`, in a 0700 directory. It is hashed
while it streams, and the server checks only the `%PDF-` magic in the first 1,024 bytes.

*Publish:*
1. fsync the file.
2. Rename it to `<id>/document.pdf`.
3. Insert the row, in one transaction that calls `recheck_owner_write` again (the session can
   end during a long upload), as `sources/materials.rs:102-106` does.
4. An `inspect` job is queued automatically.

The original file name is not stored or logged, since it can carry the buyer's name. The page
shows the size, the hash prefix and the page count.

**Staging.**
- **Files:** in `data/staging/<id>/` (0700, files 0600):
  - `document.pdf`;
  - `page-<n>.png`;
  - `text-<n>.json`.
- **Migration 026 (outside 007's immutability):** the database holds only metadata and hashes,
  never page images or text.
  - **`source_uploads`:**
    - id, SHA-256, size, actor (the Owner's attribution) and times;
    - `revision`, raised by every change to the page selection or the metadata;
    - state: `uploaded`, `inspected`, `rendering`, `rendered` or `failed`;
    - the inspection JSON (bounded and validated);
    - the selected pages;
    - system id, edition and language.
  - **`source_upload_pages`:** upload, PDF page, the revision it was rendered for, the PNG's
    hash, pixel size, displayed size, transform, text statistics and the text file's hash.
  - **`source_upload_exclusions`:** 3c.
- **Selection:** at most 64 pages per upload, the existing per-source limit (`import.rs`
  limits a run's 20,000 units and 128 MiB of images). A longer book is several uploads, and so
  several readings of one document.
- **Budgets:**
  - per upload: 128 MiB of PDF, 128 MiB of PNG (the per-source image limit, `import.rs`, so a
    rendered upload can always become a source) and 512 MiB of text; a render past the PNG
    budget fails as "too large for one source: select fewer pages";
  - all staging: 2 GiB;
  - **reservations:** a process-wide staging ledger holds the bytes staging uses (measured
    from the files at start-up, then kept up to date on every write and delete) and the open
    reservations. Before each write phase it reserves that phase's worst case (an upload:
    128 MiB + 64 KiB; a render batch: 8 × 20 MiB, covering retries, whose earlier files are
    replaced, not added) and releases it after, keeping only what was written. A phase starts
    only when used plus reserved stays within 2 GiB and free space covers every open
    reservation plus a 512 MiB margin kept for the database and its WAL (publication writes
    only rows). Disk-full tests use an injected free-space source and
    a real `ENOSPC` on a small test filesystem.
  Over a budget the job fails as "not enough space".
- **Uploads:** one open upload at a time. A PDF whose hash matches a `source_documents` row
  says so ("already document N"); going on gives a new reading of the same document.

**Jobs (same migration).** `source_jobs` holds:
- the upload, and the upload revision the job was made for;
- the kind: `inspect` or `render`;
- the state: `queued`, `running`, `succeeded`, `failed` or `cancelled`;
- the pages done and the total;
- a bounded reason code;
- the times.

The state machine is transactional:
- **Queue:**
  - at most one job per upload, and one upload, so at most one job in the process;
  - one worker runs at a time;
  - this is separate from SRC-8's crop slot and from the model-work gate (CPU only, nice 10).
- **Results:**
  - A batch's pages are published with a conditional write: only while the job is still
    `running` and the upload's `revision` still equals the job's.
  - A stale result is dropped and its files deleted.
  - A change of selection cancels a running render. Pages no longer selected are deleted with
    the change; a selected page whose rendering has the current render settings stays current
    whatever revision it was rendered for (the staged contract's rule). "Render the remaining
    pages" renders the selected pages without a current rendering. Tests: a stale batch
    published after a selection change, and a sweep racing a publication.
- **Cancel:**
  - A queued job goes straight to `cancelled`.
  - A running one is killed through the pidfds and shows `cancelled` once the tree is reaped.
  - Pages already published stay.
- **Discard:**
  1. Cancel and reap the upload's job.
  2. Delete the rows (cascade).
  3. Delete the folder.
  It holds the job lock throughout.
- **Sweeps:**
  - Uploads with no activity for 7 days are deleted at start-up and every hour, under the job
    lock, so a sweep never touches an upload whose job is running.
  - At start-up, `.part` files, folders without a row and files without a row are deleted.
- **Restart:** `queued` and `running` jobs become `failed` ("interrupted by a restart"). The
  upload page offers "Inspect again" or "Render the remaining pages".
- **After step 4:** when step 4 makes a source, it copies the PNGs, the page facts and the
  exclusion's geometry and rationale into its permanent tables. It then deletes the upload's
  folder and rows. Nothing else is copied: no text files, no proposals.

**Screens** (`/admin/materials/uploads`, strict CSP, desktop and tablet first):
1. **Upload form:** system, edition, language, then the file, last (the field order above).
2. **Upload page:**
   - the inspection: page count, sizes, rotation, encryption note;
   - a page picker (at most 64);
   - Render, then the text-layer summary per page;
   - job progress with Cancel, refreshed by `<meta http-equiv="refresh" content="2">` without
     JavaScript and by a fragment poll with it;
   - the reading routes, disabled with what each needs;
   - Discard.
3. **Rendered pages:** shown as on the review screen, with zoom and pan.

**Browser checks** extend `tools/review_browser.py`:
- upload, inspect, render, cancel, a restart mid-job and discard;
- a refused field order;
- 1280 and 390 px, with and without JavaScript;
- no overflow;
- the strict CSP.

## 3c. Repeated-text exclusion

This step is optional for the owner and is labelled "repeated text", never "watermark":
running headers also repeat. The proposal is a deterministic function on the validated text
files, unit-tested on synthetic pages.

**Normalisation:** NFKC, whitespace collapsed, case-folded, and each run of digits replaced by
`#` (an order number or "page N" can vary).

**Grouping.**
1. Lines are visited in a fixed order: page, then top, left, bottom and right, then normalised
   text bytes, then MuPDF's line order on the page. Each
   joins the first group (in creation order) with the same normalised text whose seed box (its
   first line's box, as a fraction of the page's width and height) it overlaps with IoU ≥ 0.8;
   otherwise it starts a group. At most one line per page joins a group.
2. Groups are ranked by page count, then text length, then the seed box's top, left, bottom
   and right, then the normalised text's bytes, then creation order: a total order.

**Proposal.**
- Groups of 4 to 200 characters, present on at least 60% of the selected pages and on at least
  3 pages. The top 3 are proposed.
- Each page gets its own box: the group's box on that page plus 2 pt, clipped to the page.
- **Pages where the group is absent:**
  - they are listed;
  - a proposed box goes on them only if the owner extends the exclusion to them, after a
    warning; there the box is the group's median box in page fractions, converted to that
    page's points and clipped to it.
- Watermarks drawn as paths or images, and pages without a text layer, give no proposal: the
  owner draws the box.

**Confirmation.**
- **What the owner sees:** the proposal highlighted on a page, and the list of pages it
  affects.
- **What the owner does:**
  - confirms the proposal, adjusts its box (handles with JavaScript, numeric inputs without),
    or draws one with the step-2 drawing tool;
  - picks the pages;
  - gives a rationale.
- **What the row records:** the actor, the rationale, the pages, a box per page in displayed
  points, and the proposal's fingerprint (a hash of the normalised text), never the text.
- **Changes:**
  - Replacing or removing the exclusion adds a new row and raises the exclusion revision.
  - The rows are deleted with the upload.

**Display.** The staged pages show the region masked with an opaque overlay. The stored PNG
stays the original render (SRC-13). The effect on readings (units inside it rejected, units
crossing it flagged, hints blacked out) is applied by each route from step 4 on.

**Personal data.** Proposal text exists only in the staging text files and in the page shown
to the Owner. A test scans the log, worker stderr handling, the database and the error
messages for planted marker strings in the text, the uploaded file name and the rationale
(the rationale is stored, by design, and must appear only in its row).

## Checks and gate

- **Every part:**
  - the full gate: fmt, strict Clippy, locked tests, helper lint and tests, audit, licences,
    release build, and the unpacked-release smoke;
  - a failing-first regression test for each issue found.
- **3a:**
  - the DEP-29 gate first;
  - then the fake-worker tests, the live sandbox tests and the hostile corpus, in CI's
    confinement job.
- **Local live checks:**
  - inspect and render the MÖRK BORG IT PDF and Bare Bones EN, recording the seconds per page
    and the peak memory in `docs/results.md`;
  - the PDFs stay local.
- **3c:**
  - the proposal finds the IT PDF's buyer mark: only the page count and the box are recorded,
    never the text;
  - the result on Bare Bones is reported (its running headers may be proposed).

## Docs when it lands

- `docs/sources-and-retrieval.md`: upload, staging, jobs, the staged contract and the exclusion.
- `docs/model-runtime.md`: the sandbox section moves to a shared reference, with both profiles.
- `docs/release-scope.md`:
  - upload is Linux-only and needs the sandbox;
  - until step 4, upload stages pages only.
- `docs/decisions.md`: SRC-1, SRC-3, DEP-29 and DEP-30.
- `docs/limitations.md`:
  - substituted fonts;
  - macOS and Windows have no upload;
  - proposals miss watermarks drawn as paths or images.
- TODO: the MuPDF advisory watch and the fuzzing follow-up.

## Risks

- **MuPDF is C code parsing hostile input.**
  - The boundary is the sandbox, the limits, the second filter and the pipe-only output.
  - The server binary contains the library, but only `mimik pdf-worker` calls it (the CI check).
- **Build cost and binary size grow.** The DEP-29 gate measures both before any other work.
- **A first filter that is too tight for MuPDF,** or an address-space limit that is too low for
  real manuals. 3a measures both on the valid difficult fixtures and the two local PDFs.
