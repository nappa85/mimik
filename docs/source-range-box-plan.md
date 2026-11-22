# Plan: manual boxes over a page range (SRC-15 follow-up)

Remaining steps only. Context: `L-SRC-PROPOSAL-BUDGET` (`docs/limitations.md`). Today a manual
exclusion box is drawn or typed per page on the exclusion screen (100 pages a screen); a header
the repeated-text proposer misses or refuses must be entered on every page, screen by screen.

## Behaviour

A separate form on the exclusion screen, "Boxes on a page range", a sibling of the `exclude`
form (never nested in it), posted as a new action `exclude-range` of the existing
`/admin/materials/uploads/{id}/{action}` route (same body limit, `form_map`, owner session and
CSRF admission). It acts on the saved exclusion only, so the screen form's window and
unticked-page rules stay as they are; the form says that posting it discards unsaved edits in
the screen form.

- **Fields:** the pages, either a range (`parse_page_ranges`, e.g. `1-350, 360`) or "all
  selected pages"; a mode, `add`, `remove` or `clear` (a strict enum); for `add` and `remove`
  one to eight boxes, one per line as *left top right bottom* in displayed points, origin
  top-left (`box_lines`), and the source page they were drawn on; for `clear` an
  acknowledgement that every box on those pages, proposal boxes included, goes; the rationale
  (prefilled with the current one: an exclusion has one rationale, and a range edit replaces it);
  the upload and exclusion revisions (stale ones are refused with `Conflict`).
- **Validation before any scaling:** the source page must be a selected rendered page; each box
  passes `check_box` against the source page's extent (refuses NaN, infinities, negatives, empty
  and out-of-page boxes).
- **Scaling:** each box is divided by the source page's `width_points`/`height_points` (displayed
  sizes, so a /Rotate 90 page is simply landscape), multiplied by the target page's, clamped to the
  target's extent and rounded outward like every stored box (`round_boxes`). Same-size pages get
  the same points; a mixed-size manual keeps a footer at the foot of each page. A box that
  collapses to zero width or height on a target page refuses the request.
- **Add:** the scaled boxes join each page's boxes; an exactly equal box is merged (`dedup`; a
  near-equal box from a slightly different source size is not, and counts toward the eight).
- **Remove:** the same boxes, scaled and rounded identically, are removed where they match
  exactly; pages without a match are counted in the feedback, not refused. Removing what an `add`
  from the same source added restores the previous exclusion.
- **Clear:** every box on the pages goes. `proposal_fingerprints` are kept in every mode (they
  record what was confirmed, as windowed edits do). If no page is left, the current exclusion is
  appended with `removed=true`, as `remove_exclusion` does; an empty set is never stored.
- **Refusals:** an explicit range that names a page that is not selected and rendered, a page that
  would exceed eight boxes, or a box that collapses refuses the whole request (no silent skips).
  A structured refusal (an enum carrying the page and the reason) is mapped to text in the route,
  and the screen is rendered again at 422 with the posted values and the message naming the first
  such page. A result equal to the current exclusion is refused ("nothing to change on these
  pages") and appends nothing; `clear` with no exclusion is refused.
- **Feedback:** success redirects to the same screen (`?p=`) with the number of pages changed and
  the number of distinct page sizes they have (e.g. "350 pages changed, 2 page sizes"), so the
  owner knows which screens to inspect; `remove` also states the pages without a match.
- **Script:** each page's region editor (`assets/source-regions.js`) gets a button "Copy this
  page's current boxes to the range form" (current inputs, possibly unsaved). Without
  JavaScript everything is typed; the form shows each screen page's size beside its editor, so
  the numbers can be typed against a known page.

## Steps

1. **Store** (`src/sources/upload_exclusions.rs`): `RangeRequest`, `RangeMode`, `RangeRefusal`
   and `UploadManager::set_exclusion_range`, all inside one `self.write` with `load_upload` under
   the upload gate (no proposal run, so no fingerprint-key check), ending in `validate` and
   `append`. Pure helper `scale_box`. Tests first: scaling between A4, Letter and a landscape
   (rotated) page; a box touching the edge of a 595.2756 pt page; NaN, infinity, negative,
   empty and out-of-page boxes and an unknown mode refused before scaling; the eight-box and
   unrendered-page refusals naming the page; a collapsing box; add then remove restores the
   exclusion; clear over pages without boxes is a no-op refusal; clear of every page appends a
   removal; "all selected pages"; a stale exclusion revision (a screen form opened before a range
   edit) is refused; a range edit followed by a windowed screen edit keeps the other screens'
   boxes; a 20,000-page range within the history bound.
2. **Route and form** (`src/auth/uploads.rs`, `templates/upload_exclusion.html`): the action,
   its form parsing (duplicate fields refused by `form_map`), owner-only and CSRF refusals, the
   422 re-render with values and message, the redirect with the counts. Tests for each.
3. **Script and browser check:** the copy button; a browser check that it fills the range form and
   that the posted result shows on another screen's page.
4. **Docs:** `docs/sources-and-retrieval.md` (exclusions), `L-SRC-PROPOSAL-BUDGET` loses the
   follow-up sentence, `L-SRC-EXCL-HISTORY` names range edits (each is one whole revision),
   `TODO.md`; this plan is deleted.
