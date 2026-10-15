# Lantern Reach — synthetic sheet/context demo

**NOT YET IMPORTABLE: Mimik's template load/binding/model-review workflow is not
implemented.** These files prepare the test input; they do not enable a live model
or a new route. Open `sheet.html` directly only to preview the static layout. The
example character is not populated by that preview. The first usable milestone
must supply a working load/open path, not require you to hand-author definitions.

This is a small original fictional game package, not an extract from an existing
RPG or a claim of realistic game coverage. All files in this directory are original
synthetic project material distributed under **AGPL-3.0-only**, like Mimik's code.
They are intentionally distributable examples under the project's local-content
exclusion policy. Private manuals, extracted corpora and credentials stay excluded.

## Files

- `rules.md`: numbered text-only evidence, including deliberately incomplete rules.
- `sheet.html`: script-free web template with **proposed**, not implemented bindings.
- `character.json`: canonical typed values, template pin and stable equipment IDs.
- `review-cases.json`: expected findings/proposals for the evaluator, not model input.
- `decision-cases.json`: separate bounded-decision inputs/expected labels and
  cache-probe edits for AnyJev/Kev evaluation. Only its explicit input projection
  reaches a model; labels/source anchors stay evaluator-only. Not a game engine.

The toy game knowledge lives in these files. Mimik must not recognize the name or
game ID in Rust/JavaScript to supply special behavior.

## Proposed field binding convention

`data-field` is a dot-separated canonical path into `character.json`'s `values`;
`data-value-type` declares text/integer/boolean/array for context discovery. Labels
and sections supply readable context. No attribute contains executable rules.

`data-repeat="equipment"` identifies a collection; `data-entry-id="id"` declares
its stable identity field. Within its inert HTML template, `data-entry-field`
paths are relative to each entry. Updates identify entries by ID, never by their
current row index or name. The proposed runtime would clone/render these controls
and bind them to canonical values; the HTML does not do that itself.

An empty string is an explicitly empty text value, `false` is an explicit boolean,
and an absent field means unknown/missing. This example does not use `null` as an
alias for missing. Context must use live canonical values, not HTML defaults.
Template revisions must retain field identity where meaning is unchanged; they
do not silently migrate a character's immutable template pin.

## Tasks for the eventual live milestone

1. Load this package and example character without editing a database or writing code.
2. Ask: “I spend two focus; what changes?” Review the proposed `focus: 3 → 1`.
3. Ask: “Equip the iron vest instead of my coat.” Review all equipment/armor/pace
   changes as one proposal; nothing applies until you confirm.
4. Try to set vitality above its maximum. See a cited advisory finding; generic
   application enforcement remains separate from model advice.
5. Ask about a short rest: the model should identify the missing rule, not invent it.
6. Ask whether you can cross a gap: the model should ask for its width.
7. Enter long notes and a new equipment entry; check desktop/mobile reflow, saving
   and reopening, stable entry IDs and absence of unrelated changes.
8. Keep an old proposal open, change the character, then try applying the old one:
   application revision checks must reject it regardless of the model's opinion.

For a local-model test, send numbered rules plus canonical sheet/template context
and one case's request. **Do not send expected outputs, forbidden changes or the
walkthrough above.** The evaluator compares against `review-cases.json`, includes
held-out paraphrases and records all attempts/failures, latency and memory.

Expected outputs describe useful behavior, not an exact wording or model protocol.
Syntactically valid JSON is not proof of a correct rule interpretation. The model
must not apply changes or claim legality. Synthetic success is only a preliminary
interaction/model baseline; actual game materials are needed for a viability test.

## Packaging status

This preparatory pack is included through three narrow Cargo allowlist patterns
for this directory's Markdown, JSON and HTML: benchmark helper regression tests
need these exact synthetic fixtures in corresponding source. It is still not an
application import format or a bundled runtime demo. Model weights/results,
private corpora and arbitrary example directories remain excluded. Verify actual
archive contents when packaging.
