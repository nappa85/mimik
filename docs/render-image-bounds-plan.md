# Render-stage image bounds without retained images

Finding (review 2026-11-20, Major): `text()` in `src/sources/worker/operations.rs` builds its
text page with `PRESERVE_IMAGES` only to measure `Stats::image_area`.
- **What the flag does:** with it, MuPDF's stext device (`fitz/stext-device.c`) keeps one image
  block per image paint. For every shading paint it first rasterises the shading into a pixmap
  of its bounds, clipped to the scissor and the mediabox (`fz_new_image_from_shade`), and keeps
  it until the text page is dropped.
- **Cost:** a page of a few hundred full-page `sh` operators holds hundreds of page-sized
  pixmaps.
- **Late check:** the 20,000-block check runs only after `to_text_page` returns.

Owner decisions 2026-11-23:
- **Shading bounds:** take the scissor and shading bounds from MuPDF through the vendored
  mupdf-rs fork. The fork stays until that change is released upstream too.
- **Shading coverage:** a separate stat (`shading_area`), so route A's probe thresholds can
  choose with data. `image_area` keeps its current meaning, images and shadings together.

## 1. Fork change (mupdf-rs, `/home/marco/Progetti/mupdf-rs`)

- **Branch:** a new branch `mimik/scissor-shade-bounds` from the vendored base 1adf801.
- **Changes:** additive, safe, upstream-acceptable API, with tests in the fork:
  - `Shade::bounds(&self, ctm: &Matrix) -> Result<Rect, Error>` over `fz_bound_shade`.
  - The device's current scissor available to `NativeDevice` image and shading callbacks
    (over `fz_device_current_scissor`). The design is the implementer's choice but must not
    break the existing trait; for example, a provided method with a scissor argument that
    defaults to the existing one, or a scissor accessor valid only during a callback. Record
    the choice in the fork commit.
- **Vendoring:** re-vendor into `vendor/mupdf` exactly as `vendor/mupdf/MIMIK-VENDOR.md`
  describes, with the new commit list and diff hash. Then run
  `python tools/vendored_fork.py inventory` and update DEP-29 in `docs/decisions.md` (or the
  file that holds it).
- **Upstream:** the owner opens the upstream PR (rebased onto upstream main). Record it in
  `TODO.md` next to the mupdf-rs release item.

## 2. Text without images

- **Flags:** `text()` uses `WORDS_FLAGS` (`CLIP | USE_CID_FOR_UNKNOWN_UNICODE`).
- **Line changes:** lines may change. Today an image or shading painted between two text runs
  on one baseline forces a new block and line (stext-device.c ~803). Without the flag the runs
  can merge into one line with a space, as the words stage already does. ActualText bounds
  around a shading also change (stext-device.c 1711-1722).
- **Decision:** this is accepted and documented in `docs/sources-and-retrieval.md` (or wherever
  the render stage's lines are described).
- **No decode saving:** image decoding is not avoided (stext-device.c 2936), so the change
  claims none.

## 3. Image bounds from a recording device

A `NativeDevice` pass runs the page with `page.run` (contents, annotations and widgets, as
`to_text_page` does) and the identity matrix. It records rectangles only:
- **Images:** `fill_image` and `fill_image_mask` with `alpha >= 0.5` give the unit square
  through `cmt`, intersected with the current scissor and `page.bounds()`. This is exactly
  stext's rule (stext-device.c 1637-1650).
- **Shadings:** `fill_shade` with `alpha >= 0.5` gives `Shade::bounds(cmt)`, intersected with
  the scissor and `page.bounds()`. This is the rectangle `fz_new_image_from_shade` rasterises
  today. A shading's rectangle counts towards both `image_area` and `shading_area`.
- **Shared checks:** each rectangle then goes through `clipped()`, the empty-box check and the
  20,000 cap.
- **Parity cases:**
  - **Tiling:** `begin_tile` returns `None`, as stext does, so a tiled pattern's cell content
    runs once (pdf-op-run.c ~711) and a tiled image background counts as one cell, as today.
  - **Soft masks:** images inside soft-mask content count, as today (stext has no
    `begin_mask`).
- **Callback rules:** callbacks cannot return errors or stop the run, and a panic across
  `extern "C"` aborts the worker. So the device:
  - latches the first error (`TooLarge` at the 20,001st box; an `Err` from `Shade::bounds`)
    and ignores later events;
  - has no indexing, uses checked arithmetic and never panics.

  After `run` the latched error is returned, mapped like the text pass's errors. Use
  `run_with_cookie` to abort early if the fork's API allows it; otherwise the parent's deadline
  bounds the rest of the page.
- **Stats:** `Stats::shading_area` (0–1, validated like `image_area` in `records.rs`) is added
  to the record, the store and the places that build stats. Data from before the release is
  disposable, so there is no backfill.

## 4. Cost

The page is now interpreted three times: draw, text, and bounds.
- **Measure:** render-stage time and `image_area` on the real manuals (host live test
  `live_local_pdfs_inspect_and_render_every_page`), before and after.
- **Record:** both in the commit message. Explain any page whose `image_area` changes by more
  than 0.01, and report how many pages' lines changed.

## Tests

- **In-process** (`src/sources/worker/tests.rs`, container gate). Parity tests, which also pass
  before the change:
  - the existing 1/6 case;
  - an image clipped by a smaller rectangle path counts only the clip;
  - an image at `ca 0.3` counts nothing;
  - an image in a soft mask counts;
  - a full-page `sh` gives `image_area` and `shading_area` 1.0;
  - an `sh` inside a half-page clip gives 0.5;
  - an axial shading with a `/BBox` gives the box.

  A new fixture puts text, an image, then text on one baseline; its expected lines are those
  without `PRESERVE_IMAGES`.
- **Fork tests:** `Shade::bounds` and the scissor access, in the fork's own suite.
- **Hostile corpus** (`live_tests/corpus.rs`, sandboxed, host run from the main checkout): new
  case `shading-only`, the biggest MediaBox inspection accepts, with 400 full-page `sh`
  operators of an axial shading and no tiling pattern.
  - Expected: `RenderOk`, peak RSS within `check_rss`, `shading_area` 1.0.
  - Fail-before: not `RenderOk` or above the RSS bound; record which in the commit message.
- **Existing live checks:** the worker live corpus and the real-manual live test pass on the
  host.
