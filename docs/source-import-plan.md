# Source import plan: reading routes and in-app review

Goal: the owner turns a PDF they own into reviewed evidence from the browser, choosing how it
is read, without command-line tools, Docker or JSON files. Today an extraction needs the
prototype containers, `prepare_review.py` and `mimik import-source`; imported suggestion
reports are shown on the review screen (`docs/sources-and-retrieval.md`).

What stays: units start `unreviewed`; every accept, correction, rejection and page completion is
an explicit owner decision with actor, rationale and revision (SRC-4, SRC-5); raw extraction
text is immutable; a source is one finalized run (SRC-2, migration 007); model and tool output is
a suggestion or an unreviewed reading, never a decision (SRC-6); manuals stay local except on a
route the owner explicitly sends out (route C); weaker sandboxing only with an owner opt-in
(ASSIST-44).

This is the umbrella plan. Steps 2, 3, 5 and 6 each get their own plan and plan review before
work starts; steps 1 and 4 are specified here.

## Decisions this plan needs (owner)

Changes to decisions in force:

- **SRC-1** (no browser uploads): the Owner may upload a PDF from `/admin/materials` (step 3).
  `import-source` stays for prepared bundles.
- **SRC-3** (Mimik never renders PDFs at run time): Mimik renders selected pages itself, in a
  sandboxed child (step 3).
