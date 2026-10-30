# Source review screen plan (source import plan, step 2)

Goal: the Owner reviews a reading of a page in the browser, with the page image, the units and
every stored suggestion side by side, and records each decision without command-line tools,
JSON files or copying text by hand. Umbrella: `docs/source-import-plan.md` (step 2). Schema
from step 1 (migration 024): `docs/sources-and-retrieval.md`.

What stays: every state change is an explicit, attributed Owner decision with a rationale and
an expected revision (SRC-4, SRC-5); suggestions never decide (SRC-6); raw HTML is immutable;
the pages work without JavaScript; the strict CSP stays (scripts only from `/assets`, no
inline script or style attributes that need `unsafe-inline`); manual-derived data stays local.

Out of scope: PDF upload, rendering and the watermark exclusion (step 3); readers (steps 4–6);
the agreement view and batch accept (step 8). The masked watermark display arrives with step 3.

## Decisions this plan needs

- **Report import is a command for now** (no SRC-1 change): `mimik import-suggestions --source
  ID --report FILE`. Reports come from the prototype tools, which are command-line tools today;
  the in-app readers (steps 5–7) store their suggestions directly. A browser report upload, if
  still wanted, comes with step 3's upload route.
- **Report schema v2** (`schemas/suggestion-report.v2.json`, closed): v1 plus an explicit
  `action` per issue (`correction`, `reject`, `merge`, `missing_text`, `note`), `merge_block_ids`
  and `region` (four points, PDF points). A v1 issue maps only by shape: a block and a correction
  is a `correction`; anything else is a `note`. The free-text `kind` never selects an action.
