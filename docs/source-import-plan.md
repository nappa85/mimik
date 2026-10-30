# Source import plan: reading routes and in-app review

Goal: the owner turns a PDF they own into reviewed evidence from the browser, choosing how it
is read, without command-line tools, Docker or JSON files. Today an extraction needs the
prototype containers, `prepare_review.py` and `mimik import-source`, and suggestion reports are
JSON files the review page never shows (`docs/sources-and-retrieval.md`).

What stays: units start `unreviewed`; every accept, correction, rejection and page completion is
an explicit owner decision with actor, rationale and revision (SRC-4, SRC-5); raw extraction
text is immutable; model and tool output is a suggestion only (SRC-6); manuals stay local
except on a route the owner explicitly sends out (route C); weaker sandboxing only with an
owner opt-in (ASSIST-44).

## Decisions this plan needs (owner)

Changes to decisions in force:

- **SRC-1** (no browser uploads): the Owner may upload a PDF from `/admin/materials` (bounded
  size, stored like today's imports). `import-source` stays for prepared bundles.
- **SRC-3** (Mimik never renders PDFs at run time): Mimik renders the selected pages to PNG
  itself, in a sandboxed child process (step 2), because the reader routes need page images.
- **ASSIST-58** (downloads go only to Hugging Face): route B downloads uv and a standalone Python
  from GitHub releases and wheels from PyPI (`files.pythonhosted.org`), each pinned by hash.

New decisions:

- **Reading routes** (owner 2026-10-30): four routes, chosen per import, all producing the same
  unreviewed units: A, the PDF's own text; B, local layout and OCR (Marker); C, an online vision
  model with the owner's key; D, text the owner supplies. A route whose requirements are missing
  is shown disabled with what it needs.
- **Route B runtime** (owner 2026-10-30): Mimik downloads a pinned `uv` binary, which installs a
  pinned standalone Python and a hash-locked environment in Mimik's data folder; no system
  Python is used. Marker runs inside the Hakoniwa sandbox.
- **Owner-added units:** the owner can draw a box on the page image and enter text the reader
  missed (the page 29 footnote case, L-SRC-1). It is stored apart from the raw extraction, marked
  as owner-entered, with actor and rationale, and is reviewed like any other unit.

Open questions:

1. **Batch accept of agreeing readings:** where two independent readings of a unit agree word for
   word, may the owner accept all of them in one action (still their decision, per page,
   listed)? Or one decision per unit, as now?
2. **Route C provider:** which providers to offer first (through rig-core, D-H), and whether
   to black out a user-drawn region (a buyer watermark) on every image before sending.
3. **Route D evidence:** owner text has no page image to compare against. Is it allowed as
   evidence (marked "owner-supplied text", attested by the owner), or only as a cross-check
   reading next to a PDF route?
4. **Dev host and route B:** Mimik installing a Python environment in its own data folder is a
   Mimik-managed runtime, like the llama-server of ASSIST-40. May the dev host run it natively
   for the route-B tests, or must they run in a container?

## Common model (step 1)

- **Reading:** one run of one route over selected pages of one stored PDF, with route,
  versions, settings and hashes recorded. A source can hold several readings of the same pages;
  each is its own set of units (runs are never merged, SRC-2).
- **Primary reading:** the one whose units the owner reviews. Other readings, the text-layer
  cross-check and review reports attach to the primary reading's units as **suggestions**:
  stored apart, with origin and kind (spelling, difference, unread words, merge, missing
  region, reject), never applied.
- **Bundle contract v3:** a reader-neutral raw format (`mimik-reading-v1`: pages, blocks with
  kind, polygon, sanitised HTML, reading order and parent), so routes A, C and D need no
  Marker-shaped JSON. Marker output is converted by the same rules as SRC-10's eligibility.
  v1 and v2 bundles keep importing as now.

## Steps

1. **Review screen with suggestions.** Store suggestions (migration and import of the existing
   report shape). On `/admin/materials/pages/{id}`: the page image beside the units; each
   suggestion as an inline difference with Accept (records `corrected` with the suggested text),
   Edit and Reject; keyboard navigation; the effective text of the page shown in place, so no
   Markdown preview. Owner-added units by drawing a box. Tested with the existing MÖRK BORG IT
   bundle and its two reports.
2. **Upload, rendering and the job runner.** Upload a PDF, pick pages, choose a route; Mimik
   checks it (`lopdf`, today's limits), renders the selected pages in a sandboxed child process
   (MuPDF, AGPL like Mimik, or PDFium; selection recorded as a DEP decision) and runs the reading
   as one background job with progress and cancel, through the same import transaction as
   `import-source`. A malformed PDF fails the job, never the server.
3. **Route A: the PDF's own text.** Blocks, lines and boxes from the text layer, and the tag
   structure (headings, lists, tables) when the PDF is tagged. Pure Rust plus the step-2
   renderer; no models. A quality probe (words per page against image area, fonts without
   Unicode maps, undecodable glyphs) recommends a route before the owner chooses. Measured on
   MÖRK BORG Bare Bones EN (tagged, text layer complete) against a hand-checked reference, and on
   the IT pages against the existing 161-row reference, where it should score like Marker without
   OCR (140/161) and the probe should recommend route B or C.
4. **Route C: online vision model.** The owner's key (stored like other owner secrets, never
   shown back), the provider through rig-core, an explicit per-import confirmation listing pages
   and provider, the optional blacked-out region, a cost estimate before sending. The model reads
   each page image with the text layer as a hint and returns `mimik-reading-v1` blocks, validated
   (schema, polygons inside the page, bounded sizes); invalid output fails that page. Used as the
   primary reading or as suggestions on another reading. Measured on the 161-row reference.
5. **Route B: Marker through uv.** A runtime manifest like `schemas/llama-runtime-v1.json`: uv
   (pinned version and hash per platform), the standalone Python build, `uv.lock` with hashes
   (Marker 2.0.0, surya 0.22.1, CPU PyTorch), the layout model files (pinned revisions, hashes)
   and surya's OCR model served by the existing llama-server runtime. An owner-started setup on
   `/admin/materials` (size shown, about 2 GB, progress, cancel). Marker runs in Hakoniwa with no
   network except a private loopback shared with its llama-server (both in one sandbox), read-only
   environment and models, the rendered pages and an output folder only; a seccomp profile for
   Python and PyTorch. Missing or failed setup disables the route with the reason. The extraction
   prototype's checks (OCR failures fail the run, coverage) move into the job.
6. **Route D: owner text.** Markdown or HTML pasted or uploaded, split into blocks; per question
   3, either a source of its own or a cross-check reading for a PDF source.
7. **Agreement view.** Where a page has two readings, units are marked agree / differ; per
   question 1, batch accept or not.
8. **Docs and gate.** `docs/sources-and-retrieval.md` rewritten for the routes,
   `docs/release-scope.md`, licences and notices (uv, the Python build and every locked wheel
   for route B, which are downloaded, not shipped), the prototype tools retired or kept as
   developer tools, limitations updated.

Order: 1, 2, 3 first (they unblock the prompt tests with Bare Bones), then 4, 5, 6, 7. Each
step is its own reviewed change; steps 4 and 5 each get their own plan review before work starts.

## Risks

- **Rendering and parsing untrusted PDFs** is the largest new attack surface: the renderer and
  route A's parser run in the sandbox, with bounded time, memory and output.
- **Route B's environment** is about 2 GB of third-party code; its licences (Marker's and surya's
  weights under modified OpenRAIL-M, surya's OCR error model CC BY-NC-SA 4.0, not loaded) must be
  shown before setup; PyTorch under the sandbox's seccomp filter is untested.
- **Route C** sends manual pages to a third party; the owner confirms each send, and nothing is
  sent by default.
- **Owner-added units** change the "units equal the raw leaf nodes" invariant: they are a
  separate kind with their own table, never mixed into raw extraction.
