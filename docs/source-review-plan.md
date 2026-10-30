# Source review screen plan (source import plan, step 2)

Goal: the Owner reviews a reading of a page in the browser, with the page image, the units and
every stored suggestion side by side, and records each decision without command-line tools,
JSON files or copying text by hand. Umbrella: `docs/source-import-plan.md` (step 2). Schema
from step 1 (migration 024): `docs/sources-and-retrieval.md`.

What stays: every evidence change is an explicit, attributed Owner decision with a rationale
and an expected revision (SRC-4, SRC-5); suggestions never decide (SRC-6); raw HTML is
immutable; the pages work without JavaScript; the strict CSP stays (scripts only from
`/assets`, no inline script); manual-derived data stays local.

Out of scope: PDF upload, rendering and the watermark exclusion (step 3); readers (steps 4–6);
the agreement view and batch accept (step 8). The masked watermark display comes with step 3.

## Decisions this plan needs

- **Report import is a command for now** (no SRC-1 change): `mimik import-suggestions --source
  ID --report FILE`. Reports come from the prototype tools, which are command-line tools today;
  the in-app readers (steps 5–7) store their suggestions directly. A browser report upload, if
  still wanted, comes with step 3's upload route.
- **Report schemas** (closed, `schemas/suggestion-report.v1.json` and `.v2.json`). v1 is written
  from what both tools emit today: `tools/review_extraction.py` (with `artifact_sha256`, failed
  pages carrying `error`, `render_sha256`, `returned_model`, `usage`) and
  `tools/crosscheck_text_layer.py` (with `bundle_sha256`). v2 adds `bundle_sha256` as required
  and an explicit `action` per issue (`correction`, `reject`, `merge`, `missing_text`, `note`)
  with `merge_block_ids` and `region` (four points, PDF points). Both tools move to v2. A v1
  issue maps only by shape (below); its free-text `kind` never selects an action.
- **A `note` suggestion kind**: advice that decides nothing, on one unit or on the page, with
  optional text shown as quoted (the reports' unread words, differences, needs-visual-review
  findings, and v1 corrections that name no unit). A note has no Accept.
- **Merge acceptance makes one owner unit.** Accepting a merge adds one owner-added unit holding
  the merged text, with the axis-aligned bounding rectangle of the merged units' polygons
  (inside the page by construction) as its region, and rejects every merged unit, in one
  transaction. So the evidence unit's region covers its text, and its crop shows all of it.
  The rectangle may overlap other units; the overlay shows that.
