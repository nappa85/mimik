# Catalogue publication

Read-only game catalogues: spells, items and other entries that a sheet can cite. Plan:
[catalogue-plan.md](catalogue-plan.md). **Implemented so far: step 1, drafts only.**
Comparison, publication, sealed versions, the name index, definition linkage and the sheet
routes are later steps; until they land, a catalogue draft is never visible to players and
never changes a sheet.

## Drafts

### Operator commands

Register the exact nonnative game/edition pair first (`runtime-registry.md`) and import every
cited source (`sources.md`). Then:

```sh
./target/release/mimik import-catalogue-draft --file spells.json --expected-revision 0
./target/release/mimik show-catalogue-draft --id 1
./target/release/mimik import-catalogue-draft --file spells.json --expected-revision 1
```

- Revision 0 creates only; an existing `(game, edition, catalogue)` draft conflicts. A matching
  revision updates and increments it once, including for identical content; a stale revision
  changes nothing.
- The row ID is a positive database ID, distinct from the catalogue id. Draft identity
  (`game_id`, `edition_id`, `catalogue_id`) is immutable.
- `show-catalogue-draft` re-decodes the stored draft and checks its canonical bytes, hash and
  identity. It does not re-check source references, which are checked at every save.
- Commands share the definition admission permit: one nonqueued job per process. A second job
  in the same process answers busy. A save takes the SQLite write lock (`BEGIN IMMEDIATE`)
  before its reference checks and revision lookup, so concurrent saves from other processes
  wait (5 s busy timeout); saves of different catalogues all succeed, and of two saves of one
  catalogue with the same expected revision one wins and the other answers a conflict.

### Redaction

Catalogue rows hold manual-derived names. **No command prints names, entry keys, property ids
or values, labels, the title, owner text or unit text** (plan D6).

- Success prints only `id`, `game_id`, `edition_id`, `catalogue_id`, `revision`,
  `content_hash`, `status` and `counts` (sources, entry types, entries, retired, owner-written,
  unit references, distinct units).
- Errors are static codes with index-only paths such as `/entries/12/description:
  unit-page-source-mismatch`. Schema paths are walked against the envelope structure: indices
  are kept only at array positions and member names only where that member exists; the first
  other segment and everything after it become `*`. Entry-property keys are always redacted,
  whatever their spelling.
- Sentinel tests (`tests/catalogues.rs`, `src/catalogues/admission_tests.rs`) place distinctive
  synthetic strings in every author-chosen member and check stdout and stderr on success and on
  failure paths: decode, schema, domain, source references, a non-finalized source, conflicts,
  reads of unknown and corrupt drafts, and oversize and symlink inputs.

### Envelope

Trusted host schema `schemas/catalogue-v1.json` (JSON Schema 2020-12, compiled offline),
then serde with `deny_unknown_fields`, then domain checks. `schemas/catalogue-example.json` is
an original synthetic example.

```json
{ "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
  "catalogue_id": "spells", "title": "Spells",
  "sources": [{ "source_id": 3, "pdf_sha256": "…64 lowercase hex…" }],
  "entry_types": [{ "id": "spell", "label": "Spell", "properties": [
      { "id": "level", "label": "Level", "control": "integer" },
      { "id": "school", "label": "School", "control": "enum",
        "options": [{ "id": "light", "label": "Light" }] }] }],
  "entries": [
    { "key": "lantern-flare", "type": "spell", "name": "Lantern Flare",
      "properties": { "level": 1, "school": "light" },
      "description": { "units": [{ "source_id": 3, "page_id": 41, "unit_id": 977 }] } },
    { "key": "house-charm", "type": "spell", "name": "House Charm", "retired": false,
      "properties": { "level": 0, "school": "light" },
      "description": { "owner_text": "…" } }] }
```

- Every object is closed (`additionalProperties: false`), except an entry's `properties`, which
  is a map keyed by the type's property ids; its values are closed to string, integer and
  boolean, and the domain check requires exactly the declared key set.
- IDs, keys and option ids follow the registry slug grammar: a leading lowercase letter, then
  lowercase letters, digits or `-`, 1–64 bytes.
- Each entry's `type` must be declared; its properties must be exactly that type's properties,
  each matching its control (`text` string, `integer`, `boolean`, `enum` one of the option
  ids). Options are allowed only on `enum`, which needs 1–128 unique options.
- A description is either 1–16 unit references or `owner_text` (non-blank). An entry may not
  cite the same unit twice. Every cited source must be listed in `sources`.
- Labels, names, the title and owner text may not be blank. Text is not normalised.
- Names, labels, the title and text property values may not contain control characters
  (`char::is_control`, C0 and C1). Owner text allows line feed and tab only.
- `retired` absent and `retired: false` are the same state.

### Source references (checked at every save, in the write transaction)

- The pair is registered, nonnative and has no built-in adapter (as for definition drafts).
- Every listed source exists with that PDF hash, belongs to exactly that pair, and is
  finalized.
- Every `(source_id, page_id, unit_id)` names a unit on that page of that source.
- A unit's review state is not checked: rejected and unreviewed units are allowed in drafts.
  Publication (step 2) refuses rejected units.

### Limits

| Resource | Limit |
|---|---|
| Raw input and canonical encoding | 4 MiB each (4,194,304 bytes) |
| Decoder nodes (catalogue profile) | 524,288 |
| Container depth | 8 levels |
| Members per object / key length | 32 / 64 scalars |
| Sources | 32 |
| Entry types | 1–32 |
| Properties per type | 16 |
| Options per enum | 128 |
| Entries | 8,192 |
| Units per entry | 1–16 |
| Distinct units per catalogue | 16,384 |
| Names, labels, title | 100 scalars |
| Text property value | 1,000 scalars |
| `owner_text` | 20,000 scalars per entry, 1 MiB (UTF-8 bytes) in total |

The catalogue profile is separate from the definition decoder: `definitions::decode::MAX_NODES`
(32,768), its 256 KiB input cap and the list form budget are unchanged, and a regression test
asserts it. Duplicate keys, including escaped-equivalent spellings, fractions, exponents,
integers outside i64 and trailing JSON are rejected.

Every canonical spelling is no longer than any accepted input spelling (no member is added by
canonicalisation), so the canonical cap is a defensive check that admitted input cannot reach.

### Canonical identity

Hash: SHA-256 over `b"mimik-catalogue-v1\0"` plus the canonical bytes, lowercase hex.

- Members are written in a fixed order (the schema's order above). Optional members are
  omitted when absent; `retired: false` is omitted.
- Sources are sorted by id. Entry types, properties, options and entries keep the author's
  order. Each entry's `properties` are written in its type's declared order.
- **Unit order is the author's order and is part of the identity; it is never sorted.**
- A permanent frozen hash vector guards the encoding
  (`canonical_encoding_has_a_frozen_independent_hash_vector`).

### Storage

Schema 19 (`migrations/019_catalogues.sql`) adds the STRICT table `catalogue_drafts`:
positive autoincrement ids and revisions, a registry foreign key, NUL-free slug checks, a JSON
object of at most 4 MiB whose `contract_version` and identity match the columns, an exact
64-byte lowercase-hex hash, a unique identity and the trigger
`catalogue_draft_identity_immutable`. The migration is additive and atomic. Later steps extend
the same migration before it is merged.
