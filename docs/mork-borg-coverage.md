# MÖRK BORG first adapter slice

## Source and review

Adapter edition key: `MÖRK BORG — IT 2025`. Source: `mork-borg-core-it-2025` with SHA-256 in `sources.md`. On 2026-10-11 the following original PDF pages were rendered with MuPDF and visually read alongside text extraction:

| Printed page | One-based PDF page | Reviewed mechanics |
| ---: | ---: | --- |
| 24 | 28 | Armor tiers/reduction; medium armor Agility +2 DR; heavy armor Agility +4 DR with Defense +2 |
| 26 | 30 | Four ability modifiers; base advancement bounds −3/+6; meet-or-exceed test difficulty |
| 27 | 31 | Standard-item capacity Strength+8; maximum double that; excess load adds +2 to Strength/Agility DR |
| 28 | 32 | Initial base HP minimum and zero/negative HP states |
| 29 | 33 | Base melee/ranged/defense tests; armor penalties remain after damage |

This is a manual visual source audit for the implemented mechanics, not approval of a whole-book Markdown conversion. Extracted full text and rendered pages were temporary local inspection artifacts, not repository assets.

## Implemented

- Separate typed sheet schema, stored in SQLite with schema version 1.
- Explicit edition pinning; arbitrary existing game/edition strings do not silently enable this adapter.
- Editable abilities, current/max HP, original worn armor tier, current protection tier, standard-sized item count, silver, omens, and equipment notes.
- Persistent description, class/power notes, two weapon notes, and optional manually entered campaign miseries. Unknown miseries remain distinct from explicit zero; no calendar or power mechanics are inferred from these notes. Sheet-name saves update canonical identity atomically.
- Adaptive grouping based on the owner-supplied landscape blank sheet: identity/class/powers; HP/abilities/omens; weapons/armor/equipment/silver/miseries. Legacy saved sheets default the added fields without losing stats; omitted fields in old posts preserve existing values.
- Derived load thresholds, load warnings, armor reduction die, HP state, and baseline combat/Agility difficulty references with source pages.
- Distinct original armor tier and damaged protection so reduced protection does not erase worn-armor penalties.
- Shared transactional character revision: identity edits and stats edits cannot overwrite stale versions silently.
- Coherent identity/stats read snapshots under concurrent connections.
- Read-only ordinary classless contextual checklist for attacks, room movement, triggered defense and normal scroll attempts, with unknown-fact gates and explicit source explanations.

Ability bounds are base-rule checks; class/special-item exceptions and overrides are not implemented. HP/resource limits above ordinary game values are storage bounds, not claims about legal maxima. Item counts and maximum HP are entered from an existing character, not generated automatically. Over-maximum load is warned about rather than deleting or refusing a record of the current situation.

## Not yet implemented

- Approved official-sheet fidelity, import/export, and print layout. The supplied blank sheet has informed the grouping; publisher artwork/fonts are not copied. The current UI remains a structured editor, not an approved official sheet.
- Class features, special items, shields, conditions, house-rule overrides, dice rolling, damage resolution, healing, character generation, and powers/scrolls.
- Full action legality, class/special-effect/house-rule resolution and particular power effects. Checklist facts are player declarations, not complete world or legality evidence.
- AI explanations and general OCR ingestion. Strict prepared-source CLI registration, review UI/history and reviewed-only lexical lookup now exist; a one-page Marker text-layer sample was tested and left unreviewed. This evidence workflow does not automatically generate or change adapter rules.

## Verification

Tests cover load boundary cases, negative/zero HP, damaged armor penalties, invalid values, edition isolation, persistence, version-1 database migration, shared stale-edit protection, and htmx error preservation. Whole-corpus or 100% rules coverage is not claimed.

The contextual slice additionally compared original PDF34 (round text, printed number not visible) and printed34/PDF38 (powers). It preserves success-only use cost, failure d2HP/disorientation and catastrophic further attempts without applying effects. Defense's natural20 free attack is outside ordinary-action budgeting. Independent review/regressions: `reviews/2026-10-11-context-checklist.md`.
