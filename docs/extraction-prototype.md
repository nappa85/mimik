# Layout-aware extraction and optional API review prototype

## Design and status

`pdftotext` is for quick inspection only. The ingestion prototype uses **Marker** for layout-aware conversion and preserves JSON blocks, table HTML, page polygons, images, and metadata, plus paginated Markdown. Markdown is not the canonical review representation.

The old Pathfinder converter used `force_ocr=False` and saved only `rendered.markdown`, not the renderer's images or metadata. Mimik extracts/processes once, then renders JSON and Markdown from the same document. Each run has source/artifact hashes, exact page coverage, extractor version/settings, and an unreviewed page/block ledger; existing Markdown is not reused by filename alone.

Marker 2.0.0 text-layer conversion was exercised on Italian MÖRK BORG PDF page 30 in a bounded CPU container, followed by snapshot-bound preparation and Rust registration. The table survived but visual/text-layer discrepancies remain unreviewed. General OCR, other manuals and live API review are unverified. Python remains containerized optional tooling; shipping source review is Rust/native.

Check code and weight licenses separately. The upstream README reviewed on 2026-10-11 states Apache-2.0 for current code and modified OpenRAIL-M terms/eligibility limits for weights. Preserve the actual installed version's terms. Its runtime/backend support also needs testing on the AMD/Vulkan target; do not assume a Python CUDA pipeline meets shipping requirements.

Sources: [Marker README](https://github.com/datalab-to/marker), [output saving](https://github.com/datalab-to/marker/blob/master/marker/output.py), [PDF converter](https://github.com/datalab-to/marker/blob/master/marker/converters/pdf.py).

## Local extraction

Build the isolated extraction environment:

```sh
docker build -f containers/extraction.Dockerfile -t mimik-extraction .
```

Use the bounded, read-only-input run instructions in `source-review-setup.md`. General OCR needs separately tested local inference prerequisites; first use may download model/font files into a private cache. Do not install tooling on the host.

Use **one-based PDF indexes**, not printed page numbers:

```sh
# Arguments passed to the container entrypoint after mounting PDF/output/cache:
# /input/manual.pdf --source-id mork-borg-core-it-2025
# --pages 28,30-33 --mode fast --ocr auto --output /output
```

- `auto` does not force OCR everywhere; Marker may repair bad/scanned regions.
- `force` is for selected batches that need OCR; `disabled` is a text-layer-only diagnostic.
- `fast` is the default CPU-oriented mode; compare `balanced` if supported.
- No generative `use_llm` repair/cloud provider is enabled during extraction.
- Required page selection is translated into Marker's zero-based indexes.

The ignored `data/extractions/<source-id>-<run-id>/` bundle contains:

- `source.json`, `source_meta.json`: structured blocks and metadata.
- `reading.md`, `reading_meta.json`, extracted images: reading view.
- `manifest.json`: source/page/version/config/artifact identity and run status.
- `review.json`: region identities, flags, empty corrections, all `unreviewed`.

Incomplete runs are marked `failed`. Missing requested pages or unsupported page-index schemas fail instead of silently reducing coverage. These bundles are not automatically indexed or used as verified advice.

## Optional commercial-model comparison

Training knowledge is not source provenance and may reflect another edition/translation. Reviewers must compare **the original PDF page image** with **this extraction's blocks**, without reconstructing omissions from memory.

The owner chooses an image-capable model supporting JSON responses and an HTTPS OpenAI-compatible chat/completions endpoint. Other vendor-native API shapes are not implemented yet; final Rust support will use an established provider abstraction (see `provider-abstraction.md`).

The prototype accepts `OPENAI_API_KEY` or `MIMIK_REVIEW_API_KEY` from the process environment or `.env` in the working directory. Explicit environment values override file values; within each source the reviewer-specific key wins when nonempty. Use `--env-file` for another path. The file is parsed with the existing `python-dotenv` library, without shell execution or environment mutation. Install the optional dependency in an isolated review environment:

```sh
docker build -f containers/extraction.Dockerfile -t mimik-extraction .
```

Invoke the reviewer inside the tooling container with only selected source inputs/output and, if desired, an explicitly mounted dotenv file; use `--env-file` for that mount. `.env` is ignored and excluded from build contexts; `.env.example` is blank. Do not put keys in Git, command arguments, or chat. Loading configuration sends no request.

This explicitly invoked command sends the selected page images and blocks to the provider under the owner's terms and billing:

```sh
python3 tools/review_extraction.py \
  data/extractions/<source-id>-<run-id> \
  --pdf "$HOME/temp/MORK-BORG-ita.pdf" --pages 28,30-33 \
  --api-url https://api.openai.com/v1/chat/completions \
  --model <owner-selected-image-capable-model>
```

The endpoint illustrates the API shape; no model selection/live call has been made. Local deployment does not make optional provider requests local. The default flow makes no provider calls.

The reviewer verifies source/artifact hashes, renders pages with MuPDF (`mutool`), and sends each selected page separately. It records model identity/usage and findings in `api-review-<id>/`. Unknown block citations/malformed findings are rejected; failed pages stay failed. Redirects are refused to avoid forwarding credentials. Keys are not saved in reports.

**Findings are suggestions, never approval.** The reviewer does not overwrite text, apply corrections, or promote blocks/pages to `reviewed`. Resolve each issue against the source, retain correction evidence, and verify rule/retrieval fixtures independently. A no-issue response does not certify correctness.

## Accuracy target and checks

Aim for **100% reviewed coverage of source fields required by each supported rule**, with zero unresolved known discrepancies. Measure table-cell/relationship fidelity, numbers/symbols, paragraph/exception completeness, and citation accuracy separately. Unresolved evidence must not support verified advice.

This measurable coverage target is not proof of universal 100% correctness by any extractor/model/reviewer. Whole-manual approval needs whole-manual comparison, including graphical regions absent from text layers. MÖRK BORG/V5 need visual/layout review; a generic benchmark score cannot substitute for it.

```sh
python3 -m unittest discover -s tools -p 'test_*.py'
python3 tools/extract_manual.py --help
python3 tools/review_extraction.py --help
```

Helper tests cover page mapping, table-region/unreviewed-state preservation, artifact tampering, and fabricated/malformed model findings. They do not test Marker inference or live provider behavior.