- **ASSIST-28, ASSIST-35, ASSIST-58** (downloads only from Hugging Face): route B also downloads
  from GitHub releases (uv, the standalone Python), PyPI (`pypi.org`, `files.pythonhosted.org`),
  `download.pytorch.org` (CPU PyTorch; PyPI's Linux wheels pull several GB of CUDA wheels) and
  the hosts Marker and surya fetch their layout weights from (to be verified in step 6; possibly
  `models.datalab.to`), each artifact pinned by hash.
- **AGENTS.md** ("Rust is the shipping language", "model runtimes in containers"): route B is a
  Mimik-managed Python runtime downloaded at the owner's request, never shipped in the release.
- **REL-2** (shipped binaries come with their source): settled by the renderer choice in step 3
  (MuPDF built from source keeps REL-2; prebuilt PDFium would be a new exception).

New decisions:

- **Reading routes** (owner 2026-10-30): four routes, chosen per import: A, the PDF's own
  text; B, local layout and OCR (Marker); C, an online vision model with the owner's key, each
  producing an unreviewed source; D, text the owner supplies, producing suggestions only. Route B and C output are raw unreviewed
  units on the same footing as Marker's machine-learning output today, with provider, model,
  versions and prompt hash recorded as provenance. A route whose requirements are missing is
  shown disabled with what it needs. Route B needs Hakoniwa, so it is Linux-only for now.
- **Route B runtime** (owner 2026-10-30): a pinned `uv` installs a pinned standalone Python and a
  hash-locked environment in Mimik's data folder; no system Python.
- **Documents and readings:** a source stays one finalized run. Several runs of one PDF are
  several sources sharing a new `source_documents` row (the PDF stored once, keyed by hash). Primary
  is chosen **per document page**: each page of a document has at most one primary source, so
  overlapping ranges never return the same page twice; lookup and coverage use only the primary
  source of each page. Changing it is an explicit owner decision with actor, rationale and an
  authenticated sidecar like review events; it changes which reviewed units are used, not their
  state.
- **Owner-added units:** text the reader missed, entered by the owner on a box drawn on the page
  image. They are rows of `source_units` with `origin = 'owner'` (raw extracted units are
  `origin = 'extracted'`), so review history, completeness, the revision cascade, search and
  citations cover them unchanged. The 007 triggers are rewritten to allow inserting an
  owner unit into a finalized source and nothing else; raw-leaf equality applies to extracted
  units only. Adding one resets page completeness. Entering it is the owner's decision, in one
  transaction: the unit is inserted `unreviewed` with its search projection (017 refuses
  inserting reviewed units), then corrected through the normal revision, event and sidecar path,
  so its history starts like any other unit's. Nothing else can create one; `origin` never
  changes, and extracted identities cannot be inserted or replaced this way.
- **Suggestions:** stored apart from units, bound to the exact source (bundle hash) they were
  made for; a report for another bundle is refused. Each targets one unit, several units (merge)
  or a page region (missing text). Accepting maps to existing decisions: a correction records
  `corrected`; accepting a merge atomically creates one corrected owner unit with the targets'
  bounding rectangle and rejects every target, with one rationale and every expected revision;
  a missing-region suggestion pre-fills an
  owner-added unit; a reject suggestion records `rejected`. Dismissing a suggestion is not a
  decision on the unit. A suggestion made on an older unit revision is shown as stale. Open
  suggestions do not block page completion.

- **Watermark exclusion** (owner 2026-10-30): at import the owner can exclude a buyer watermark
  easily. Mimik proposes one when the same text appears at the same place on most selected pages
  (for example a name and order number in the margin), shown highlighted on a page; the owner
  confirms it, adjusts the box or draws one, once for all pages. The exclusion is recorded
  with actor, rationale, pages and geometry, and has three effects. Raw output is never filtered:
  extracted units wholly inside the box stay in the reading and are rejected through attributed
  owner decisions citing the exclusion (listed, each undoable); a unit crossing the box's edge is
  left unreviewed and flagged. Route A and B's text hints and every image and hint sent to route
  C are blacked out inside the box before any reading, and that redaction is recorded with the
  reading's provenance (content the provider never saw gets no rejected-unit history). Stored page images stay
  the original renders (evidence); the review screen shows the region masked.
- **Batch accept** (owner 2026-10-30): where two readings agree (the agreement rule `agreement-v1`, built: `docs/sources-and-retrieval.md`), the
  owner may accept all agreeing units of a page in one action; each is recorded as its own
  decision and listed. Disagreements stay one by one.
- **Multi-unit decisions** (merge, exclusion, batch accept): one transaction checks every expected
  revision first, then for each unit writes the update, its `source_review_events` row and its
  `source_web_events` sidecar in turn (015 attributes only the latest event), all sharing one
  operation id and rationale. Any stale revision or attribution failure rolls back all of it;
  tests cover both.
- **Route C providers** (owner 2026-10-30): Anthropic and OpenAI first, through rig-core.
- **Route D** (owner 2026-10-30): owner-supplied text is a cross-check only: it produces
  suggestions on a PDF reading and is never evidence of its own.
- **Dev host** (owner 2026-10-30): the dev host may run Mimik's route-B runtime natively, in
  Mimik's data folder and sandbox, for its tests (as ASSIST-40 allows for llama-server).

## Steps

Steps 1 (the schema: documents, readings, primary pages, owner units, suggestions) and 2 (the
review screen: report import, suggestions, merges, owner-added text, primary choice) are done;
they are described in `docs/sources-and-retrieval.md`. Numbering is kept so references stay
valid.

3. **Sandboxed PDF work, upload and jobs** (own plan). First the runner generalisation: launch
   kinds besides llama-server (a re-exec of `mimik` as the PDF worker), each with its own
   seccomp, Landlock, rlimits and wall-clock limit. Every parse of an uploaded PDF (the `lopdf`
   checks, page enumeration, rendering, route A's text) runs in the worker, which returns
   bounded, validated results; the server never parses the uploaded PDF. Renderer: MuPDF through
   a maintained crate, built from source (selection, CI build cost and REL-2 recorded as a DEP
   decision). The watermark proposal (repeated text at the same place) and the exclusion box are
   part of the upload flow. Upload: its own route with a streaming 128 MiB limit, Owner, CSRF and REL-5 checks,
   spooled to a private file, a staging table outside 007 with cleanup of abandoned uploads, one
   upload and one job at a time. Jobs: progress, cancel (kills the worker tree), and a clean
   failure across a restart. A hostile-PDF corpus (deep nesting, decompression bomb, xref loop,
   huge page count) fails the job, never the server.
4. **Route A: the PDF's own text.** MuPDF structured text (blocks, lines, glyph boxes) and the tag
   structure when the PDF is tagged, in the step-3 worker. Units are derived from the raw reading
   by one function that the tests pin (the "units equal raw leaves" check becomes that function's
   contract, contract v3, `mimik-reading-v1`). A probe (words per page against image area, fonts
   without Unicode maps, undecodable glyphs) recommends a route. Bars, set before measuring:
   - MÖRK BORG Bare Bones EN (local, licence and hash recorded in the corpus table): a
     hand-checked reference of the weapons, armour, equipment and character-creation pages (rows
     counted as in the IT reference); at least 98% of rows intact in one unit, no digit, dice or
     sign error.
   - MÖRK BORG IT pages 22–39: the probe recommends route B or C on every art-heavy page (all
     18), and route A's scores are reported, with no bar.
   - Ten synthetic hostile or odd PDFs (rotated, no text layer, broken tags): no crash, correct
     probe result.
   Own plan: `docs/source-route-a-plan.md`.
5. **Route C: online vision model** (own plan). Key storage (a 0600 file outside the database and
   backups, never shown back, replace and delete) and an HTTPS transport for rig-core (a DEP
   decision; DEP-27's rustls stack, redirects refused, bounded responses, keys redacted from
   logs); verify the pinned rig-core sends images and the structured output, else stop and ask
   (D-H). Each send shows a preview of exactly what goes out (pages, hints, provider, model), and
   the confirmation is bound to the payload hashes; the PDF itself is never sent. The watermark exclusion is
   applied to images and hints alike. Output validated (schema, polygons, sizes);
   invalid output fails that page. Bar: on the IT 161-row reference, at least the 145 rows of the
   current best route, with no digit, dice or sign error; cost per page reported.
6. **Route B: Marker through uv** (own plan). A manifest of every artifact (uv per platform, the
   Python build, wheels, layout weights, surya's OCR model and its projector), each pinned by
   hash. Setup is owner-started (size shown, progress, cancel), runs confined with network only to
   the listed hosts, installs offline from verified files with `uv sync --frozen --no-build`, and
   publishes the environment atomically. Runtime auto-downloads off; the CC BY-NC-SA error model is
   never fetched. Marker runs in its own sandbox kind (threads and processes allowed as PyTorch
   needs, no network); the OCR llama-server runs in the existing sandbox kind on its Unix socket,
   bind-mounted into Marker's (or the minimal change if surya cannot use a socket), and takes
   turns with the assistant's server through the model-work gate. Code licences of Marker and
   surya re-verified before setup shows them. The prototype's checks (OCR failures fail the run,
   coverage) move into the job. Bar: the IT reference scores of the prototype route (145/161 with
   the text-layer cross-check).
7. **Route D: owner text.** Markdown or HTML split into blocks, aligned to a PDF reading by the
   agreement rule (`agreement-v1`), giving suggestions only, bound to the
   target source's bundle hash and unit revisions, with the same stale and decision rules.
9. **Docs and gate.** `docs/sources-and-retrieval.md` rewritten for the routes,
   `docs/release-scope.md` (route B Linux-only), notices, the prototype tools retired or kept as
   developer tools, limitations updated; migration, browser, hostile-input and unpacked-release
   checks in the gate. Manual-derived fixtures stay local; CI uses synthetic ones.

Order: 1, 2, 3, 4 first (Bare Bones then needs no prototype tools), then 5, 6, 7 (step 8, agreement and batch accept, is built). Each step is
its own reviewed change. Results that drive a product decision get an independent review.

## Risks

- **Untrusted PDFs** are the largest new attack surface: all parsing moves to the sandboxed
  worker, with bounded time, memory and output.
- **Route B's environment** is about 2 GB of third-party code under the owner's control; Python
  and PyTorch under a seccomp filter are untested.
- **Route C** sends manual pages to a third party; nothing is sent without a confirmed preview.
- **Owner-added units** relax 007's frozen membership by exactly one kind of insert; the rewritten
  triggers are the boundary and get their own tests.