- **Suggestion state is recorded, not inferred.** A suggestion is *open*, *applied* (an
  immutable link to the Owner decision that applied it), *dismissed*, or *stale* (open, and a
  target unit's revision has moved past the one it was made on). A revision change alone never
  makes a suggestion applied.
- **Dismissal** needs no expected revision (it changes no evidence) and takes an optional reason
  (≤5,000); it is recorded with actor, time and the authenticated sidecar.

## Schema (migration 025)

`source_suggestions` is rebuilt (SQLite cannot alter a column CHECK, and an added `report_id`
must be covered by the update guard): create the new table, `INSERT … SELECT` keeping ids,
drop the old one, rename, and recreate the index and every trigger.
- `kind` gains `note`. The envelope trigger allows a note on `{"type":"unit",…}` or
  `{"type":"page","page":<page id of this source>}`, with nullable text; all checks stay
  fail-closed on missing or null JSON fields (step 1's `coalesce` rule). Dismissal reason column
  (nullable, ≤5,000, no NUL), set only with the dismissal.
- `report_id` (nullable): on insert it must name a report of the same source; immutable (in the
  update guard).
- `source_suggestion_reports`: id, source, report SHA-256 (unique per source), origin, schema
  version, binding used (`bundle` or `extraction`), report `created_at` as given, imported time,
  counts (issues by action, failed pages). Immutable and undeletable.
- `source_suggestion_applications`: suggestion id (unique), the review event id or operation id
  that applied it, the owner unit id when one was created, whether the Owner edited the text.
  Insert-only, and only in the same transaction as the decision it names (a trigger requires the
  event to be the latest, as 015 and 024 do for sidecars).
- `source_suggestion_web_events`: the authenticated sidecar for a dismissal, bound to the latest
  dismissal like 024's primary sidecar.
- Tests: a schema-24 database with suggestions keeps every row and id (`report_id` NULL); the
  new envelope cases; report, application and sidecar immutability; a report of another source
  refused as `report_id`; the rewind helper.

## Report import (`Store::import_suggestion_report`)

- Read with `local_file::read_path` (the final path component is opened `O_NOFOLLOW`; regular
  files only; bounded read): at most 4 MiB and 5,000 issues. Tests: symlink and FIFO refused.
- **Binding** (whole report refused otherwise; the binding used is recorded): `source_sha256` must
  equal the source's PDF hash, and either `bundle_sha256` equals its `bundle_hash`, or (v1 only)
  `artifact_sha256["source.json"]` equals the SHA-256 of its stored raw extraction. A report bound
  to source A and imported for source B is refused (test).
- **Pages:** every report page must be a page of this source, else the report is refused. Failed
  pages import no issues and are counted.
- **v1 mapping by shape:** a `block_id` naming an extracted unit on that page with a correction is
  a `correction`; a unit without a correction is a unit `note`; a `block_id` naming a raw node
  that is not a unit (a container, the page) is a page `note` that names the block id; a null
  `block_id` is a page `note`, keeping any suggested text. A `block_id` not in the raw tree of that
  page refuses the report. `description` becomes the rationale; an empty one refuses the report
  with the page and issue index.
- **v2:** the `action` must agree with the target shape and text (the SQL envelope rules); merges
  need 2–1,024 distinct units on one page; regions pass step 1's polygon rules.
- Unit targets are recorded at revision 1, since a report is made on the raw extraction; a unit
  decided since then shows the suggestion as stale.
- Origin is `tool:<name>` or `reviewer:<requested model>` from the report (≤100 bytes), shown as
  the report's claim. One transaction (5,000 inserts and their triggers; the synthetic-fixture
  test records its duration in the merge message); the report hash makes a repeat import a no-op
  that says so.

## Review screen (`/admin/materials/pages/{id}`)

Server-rendered (askama), progressive enhancement through one new asset,
`assets/source-review.js`, served like `source-regions.js`. The authenticated store wrappers
for owner units, atomic decisions and primary choice (today `#[cfg(test)]` in
`src/sources/materials.rs`) become live and are the only browser write paths, each rechecking
Owner and CSRF in its IMMEDIATE transaction.

1. **Page image with unit overlay.** An SVG overlay of every unit on the page (not only the
   current card page), drawn from the polygons in PDF points with the page size as the
   `viewBox`, so alignment does not depend on the render scale; at most 2,000 units per page,
   beyond that the overlay is omitted with a note. Each box has an accessible name and links to
   its card, with `?units=<cursor>` when the card is on another card page. Zoom (fit width,
   100%, 200%) needs the script; the full-size image link stays.
2. **Text shown.** Each card shows the rich extraction (sanitised HTML) and, separately, the
   lookup text: the search projection for `accepted`, the correction for `corrected`, nothing for
   `rejected`, and "not evidence yet" for `unreviewed`. A page panel lists the lookup text of the
   current card page in unit order (bundle order; owner-added units last), labelled as not
   evidence until the page is complete and, for a non-primary reading, as not used by lookup.
3. **Suggestions on the card**, with their state, origin, report and rationale. For text, a
   word-level difference between the unit's current lookup text and the suggested text, with
   `similar` (licence and maintenance checked and recorded at selection; `TextDiff::configure().deadline(…)`, Unicode words; recorded as a
   DEP decision): plain text in, every segment escaped by the template, only fixed `<ins>`/`<del>`
   markup out. Bounds: at most 20 suggestions per card and 100 differences per response, each
   with a 50 ms deadline, computed off the async executor; past a bound or the deadline, both
   texts are shown in full instead.
   - **Accept** posts to `POST /admin/materials/suggestions/{id}/accept` with the Owner's
     rationale (required; prefilled "Accepted suggestion #N (origin)", editable) and, from Edit,
     an optional edited text. In one IMMEDIATE transaction the server loads the suggestion,
     checks Owner, CSRF, source, page, kind (not a note), that it is open, and that every target
     unit's current revision equals the one the suggestion was made on; it then applies the
     decision through the existing paths (correction: `corrected` with the stored or edited text;
     reject: `rejected`; merge: below; missing text: an owner unit), writes the review sidecars and
     the application row. The client never supplies the suggested text, only an edit of it.
   - **Edit** opens the accept form with the suggested text in an editable field; submitting it
     records the application as edited.
   - **Dismiss** records the dismissal with an optional reason.
   - A stale suggestion shows as such: Accept and Edit are disabled, Dismiss stays, and the
     normal unit form is still there for the Owner's own decision.
4. **Merge** is shown on every merged unit's card, naming the others, with their boxes
   highlighted. Accept is one transaction: every target revision checked, one owner unit with
   the merged text and the bounding rectangle, every merged unit `rejected`, one operation id and
   rationale.
5. **Missing text and owner-added units.** A `missing_text` suggestion's region is drawn on the
   image; Accept (or Edit) adds the owner unit with its region and text. The "Add missing text"
   form takes a drawn box (pointer drag, arrow-key adjustable handles) or four corners in PDF
   points without JavaScript, the text and a rationale. Page notes are listed in the same panel.
6. **Primary reading.** The source's page list shows each page's primary state (this reading,
   another reading, needs a choice), and an Owner form chooses the primary reading for selected
   pages with expected tokens and a rationale. The search page's unresolved badge links to it.
7. **Navigation.** "Next unreviewed unit" and "next page needing work" links. With the script,
   `j`/`k` move between cards and `a` moves focus to the focused card's accept rationale (never
   submits), only while focus is inside the review list and not in a form field or editable
   element; a help panel lists them, with an off switch kept per browser. Visible focus
   throughout; validation errors announced (`role="alert"`).
