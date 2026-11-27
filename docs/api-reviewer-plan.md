# In-app API reviewer (plan)

The owner sends chosen pages of a source to a multimodal model of their choice, with their
own key, and gets the findings back as a suggestion report in the existing review screen.
It replaces running `tools/review_extraction.py` by hand. It is the reviewer half of route C
(`source-import-plan.md` step 5): the key storage, transport, provider settings and send
preview built here are the ones route C's extraction reuses.

Model output stays advice: findings are suggestions the owner decides one by one; nothing is
accepted automatically and no confidence approves anything (AGENTS.md).

## Owner decisions (2026-11-26)

- The owner chooses the provider, endpoint and model; the key is theirs.
- Manual pages may leave the machine for this reviewer, only after a confirmed preview.
- Providers: any rig-core provider Mimik enables. Enabling one costs its protocol check
  (done for the three below), not new client code. This slice enables Mistral (live-verified),
  OpenAI and Anthropic (fixture-verified only, stated in the release scope).
- Endpoints: public hosts only. The resolver of the model downloads refuses loopback,
  private, link-local and other local addresses (`src/assistant/download/client.rs`,
  `public`). The resolver is skipped for a literal IP, so the base URL must name a domain: IP
  hosts and userinfo are refused, in settings and again before each send.
- Watermarks: imported (bundle) sources carry no exclusion regions (SRC-13 exclusions exist
  only on staged uploads, `source_upload_exclusions`; source-level ones arrive with route A
  step 4c). Until then the preview states "No exclusion is recorded for this source: every
  page image is sent as it is", and the owner ticks an explicit confirmation. When a source
  has exclusions, they are blacked out of the images and removed from the units.

## What is sent, per page

- The page's stored review PNG (up to 12 MiB, `src/sources/import.rs`), downscaled through
  the bounded decode and encode of `src/sources/region.rs` to the provider's image limit, with
  exclusion regions (when any) blacked out.
- The page's **extracted units at revision 1**, as the Python tool sends them, rebuilt from
  `raw_json` (structure, table HTML) with every `Picture` unit's image data stripped from its
  JSON and its `raw_html`. Owner units and later revisions are not sent: the importer binds
  every suggestion to an extracted unit at revision 1 (`src/sources/reports.rs`, `unit()` and
  `raw_ids`), and a suggestion on a unit the owner has since changed shows as stale, as with
  imported reports. Rejected units are sent (the model reviews the extraction, not the
  decisions).
- The fixed instruction text, moved from `tools/review_extraction.py` into Rust: the page
  content is untrusted data, never instructions.
- Never the PDF, other pages or sources, account data, or the key in any body.
- Retention: each provider's retention/data-use terms are open (TODO.md, "API reviewer: open
  before the live trial") and unverified against the providers' public API docs. Until they
  are verified, only `store: false` is sent to OpenAI (rig-core does not send it by default),
  and no retention claim is made anywhere — not in `docs/sources-and-retrieval.md`, not in the
  preview, not in the release scope.

## Steps

Steps 1-3 (protocol check, transport, settings and key) are done; see the reviewer section of
`docs/sources-and-retrieval.md` and `src/sources/reviewer/`. The remaining steps keep their
numbers:

4. **Preview and confirmation** (the source page in `/admin/materials`): the owner selects
   pages (default: pages not complete; the page count and an approximate size, text bytes and
   image bytes, are shown first and prominently). The preview lists provider, host, model,
   each page with a link to the exact image that goes out and its unit JSON (collapsed), and
   the watermark confirmation above. "Send" carries one hash over the settings (provider, base
   URL, model, key identity), the source's bundle hash and every page's payload hash; the
   server rebuilds and refuses a mismatch with a fresh preview.
