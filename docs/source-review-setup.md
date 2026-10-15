# Source review: local end-to-end workflow

## What is implemented

Rust can register a prepared source, preserve PDF/JSON/PNG snapshots privately in SQLite, show source images and safe extracted units, record explicit review/correction events, and perform exact-edition reviewed-only lexical lookup. Python/Marker preparation is optional tooling in containers; it is not a shipping application dependency.

Imported pages start unreviewed. A unit decision alone does not make a page searchable. Resolve every unit, inspect the entire original page for omissions/reading-order/context problems, confirm its printed label and completeness, then look it up. Changing a unit invalidates page completeness. Stale decisions retain their submitted token/draft and require reassessment.

Original document coverage is separate from selected-page review: the UI reports selected pages against the parsed original PDF total and flags partial/unknown extraction runs. Different runs are not silently merged into a whole-book certificate. Legacy unknown totals can be filled by re-importing the same verified bundle/PDF without changing review decisions.

## Container tools

```sh
docker build -f containers/extraction.Dockerfile -t mimik-extraction .
```

The image uses CPU-only PyTorch, Marker 2.0.0 and Surya 0.22.1. It does not include owner manuals/credentials. Build context is explicitly allowlisted. Resolved runtime dependencies are recorded in the image and copied into new extraction manifests.

Prepare a private writable output directory and cache volume. Run with your user ID, bounded resources, read-only input PDF, writable output/cache, dropped capabilities and no privileged mode. The cache may need its ownership initialized to your container user ID; never make the owner corpus globally writable.

Example selected-page conversion, with paths adapted to your machine:

```sh
docker run --rm --user "$(id -u):$(id -g)" --cpus=4 --memory=8g --pids-limit=256 \
  --cap-drop=ALL --security-opt=no-new-privileges -e HOME=/tmp \
  --mount "type=bind,src=$HOME/temp/MORK-BORG-ita.pdf,dst=/input/manual.pdf,readonly" \
  --mount "type=bind,src=$PWD/data/extractions,dst=/output" \
  --mount type=volume,src=mimik-extraction-cache,dst=/cache \
  mimik-extraction /input/manual.pdf --source-id mork-borg-core-it-2025 \
  --pages 30 --mode fast --ocr disabled --output /output
```

`disabled` was used for the first reproducible text-layer sample; it is not the intended quality guarantee. `auto`/forced OCR need separately verified local inference prerequisites. No real model-provider API call is made by extraction.

For preparation, mount the successful extraction directory read-only at `/input/extraction`, the original PDF read-only, and output writable. Override entrypoint to `python` and invoke:

```text
/tools/prepare_review.py /input/extraction --pdf /input/manual.pdf
  --title "MÖRK BORG Italian core" --system mork-borg
  --edition "MÖRK BORG — IT 2025" --language it --output /output
```

This phase can run with `--network=none`. It freezes and rechecks the exact PDF/JSON snapshots before rendering, never silently adopting a replacement identity. It emits `review-ready-<id>/bundle.json`, `source.json` and original `page-<PDF index>.png` images. See `source-import-contract.md` for strict supported structures, geometry and limits.

## Import and inspect

```sh
./mimik --database data/mimik.sqlite3 import-source \
  --bundle data/extractions/review-ready-<id> \
  --pdf "$HOME/temp/MORK-BORG-ita.pdf"
./mimik --database data/mimik.sqlite3
```

After one-time Owner setup, sign in as Owner and choose **Inspect sources and review evidence** at `/admin`, or open `/admin/materials` on the printed address. Use the same database as the import. Players and campaign Masters cannot administer these materials.

The source directory distinguishes selected-page coverage, original-book total, exact registered game/edition (or unresolved legacy identity) and artifact finalization. Finalization alone does not review evidence. Source, page, unit and history lists use50-row pages; unit/history cursors are independent. Always compare the original full page and derived region alongside safe extracted text. Explicit accepted/corrected/rejected decisions require a rationale; corrected text remains separate from immutable raw HTML. Browser reviewer identity is derived from your signed-in account, with immutable account/epoch/session-digest attribution committed with the decision.

Page completion checks every unit, including other list pages, and requires an explicit whole-page omission check and confirmed printed label. Changing any unit invalidates page completeness and advances its revision, making dependent publication evidence stale without rewriting history. Validation/conflict pages retain unsaved drafts and submitted revisions separately from current stored revisions: copy, reload and reassess before approving. Corrections allow20,000 Unicode scalars; rationale5,000; printed label50; NUL is rejected. Material POSTs permit320KiB encoded bodies; the server enforces scalar limits. Original image reads are bounded to16MiB; region input24MiB/output4MiB with one admitted renderer. Invalid/oversized stored evidence fails explicitly.

The explicit isolated loopback prototype remains available with `--no-auth` at `/sources`; it uses local reviewer labels rather than authenticated attribution. In authenticated mode those old global paths remain denied. Import, extraction, registry/alias changes, source finalization and definition publication remain local operator workflows.

Prototype lookup at `/sources/search` requires exact system and edition. It is literal substring matching using SQLite's basic case behavior, not semantic question answering or a declaration of action legality. Unsupported sources, partial/incomplete coverage and no reviewed match are distinct visible states.

## Current manual sample

The supplied Italian MÖRK BORG PDF page 30 (printed 26) was extracted and prepared in containers and registered as source 1 in the ignored local default database. Its units remain **unreviewed**; no real manual was bulk-approved. The sample retained its modifier table but exposed an unwanted invisible-text heading and reading-order problems. Those require explicit source comparison/correction, not confidence-based approval.

The structured editor remains separate from the official sheet-layout work. A generic sheet candidate in `~/temp` was visually identified as D&D, not MÖRK BORG. The official blank MÖRK BORG sheet still needs identification/mapping before claiming layout fidelity.
# Visual region comparison

For unknown games, first register exact game/edition IDs and import a v2 prepared
bundle as described in [runtime registry](runtime-registry.md). Registry CLI/aliases
identify sources without approving them or enabling generic character sheets.
On lookup use stable `edition_id` or an exact legacy/display `edition` label,
not both; Unicode labels are accepted within the documented scalar limits.

Each review unit now shows a derived bounding-region crop from the stored original page render, alongside the extracted content. Use **Open region PNG** or **Download region PNG** for a larger view/export. The extraction identifier and numeric coordinates are under a details disclosure; the full-page image remains available to inspect omissions and surrounding context.

Crops do not modify or approve evidence. Invalid/unsupported geometry, image color metadata or size can make a preview unavailable; transient busy previews retry up to four times with JavaScript. Follow the full-page or region link when the preview remains unavailable.16-bit inputs are normalized to8-bit previews; supported color profiles and alpha are retained.
