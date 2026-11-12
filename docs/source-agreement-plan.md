# Agreement view and batch accept (source import step 8)

Goal: on a page under review, Mimik compares each unit's text with a second reading of the same
place on the page and shows which units match word for word. The Owner accepts all matching
units of the page in one action after looking at the page image. Units that differ, have no
second reading or are excluded are still reviewed one by one, with the difference or the reason
shown. Umbrella: `docs/source-import-plan.md`, step 8 (SRC-14). Remaining work only.

What stays: every acceptance is the Owner's decision, recorded per unit with actor, rationale,
revision, operation id and audit sidecar (SRC-4, SRC-5); agreement is never a decision and never
makes text evidence by itself (SRC-6); nothing is accepted automatically, by score or by a
remembered rule. Raw extraction text and the stored word rows are unchanged (SRC-20,
append-only).

## The second reading

The first comparison is with the PDF's text layer: the word boxes `import-source` stores
(`source_page_words`, SRC-20). A source imported before migration 029 gets them by running
`import-source` again on the same bundle; no new extraction and no restart of the Owner's review
are needed.

What an agreement proves depends on where the unit's text came from:

- **Independent** (units read from the page image, such as Marker with forced OCR on the IT
  pages 22–39 bundle): an OCR reading of the pixels and the PDF's text layer produced the same
  words. A wrong digit, dropped word or misread letter would have to happen identically in
  both. The text layer may itself be an OCR layer (a scanned PDF); the two readings are then
  two OCR runs and less independent, so the label says "the PDF text layer (may itself be
  OCR)".
- **Same layer** (units taken from the text layer, such as Marker without OCR on a digital
  PDF): agreement shows the extractor carried the text layer over intact (no dropped, merged,
  duplicated or reordered words inside the unit). It cannot catch a wrong character map (a
  ligature mapped to nothing, a symbol font), because both sides share it.
- **Derived** (a route A source, whose units are built from these same words): agreement holds
  by construction; no agreement view.
- **Unknown**: anything else.

The kind comes from the source's stored extractor provenance (`prepared_json`: `extractor.name`,
`version`, `settings`) through a pinned table in code keyed by name and exact version: the
Marker versions the prototype used (`marker-pdf` 2.0.0) with `force_ocr: true`, `disable_ocr`
not true and `use_llm` absent or false (Marker's model processors may rewrite text) are
independent, the same versions without forced OCR are same layer, route A is derived, and
everything else is unknown. Provenance is self-declared bundle JSON (a hand-written v2 bundle
can claim anything, L-SRC-4); the label states what the bundle declares, and the limit goes to
`docs/limitations.md`.

Later readings (not in this plan): a second source of the same document (route A against route
B, through `source_documents`) and an OCR reading of a same-layer page, through the same rule
with another word source.

## Decisions

Decided (SRC-21, 2026-11-12):

- **D1.** Batch accept only for independent readings; same-layer, unknown and derived pages show
  differences and orphan words, and their units are accepted one by one. Decided again after
  slice 2 measures same-layer pages (a change there also changes SRC-14).
- **D2.** Each batch decision's rationale starts with a prefix the server writes and the Owner
  cannot edit (`agreement-v1; independent:`), followed by the Owner's text, prefilled with
  "matches the PDF text layer word for word; checked against the page image".

Still owed with slice 5:

- **SRC-20 change.** Stored words may be shown to the Owner as the other side of a difference
  (labelled "PDF text layer"), never as text of their own, never as evidence, never searched.

## The agreement rule (agreement-v1)

Pure and deterministic, in `src/sources/agreement.rs`. Prior art: the prototype cross-check
`tools/crosscheck_text_layer.py` (centre-in-box words, shadow duplicates dropped); this rule
differs in being exact, with no similarity matching.

1. **Words of the page.** The decoded word row. A shadow duplicate is dropped: the same text
   drawn again with every edge shifted by less than 3 points and less than 0.3 of the smaller
   word's height, overlapping the smaller box by at least half (stricter than the prototype, so
   a repeated word in tiny type is kept). Words whose centre lies inside a confirmed exclusion
   box are removed.