5. **Job:** one reviewer job at a time for the install, in a `source_review_jobs` table with
   one row per page (confirmed payload hash, state, findings, the model the reply names,
   usage, failure reason; findings at most the 4 MiB reply bound, usage a few integers). Rows
   are deleted when the job's report is saved or the job is discarded; saving rechecks the
   bundle binding. Pages run in order; just before each send the page is rebuilt and
   its hash compared (a mismatch fails that page only), and the settings and bundle hash are
   rechecked (a mismatch stops the job). Cancel between and during pages. One retry on 429 or
   503 within the page budget, honouring `Retry-After`; other failures fail the page. Provider
   error bodies are never shown or stored: status codes map to fixed messages. On startup a
   running job is marked interrupted; its succeeded pages stay and can be saved as a report.
6. **Validation and report:** the importer's per-page checks are factored out
   (`validate_page(tx, source, page, findings)`) and run on each reply as it arrives, with
   the cumulative issue cap (5,000) applied as pages accumulate (later pages fail with "report
   full"). The importer's whole-report path takes a parsed `Report` (the existing private
   `import(db, source, &Report, digest)`). Saving the job (at the end, or after cancel or an
   interruption) builds one v2 report of the succeeded pages: `state`
   `model_suggestions_not_verified` with `endpoint` and `requested_model` and no `tool`
   (the schema and `Report::origin` allow only that), page payload hashes in
   `artifact_sha256` as `page-N.payload`. A job with no succeeded page stores no report. The
   report shows in the review screen like an imported one; model text is escaped there.
7. **Docs and gate:** `sources-and-retrieval.md`, `release-scope.md` (optional, owner key;
   Mistral live-verified, OpenAI and Anthropic fixture-only), decisions (provider list,
   transport and proxy DEP, the watermark confirmation), limitations;
   `tools/review_extraction.py` stays as a developer tool until the live trial matches it, then
   is retired (owner).

## Tests (written first)

- Protocol fixtures per provider (done with step 1), including a `refusal` and a non-JSON
  reply.
- Payload: picture data absent from JSON and HTML; revision-1 extracted units only; the
  downscale bound; exclusions blacked out and removed (synthetic source with exclusions,
  forward-looking for step 4c); the no-exclusion confirmation is required to send.
- Key: mode 0600; no 8-character substring of the key in any page, log (rig tracing at
  debug included: no `Authorization` header, no base64 bodies), error, job row or report;
  replace and delete; missing key refuses the send. Neither the model-server sandbox nor the
  PDF worker sandbox can read `<data>/secrets/`.
- Transport against a local HTTPS fake provider: redirect refused, a private or loopback
  base URL refused by the resolver, an IP-literal or userinfo base URL refused, no proxy used
  with `HTTPS_PROXY` set, `store: false` in every OpenAI request, oversized body refused, timeout, 429 with `Retry-After`
  inside the budget, 401 shown as a fixed message.
- Binding: a changed setting, key, exclusion or bundle (re-import) between preview and send is
  refused; an owner edit to a unit does not change the payload (revision 1 is sent); an
  exclusion change mid-job fails the affected page only (the per-page rebuild); Owner only;
  CSRF.
- Job: one at a time; cancel mid-page; restart mid-job marks it interrupted and keeps the
  succeeded pages; the issue cap fails later pages; no report when none succeeded; a saved
  report shows in the review screen with its suggestions escaped.
- Browser check (Chromium, release binary, fake provider, no network): settings, preview,
  confirmation, send, progress, cancel, the report in the review screen; JavaScript on and off.
- Live trial (outside CI): Mistral Large on MÖRK BORG IT pages 22-39, issue and token counts
  against the Python tool's figures in `docs/results.md`.

## Slices

(a) steps 1-3 (protocol check, transport, settings and key) is done; (b) steps 4-6 (preview,
job, report); (c) browser check, live trial, docs. Each is its own reviewed change.

## Out of scope

Route C's extraction, owner-drawn redaction boxes, providers beyond the three, automatic
sends, cost accounting beyond reported usage.