8. **Drafts kept** on 409/422 for every form, keyed by form kind and target (unit, suggestion,
   page, merge), and re-rendered on the matching card or panel, which the response links to
   (`?units=<cursor>#…`).

## Tests

Rust (store and handlers, synthetic fixtures only):
- Migration 025, the rebuild keeping rows and ids, the rewind; envelope, report, application and
  sidecar rules in direct SQL.
- Report import: both bindings; hash mismatch, a report of another source, a page outside the
  source refused; v1 mapping for unit, container, page and null block ids; an unknown block id
  and an empty description refused with the issue index; v2 actions; over-limit sizes; symlink
  and FIFO; repeat import a no-op; a free-text v1 `kind` of "reject" or "merge" stays a note.
- Accept: Owner and CSRF; a stale suggestion refused even with a fresh unit revision in the form;
  a stale second merge target rolls back everything; a dismissed suggestion and a note refused;
  the stored text used, not a client value, unless edited (then recorded as edited); the
  application row and sidecars written; an unrelated decision makes a suggestion stale, never
  applied; two competing suggestions: applying one leaves the other stale.
- Dismissal: no change to units, history, search or completeness; repeat dismissal conflicts.
- Difference renderer: HTML and entity payloads escaped; only `<ins>`/`<del>`; repetitive
  worst-case input hits the deadline and falls back; the per-response bound.
- Drafts for each form on 409 and 422.

Browser (`tools/material_browser.py` in the Chromium container, the synthetic fixture with a
generated v2 report and more than 50 units on a page, 1280×720 and 390×844, with and without
JavaScript):
- Overlay boxes within 1 CSS pixel of the expected rectangles at each zoom; links to cards on
  another card page.
- Accept, edit, dismiss, a stale suggestion, a merge, drawing a box with pointer and keyboard,
  choosing a primary reading, `j`/`k`/`a` and the off switch, a 409 keeping each draft.
- No horizontal scroll at 390 px; no CSP violations in the console.

Local live check (not in CI): the MÖRK BORG IT bundle with its two reports (both carry its
`bundle_sha256`) imported into a scratch database, a sample of pages reviewed through the
screen, and the imported counts per action in the merge message.

## Order

1. Migration 025 and the report import command (with the v1/v2 schemas and the tools' v2 output).
2. Screen: overlay, text, suggestion actions, merge, add text, primary choice, navigation.
3. Browser checks and the live check; `docs/sources-and-retrieval.md` updated; this plan deleted
   and the umbrella's step 2 marked done.

Each is a reviewed change set (Sol and Opus).

## Risks

- **The overlay misleads** if polygons and the image disagree: the browser check pins alignment,
  and the crop previews stay as an independent view.
- **One-click accept** makes approving model text easy: Accept needs the Owner's rationale
  (prefilled, editable, required), stays per suggestion, and there is no "accept all".
- **Report claims** (origin, model) are unverified and shown as claims.