2. **Candidates.** Unreviewed extracted units with text. Each other unit gets `Excluded` with its
   reason:
   - owner units; kinds `Table`, `TableGroup`, figures, pictures, equations and forms (the kind
     list is fixed from slice 2's counts);
   - units inside a table proposal, or whose text alone a table shape scores at 80 or more (the
     "looks like a table" exclusion the umbrella plan owed);
   - units past the proposals' 64 KiB text budget (not computed);
   - units targeted by an open suggestion (not dismissed, not applied), and every unit of a page
     with an open page-level `missing_text` or `note` suggestion;
   - units holding a word whose centre lies inside two units' polygons (checked against every
     unit of the page, reviewed and owner units included);
   - units whose polygon holds a word removed by an exclusion box, or touches such a box (the
     umbrella plan's "a unit crossing the box's edge is left unreviewed and flagged").
   `evaluate` itself refuses a page of more than 2,000 units or 64 KiB of unit text ("not
   computed"), so no caller can skip the bound.
3. **Words of a unit.** The words whose centre lies inside the unit's polygon
   (`table_words::words_among`), ordered geometrically: grouped into lines by vertical overlap
   over half the smaller height, lines top to bottom, words left to right. Stream order is not
   used.
4. **Tokens.** Both sides, in this order:
   1. On the unit side only, the markup html2text itself writes: a heading prefix (`#` to
      `######` and a space), a `* ` list marker at line start, and paired emphasis (`*x*`,
      `**x**`); an unpaired `*`, `- ` and ordered markers stay (stricter than
      `shapes::normalise`, which would strip `- ` and lose a minus sign). A text-layer bullet
      glyph (`•`, `◦`, `▪`) at a line start is dropped at most as many times as list markers
      were removed. Then Unicode NFC on both sides (not NFKC: superscripts, subscripts,
      fractions and fullwidth forms stay distinct).
   2. **Line-end hyphenation**, text-layer side only: a word ending in U+002D, U+2010 or U+00AD
      at the end of a line, with a letter right before that hyphen, followed by a word on the
      next line that starts with a lowercase letter, matches the unit's token joined without
      the hyphen or with it. No digit on either side; a word that is only a hyphen is never
      dropped; en dash and minus never take part.
   3. **Folds**, an explicit table and nothing else: ligatures U+FB00–FB06 to their letters,
      apostrophes and single quotes to `'`, double quotes to `"`, U+2010, U+2011, en dash and em
      dash to `-` (the minus sign U+2212 stays itself), `…` to `...`, every space character to
      a space. Case, accents, digits, letters and other punctuation are compared as they are.
   4. Split on whitespace.
5. **Verdict per unit.**
   - `Agrees` when the token sequences are equal and non-empty (an O(n) comparison) and no
     orphan word's box (below) intersects the box of the unit's polygon and words grown by one
     line height on every side;
   - `Differs` otherwise, with the reason (tokens differ, or orphan words next to it); the
     token difference (`similar`) is computed for display only, for sequences of at most 2,000
     tokens per side, and shows the first 20 changes;
   - `NoWords` when no word falls inside the unit (art, image text, no row, invalid row).
   No similarity threshold: one different token is a difference.
6. **Page facts.** Orphan words (inside no unit, missed text of the L-SRC-1 kind) are counted
   and outlined on the page image. Words inside a confirmed exclusion box, where the source has
   one, are left out of every count and display (and exclude the units they fall in, step 2).
   There is no per-view watermark detection.

`agreement-v1` is a constant; a change to steps 1 to 5 bumps it. Stored word rows carry no
splitter version (L-SRC-TABLE), so a change to the worker's word splitter can change verdicts
under the same rule version; such a change must bump it too.

## Slices

Each slice is its own reviewed change; slices 3 and 4 may merge together.

1. **Agreement core.** `agreement.rs` with the rule above and the provenance table. Tests:
   - a proptest: one changed character of an agreeing unit (digit, letter, sign), two swapped
     words, or a word added on either side makes it `Differs`;
   - regressions: `1–` / `6` against `16`, `-` / `2` against `2`, `10²` against `102`,
     `armatura¹` against `armatura1`, all `Differs`; a word missing from both sides but present
     as an orphan beside the unit is `Differs`;
   - hyphenation, ligature, quote, shadow-duplicate and two-units-claim-one-word cases;
   - drop caps and a tall glyph that chains two lines are `Differs` (documented limits).
2. **Measurement (prototype tier, local only).** An ignored test or example run on a copy of
   the Owner's database (never the live file; `import-source` re-run on the copy first if its
   word rows are missing), printing aggregates only, never word or unit text: per source and
   comparison kind, units by verdict, by kind and by exclusion reason, and orphan words.
   Bars, set now:
   - **Safety:** no unit the Owner corrected may come out `Agrees` against its original
     extraction text; every such case is examined, and if the correction changed the words,
     the rule is fixed before slice 3. Rejected units that agree and units accepted unchanged
     that differ are counted too. Each number is reported with its count; a zero on a few
     hundred units from one manual is weak evidence, and the report says so.
   - **Yield:** reported, no bar: the share of unreviewed prose units that agree, per manual
     and kind. The Owner decides from it whether slices 3 and 4 go ahead and revisits D1.
   Conclusions (aggregates only) go to `docs/results.md`.
3. **Review screen.** On the page: a summary ("41 units match, 6 differ, 3 have no second
   reading, 4 are excluded, 12 text-layer words are in no unit"), each card's verdict or
   exclusion reason, each `Differs` card showing the difference (unit text against "PDF text
   layer"), and a filter "only units that need attention" that hides agreeing and reviewed
   cards. Computed in `spawn_blocking` per page view like the proposals and memoised with them
   (page revision, words size, rule version); a page over the 2,000-unit bound shows no
   agreement and says so.
4. **Batch accept.** A form under the page image, "Accept the N matching units", listing their
   numbers, with the D2 rationale. It is offered only for the kinds D1 allows. The POST carries
   the page id and revision, the word row's size, and the unit ids with their revisions (at
   most 1,024, the atomic bound; a larger set is accepted in parts). The handler recomputes the
   page's verdicts in `spawn_blocking` from the stored rows and refuses if any submitted unit
   does not agree. A new store method, `Store::material_batch_accept_agreeing`, then runs one
   IMMEDIATE transaction:
   - rechecks Owner and CSRF;
   - checks the page revision (any unit review or owner unit bumps it), the stored word row's
     length, and each unit's revision, state `unreviewed` and origin `extracted`, all on that
     page;
   - records one `accepted` decision per unit through `review_batch` with the audit sidecar,
     under one operation id, with the D2 rationale.
   Any change refuses the whole batch with the reason. Page completion stays a separate Owner
   action. Tests: stale page or unit revision, a unit reviewed meanwhile, a unit that no longer
   agrees, a unit id from another page, a forged rationale prefix, CSRF, a non-owner, and that
   no word row or raw text changes.
5. **Docs.** `docs/sources-and-retrieval.md` (rule, screen, batch); decisions (D1, D2, the
   SRC-20 change, SRC-14 if D1 is widened later); limitations (drop caps, chained lines, a word claimed
   by two units, self-declared provenance, a text layer that is itself OCR, same-layer limits,
   words inside image text); the umbrella's step 8 removed. Owner browser check of the page and
   of one batch.

## Risks

- **False agreement.** The rule is exact token equality after a short, listed fold, with the
  hyphen rule fenced off from digits and signs. A word missing from both readings and outside
  the unit's box is caught only as an orphan beside the unit; a word missing from both and from
  the text layer's box entirely (a word the PDF draws as art) is not caught. Structure is not
  checked: a heading read as body text, or units in the wrong order across columns, agree word
  for word; the Owner's look at the page and the kind label on each card cover that, as today.
  Raised text is invisible to both readings: a superscript or footnote number drawn right after
  a word becomes one text-layer word (`10` and a raised `2` read as `102`), and OCR often reads
  it the same way, so both are wrong together; slice 2 counts small raised words beside units.
  Words claimed only by a neighbour that is not compared (reviewed, owner, table, figure) are
  not orphans, so they do not flag the candidate beside them.
  Likewise a word removed by an exclusion box is never an orphan, so a body word under a box
  drawn just outside a unit's box (within one line height) does not flag that unit.
  Slice 2's safety bar checks the rule against the Owner's own corrections before anything is
  offered.
- **Polygon misses.** A unit whose polygon misses some of its words differs, or is flagged by the
  orphan rule: safe, only less help. Slice 2 shows how often.
- **Personal data.** A buyer watermark inside a unit can appear on the "PDF text layer" side of
  a difference on a bundle page without an exclusion box; it stays on the local review screen
  like the page image that shows it, and never reaches docs, logs or measurement output.
- **Cost.** One linear pass per page view over at most 64 KiB of text and the page's words,
  memoised, off the async runtime; the diff is bounded; the batch transaction does checks only.