- **A `note` suggestion kind** (migration 025): advice without text that decides nothing, on one
  unit or on the page (the reports' "unread words", "differences" and "needs visual review").
  Today these cannot be stored.

## Schema (migration 025)

- `source_suggestions.kind` gains `note`; the envelope trigger allows a `note` on `{"type":"unit"}`
  or `{"type":"page","page":<page id of the source>}` with `suggested_text` NULL. Tests extend the
  step-1 direct-SQL cases (missing and null fields, a page of another source).
- `source_suggestion_reports`: id, source, report SHA-256 (unique per source), origin
  (tool or reviewer and model), schema version, created/imported times, issue counts by action.
  Immutable and undeletable. `source_suggestions.report_id` (nullable, immutable) links each
  suggestion to the report it came from.
- `source_suggestion_web_events`: the authenticated sidecar for a dismissal (account, epoch,
  session digest), like `source_web_events`. Dismissal through the browser rechecks Owner and
  CSRF in the IMMEDIATE transaction that writes it.
- Rewind helper and migration tests as in step 1 (a schema-24 database with suggestions keeps
  them; each has `report_id` NULL).

## Report import (`Store::import_suggestion_report`)

- The file is read with `local_file::read` (no symlinks or special files), at most 4 MiB, parsed
  with `deny_unknown_fields` against the v1 or v2 schema; at most 5,000 issues.
- Admission: `bundle_sha256` must equal the source's `bundle_hash` and `source_sha256` its PDF
  hash; otherwise the whole report is refused. `tools/review_extraction.py` starts writing
  `bundle_sha256` (it writes only `artifact_sha256` today), with a helper test.
- Each `block_id` must name an extracted unit on that page of the source; a report is
  made on the raw extraction, so its unit targets are recorded at revision 1, and a unit since
  decided shows the suggestion as stale. A merge needs 2–1,024 distinct units on one page; a
  region must pass the step-1 polygon rules. Any invalid issue refuses the whole report, with
  the page and issue index in the error (no partial import).
- One transaction; the report hash makes a repeat import a no-op that says so.
- Origin is `tool:<name>` or `reviewer:<model>` from the report, at most 100 bytes, recorded as
  given (it is the report's claim, shown as such).

## Review screen (`/admin/materials/pages/{id}`)

Server-rendered (askama), progressive enhancement through one new asset,
`assets/source-review.js`, served like `source-regions.js`.

1. **Page image with unit overlay.** The image sits in a figure with an SVG overlay drawn from the
   unit polygons in PDF points (the manifest's page size is the `viewBox`, so alignment does not
   depend on the render scale). Each box links to its unit card (`#unit-<id>`), and the card
   highlights its box (`:target` and focus, CSS only). Zoom: fit width, 100% and 200%, through
   buttons that change a class; the full-size image link stays. Without JavaScript the overlay
   and the links still work; only zoom needs the script.
2. **Effective text in place.** Each card shows the text lookup would use: the correction for
   `corrected`, the sanitised raw HTML for `accepted`, nothing for `rejected` (raw shown
   collapsed), and `unreviewed` marked as not yet evidence. A page panel lists the page's
   effective text in reading order, as lookup will see it once the page is complete.
3. **Suggestions inline.** Each open suggestion on a unit shows its origin, report, rationale,
   and for text a word-level difference between the unit's current effective text and the
   suggested text (computed server-side; insertions and deletions marked with `<ins>`/`<del>`,
   not colour alone). Actions:
   - **Accept** posts the normal unit decision (`corrected` with the suggested text, or
     `rejected` for a `reject`), with the expected unit revision and an Owner-written rationale
     prefilled with "Accepted suggestion #N (origin)" and editable. The suggestion is not marked
     by accepting; it shows as resolved because the unit's revision moved past it.
   - **Edit** opens the unit's decision form with the suggested text in the correction field.
   - **Dismiss** records the dismissal (actor, time, sidecar); it changes nothing on the unit.
   - A stale suggestion (made on an older revision) shows as such: Accept is disabled, Edit and
     Dismiss stay.
   - A `note` has only Dismiss.
4. **Merge.** Shown on the first unit's card with the other units named and their boxes
   highlighted. Accept is one `review_units_atomic` decision: the first unit `corrected` with the
   merged text, the others `rejected`, one rationale, every expected revision.
5. **Missing text and owner-added units.** A `missing_text` suggestion is shown on the page with
   its region drawn on the image; Accept pre-fills the "Add missing text" form (region and text,
   both editable) which calls `add_owner_unit`. The same form lets the Owner draw a box on the
   image (pointer drag, keyboard-adjustable handles) or type the four corners in PDF points
   without JavaScript. Page-level notes are listed in the same panel.
6. **Primary reading.** The source's page list shows each page's primary state (this reading,
   another reading, needs a choice), and an Owner form chooses the primary reading for selected
   pages through the existing Owner-bound `set_primary_source` wrapper, with expected tokens and a
   rationale. The search badge links to it.
7. **Navigation.** "Next unreviewed unit" and "next page needing work" links; `j`/`k` move between
   unit cards and `Alt+A` accepts the focused suggestion, only when focus is not in a form
   field, all listed in a help panel and never the only way.
8. **Drafts kept.** Every new form re-renders with its draft on 409/422 (the existing
   `TrustedDraft` pattern), including the merge and add-text forms.
9. **Bounds.** Suggestions per unit card ≤20 with a link to the rest; page-level panel paged at
   50; difference input ≤20,000 scalars each side with a fallback to showing both texts.

## Tests

Rust (handler and store, synthetic fixtures only):
- Migration 025 and the rewind; the `note` envelope cases; report table immutability.
- Report import: v1 and v2 mapping per shape; bundle and PDF hash mismatch refused; unknown
  block, block on another page, bad region, over-limit sizes refused whole with the issue index;
  repeat import a no-op; v1 free-text kinds never map to `reject` or `merge`.
- Each new route: Owner only, CSRF, 409 on a stale revision with the draft kept, 422 with the
  draft kept; accept of a stale suggestion refused; merge accept atomic (a stale second unit
  rolls back the first); dismissal does not change the unit, its history or completeness;
  attribution sidecars written.
- The difference renderer: escaping, `<ins>`/`<del>` only, bounds.

Browser (`tools/material_browser.py` in the Chromium container, the synthetic fixture with a
generated report, 1280×720 and 390×844, with and without JavaScript):
- Overlay boxes align with the polygons at each zoom (within 1 CSS pixel of the expected rect).
- Box-to-card and card-to-box links; accept, edit and dismiss; a stale suggestion; a merge; draw
  a box and add text; choose a primary reading; `j`/`k`; a 409 keeps the draft.
- No horizontal scroll at 390 px; no console CSP violations.

Local live check (not in CI): the MÖRK BORG IT bundle with its two reports (the Claude visual
review and the text-layer cross-check, both bound to its bundle hash) imported into a scratch
database; a sample of pages reviewed through the screen; counts of imported suggestions per
action recorded in the merge message.

## Order

1. Migration 025 and the report import command, with tests.
2. Screen: overlay, effective text, suggestion actions, merge, add text, primary choice,
   navigation; handler tests.
3. Browser checks and the live check; `docs/sources-and-retrieval.md` updated; this plan deleted
   and the umbrella's step 2 marked done.

Each is a reviewed change set (Sol and Opus); 1 and 2 may be one if small.

## Risks

- **The overlay misleads** if polygons and the image disagree: the browser check pins alignment,
  and the crop previews stay as the independent view.
- **One-click accept** makes approving model text easy: Accept needs the Owner's rationale field
  (prefilled, editable, required) and stays a per-unit decision; no "accept all".
- **Report claims** (origin, model) are unverified; they are shown as the report's claims.
