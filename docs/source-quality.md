# PDF extraction quality workflow

## Requirement

The user's older Pathfinder RAG produced insufficient quality. Extracted Markdown must be double-checked for correctness against the original PDFs. Markdown is an intermediate representation, never the authority by itself.

Use layout-aware extraction rather than a plain `pdftotext` dump. The Marker prototype preserves tables/regions/metadata with OCR not forced by default. Optional owner-key commercial-model review compares page images against extracted blocks, not training memory. See `extraction-prototype.md`; model findings alone never promote evidence to verified.

## Review unit and provenance

Track quality per page/section or rule entry, not just per book. Store:

- Source ID, SHA-256, game, edition/printing, language, enabled source set.
- One-based PDF page index and independently verified printed page label.
- Section heading, page region/bounding boxes when available, and extraction tool/version/settings.
- Immutable raw output, normalized text, and a separate correction record.
- Review state, reviewer, timestamp, source/extraction hash, findings, and evidence region.

Suggested states: `unreviewed`, `needs_correction`, `reviewed`, `excluded`. A changed source/extraction invalidates affected reviews. A book with reviewed sections must not be presented as entirely reviewed.

## Process

1. **Identify the source:** inspect title/credits and contents, hash the file, and confirm edition/printing. Preserve language/version differences. Do not treat PDF creation dates as publication dates.
2. **Extract page-aware data:** use the text layer where useful and OCR where required. Retain table structure, paragraph/column boundaries, sidebars, page regions, and links to visual page renders. Do not discard images/regions that may contain rule information.
3. **Flag likely problems automatically:** missing/unknown glyphs, replacement/control characters, suspicious empty table cells, column interleaving, detached headings, incomplete entries, and broken references. An independent extractor can expose disagreement, but agreement is not proof of correctness.
4. **Compare with the rendered PDF:** check every passage used for verified rules/advice. Prioritize action costs, dice expressions, signs/inequalities, numerical thresholds, units, prerequisites, negations, exceptions, table headers, and footnotes. Read the relevant surrounding paragraphs to avoid changing scope.
5. **Record corrections:** preserve raw output; patch normalized text with a reason and exact source location. Recover symbols as semantic values such as `one_action`, `two_actions`, `three_actions`, `free_action`, or `reaction`; never guess a cost from missing text. Keep optional/variable costs distinct.
6. **Separate source conflict from extraction error:** if two visual originals disagree, record both. Resolve edition/errata/translation precedence explicitly instead of blending sources into a fabricated rule.
7. **Build reviewed evidence:** chunk along validated rule/section boundaries with enough surrounding context. Carry page provenance and review state into every chunk. Only reviewed passages support verified explanations or deterministic adapter implementation; unresolved sections may be displayed as unchecked source material with links to the PDF.
8. **Check implementation and retrieval:** independently compare rule fixtures and explanations against reviewed originals; verify exact-edition retrieval, correct page citations, and insufficient-evidence behavior. Passing retrieval/model tests does not certify extraction fidelity.

## First review batch

### Scaling and book hierarchy clarification (2026-10-15)

Full human rereading of a manual is not the intended routine preparation workflow.
Evaluate bounded frontier-model image/block review across all pages with global
chapter/section reconciliation, then targeted human discrepancy/high-impact review
and stratified checks of unflagged pages to measure missed defects. Keep automated
checked coverage distinct from human-reviewed coverage; current verified-evidence
policy is unchanged and unsampled pages do not become human-reviewed by inference.
Whole-book scalable admission remains an explicit research/design gap.

Preserve stable book-wide parent IDs/source spans across page and batch boundaries,
including chapters hundreds of pages long. Chunk ancestry, semantic applicability
and cross-references are distinct; a class spell list may reference sibling spell
descriptions rather than contain them. Wrong ancestry can change rule meaning even
when text is intact. See [hierarchy and scalable review experiments](accuracy-experiments.md).

After selecting the first playable game, audit a small end-to-end set: the official sheet, fields/derived-value definitions, basic action economy, core available actions, and representative powers/spells. Implement and test that slice before extracting the entire library.

For Pathfinder, the visually confirmed action-glyph loss on the reference sheet is the first concrete regression case. For D&D, include OCR-noisy numeric rules and tables. For MÖRK BORG, include rotated/stylized layouts and the observed English/Italian source discrepancy. For V5, distinguish rule text from narrative columns and reference summaries.

## Current status

Rust import/review UI/history and reviewed-only lookup are implemented. A containerized text-layer Marker sample, snapshot preparation and registration were exercised; general OCR and live API review remain pending. Five MÖRK BORG rule pages were visually checked for base-rule code; the actual imported sample stays unreviewed and no whole corpus is approved. Partial original-PDF coverage is explicit. Local PDFs/extracts/images remain ignored private runtime data.
